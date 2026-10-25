package torrent

import (
	"cmp"
	"math"
	"slices"
	"sync/atomic"
	"time"
)

// Webtor seek extension. Urgent (PiecePriorityNow) blocks normally stay with
// whichever peer first requested them unless the upstream "don't steal from
// the poor" rule allows a move. After a seek that lets one slow peer hold the
// blocks the player is waiting for. With urgent stealing enabled, a block of an
// urgent piece that has been outstanding longer than the configured age moves
// to a requesting peer that delivered useful data more recently.

const (
	peerUpdateRequestsUrgentNudge updateRequestReason = "webtor urgent nudge"
	peerUpdateRequestsRefocus     updateRequestReason = "webtor refocus"
)

// SeekTuning holds the playhead request rules, so they can be searched.
type SeekTuning struct {
	// FocusBlocks is how many blocks from the playhead get the rules below.
	FocusBlocks int
	// FocusStealDivisor divides the urgent stealing age for those blocks.
	FocusStealDivisor int
	// FasterRatio is how much faster another peer must be to take a
	// playhead block from the peer about to request it. Zero turns it off.
	FasterRatio float64
	// QueueAware compares peers by when they would deliver the block (their
	// outstanding requests at their rate) instead of by rate alone.
	QueueAware bool
	// BoundCandidates stops a peer's request refill once it has enough free
	// blocks, after every Now and Next piece (see candidateBudget).
	BoundCandidates bool
	// PipelineSeconds caps a peer's outstanding requests during playback at
	// this many seconds of its own rate, at least MinPipeline. Zero is off,
	// and it ships off: the rate is measured through the cap, so a peer whose
	// round trip is longer than the cap looks slower, gets a smaller cap, and
	// its throughput spirals down. It needs a per-peer round trip estimate.
	PipelineSeconds float64
	MinPipeline     int
	// FullPipeline keeps every peer's request pipeline full. Upstream
	// refills a peer only once it is empty, counts cancels awaiting their
	// reject against its limit, and shrinks its limit by one per cancel, so
	// after a seek moved every request to the new place each peer was back
	// to one request and doubled per round trip. With it the limit survives
	// the move, only uncancelled requests count (the peer reads our cancels
	// before the new requests), and a peer refills at half its limit.
	FullPipeline bool
	// PipelineRTT sizes a peer's pipeline during playback to its own rate
	// times this many of its round trips (bandwidth-delay product), plus
	// MinPipeline. Zero is off. Unlike PipelineSeconds the window follows
	// the peer's round trip, and at 2 or more it holds enough for the rate
	// to grow, so it does not spiral down. Each peer then holds a few round
	// trips of blocks, so the playhead's blocks spread over many peers and
	// none sit behind a long queue.
	PipelineRTT float64
	// ArrivalAware picks the peer for the playhead block by when the block
	// would arrive: its round trip, plus its queue and the block at its rate.
	// The fastest downloader can be the farthest away. A peer must be
	// ArrivalRatio times sooner to take the block from the one about to ask.
	ArrivalAware bool
	ArrivalRatio float64
	// NoBalanceSteal turns off the upstream balance rule during playback: a
	// block moves to another peer only when overdue (shouldStealUrgent). The
	// balance rule let any peer that had just received a block take one from
	// a peer with more queued, including blocks already on their way, so both
	// copies arrived: 4–8% of downloaded data was duplicate. Right after a
	// seek one playhead block moved between ten peers within 100 ms. Off:
	// on in 2.1.39, seeking on a real swarm was slower than in 2.1.38.
	NoBalanceSteal bool
	// RefillAt tops a peer's requests up during playback once they fall to
	// this fraction of its limit, as TCP sends on each acknowledgement. 0
	// waits until every request is answered, so each peer's pipe runs dry
	// before it refills.
	RefillAt float64
	// HOLRescue moves the block the player's stream is stuck on: the first
	// block after the playhead that has not arrived, once it is overdue
	// (HOLAfterMs, or twice its peer's round trip if longer), to a peer
	// expected to deliver it sooner. On the phone the first byte after a seek
	// came in about 240 ms, then the stream waited 0.7-2.7 s on one slow
	// block while the blocks after it had already arrived.
	HOLRescue  bool
	HOLAfterMs float64
	// Hedge asks a second peer, the one expected to deliver soonest, for an
	// overdue block of the first 2 MiB after the playhead, without
	// cancelling the first (HOLAfterMs, or twice the holder's round trip
	// if longer). Whichever arrives first is used and the other is
	// cancelled. HOLRescue cancelled the slow copy and so lost it when it
	// was nearly there; on the phone it did not seek faster.
	Hedge bool
	// SlowPeerYield keeps a slow peer (under 48 KiB/s) off a playback piece
	// (Next or higher) that faster peers holding it could finish in the time
	// the slow one needs for its own queue; it takes a farther piece
	// instead (WebTorrent and torrent-stream's speed ranker). Otherwise a
	// 40 KiB/s peer held a block 1-3 MiB ahead, and playback froze when it
	// got there.
	SlowPeerYield bool
	// PeakMode is how a request update sets the peer's request limit
	// (twice its peak). See the PeakMode constants.
	PeakMode PeakMode
}

