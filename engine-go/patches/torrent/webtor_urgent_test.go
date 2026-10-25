package torrent

import (
	"testing"
	"time"
	"weak"

	g "github.com/anacrolix/generics"
	"github.com/go-quicktest/qt"

	request_strategy "github.com/anacrolix/torrent/internal/request-strategy"
	infohash_v2 "github.com/anacrolix/torrent/types/infohash-v2"

	"github.com/anacrolix/torrent/metainfo"
	pp "github.com/anacrolix/torrent/peer_protocol"
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

func TestRequestFocusFetchesThePieceFromThePlayheadFirst(t *testing.T) {
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
	c := cl.newConnection(nil, newConnectionOpts{network: "test"})
	c.setTorrent(tor)
	states := make([]g.Option[request_strategy.PieceRequestOrderState], 4)
	states[1].Set(request_strategy.PieceRequestOrderState{Priority: PiecePriorityNow, Availability: 1})
	heap := desiredPeerRequests{peer: c, pieceStates: states}
	begin := tor.pieceRequestIndexBegin(1)
	first, mid, last := begin, begin+8, begin+15
	less := func(l, r RequestIndex) bool {
		tor.cl.lock()
		defer tor.cl.unlock()
		return heap.lessByValue(l, r)
	}
	qt.Check(t, qt.IsTrue(less(first, mid)), qt.Commentf("without a focus blocks go in order"))

	tor.SetRequestFocus(pieceLength + 8*int64(tor.chunkSize) + 100)
	qt.Check(t, qt.IsTrue(less(mid, first)), qt.Commentf("the playhead block goes before the piece start"))
	qt.Check(t, qt.IsTrue(less(mid, last)), qt.Commentf("then forward in order"))
	qt.Check(t, qt.IsTrue(less(last, first)), qt.Commentf("blocks behind the playhead go last"))

	tor.SetRequestFocus(3 * pieceLength)
	qt.Check(t, qt.IsTrue(less(first, mid)), qt.Commentf("a focus in another piece leaves this one in order"))
	tor.SetRequestFocus(-1)
	qt.Check(t, qt.IsTrue(less(first, mid)), qt.Commentf("cleared focus"))
}

func TestRequestFocusFetchesPiecesNearestThePlayheadFirst(t *testing.T) {
	cl := newTestingClient(t)
	tor, _ := cl.AddTorrentOpt(AddTorrentOpts{
		InfoHash:   testingTorrentInfoHash,
		InfoHashV2: g.Some(infohash_v2.FromHexString("deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef")),
		Storage:    &storageClient{},
	})
	tor.disableTriggers = true
	const pieceLength = 1 << 8 << 10
	qt.Assert(t, qt.IsNil(tor.setInfoUnlocked(&metainfo.Info{
		Pieces:      make([]byte, 8*metainfo.HashSize),
		PieceLength: pieceLength,
		Length:      pieceLength * 8,
	})))
	c := cl.newConnection(nil, newConnectionOpts{network: "test"})
	c.setTorrent(tor)
	states := make([]g.Option[request_strategy.PieceRequestOrderState], 8)
	// Piece 6 is rarer, so upstream would fetch it before the playhead's.
	states[2].Set(request_strategy.PieceRequestOrderState{Priority: PiecePriorityNow, Availability: 5})
	states[6].Set(request_strategy.PieceRequestOrderState{Priority: PiecePriorityNow, Availability: 1})
	states[1].Set(request_strategy.PieceRequestOrderState{Priority: PiecePriorityNow, Availability: 1})
	states[0].Set(request_strategy.PieceRequestOrderState{Priority: PiecePriorityNow, Availability: 1})
	heap := desiredPeerRequests{peer: c, pieceStates: states}
	less := func(l, r int) bool {
		tor.cl.lock()
		defer tor.cl.unlock()
		return heap.lessByValue(tor.pieceRequestIndexBegin(l), tor.pieceRequestIndexBegin(r))
	}
	qt.Check(t, qt.IsTrue(less(2, 6)), qt.Commentf("without a focus pieces go in file order, not rarest first"))

	tor.SetRequestFocus(2*pieceLength + 10)
	qt.Check(t, qt.IsTrue(less(2, 6)), qt.Commentf("the playhead piece goes first"))
	qt.Check(t, qt.IsTrue(less(6, 1)), qt.Commentf("pieces ahead go before pieces behind"))
	qt.Check(t, qt.IsTrue(less(1, 0)), qt.Commentf("pieces behind go nearest first"))

	// Below the top tier too: the next block goes before a rarer far one.
	states[3].Set(request_strategy.PieceRequestOrderState{Priority: PiecePriorityNext, Availability: 9})
	states[7].Set(request_strategy.PieceRequestOrderState{Priority: PiecePriorityNext, Availability: 1})
	qt.Check(t, qt.IsTrue(less(3, 7)), qt.Commentf("next blocks go in order from the playhead"))
}

func TestRefocusClearsOldQueuesAndPutsThisBlockFirst(t *testing.T) {
	cl := newTestingClient(t)
	tor, _ := cl.AddTorrentOpt(AddTorrentOpts{
		InfoHash:   testingTorrentInfoHash,
		InfoHashV2: g.Some(infohash_v2.FromHexString("deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef")),
		Storage:    &storageClient{},
	})
	tor.disableTriggers = true
	const pieceLength = 1 << 8 << 10
	qt.Assert(t, qt.IsNil(tor.setInfoUnlocked(&metainfo.Info{
		Pieces:      make([]byte, 8*metainfo.HashSize),
		PieceLength: pieceLength,
		Length:      pieceLength * 8,
	})))
	newPeer := func(bytesPerSecond int64) *PeerConn {
		c := cl.newConnection(nil, newConnectionOpts{network: "test"})
		c.setTorrent(tor)
		c.peerSentHaveAll, c.peerChoking = true, false
		c._stats.BytesReadUsefulData.Add(bytesPerSecond)
		c.cumulativeExpectedToReceiveChunks = time.Second
		c.PeerMaxRequests = 4
		c.peakRequests = 4
		c.initMessageWriter()
		tor.conns[c] = struct{}{}
		return c
	}
	slow, fast := newPeer(40<<10), newPeer(400<<10)
	defer func() {
		for c := range tor.conns {
			delete(tor.conns, c)
		}
	}()
	locked := func(f func()) {
		tor.cl.lock()
		defer tor.cl.unlock()
		f()
	}
	// Playing piece 1: both peers fill their queues with blocks there.
	locked(func() {
		tor.pieces[1].priority.Raise(PiecePriorityNow)
		tor.updatePiecePriorityNoRequests(1)
	})
	tor.SetRequestFocus(pieceLength)
	locked(func() {
		fast.updateRequestsWithReason("test")
		slow.updateRequestsWithReason("test")
	})
	qt.Assert(t, qt.Equals(fast.requestState.Requests.GetCardinality(), uint64(4)))
	qt.Assert(t, qt.Equals(slow.requestState.Requests.GetCardinality(), uint64(4)))

	// Seek to piece 5. Piece 1 is no longer wanted, but its blocks still
	// fill both queues until a refocus.
	locked(func() {
		tor.pieces[1].priority = 0
		tor.updatePiecePriorityNoRequests(1)
		tor.pieces[5].priority.Raise(PiecePriorityNow)
		tor.updatePiecePriorityNoRequests(5)
		tor.pieces[6].priority.Raise(PiecePriorityNext)
		tor.updatePiecePriorityNoRequests(6)
	})
	tor.SetRequestFocus(5*pieceLength + 2*int64(tor.chunkSize))
	tor.RefocusRequests()

	playhead := tor.pieceRequestIndexBegin(5) + 2
	locked(func() {
		for _, c := range []*PeerConn{fast, slow} {
			c.requestState.Requests.Iterate(func(r RequestIndex) bool {
				qt.Check(t, qt.Equals(tor.pieceIndexOfRequestIndex(r), 5), qt.Commentf("only this block is queued after a refocus"))
				return true
			})
		}
		qt.Check(t, qt.IsTrue(fast.requestState.Requests.Contains(playhead)), qt.Commentf("the fastest peer takes the playhead block"))
	})
}

func TestPlayheadBlocksGoToFasterPeersAndStealSooner(t *testing.T) {
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
	newPeer := func(bytesPerSecond int64) *PeerConn {
		c := cl.newConnection(nil, newConnectionOpts{network: "test"})
		c.setTorrent(tor)
		c.peerSentHaveAll, c.peerChoking = true, false
		c._stats.BytesReadUsefulData.Add(bytesPerSecond)
		c.cumulativeExpectedToReceiveChunks = time.Second
		tor.conns[c] = struct{}{}
		return c
	}
	slow, fast := newPeer(40<<10), newPeer(400<<10)
	tor.pieces[1].priority.Raise(PiecePriorityNow)
	playhead := tor.pieceRequestIndexBegin(1) + 3
	tor.SetRequestFocus(pieceLength + 3*int64(tor.chunkSize))
	locked := func(f func() bool) bool {
		tor.cl.lock()
		defer tor.cl.unlock()
		return f()
	}
	faster := func(p *PeerConn, r RequestIndex) bool {
		return locked(func() bool { return p.fasterFocusPeer(r) != nil })
	}

	qt.Check(t, qt.IsFalse(faster(slow, playhead)), qt.Commentf("off unless urgent stealing is on"))
	cl.SetUrgentRequestStealing(time.Second)
	qt.Check(t, qt.IsTrue(faster(slow, playhead)), qt.Commentf("a slow peer leaves the playhead block to a fast one"))
	qt.Check(t, qt.IsFalse(faster(fast, playhead)), qt.Commentf("the fastest peer takes it"))
	qt.Check(t, qt.IsFalse(faster(slow, playhead+RequestIndex(DefaultSeekTuning.FocusBlocks))), qt.Commentf("blocks past the playhead are unaffected"))
	fast.peerChoking = true
	qt.Check(t, qt.IsFalse(faster(slow, playhead)), qt.Commentf("a choking peer cannot take it, so the slow peer must"))
	fast.peerChoking = false

	// A playhead block moves after the stealing age over the tuned divisor.
	slow.lastUsefulChunkReceived = time.Now().Add(-5 * time.Second)
	fast.lastUsefulChunkReceived = time.Now().Add(-50 * time.Millisecond)
	ahead := playhead + RequestIndex(DefaultSeekTuning.FocusBlocks)
	for _, r := range []RequestIndex{playhead, ahead} {
		// Past the playhead age (500 ms) but short of the full second.
		tor.requestState[r] = requestState{peer: weak.Make(slow), when: time.Now().Add(-700 * time.Millisecond)}
	}
	qt.Check(t, qt.IsTrue(locked(func() bool { return fast.shouldStealUrgent(playhead, slow) })), qt.Commentf("playhead block stalled 700ms"))
	qt.Check(t, qt.IsFalse(locked(func() bool { return fast.shouldStealUrgent(ahead, slow) })), qt.Commentf("other urgent blocks keep the full age"))
	for _, r := range []RequestIndex{playhead, ahead} {
		delete(tor.requestState, r)
	}
	for c := range tor.conns {
		delete(tor.conns, c)
	}
	cl.SetUrgentRequestStealing(0)
}

func TestRoundTripComesFromRequestsSentToAnIdlePeer(t *testing.T) {
	var e rttEstimator
	e.sent(7)
	e.probeAt = time.Now().Add(-100 * time.Millisecond)
	e.sent(8) // queued behind 7: not a probe
	e.received(8)
	qt.Check(t, qt.Equals(e.estimate, time.Duration(0)), qt.Commentf("a queued block does not measure the trip"))
	e.received(7)
	qt.Check(t, qt.IsTrue(e.estimate >= 100*time.Millisecond && e.estimate < 150*time.Millisecond), qt.Commentf("estimate %v", e.estimate))
	e.sent(9)
	e.probeAt = time.Now().Add(-900 * time.Millisecond)
	e.received(9)
	qt.Check(t, qt.IsTrue(e.estimate < 300*time.Millisecond), qt.Commentf("one slow sample moves the estimate an eighth: %v", e.estimate))
}

func TestPlayheadBlockGoesToThePeerThatAnswersSoonest(t *testing.T) {
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
	newPeer := func(bytesPerSecond int64, rtt time.Duration) *PeerConn {
		c := cl.newConnection(nil, newConnectionOpts{network: "test"})
		c.setTorrent(tor)
		c.peerSentHaveAll, c.peerChoking = true, false
		c._stats.BytesReadUsefulData.Add(bytesPerSecond)
		c.cumulativeExpectedToReceiveChunks = time.Second
		c.webtorRTT.estimate = rtt
		tor.conns[c] = struct{}{}
		return c
	}
	defer func() {
		for c := range tor.conns {
			delete(tor.conns, c)
		}
	}()
	// The fastest downloader is far away; a slower one nearby answers first.
	far, near := newPeer(4<<20, 400*time.Millisecond), newPeer(1<<20, 40*time.Millisecond)
	tor.pieces[1].priority.Raise(PiecePriorityNow)
	playhead := tor.pieceRequestIndexBegin(1)
	tor.SetRequestFocus(pieceLength)
	cl.SetUrgentRequestStealing(time.Second)
	defer cl.SetUrgentRequestStealing(0)
	locked := func(f func() *PeerConn) *PeerConn {
		tor.cl.lock()
		defer tor.cl.unlock()
		return f()
	}
	qt.Check(t, qt.Equals(locked(func() *PeerConn { return far.fasterFocusPeer(playhead) }), near),
		qt.Commentf("the far peer leaves the playhead block to the near one"))
	qt.Check(t, qt.IsNil(locked(func() *PeerConn { return near.fasterFocusPeer(playhead) })),
		qt.Commentf("the near peer keeps it"))
}

func TestPlaybackBlocksStayWithThePeerThatAskedUntilOverdue(t *testing.T) {
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
		c.peerSentHaveAll, c.peerChoking = true, false
		c._stats.BytesReadUsefulData.Add(1 << 20)
		c.cumulativeExpectedToReceiveChunks = time.Second
		c.PeerMaxRequests, c.peakRequests = 64, 64
		c.initMessageWriter()
		tor.conns[c] = struct{}{}
		return c
	}
	defer func() {
		for c := range tor.conns {
			delete(tor.conns, c)
		}
	}()
	holder, other := newPeer(), newPeer()
	other.lastUsefulChunkReceived = time.Now()
	cl.SetUrgentRequestStealing(time.Hour)
	defer cl.SetUrgentRequestStealing(0)
	locked := func(f func()) {
		tor.cl.lock()
		defer tor.cl.unlock()
		f()
	}
	block := tor.pieceRequestIndexBegin(2)
	locked(func() {
		tor.pieces[2].priority.Raise(PiecePriorityNext)
		tor.updatePiecePriorityNoRequests(2)
	})
	tor.SetRequestFocus(2 * pieceLength)
	for _, noBalance := range []bool{false, true} {
		tuning := DefaultSeekTuning
		tuning.NoBalanceSteal = noBalance
		SetSeekTuning(tuning)
		locked(func() {
			for _, c := range []*PeerConn{holder, other} {
				c.requestState.Requests.IterateSnapshot(func(r RequestIndex) bool {
					c.cancel(r)
					return true
				})
				c.requestState.Cancelled.Clear()
				clear(c.validReceiveChunks)
			}
			// The holder asked for the whole piece; the other peer has nothing.
			for r := block; r < block+tor.chunksPerRegularPiece(); r++ {
				_, err := holder.request(r)
				qt.Assert(t, qt.IsNil(err))
			}
			other.needRequestUpdate = ""
			other.updateRequestsWithReason("test")
			qt.Check(t, qt.Equals(other.requestState.Requests.IsEmpty(), noBalance),
				qt.Commentf("NoBalanceSteal=%v: blocks on their way stay put", noBalance))
		})
	}
	SetSeekTuning(DefaultSeekTuning)
}

