package main

import (
	"context"
	"fmt"
	"io"
	"sync"
	"time"

	"github.com/anacrolix/torrent"
)

// Streaming piece orchestration:
//   - Tip (urgent): PiecePriorityNow — peers focus here for resume-after-seek
//   - Forward readahead: PiecePriorityReadahead — sequential within the window
//   - Lookbehind + seek hotspots: PiecePriorityHigh — warm recent timestamps
//
// Flat Now across the whole window diluted urgency and lost readahead's
// sequential request order, so seeks competed with far-ahead pieces.
const (
	playbackUrgentBytes        = int64(2 * 1024 * 1024)
	playbackReadaheadBytes     = int64(24 * 1024 * 1024)
	playbackFastReadaheadBytes = int64(6 * 1024 * 1024)
	playbackLookbehindBytes    = int64(512 * 1024)
	playbackSeekHotspotBytes   = int64(2 * 1024 * 1024)
	playbackFastSeekJumpBytes  = int64(8 * 1024 * 1024)
	playbackMaxSeekHotspots    = 4
	playbackSeekSettleReads    = 3
	// Kept for HTTP ServeContent and as the settled forward span.
	playbackWindowBytes = playbackUrgentBytes + playbackReadaheadBytes

	// Background download hints for selected videos (also used by /play and
	// OpenPlayback head/tail boost). High, not Now, so an active player still wins.
	videoStartupHeadBytes = int64(5 * 1024 * 1024)
	videoStartupTailBytes = int64(3 * 1024 * 1024)
)

type pieceSpan struct {
	begin, end int // half-open piece indices
}

func (s pieceSpan) empty() bool { return s.begin >= s.end }

func (s pieceSpan) contains(i int) bool { return i >= s.begin && i < s.end }

func (s pieceSpan) overlaps(o pieceSpan) bool {
	return !s.empty() && !o.empty() && s.begin < o.end && o.begin < s.end
}

type seekHotspot struct {
	span   pieceSpan
	offset int64
}

// playbackPriorityCoordinator owns the torrent-global piece priorities used
// by playback. anacrolix exposes one priority per piece, while Android can
// have several media readers for a torrent (player probes, range requests,
// and a second player handle). Keep one demand per reader and apply the
// maximum demand across readers. A closed reader can therefore release only
// its own demand.
type playbackPriorityCoordinator struct {
	mu        sync.Mutex
	torrent   *torrent.Torrent
	owners    map[*playbackReader]map[int]torrent.PiecePriority
	effective map[int]torrent.PiecePriority
}

func newPlaybackPriorityCoordinator(t *torrent.Torrent) *playbackPriorityCoordinator {
	return &playbackPriorityCoordinator{
		torrent:   t,
		owners:    make(map[*playbackReader]map[int]torrent.PiecePriority),
		effective: make(map[int]torrent.PiecePriority),
	}
}

func (c *playbackPriorityCoordinator) update(owner *playbackReader, demand map[int]torrent.PiecePriority) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if len(demand) == 0 {
		delete(c.owners, owner)
	} else {
		copyDemand := make(map[int]torrent.PiecePriority, len(demand))
		for index, priority := range demand {
			copyDemand[index] = priority
		}
		c.owners[owner] = copyDemand
	}
	c.applyLocked()
}

func (c *playbackPriorityCoordinator) remove(owner *playbackReader) {
	c.mu.Lock()
	defer c.mu.Unlock()
	delete(c.owners, owner)
	c.applyLocked()
}

func (c *playbackPriorityCoordinator) applyLocked() {
	next := make(map[int]torrent.PiecePriority)
	for _, demand := range c.owners {
		for index, priority := range demand {
			if current, ok := next[index]; !ok || priority > current {
				next[index] = priority
			}
		}
	}
	changed := make(map[int]struct{}, len(c.effective)+len(next))
	for index := range c.effective {
		changed[index] = struct{}{}
	}
	for index := range next {
		changed[index] = struct{}{}
	}
	for index := range changed {
		priority := next[index]
		if previous, ok := c.effective[index]; ok && previous == priority {
			continue
		}
		// None deliberately clears only the playback-owned piece override;
		// the selected file's base priority remains authoritative.
		c.torrent.Piece(index).SetPriority(priority)
	}
	c.effective = next
}