type PeakMode int

const (
	// PeakAdded is upstream: the peak is the blocks the update added.
	PeakAdded PeakMode = iota
	// PeakWholeQueue sets the peak to every outstanding request.
	PeakWholeQueue
	// PeakKeepOnTopUp keeps the peak when a busy peer is topped up, so
	// only a refill from empty sets it.
	PeakKeepOnTopUp
)

// DefaultSeekTuning is the shipped rule set, chosen by the engine's seek rule
// search (seek_search_test.go) on a 55-peer swarm with 40–400 ms round trips.
var DefaultSeekTuning = SeekTuning{FocusBlocks: 8, FocusStealDivisor: 2, FasterRatio: 3, BoundCandidates: true, PipelineRTT: 2, ArrivalAware: true, ArrivalRatio: 1.5, HOLAfterMs: 300, PeakMode: PeakWholeQueue}

var seekTuning atomic.Pointer[SeekTuning]

// SetSeekTuning replaces the playhead request rules for every client.
func SetSeekTuning(tuning SeekTuning) {
	tuning.FocusBlocks = max(tuning.FocusBlocks, 1)
	tuning.FocusStealDivisor = max(tuning.FocusStealDivisor, 1)
	seekTuning.Store(&tuning)
}

func currentSeekTuning() SeekTuning {
	if t := seekTuning.Load(); t != nil {
		return *t
	}
	return DefaultSeekTuning
}

// SetUrgentRequestStealing enables stealing of stalled urgent blocks after the
// given age. Zero or negative disables it, restoring upstream behaviour.
func (cl *Client) SetUrgentRequestStealing(after time.Duration) {
	if cl == nil {
		return
	}
	cl.urgentStealAfter.Store(int64(max(after, 0)))
}

func (cl *Client) urgentStealAge() time.Duration {
	if cl == nil {
		return 0
	}
	return time.Duration(cl.urgentStealAfter.Load())
}

// shouldStealUrgent reports whether p may take r from existing even though the
// upstream balance rule declined. Called with the client lock held.
func (p *PeerConn) shouldStealUrgent(r RequestIndex, existing *PeerConn) bool {
	t := p.t
	after := t.cl.urgentStealAge()
	if after <= 0 {
		return false
	}
	if t.piece(t.pieceIndexOfRequestIndex(r)).purePriority() < PiecePriorityNow {
		return false
	}
	// The blocks at the playhead are what the player is frozen on; they move
	// sooner, by the tuned divisor.
	tuning := currentSeekTuning()
	if focus, ok := t.requestFocusIndex(); ok && r >= focus && r < focus+RequestIndex(tuning.FocusBlocks) {
		after /= time.Duration(tuning.FocusStealDivisor)
	}
	state, ok := t.requestState[r]
	if !ok || time.Since(state.when) < after {
		return false
	}
	// Only a peer that is currently delivering may take the block, and never
	// from a peer that is delivering more recently than the stealer.
	if p.lastUsefulChunkReceived.IsZero() || time.Since(p.lastUsefulChunkReceived) > after {
		return false
	}
	return p.lastUsefulChunkReceived.After(existing.lastUsefulChunkReceived)
}