func TestStaysInterestedInPeersWithAnyPartOfThePlayingFile(t *testing.T) {
	cl := newTestingClient(t)
	tor, _ := cl.AddTorrentOpt(AddTorrentOpts{
		InfoHash:   testingTorrentInfoHash,
		InfoHashV2: g.Some(infohash_v2.FromHexString("deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef")),
		Storage:    &storageClient{},
	})
	tor.disableTriggers = true
	const pieceLength = 1 << 8 << 10
	qt.Assert(t, qt.IsNil(tor.setInfoUnlocked(&metainfo.Info{
		Pieces:      make([]byte, 8*metainfo.HashSize),
		PieceLength: pieceLength,
		Length:      pieceLength * 8,
	})))
	// The peer holds only piece 6, far from the playhead, and chokes us.
	c := cl.newConnection(nil, newConnectionOpts{network: "test"})
	c.setTorrent(tor)
	c.peerChoking = true
	c._peerPieces.Add(6)
	c.initMessageWriter()
	tor.conns[c] = struct{}{}
	defer delete(tor.conns, c)
	interested := func() bool {
		tor.cl.lock()
		defer tor.cl.unlock()
		c.needRequestUpdate = ""
		c.updateRequestsWithReason("test")
		return c.requestState.Interested
	}
	qt.Check(t, qt.IsFalse(interested()), qt.Commentf("nothing played, nothing wanted"))
	tor.SetPlaybackInterest(4, 8)
	qt.Check(t, qt.IsTrue(interested()), qt.Commentf("the peer has part of the playing file"))

	other := cl.newConnection(nil, newConnectionOpts{network: "test"})
	other.setTorrent(tor)
	other.peerChoking = true
	other._peerPieces.Add(6)
	other.initMessageWriter()
	tor.conns[other] = struct{}{}
	defer delete(tor.conns, other)
	tor.SetPlaybackInterest(0, 4)
	tor.cl.lock()
	other.needRequestUpdate = ""
	other.updateRequestsWithReason("test")
	qt.Check(t, qt.IsFalse(other.requestState.Interested), qt.Commentf("piece 6 is outside the playing file"))
	tor.cl.unlock()
}