// Each Android proxy descriptor owns a reader. Its cursor/readahead survive
// successive reads, and closing the descriptor cancels any blocked read.
type playbackReader struct {
	mu      sync.Mutex
	reader  torrent.Reader
	torrent *torrent.Torrent
	file    *torrent.File
	length  int64
	ctx     context.Context
	cancel  context.CancelFunc
	closed  bool

	positioned bool
	cursor     int64
	lastOffset int64
	haveOffset bool

	urgent     pieceSpan
	readahead  pieceSpan
	lookbehind pieceSpan
	hotspots   []seekHotspot

	headBegin, headEnd int
	tailBegin, tailEnd int
	headBoostActive    bool
	tailBoostActive    bool

	fastSeek      bool
	settleReads   int
	priorityOwner *playbackPriorityCoordinator
	// refreshCompletion is set for Watch/stream storage so discarded cache
	// pieces are re-queued when the playhead returns to them.
	refreshCompletion bool
}

func (s *EngineServer) OpenPlayback(id string, index int) (*playbackReader, error) {
	s.mu.RLock()
	rec := s.records[id]
	s.mu.RUnlock()
	if rec == nil {
		return nil, fmt.Errorf("torrent not found")
	}
	rec.mu.RLock()
	defer rec.mu.RUnlock()
	files := rec.Torrent.Files()
	selected := false
	for _, i := range rec.Selected {
		if i == index {
			selected = true
		}
	}
	if index < 0 || index >= len(files) || !rec.Configured || (!selected && !fileIsVerified(rec.Torrent, files[index])) {
		return nil, fmt.Errorf("file is not configured for playback")
	}
	file := files[index]
	reader := file.NewReader()
	reader.SetReadaheadFunc(nil)
	reader.SetReadahead(playbackWindowBytes)
	ctx, cancel := context.WithCancel(context.Background())
	p := &playbackReader{
		reader:  reader,
		torrent: rec.Torrent,
		file:    file,
		length:  file.Length(),
		ctx:     ctx,
		cancel:  cancel,
	}
	if st := s.storage.GetStorage(rec.InfoHash); st != nil && st.StreamMode() {
		p.refreshCompletion = true
	}
	s.playbackMu.Lock()
	if s.playbackOwners == nil {
		s.playbackOwners = make(map[*torrent.Torrent]*playbackPriorityCoordinator)
	}
	coordinator := s.playbackOwners[rec.Torrent]
	if coordinator == nil {
		coordinator = newPlaybackPriorityCoordinator(rec.Torrent)
		s.playbackOwners[rec.Torrent] = coordinator
	}
	s.playbackMu.Unlock()
	p.priorityOwner = coordinator
	p.headBegin, p.headEnd = filePieceRange(file, 0, minInt64(file.Length(), videoStartupHeadBytes))
	p.headBoostActive = p.headBegin < p.headEnd
	if file.Length() > videoStartupTailBytes {
		p.tailBegin, p.tailEnd = filePieceRange(file, file.Length()-videoStartupTailBytes, file.Length())
		p.tailBoostActive = p.tailBegin < p.tailEnd
	}
	p.updateWindowLocked(0)
	return p, nil
}

func (p *playbackReader) ReadAt(data []byte, offset int64) (int, error) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.closed {
		return 0, fmt.Errorf("playback closed")
	}
	if offset < 0 {
		return 0, fmt.Errorf("invalid playback offset")
	}
	if offset >= p.length || len(data) == 0 {
		return 0, nil
	}
	data = data[:min(int64(len(data)), p.length-offset)]
	p.updateWindowLocked(offset)
	ctx, cancel := context.WithTimeout(p.ctx, 45*time.Second)
	defer cancel()
	p.reader.SetContext(ctx)
	if !p.positioned || offset != p.cursor {
		if _, err := p.reader.Seek(offset, io.SeekStart); err != nil {
			return 0, err
		}
		p.positioned = true
	}
	// Android's proxy callback requires the requested length except at EOF.
	// Never translate missing torrent bytes into zeros or premature EOF.
	n, err := io.ReadFull(p.reader, data)
	if n > 0 {
		p.cursor = offset + int64(n)
	}
	return n, err
}

func (p *playbackReader) Close() {
	p.cancel()
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.closed {
		return
	}
	p.closed = true
	p.clearPrioritiesLocked()
	_ = p.reader.Close()
}

func (p *playbackReader) clearPrioritiesLocked() {
	p.urgent, p.readahead, p.lookbehind = pieceSpan{}, pieceSpan{}, pieceSpan{}
	p.hotspots = nil
	p.headBoostActive, p.tailBoostActive = false, false
	if p.priorityOwner != nil {
		p.priorityOwner.remove(p)
	}
}