// NudgeUrgentRequests asks connected peers that hold an urgent missing piece
// and have spare request slots to recompute their requests now, rather than
// waiting for their pipeline to drain. With urgent stealing enabled this is
// when stalled urgent blocks move. Returns the number of peers nudged.
func (t *Torrent) NudgeUrgentRequests() int {
	t.cl.lock()
	defer t.cl.unlock()
	if t.closed.IsSet() || !t.haveInfo() || t.dataDownloadDisallowed.Bool() {
		return 0
	}
	t.webtorRescueHeadOfLine()
	t.webtorHedgeHeadOfLine()
	var urgent []pieceIndex
	t._pendingPieces.Iterate(func(x uint32) bool {
		i := pieceIndex(x)
		if !t.pieceComplete(i) && t.piece(i).purePriority() >= PiecePriorityNow {
			urgent = append(urgent, i)
		}
		return true
	})
	if len(urgent) == 0 {
		return 0
	}
	nudged := 0
	for c := range t.conns {
		if c.closed.IsSet() || c.needRequestUpdate != "" {
			continue
		}
		pending := c.pendingRequests()
		if pending >= c.nominalMaxRequests() {
			continue
		}
		for _, i := range urgent {
			if !c.peerHasPiece(i) {
				continue
			}
			if c.peerChoking && !c.peerAllowedFast.Contains(i) {
				continue
			}
			c.onNeedUpdateRequests(peerUpdateRequestsUrgentNudge)
			nudged++
			break
		}
	}
	return nudged
}

// SetRequestFocus marks the torrent byte offset a playback read needs next.
// Peers then fetch that piece's blocks from the offset onward before the ones
// behind it. A negative offset clears the focus.
func (t *Torrent) SetRequestFocus(off int64) {
	t.requestFocus.Store(max(off+1, 0))
}

// requestFocusIndex is the request index holding the focus offset. Called
// with the client lock held.
func (t *Torrent) requestFocusIndex() (RequestIndex, bool) {
	off := t.requestFocus.Load() - 1
	if off < 0 || !t.haveInfo() || off >= t.info.TotalLength() {
		return 0, false
	}
	pieceLen := t.info.PieceLength
	return t.pieceRequestIndexBegin(pieceIndex(off/pieceLen)) + RequestIndex(off%pieceLen/int64(t.chunkSize)), true
}

// focusDistance orders pieces from the focus piece forward, then those behind
// it, nearest first.
func focusDistance(piece, focus pieceIndex) int {
	if piece >= focus {
		return piece - focus
	}
	return math.MaxInt32/2 + (focus - piece)
}

// fasterFocusPeer returns a connected peer at least twice as fast as p that
// could request r, one of the blocks at the playhead, right now. After a seek
// every peer recomputes its requests in no particular order, and the first to
// do so took the block the player is waiting on, even at a few KiB/s. Only
// with urgent stealing on. Called with the client lock held.
func (p *PeerConn) fasterFocusPeer(r RequestIndex) *PeerConn {
	t := p.t
	if t.cl.urgentStealAge() <= 0 {
		return nil
	}
	tuning := currentSeekTuning()
	if tuning.FasterRatio <= 0 {
		return nil
	}
	focus, ok := t.requestFocusIndex()
	if !ok || r < focus || r >= focus+RequestIndex(tuning.FocusBlocks) {
		return nil
	}
	piece := t.pieceIndexOfRequestIndex(r)
	if t.piece(piece).purePriority() < PiecePriorityNow {
		return nil
	}
	if tuning.ArrivalAware {
		ratio := max(tuning.ArrivalRatio, 1)
		var best *PeerConn
		bestAt := p.expectedArrival() / ratio
		for c := range t.conns {
			if c == p || c.closed.IsSet() || c.peerChoking || !c.peerHasPiece(piece) {
				continue
			}
			if c.pendingRequests() >= c.nominalMaxRequests() {
				continue
			}
			if at := c.expectedArrival(); at < bestAt {
				best, bestAt = c, at
			}
		}
		return best
	}
	// Higher is better: the rate, or the inverse of when the peer would
	// deliver one more block behind its outstanding requests.
	score := func(c *PeerConn) float64 {
		rate := c.downloadRate()
		if !tuning.QueueAware || rate <= 0 {
			return rate
		}
		queued := float64(c.requestState.Requests.GetCardinality()+1) * float64(t.chunkSize)
		return rate / queued
	}
	var best *PeerConn
	bestScore := tuning.FasterRatio * score(p)
	for c := range t.conns {
		if c == p || c.closed.IsSet() || c.peerChoking || !c.peerHasPiece(piece) {
			continue
		}
		pending := c.pendingRequests()
		if pending >= c.nominalMaxRequests() {
			continue
		}
		if s := score(c); s > bestScore {
			best, bestScore = c, s
		}
	}
	return best
}

