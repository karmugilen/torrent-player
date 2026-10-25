package torrent

import (
	"fmt"
	"net"
	"testing"
	"time"

	g "github.com/anacrolix/generics"
	"github.com/go-quicktest/qt"

	"github.com/anacrolix/torrent/metainfo"
	infohash_v2 "github.com/anacrolix/torrent/types/infohash-v2"
)

func newTricksTorrent(t *testing.T) (*Client, *Torrent) {
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
	return cl, tor
}

// newRatedPeer adds an unchoked peer holding every piece that has
// delivered bytesPerSec.
func newRatedPeer(t *testing.T, cl *Client, tor *Torrent, bytesPerSec int64) *PeerConn {
	local, remote := net.Pipe()
	t.Cleanup(func() { local.Close(); remote.Close() })
	c := cl.newConnection(local, newConnectionOpts{network: "test"})
	c.setTorrent(tor)
	c.peerSentHaveAll, c.peerChoking = true, false
	c._stats.BytesReadUsefulData.Add(bytesPerSec)
	c.cumulativeExpectedToReceiveChunks = time.Second
	c.PeerMaxRequests, c.peakRequests = 64, 64
	c.initMessageWriter()
	tor.conns[c] = struct{}{}
	return c
}

// A slow peer leaves a playback piece to a faster one that would finish it
// first, and takes it when no faster peer can.
func TestSlowPeerLeavesAPlaybackPieceToFasterPeers(t *testing.T) {
	cl, tor := newTricksTorrent(t)
	defer func() {
		for c := range tor.conns {
			delete(tor.conns, c)
		}
	}()
	tor.cl.lock()
	defer tor.cl.unlock()
	slow := newRatedPeer(t, cl, tor, 16<<10)
	qt.Check(t, qt.IsFalse(slow.webtorYieldToFasterPeers(1)), qt.Commentf("nobody faster"))
	fast := newRatedPeer(t, cl, tor, 1<<20)
	qt.Check(t, qt.IsTrue(slow.webtorYieldToFasterPeers(1)), qt.Commentf("the fast peer finishes 256 KiB first"))
	qt.Check(t, qt.IsFalse(fast.webtorYieldToFasterPeers(1)), qt.Commentf("a fast peer never yields"))
	fast.peerChoking = true
	qt.Check(t, qt.IsFalse(slow.webtorYieldToFasterPeers(1)), qt.Commentf("a choking peer cannot take it"))
}

// A peer that keeps us choked while we want its data is dropped once more
// peers wait than twice the free slots.
func TestPeerThatKeepsUsChokedIsDropped(t *testing.T) {
	cl, tor := newTricksTorrent(t)
	defer func() {
		for c := range tor.conns {
			delete(tor.conns, c)
		}
	}()
	tor.cl.lock()
	choker := newRatedPeer(t, cl, tor, 0)
	choker.peerChoking, choker.requestState.Interested = true, true
	choker.webtorChokedSince = time.Now().Add(-10 * time.Second)
	choker.lastBecameInterested = time.Now().Add(-10 * time.Second)
	giver := newRatedPeer(t, cl, tor, 1<<20)
	giver.requestState.Interested = true
	tor.maxEstablishedConns = len(tor.conns)
	tor.cl.unlock()
	qt.Check(t, qt.Equals(tor.WebtorDropChokers(5*time.Second), 0), qt.Commentf("no peer waiting"))
	tor.cl.lock()
	for i := range 3 {
		tor.peers.Add(PeerInfo{Addr: ipPortAddr{IP: net.ParseIP(fmt.Sprintf("10.0.0.%d", i+1)), Port: 6881}})
	}
	choker.lastBecameInterested = time.Now()
	tor.cl.unlock()
	qt.Check(t, qt.Equals(tor.WebtorDropChokers(5*time.Second), 0), qt.Commentf("just became interested"))
	tor.cl.lock()
	choker.lastBecameInterested = time.Now().Add(-10 * time.Second)
	tor.cl.unlock()
	qt.Check(t, qt.Equals(tor.WebtorDropChokers(5*time.Second), 1))
	qt.Check(t, qt.IsTrue(choker.closed.IsSet()))
	qt.Check(t, qt.IsFalse(giver.closed.IsSet()), qt.Commentf("the unchoked peer stays"))
	qt.Check(t, qt.Equals(tor.WebtorChokersDropped(), int64(1)))
}