// updateWindowLocked retargets peer piece priorities around the playhead.
// Large jumps enter fast-seek mode (tight tip + short readahead) until a few
// near-sequential reads settle; prior positions are kept as High hotspots so
// switching between recent timestamps stays warm.
func (p *playbackReader) updateWindowLocked(offset int64) {
	if p.torrent == nil || p.file == nil || p.length <= 0 {
		return
	}
	if offset < 0 {
		offset = 0
	}
	if offset >= p.length {
		offset = p.length - 1
	}

	wasPositioned := p.haveOffset
	if p.haveOffset {
		delta := offset - p.lastOffset
		if delta < 0 {
			delta = -delta
		}
		if delta >= playbackFastSeekJumpBytes {
			p.rememberHotspotLocked(p.lastOffset)
			p.fastSeek = true
			p.settleReads = 0
		} else if delta <= playbackUrgentBytes {
			if p.fastSeek {
				p.settleReads++
				if p.settleReads >= playbackSeekSettleReads {
					p.fastSeek = false
					p.settleReads = 0
				}
			}
		} else {
			// Medium jump: refresh tip urgently; keep settled readahead size
			// unless already scrubbing fast.
			p.rememberHotspotLocked(p.lastOffset)
			if p.fastSeek {
				p.settleReads = 0
			}
		}
	}
	p.lastOffset = offset
	p.haveOffset = true

	forward := playbackUrgentBytes + playbackReadaheadBytes
	if p.fastSeek {
		forward = playbackUrgentBytes + playbackFastReadaheadBytes
	}
	p.reader.SetReadahead(forward)

	urgentEndOff := offset + playbackUrgentBytes
	if urgentEndOff > p.length {
		urgentEndOff = p.length
	}
	forwardEnd := offset + forward
	if forwardEnd > p.length {
		forwardEnd = p.length
	}
	lookStart := offset - playbackLookbehindBytes
	if lookStart < 0 {
		lookStart = 0
	}

	newUrgent := spanFromFile(p.file, offset, urgentEndOff)
	newReadahead := spanFromFile(p.file, urgentEndOff, forwardEnd)
	newLookbehind := spanFromFile(p.file, lookStart, offset)

	kept := map[int]torrent.PiecePriority{}
	raiseSpan(kept, newUrgent, torrent.PiecePriorityNow)
	raiseSpan(kept, newReadahead, torrent.PiecePriorityReadahead)
	raiseSpan(kept, newLookbehind, torrent.PiecePriorityHigh)
	for _, h := range p.hotspots {
		raiseSpan(kept, h.span, torrent.PiecePriorityHigh)
	}
	if p.headBoostActive {
		raiseSpan(kept, pieceSpan{p.headBegin, p.headEnd}, torrent.PiecePriorityHigh)
	}
	if p.tailBoostActive {
		raiseSpan(kept, pieceSpan{p.tailBegin, p.tailEnd}, torrent.PiecePriorityHigh)
	}

	playhead := unionSpan(newUrgent, newReadahead, newLookbehind)
	// Drop expired head/tail demands before publishing this reader's demand.
	if wasPositioned {
		p.demoteBoostOutsideLocked(kept, playhead)
	}
	kept = p.currentDemandLocked(newUrgent, newReadahead, newLookbehind)
	if p.priorityOwner != nil {
		p.priorityOwner.update(p, kept)
	}
	if p.refreshCompletion {
		// Watch-mode cache trim may have punched pieces; refresh torrent
		// completion for the active window so discarded pieces are requested again.
		for i := range kept {
			p.torrent.Piece(i).UpdateCompletion()
		}
	}

	p.urgent, p.readahead, p.lookbehind = newUrgent, newReadahead, newLookbehind
}

func (p *playbackReader) rememberHotspotLocked(offset int64) {
	if p.file == nil || p.length <= 0 {
		return
	}
	start := offset
	if start < 0 {
		start = 0
	}
	end := start + playbackSeekHotspotBytes
	if end > p.length {
		end = p.length
	}
	span := spanFromFile(p.file, start, end)
	if span.empty() {
		return
	}
	// Drop an existing hotspot that overlaps heavily with this one.
	filtered := p.hotspots[:0]
	for _, h := range p.hotspots {
		if h.span.overlaps(span) {
			continue
		}
		filtered = append(filtered, h)
	}
	p.hotspots = append(filtered, seekHotspot{span: span, offset: start})
	for len(p.hotspots) > playbackMaxSeekHotspots {
		evict := p.hotspots[0]
		p.hotspots = p.hotspots[1:]
		// Evicted pieces may still be in the active window; next update reapplies.
		inWindow := false
		for i := evict.span.begin; i < evict.span.end; i++ {
			if p.urgent.contains(i) || p.readahead.contains(i) || p.lookbehind.contains(i) {
				inWindow = true
				break
			}
			for _, h := range p.hotspots {
				if h.span.contains(i) {
					inWindow = true
					break
				}
			}
			if inWindow {
				break
			}
		}
		_ = inWindow // the next coordinator update computes the effective demand.
	}
}