// candidateBudget is how many free blocks a peer collects before it stops
// scanning lower-priority pieces, or 0 for no limit. Each refill listed every
// block of every wanted piece, so with a large RAM cache the playback window
// held tens of thousands of blocks and every peer sorted all of them many
// times a second: a bigger cache made seeking slower. Now pieces are always
// listed in full. The rest are scanned in file order, which is playhead order,
// so the far blocks are reached by later refills as nearer pieces complete. A
// disabled budget keeps upstream behaviour.
func candidateBudget(p *PeerConn) int {
	if !currentSeekTuning().BoundCandidates {
		return 0
	}
	return max(2*int(p.nominalMaxRequests()), 256)
}

// playbackPipelineLimit is the most requests a peer may have outstanding while
// a video plays, or 0 for no limit. A peer serves requests in order, so after
// a seek the playhead block waited behind every readahead block already queued
// there: with a large cache every peer always held hundreds, seconds of data on
// a slow peer, and a bigger cache seeked slower. Holding about a second of the
// peer's own rate keeps it busy across the round trip and lets a seek through.
func (cn *PeerConn) playbackPipelineLimit() maxRequests {
	return cn.webtorPipelineLimit
}

// computePlaybackPipelineLimit is playbackPipelineLimit now; updateRequests
// stores it for the refill.
func (cn *PeerConn) computePlaybackPipelineLimit() maxRequests {
	tuning := currentSeekTuning()
	if tuning.PipelineRTT > 0 {
		rtt, rate := cn.webtorRTT.estimate, cn.downloadRate()
		if rtt <= 0 || rate <= 0 || cn.t.requestFocus.Load() == 0 || cn.t.chunkSize <= 0 {
			return 0
		}
		n := int(rate*rtt.Seconds()*tuning.PipelineRTT/float64(cn.t.chunkSize)) + max(tuning.MinPipeline, 2)
		return maxRequests(n)
	}
	if tuning.PipelineSeconds <= 0 || cn.t.requestFocus.Load() == 0 || cn.t.chunkSize <= 0 {
		return 0
	}
	n := int(cn.downloadRate()*tuning.PipelineSeconds/float64(cn.t.chunkSize)) + 1
	return maxRequests(max(n, tuning.MinPipeline, 1))
}

// RefocusRequests restarts every peer's requests from the playhead after a
// seek. A peer serves requests in the order they were sent, so blocks queued
// for the old position would all arrive before the one the player now waits
// on, and a full queue could not even take it. Every outstanding request
// outside the urgent (Now) pieces is cancelled, then peers refill fastest
// first: the urgent blocks go to the fastest peers and the rest follows in
// order from the playhead.
func (t *Torrent) RefocusRequests() {
	t.cl.lock()
	defer t.cl.unlock()
	if t.closed.IsSet() || !t.haveInfo() || t.dataDownloadDisallowed.Bool() {
		return
	}
	full := currentSeekTuning().FullPipeline
	conns := make([]*PeerConn, 0, len(t.conns))
	for c := range t.conns {
		if c.closed.IsSet() {
			continue
		}
		peak := c.peakRequests
		c.requestState.Requests.IterateSnapshot(func(r RequestIndex) bool {
			if t.piece(t.pieceIndexOfRequestIndex(r)).purePriority() < PiecePriorityNow {
				c.cancel(r)
			}
			return true
		})
		if full {
			// Moving to the new place is not a sign of a slow peer.
			c.peakRequests = peak
		}
		conns = append(conns, c)
	}
	if currentSeekTuning().ArrivalAware {
		// Soonest arrival first: the first to refill takes the playhead.
		slices.SortFunc(conns, func(a, b *PeerConn) int {
			return cmp.Compare(a.expectedArrival(), b.expectedArrival())
		})
	} else {
		slices.SortFunc(conns, func(a, b *PeerConn) int {
			return cmp.Compare(b.downloadRate(), a.downloadRate())
		})
	}
	for _, c := range conns {
		c.updateRequestsWithReason(peerUpdateRequestsRefocus)
	}
}

