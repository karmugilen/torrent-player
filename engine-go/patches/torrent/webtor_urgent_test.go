package torrent

import (
	"testing"
	"time"
	"weak"

	g "github.com/anacrolix/generics"
	"github.com/go-quicktest/qt"

	infohash_v2 "github.com/anacrolix/torrent/types/infohash-v2"

	"github.com/anacrolix/torrent/metainfo"
)

func TestUrgentStealOnlyMovesStalledUrgentBlocksToDeliveringPeers(t *testing.T) {
	cl := newTestingClient(t)
	tor, _ := cl.AddTorrentOpt(AddTorrentOpts{
		InfoHash:   testingTorrentInfoHash,
		InfoHashV2: g.Some(infohash_v2.FromHexString("deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef")),
		Storage:    &storageClient{},
	})
	tor.disableTriggers = true
	const pieceLength = 1 << 8 << 10
	qt.Assert(t, qt.IsNil(tor.setInfoUnlocked(&metainfo.Info{
		Pieces:      make([]byte, 4*metainfo.HashSize),
		PieceLength: pieceLength,
		Length:      pieceLength * 4,
	})))
	newPeer := func() *PeerConn {
		c := cl.newConnection(nil, newConnectionOpts{network: "test"})
		c.setTorrent(tor)
		return c
	}
	slow, fast := newPeer(), newPeer()
	now := time.Now()
	slow.lastUsefulChunkReceived = now.Add(-5 * time.Second)
	fast.lastUsefulChunkReceived = now.Add(-100 * time.Millisecond)

	urgentReq := tor.pieceRequestIndexBegin(1)
	normalReq := tor.pieceRequestIndexBegin(2)
	tor.pieces[1].priority.Raise(PiecePriorityNow)
	tor.pieces[2].priority.Raise(PiecePriorityNormal)
	requestedAt := func(r RequestIndex, age time.Duration) {
		tor.requestState[r] = requestState{peer: weak.Make(slow), when: now.Add(-age)}
	}

	requestedAt(urgentReq, 2*time.Second)
	requestedAt(normalReq, 2*time.Second)
	qt.Check(t, qt.IsFalse(fast.shouldStealUrgent(urgentReq, slow)), qt.Commentf("disabled by default"))

	cl.SetUrgentRequestStealing(time.Second)
	qt.Check(t, qt.IsTrue(fast.shouldStealUrgent(urgentReq, slow)), qt.Commentf("stalled urgent block"))
	qt.Check(t, qt.IsFalse(fast.shouldStealUrgent(normalReq, slow)), qt.Commentf("non-urgent piece"))

	requestedAt(urgentReq, 300*time.Millisecond)
	qt.Check(t, qt.IsFalse(fast.shouldStealUrgent(urgentReq, slow)), qt.Commentf("request too recent"))

	requestedAt(urgentReq, 2*time.Second)
	qt.Check(t, qt.IsFalse(slow.shouldStealUrgent(urgentReq, fast)), qt.Commentf("stealer idle longer than the age"))
	fast.lastUsefulChunkReceived = now.Add(-200 * time.Millisecond)
	slow.lastUsefulChunkReceived = now.Add(-50 * time.Millisecond)
	qt.Check(t, qt.IsFalse(fast.shouldStealUrgent(urgentReq, slow)), qt.Commentf("holder delivered more recently"))

	cl.SetUrgentRequestStealing(0)
	slow.lastUsefulChunkReceived = now.Add(-5 * time.Second)
	qt.Check(t, qt.IsFalse(fast.shouldStealUrgent(urgentReq, slow)), qt.Commentf("disabled again"))
	for _, r := range []RequestIndex{urgentReq, normalReq} {
		delete(tor.requestState, r)
	}

	// Peers are not attached as connections here, so the nudge finds the
	// urgent piece but has nobody to wake.
	tor.updatePiecePriorityNoRequests(1)
	qt.Check(t, qt.IsTrue(tor._pendingPieces.Contains(1)))
	qt.Check(t, qt.Equals(tor.NudgeUrgentRequests(), 0))
}