func (p *playbackReader) demoteBoostOutsideLocked(kept map[int]torrent.PiecePriority, playhead pieceSpan) {
	demote := func(begin, end int, active *bool) {
		if !*active || begin >= end {
			return
		}
		if playhead.overlaps(pieceSpan{begin, end}) {
			return
		}
		*active = false
	}
	demote(p.headBegin, p.headEnd, &p.headBoostActive)
	demote(p.tailBegin, p.tailEnd, &p.tailBoostActive)
}

func (p *playbackReader) currentDemandLocked(urgent, readahead, lookbehind pieceSpan) map[int]torrent.PiecePriority {
	demand := make(map[int]torrent.PiecePriority)
	raiseSpan(demand, urgent, torrent.PiecePriorityNow)
	raiseSpan(demand, readahead, torrent.PiecePriorityReadahead)
	raiseSpan(demand, lookbehind, torrent.PiecePriorityHigh)
	for _, h := range p.hotspots {
		raiseSpan(demand, h.span, torrent.PiecePriorityHigh)
	}
	if p.headBoostActive {
		raiseSpan(demand, pieceSpan{p.headBegin, p.headEnd}, torrent.PiecePriorityHigh)
	}
	if p.tailBoostActive {
		raiseSpan(demand, pieceSpan{p.tailBegin, p.tailEnd}, torrent.PiecePriorityHigh)
	}
	return demand
}

func spanFromFile(file *torrent.File, start, end int64) pieceSpan {
	b, e := filePieceRange(file, start, end)
	return pieceSpan{b, e}
}

func raiseSpan(dst map[int]torrent.PiecePriority, span pieceSpan, prio torrent.PiecePriority) {
	for i := span.begin; i < span.end; i++ {
		if cur, ok := dst[i]; !ok || prio > cur {
			dst[i] = prio
		}
	}
}

func collectSpan(dst map[int]struct{}, span pieceSpan) {
	for i := span.begin; i < span.end; i++ {
		dst[i] = struct{}{}
	}
}

func unionSpan(spans ...pieceSpan) pieceSpan {
	out := pieceSpan{}
	first := true
	for _, s := range spans {
		if s.empty() {
			continue
		}
		if first {
			out = s
			first = false
			continue
		}
		if s.begin < out.begin {
			out.begin = s.begin
		}
		if s.end > out.end {
			out.end = s.end
		}
	}
	return out
}

func rangesOverlap(a0, a1, b0, b1 int) bool {
	return a0 < a1 && b0 < b1 && a0 < b1 && b0 < a1
}

// testApplyOffset retargets the playhead window without blocking on torrent bytes.
func (p *playbackReader) testApplyOffset(offset int64) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.updateWindowLocked(offset)
}

// testWindow reports active playhead spans for tests.
func (p *playbackReader) testWindow() (urgent, readahead, lookbehind pieceSpan, hotspots int, fastSeek, headBoost, tailBoost bool) {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.urgent, p.readahead, p.lookbehind, len(p.hotspots), p.fastSeek, p.headBoostActive, p.tailBoostActive
}

// testPiecePriority reports this reader's demand for deterministic window
// tests. The coordinator, rather than this per-reader view, owns the actual
// torrent-global priority when multiple handles are open.
func (p *playbackReader) testPiecePriority(i int) torrent.PiecePriority {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.urgent.contains(i) {
		return torrent.PiecePriorityNow
	}
	if p.readahead.contains(i) {
		return torrent.PiecePriorityReadahead
	}
	if p.lookbehind.contains(i) {
		return torrent.PiecePriorityHigh
	}
	for _, h := range p.hotspots {
		if h.span.contains(i) {
			return torrent.PiecePriorityHigh
		}
	}
	return torrent.PiecePriorityNormal
}

// filePieceRange maps a half-open byte range within a file to torrent piece indices [begin, end).
func filePieceRange(file *torrent.File, start, end int64) (begin, endPiece int) {
	if file == nil || start >= end {
		return 0, 0
	}
	t := file.Torrent()
	if t == nil || t.Info() == nil || t.Info().PieceLength <= 0 {
		return 0, 0
	}
	pieceLen := t.Info().PieceLength
	fileOff := file.Offset()
	fileLen := file.Length()
	if start < 0 {
		start = 0
	}
	if end > fileLen {
		end = fileLen
	}
	if start >= end {
		return 0, 0
	}
	absStart := fileOff + start
	absEnd := fileOff + end
	begin = int(absStart / pieceLen)
	endPiece = int((absEnd-1)/pieceLen) + 1
	if begin < file.BeginPieceIndex() {
		begin = file.BeginPieceIndex()
	}
	if endPiece > file.EndPieceIndex() {
		endPiece = file.EndPieceIndex()
	}
	if begin > endPiece {
		return endPiece, endPiece
	}
	return begin, endPiece
}