func TestHeadOfLineBlockMovesToASoonerPeerWhenOverdue(t *testing.T) {
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
		c.peerSentHaveAll, c.peerChoking = true, false
		c._stats.BytesReadUsefulData.Add(1 << 20)
		c.cumulativeExpectedToReceiveChunks = time.Second
		c.PeerMaxRequests, c.peakRequests = 64, 64
		c.initMessageWriter()
		tor.conns[c] = struct{}{}
		return c
	}
	defer func() {
		for c := range tor.conns {
			delete(tor.conns, c)
		}
	}()
	holder, rescuer := newPeer(), newPeer()
	tuning := DefaultSeekTuning
	tuning.HOLRescue = true
	SetSeekTuning(tuning)
	defer SetSeekTuning(DefaultSeekTuning)
	tor.cl.lock()
	defer tor.cl.unlock()
	tor.pieces[1].priority.Raise(PiecePriorityNow)
	tor.updatePiecePriorityNoRequests(1)
	tor.SetRequestFocus(pieceLength)
	first := tor.pieceRequestIndexBegin(1)
	// The holder has the playhead's first 20 blocks queued; the rescuer none.
	for r := first; r < first+20; r++ {
		_, err := holder.request(r)
		qt.Assert(t, qt.IsNil(err))
	}
	stuck, alsoStuck := first+2, first+5
	for _, r := range []RequestIndex{first, first + 1, first + 3, first + 4} {
		tor.dirtyChunks.Add(r) // arrived, around the stuck ones
	}
	for r := first + 6; r < first+20; r++ {
		tor.dirtyChunks.Add(r)
	}
	age := func(r RequestIndex, d time.Duration) {
		state := tor.requestState[r]
		state.when = time.Now().Add(-d)
		tor.requestState[r] = state
	}
	age(stuck, 100*time.Millisecond)
	age(alsoStuck, 100*time.Millisecond)
	qt.Check(t, qt.Equals(tor.webtorRescueHeadOfLine(), 0), qt.Commentf("not overdue yet"))
	age(stuck, time.Second)
	age(alsoStuck, time.Second)
	qt.Check(t, qt.Equals(tor.webtorRescueHeadOfLine(), 2), qt.Commentf("both stuck blocks move in one pass"))
	qt.Check(t, qt.Equals(tor.WebtorRescues(), int64(2)))
	for _, r := range []RequestIndex{stuck, alsoStuck} {
		qt.Check(t, qt.IsTrue(rescuer.requestState.Requests.Contains(r)), qt.Commentf("the rescuer asked for %v", r))
		qt.Check(t, qt.IsFalse(holder.requestState.Requests.Contains(r)), qt.Commentf("the holder's request for %v was cancelled", r))
	}
	qt.Check(t, qt.Equals(tor.webtorRescueHeadOfLine(), 0), qt.Commentf("just asked again: not overdue"))
	// Back to the holder, overdue: it is not sooner than the rescuer's queue.
	for _, r := range []RequestIndex{stuck, alsoStuck} {
		tor.cancelRequest(r)
		holder.mustRequest(r)
		age(r, time.Second)
	}
	for r := first + 20; r < first+46; r++ {
		if tor.requestingPeer(r) == nil {
			rescuer.mustRequest(r)
		}
	}
	qt.Check(t, qt.Equals(tor.webtorRescueHeadOfLine(), 0), qt.Commentf("the rescuer is busier than the holder"))
	SetSeekTuning(DefaultSeekTuning)
	qt.Check(t, qt.Equals(tor.webtorRescueHeadOfLine(), 0), qt.Commentf("off by default"))
}