// pendingRequests is what counts against a peer's request limit.
func (p *PeerConn) pendingRequests() maxRequests {
	n := p.requestState.Requests.GetCardinality()
	if !currentSeekTuning().FullPipeline {
		n += p.requestState.Cancelled.GetCardinality()
	}
	return maxRequests(n)
}

// urgentPastCancels reports whether p may send r even though its pipeline is
// full of cancels awaiting their reject. The peer reads our cancels before the
// new request, so its queue holds no more than our uncancelled requests; only
// urgent blocks use this, so a refocus does not wait a round trip for rejects.
// Called with the client lock held.
func (p *PeerConn) urgentPastCancels(r RequestIndex) bool {
	if maxRequests(p.requestState.Requests.GetCardinality()) >= p.nominalMaxRequests() {
		return false
	}
	t := p.t
	return t.piece(t.pieceIndexOfRequestIndex(r)).purePriority() >= PiecePriorityNow
}

// rttEstimator measures a peer's round trip from requests sent while nothing
// else was queued there, so the time is the trip and one block, not a queue.
type rttEstimator struct {
	probe    RequestIndex
	probeAt  time.Time
	estimate time.Duration
}

func (e *rttEstimator) sent(r RequestIndex) {
	// A probe whose block never came (cancelled, rejected) is replaced.
	if e.probeAt.IsZero() || time.Since(e.probeAt) > 10*time.Second {
		e.probe, e.probeAt = r, time.Now()
	}
}

func (e *rttEstimator) received(r RequestIndex) {
	if e.probeAt.IsZero() || r != e.probe {
		return
	}
	sample := time.Since(e.probeAt)
	e.probeAt = time.Time{}
	if e.estimate == 0 {
		e.estimate = sample
	} else {
		e.estimate += (sample - e.estimate) / 8
	}
}

// WebtorRequestInfo reports a peer's outstanding requests and download rate
// for client callbacks, which run with the client lock held. Diagnostics only.
func (p *Peer) WebtorRequestInfo() (pending int, bytesPerSecond float64) {
	if pc, ok := p.peerImpl.(*PeerConn); ok {
		pending = int(pc.requestState.Requests.GetCardinality())
	}
	return pending, p.downloadRate()
}

// expectedArrival is how many seconds until a block asked of c now would
// arrive: its round trip, plus its outstanding blocks and this one at its
// rate. A peer with no measured rate never wins; one with no round trip yet
// is assumed 200 ms away.
func (c *PeerConn) expectedArrival() float64 {
	rate := c.downloadRate()
	if rate <= 0 {
		return math.MaxFloat64
	}
	rtt := c.webtorRTT.estimate.Seconds()
	if rtt <= 0 {
		rtt = 0.2
	}
	queued := float64(c.requestState.Requests.GetCardinality() + 1)
	return rtt + queued*float64(c.t.chunkSize)/rate
}

// WebtorEstimates reports what peer choice sees of p. Diagnostics only;
// called with the client lock held.
func (p *Peer) WebtorEstimates() (rtt time.Duration, rate float64, queued, cancelled int) {
	if pc, ok := p.peerImpl.(*PeerConn); ok {
		return pc.webtorRTT.estimate, pc.downloadRate(), int(pc.requestState.Requests.GetCardinality()), int(pc.requestState.Cancelled.GetCardinality())
	}
	return 0, p.downloadRate(), 0, 0
}

// webtorWakeWebseedsForPiece replans web seed requests at once when a piece
// becomes urgent, as after a seek. Web seeds otherwise replan only on a 5 s
// timer or when a request ends, and kept fetching the old position. Called
// with the client lock held.
func (t *Torrent) webtorWakeWebseedsForPiece(piece pieceIndex) {
	if len(t.webSeeds) == 0 || t.closed.IsSet() || t.dataDownloadDisallowed.IsSet() {
		return
	}
	if t.pieceComplete(piece) || t.piece(piece).purePriority() < PiecePriorityNow {
		return
	}
	if t.cl.webseedRequestTimer != nil {
		t.cl.scheduleImmediateWebseedRequestUpdate("webtor urgent piece")
	}
}