func TestPlaybackPiecesAreHashedFirst(t *testing.T) {
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
	tor.cl.lock()
	defer tor.cl.unlock()
	tor.piecesQueuedForHash.Add(1)
	tor.piecesQueuedForHash.Add(3)
	qt.Check(t, qt.Equals(tor.getPieceToHash().Unwrap(), 1), qt.Commentf("piece order when nothing is urgent"))
	tor.pieces[3].priority.Raise(PiecePriorityNow)
	qt.Check(t, qt.Equals(tor.getPieceToHash().Unwrap(), 3), qt.Commentf("the piece playback waits on first"))
}

// A request update of a peer that already has its queue (an urgent nudge,
// every 100 ms while the player reads) adds few blocks or none. Upstream took
// that count as the peer's capacity, so during playback every peer fell to
// about one request and only the next block downloaded.
func TestTopUpOfABusyPeerKeepsItsRequestLimit(t *testing.T) {
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
	c := cl.newConnection(nil, newConnectionOpts{network: "test"})
	c.setTorrent(tor)
	c.peerSentHaveAll, c.peerChoking = true, false
	c.PeerMaxRequests = 64
	c.initMessageWriter()
	tor.conns[c] = struct{}{}
	defer delete(tor.conns, c)
	tor.SetRequestFocus(2 * pieceLength)
	defer SetSeekTuning(DefaultSeekTuning)
	for _, mode := range []PeakMode{PeakAdded, PeakWholeQueue} {
		tuning := DefaultSeekTuning
		tuning.PeakMode = mode
		SetSeekTuning(tuning)
		tor.cl.lock()
		tor.pieces[2].priority.Raise(PiecePriorityNow)
		tor.updatePiecePriorityNoRequests(2)
		// The peer holds every block of the only wanted piece.
		c.peakRequests = 16
		block := tor.pieceRequestIndexBegin(2)
		for r := block; r < block+tor.chunksPerRegularPiece(); r++ {
			if !c.requestState.Requests.Contains(r) {
				_, err := c.request(r)
				qt.Assert(t, qt.IsNil(err))
			}
		}
		c.needRequestUpdate = ""
		c.updateRequestsWithReason(peerUpdateRequestsUrgentNudge)
		got := c.nominalMaxRequests()
		tor.cl.unlock()
		if mode == PeakAdded {
			qt.Check(t, qt.Equals(got, maxRequests(1)), qt.Commentf("upstream collapses the limit"))
		} else {
			qt.Check(t, qt.Equals(got, maxRequests(32)), qt.Commentf("the limit follows the queue"))
		}
	}
}