// SetPlaybackInterest names the pieces of the file being played, [begin, end).
// The library declares interest only in peers holding a piece it would request
// now, and during playback that is the window around the playhead. A peer with
// only other parts of the file then never heard we were interested and kept us
// choked; after a seek there, we waited to be let in, up to its next rechoke
// (about 10 s in common clients). Now we stay interested, and reciprocity
// keeps us unchoked.
func (t *Torrent) SetPlaybackInterest(begin, end int) {
	if begin >= end {
		t.webtorInterestBegin.Store(0)
		t.webtorInterestEnd.Store(0)
		return
	}
	t.webtorInterestBegin.Store(int64(begin) + 1)
	t.webtorInterestEnd.Store(int64(end) + 1)
}

// webtorHasPlayingPieceWeLack reports whether the peer has a piece of the
// playing file that we lack. Called with the client lock held.
func (p *PeerConn) webtorHasPlayingPieceWeLack() bool {
	t := p.t
	begin, end := t.webtorInterestBegin.Load()-1, t.webtorInterestEnd.Load()-1
	if begin < 0 || end <= begin || !t.haveInfo() || t.dataDownloadDisallowed.IsSet() {
		return false
	}
	if all, known := p.peerHasAllPieces(); all && known {
		for i := begin; i < min(end, int64(t.numPieces())); i++ {
			if !t._completedPieces.Contains(uint32(i)) {
				return true
			}
		}
		return false
	}
	it := p.peerPieces().Iterator()
	it.AdvanceIfNeeded(uint32(begin))
	for it.HasNext() {
		i := it.Next()
		if int64(i) >= end {
			return false
		}
		if !t._completedPieces.Contains(i) {
			return true
		}
	}
	return false
}

// WebtorPeerCounts returns the connected peers and how many of them let us
// download (have unchoked us).
func (t *Torrent) WebtorPeerCounts() (conns, unchoked int) {
	t.cl.rLock()
	defer t.cl.rUnlock()
	for c := range t.conns {
		if c.closed.IsSet() {
			continue
		}
		conns++
		if !c.peerChoking {
			unchoked++
		}
	}
	return
}

// holWindowBlocks bounds the head-of-line search: 2 MiB of 16 KiB blocks,
// about what a player buffers before it resumes.
const holWindowBlocks = 128

// webtorBlockArrived reports whether block r has been received. Called with
// the client lock held.
func (t *Torrent) webtorBlockArrived(r RequestIndex) bool {
	return t.dirtyChunks.Contains(r) || t.pieceComplete(t.pieceIndexOfRequestIndex(r))
}

// webtorHeadOfLine returns the first block at or after the playhead that
// has not arrived: the one the player's stream waits on. Called with the
// client lock held.
func (t *Torrent) webtorHeadOfLine() (RequestIndex, bool) {
	focus, ok := t.requestFocusIndex()
	if !ok {
		return 0, false
	}
	for r := focus; r < min(focus+holWindowBlocks, t.maxEndRequest()); r++ {
		if !t.webtorBlockArrived(r) {
			return r, true
		}
	}
	return 0, false
}

// holRescuesPerNudge bounds the blocks one nudge moves.
const holRescuesPerNudge = 16