// An overdue head-of-line block is also asked of the soonest other peer; the
// first request stays. Either copy may come first.
func TestOverdueHeadOfLineBlockIsHedgedOnASecondPeer(t *testing.T) {
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
		c.peerSentHaveAll, c.peerChoking = true, false
		c._stats.BytesReadUsefulData.Add(1 << 20)
		c.cumulativeExpectedToReceiveChunks = time.Second
		c.PeerMaxRequests, c.peakRequests = 64, 64
		c.initMessageWriter()
		tor.conns[c] = struct{}{}
		return c
	}
	defer func() {
		for c := range tor.conns {
			delete(tor.conns, c)
		}
	}()
	holder, hedger := newPeer(), newPeer()
	tuning := DefaultSeekTuning
	tuning.Hedge = true
	SetSeekTuning(tuning)
	defer SetSeekTuning(DefaultSeekTuning)
	tor.cl.lock()
	defer tor.cl.unlock()
	tor.pieces[1].priority.Raise(PiecePriorityNow)
	tor.updatePiecePriorityNoRequests(1)
	tor.SetRequestFocus(pieceLength)
	first := tor.pieceRequestIndexBegin(1)
	for r := first; r < first+8; r++ {
		_, err := holder.request(r)
		qt.Assert(t, qt.IsNil(err))
		if r != first+2 {
			tor.dirtyChunks.Add(r)
		}
	}
	stuck := first + 2
	age := func(d time.Duration) {
		state := tor.requestState[stuck]
		state.when = time.Now().Add(-d)
		tor.requestState[stuck] = state
	}
	age(100 * time.Millisecond)
	qt.Check(t, qt.Equals(tor.webtorHedgeHeadOfLine(), 0), qt.Commentf("not overdue yet"))
	age(time.Second)
	qt.Check(t, qt.Equals(tor.webtorHedgeHeadOfLine(), 1))
	qt.Check(t, qt.Equals(tor.requestingPeer(stuck), holder), qt.Commentf("the first request stays"))
	qt.Check(t, qt.Equals(hedger.validReceiveChunks[stuck], 1), qt.Commentf("the hedger's copy will be accepted"))
	qt.Check(t, qt.Equals(tor.webtorHedgeHeadOfLine(), 0), qt.Commentf("hedged once"))
	// The hedger's copy comes first: it is the intended one.
	intended, err := hedger.checkReceivedChunk(stuck, &pp.Message{}, tor.requestIndexToRequest(stuck))
	qt.Check(t, qt.IsNil(err))
	qt.Check(t, qt.IsTrue(intended))
	qt.Check(t, qt.IsNil(tor.webtorHedges[stuck]))
	SetSeekTuning(DefaultSeekTuning)
	age(time.Second)
	qt.Check(t, qt.Equals(tor.webtorHedgeHeadOfLine(), 0), qt.Commentf("off by default"))
}