// webtorRescueHeadOfLine moves every overdue block of the first 2 MiB after
// the playhead that has not arrived, from the head of line on, each to the
// peer expected to deliver it soonest if that is sooner than its holder (see
// HOLRescue). On the phone about three blocks were stuck per seek; moving
// only the first, then waiting for the next to be overdue, chained the waits.
// A refilling peer takes unclaimed blocks first and stops at its limit, so it
// never reached a block another peer holds; the rescue picks the peer itself.
// Called with the client lock held, from the nudger while a read waits.
// Returns the number of blocks moved.
func (t *Torrent) webtorRescueHeadOfLine() int {
	tuning := currentSeekTuning()
	if !tuning.HOLRescue {
		return 0
	}
	head, ok := t.webtorHeadOfLine()
	if !ok {
		return 0
	}
	focus, _ := t.requestFocusIndex()
	after := time.Duration(tuning.HOLAfterMs * float64(time.Millisecond))
	moved := 0
	for r := head; r < min(focus+holWindowBlocks, t.maxEndRequest()) && moved < holRescuesPerNudge; r++ {
		if t.webtorBlockArrived(r) {
			continue
		}
		holder := t.requestingPeer(r)
		if holder == nil {
			// Not asked for yet: the normal refill asks for it.
			continue
		}
		if time.Since(t.requestState[r].when) < max(after, 2*holder.webtorRTT.estimate) {
			continue
		}
		piece := t.pieceIndexOfRequestIndex(r)
		if t.hashingPiece(piece) || t.pieceQueuedForHash(piece) || t.piece(piece).marking {
			continue
		}
		var best *PeerConn
		for c := range t.conns {
			if c == holder || c.closed.IsSet() || c.peerChoking || !c.peerHasPiece(piece) ||
				c.requestState.Cancelled.Contains(r) || c.pendingRequests() >= c.nominalMaxRequests() {
				continue
			}
			if best == nil || c.expectedArrival() < best.expectedArrival() {
				best = c
			}
		}
		if best == nil || best.expectedArrival() >= holder.expectedArrival() {
			continue
		}
		t.cancelRequest(r)
		best.mustRequest(r)
		t.webtorRescues.Add(1)
		moved++
	}
	return moved
}

// WebtorRescues counts head-of-line blocks moved to a sooner peer.
func (t *Torrent) WebtorRescues() int64 {
	return t.webtorRescues.Load()
}

// WebtorSetResponsive switches whether reads return received blocks before
// their piece is verified (SetResponsive, which cannot be undone). Playback
// turns it on away from the start and end of a video, where players read the
// container index.
func (r *reader) WebtorSetResponsive(on bool) {
	r.t.cl.lock()
	changed := r.responsive != on
	r.responsive = on
	r.t.cl.unlock()
	if changed {
		r.t.cl.event.Broadcast()
	}
}

// hedgesPerNudge bounds the duplicate requests one nudge sends.
const hedgesPerNudge = 16

// webtorHedgeHeadOfLine sends a duplicate request for each overdue block of
// the first 2 MiB after the playhead that has not arrived and is not hedged
// yet, to the peer expected to deliver it soonest (see Hedge). Called with
// the client lock held, from the nudger while a read waits. Returns the
// number of requests sent.
func (t *Torrent) webtorHedgeHeadOfLine() int {
	tuning := currentSeekTuning()
	if !tuning.Hedge {
		return 0
	}
	head, ok := t.webtorHeadOfLine()
	if !ok {
		return 0
	}
	focus, _ := t.requestFocusIndex()
	after := time.Duration(tuning.HOLAfterMs * float64(time.Millisecond))
	sent := 0
	for r := head; r < min(focus+holWindowBlocks, t.maxEndRequest()) && sent < hedgesPerNudge; r++ {
		if t.webtorBlockArrived(r) {
			continue
		}
		holder := t.requestingPeer(r)
		if holder == nil {
			continue
		}
		if h := t.webtorHedges[r]; h != nil && !h.closed.IsSet() {
			continue
		}
		if time.Since(t.requestState[r].when) < max(after, 2*holder.webtorRTT.estimate) {
			continue
		}
		piece := t.pieceIndexOfRequestIndex(r)
		if t.hashingPiece(piece) || t.pieceQueuedForHash(piece) || t.piece(piece).marking {
			continue
		}
		var best *PeerConn
		for c := range t.conns {
			if c == holder || c.closed.IsSet() || c.peerChoking || !c.peerHasPiece(piece) ||
				c.validReceiveChunks[r] > 0 || c.pendingRequests() >= c.nominalMaxRequests() ||
				c.expectedArrival() == math.MaxFloat64 {
				continue
			}
			if best == nil || c.expectedArrival() < best.expectedArrival() {
				best = c
			}
		}
		if best == nil {
			continue
		}
		best.webtorSendHedge(r)
		sent++
	}
	return sent
}

// webtorSendHedge asks c for block r, which another peer holds, outside the
// library's one-peer-per-request bookkeeping. Client lock.
func (c *PeerConn) webtorSendHedge(r RequestIndex) {
	t := c.t
	if t.webtorHedges == nil {
		t.webtorHedges = make(map[RequestIndex]*PeerConn)
	}
	if len(t.webtorHedges) > 4096 {
		// A peer that drops a cancelled request without a reject leaves
		// its entry behind; forget those whose block is in.
		for b, h := range t.webtorHedges {
			if h.closed.IsSet() || t.webtorBlockArrived(b) {
				delete(t.webtorHedges, b)
			}
		}
	}
	t.webtorHedges[r] = c
	if c.validReceiveChunks == nil {
		c.validReceiveChunks = make(map[RequestIndex]int)
	}
	c.validReceiveChunks[r]++
	c._request(t.requestIndexToRequest(r))
	t.webtorHedgeSent.Add(1)
}

// webtorHedgeAnswered reports whether r was hedged with c, and forgets it:
// c sent the block or rejected the request. Client lock.
func (c *PeerConn) webtorHedgeAnswered(r RequestIndex) bool {
	if c.t.webtorHedges[r] != c {
		return false
	}
	delete(c.t.webtorHedges, r)
	return true
}

// webtorCancelHedge cancels the hedge for r once another peer delivered it.
// The entry stays until the peer rejects or sends the block anyway, so
// neither is taken for a protocol error. Client lock.
func (t *Torrent) webtorCancelHedge(r RequestIndex, from *Peer) {
	h := t.webtorHedges[r]
	if h == nil || h.peerPtr() == from || h.closed.IsSet() {
		return
	}
	h.write(makeCancelMessage(t.requestIndexToRequest(r)))
}

// WebtorHedges counts duplicate requests sent for head-of-line blocks.
func (t *Torrent) WebtorHedges() int64 {
	return t.webtorHedgeSent.Load()
}

// slowPeerBytesPerSec is three blocks a second, WebTorrent's SPEED_THRESHOLD.
const slowPeerBytesPerSec = 3 * 16 << 10

// slowPeerYieldsPerRefill bounds the pieces one refill passes over, as
// WebTorrent tries 10, so a slow peer still finds work.
const slowPeerYieldsPerRefill = 10

// webtorYieldToFasterPeers reports whether p, a slow peer, should leave the
// playback piece to faster peers (see SlowPeerYield). Client lock.
func (p *PeerConn) webtorYieldToFasterPeers(piece pieceIndex) bool {
	speed := p.downloadRate()
	if speed >= slowPeerBytesPerSec {
		return false
	}
	speed = max(speed, 1)
	t := p.t
	// How long p needs for what it has queued, at least one block.
	secs := float64(max(p.requestState.Requests.GetCardinality(), 1)) * float64(t.chunkSize) / speed
	missing := float64(t.piece(piece).bytesLeft())
	for c := range t.conns {
		if c == p || c.closed.IsSet() || c.peerChoking || !c.peerHasPiece(piece) {
			continue
		}
		other := c.downloadRate()
		if other < slowPeerBytesPerSec || other <= speed {
			continue
		}
		if missing -= other * secs; missing <= 0 {
			return true
		}
	}
	return false
}

// WebtorDropChokers disconnects peers that kept us choked for after while
// we wanted their data, when more peers wait for a connection than twice
// the free slots (WebTorrent and torrent-stream's CHOKE_TIMEOUT). A peer
// that never unchokes us otherwise holds its slot. At most two per call.
// Returns the number dropped.
func (t *Torrent) WebtorDropChokers(after time.Duration) int {
	t.cl.lock()
	defer t.cl.unlock()
	if t.closed.IsSet() || t.dataDownloadDisallowed.IsSet() {
		return 0
	}
	if t.peers.Len() <= 2*max(t.maxEstablishedConns-len(t.conns), 0) {
		return 0
	}
	var drop []*PeerConn
	for c := range t.conns {
		if len(drop) == 2 {
			break
		}
		if c.closed.IsSet() || !c.peerChoking || !c.requestState.Interested {
			continue
		}
		since := c.webtorChokedSince
		if c.lastBecameInterested.After(since) {
			since = c.lastBecameInterested
		}
		if !since.IsZero() && time.Since(since) >= after {
			drop = append(drop, c)
		}
	}
	for _, c := range drop {
		c.drop()
		t.webtorChokersDropped.Add(1)
	}
	return len(drop)
}

// WebtorChokersDropped counts peers dropped by WebtorDropChokers.
func (t *Torrent) WebtorChokersDropped() int64 {
	return t.webtorChokersDropped.Load()
}
