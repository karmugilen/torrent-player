package main

import (
	"context"
	"errors"
	"fmt"
	"io"
	"maps"
	"sync"
	"time"

	"github.com/anacrolix/torrent"
)

// Streaming piece orchestration:
//   - This block: the playhead's 2 MiB at PiecePriorityNow, fetched first
//   - Next blocks: the rest of the window at PiecePriorityNext, one after
//     another from the playhead (the patched library orders them by distance)
//   - Nothing behind the playhead is fetched
//   - Open phase: Head (~5MB) and Tail (~3MB) at PiecePriorityNow, hold fat readahead
//   - Scrubbing: Tip (~2MB) at PiecePriorityNow + full forward readahead immediately
const (
	playbackUrgentBytes    = int64(2 * 1024 * 1024)
	playbackReadaheadBytes = int64(24 * 1024 * 1024)
	playbackScrubJumpBytes = int64(2 * 1024 * 1024)
	playbackScrubDebounce  = 250 * time.Millisecond

	// Kept for HTTP ServeContent and as the settled forward span.
	playbackWindowBytes = playbackUrgentBytes + playbackReadaheadBytes

	// Startup ranges are Now while preparing a download and High while a
	// reader is active; its own open-phase metadata probes remain Now.
	videoStartupHeadBytes = int64(5 * 1024 * 1024)
	videoStartupTailBytes = int64(3 * 1024 * 1024)

	// PlayRequest.Boost only. Default plays keep the constants above.
	playbackBoostHeadBytes      = int64(16 * 1024 * 1024)
	playbackBoostTailBytes      = int64(8 * 1024 * 1024)
	playbackBoostReadaheadBytes = int64(48 * 1024 * 1024)
)

type pieceSpan struct {
	begin, end int // half-open piece indices
}

func (s pieceSpan) empty() bool { return s.begin >= s.end }

func (s pieceSpan) contains(i int) bool { return i >= s.begin && i < s.end }

func (s pieceSpan) overlaps(o pieceSpan) bool {
	return !s.empty() && !o.empty() && s.begin < o.end && o.begin < s.end
}

// playbackPriorityCoordinator owns the torrent-global piece priorities used
// by playback. anacrolix exposes one priority per piece, while Android can
// have several media readers for a torrent (player probes, range requests,
// and a second player handle). Keep one demand per reader and apply the
// maximum demand across readers. A closed reader can therefore release only
// its own demand.
type playbackPriorityCoordinator struct {
	mu               sync.Mutex
	torrent          *torrent.Torrent
	server           *EngineServer
	owners           map[*playbackReader]map[int]torrent.PiecePriority
	windows          map[*playbackReader]playbackWindowState
	hints            map[int]torrent.PiecePriority
	focus            map[int]torrent.PiecePriority
	effective        map[int]torrent.PiecePriority
	storage          *DocumentTorrentStorage
	readers          map[*playbackReader]struct{}
	resumeFile       *torrent.File
	resumeReader     *playbackReader
	resumeOffset     int64
	resumeDemand     map[int]torrent.PiecePriority
	resumeTimer      *time.Timer
	resumeGeneration uint64
	previewDemand    map[int]torrent.PiecePriority
	closed           bool
	resumeSuspended  bool
	// playBoost is the latest /play boost flag per file index. Readers copy it
	// at open so a later play does not resize an already-open window.
	playBoost       map[int]bool
	adaptiveBuffers map[int]bool
	burstBuffers    map[int]bool
	// playing is the latest reader past its open phase. It keeps its window
	// while idle: an external player stops reading when paused or when its
	// own buffer is full, which is when the engine can fill the buffer ahead.
	playing *playbackReader
	// active is, per file, the reader that last opened, seeked or moved to
	// another piece. Only it is fetched for: the others of that file are
	// parked, so an earlier seek position stops downloading.
	active map[*torrent.File]*playbackReader
	// pendingShareRefresh is set when the number of readers with a window
	// changes. The caller resizes every reader after it releases its own lock.
	pendingShareRefresh bool
}

func newPlaybackPriorityCoordinator(t *torrent.Torrent) *playbackPriorityCoordinator {
	return &playbackPriorityCoordinator{
		torrent:   t,
		readers:   make(map[*playbackReader]struct{}),
		active:    make(map[*torrent.File]*playbackReader),
		owners:    make(map[*playbackReader]map[int]torrent.PiecePriority),
		windows:   make(map[*playbackReader]playbackWindowState),
		effective: make(map[int]torrent.PiecePriority),
	}
}

// setPlayBoost records whether the next reader opened for fileIndex should use
// the larger startup window. A false play clears a previous boost for that file.
func (c *playbackPriorityCoordinator) setPlayBoost(fileIndex int, boost bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if boost {
		if c.playBoost == nil {
			c.playBoost = make(map[int]bool)
		}
		c.playBoost[fileIndex] = true
		return
	}
	delete(c.playBoost, fileIndex)
}

func (c *playbackPriorityCoordinator) setBufferExperiments(fileIndex int, adaptive, burst bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if adaptive {
		if c.adaptiveBuffers == nil {
			c.adaptiveBuffers = make(map[int]bool)
		}
		c.adaptiveBuffers[fileIndex] = true
	} else {
		delete(c.adaptiveBuffers, fileIndex)
	}
	if burst {
		if c.burstBuffers == nil {
			c.burstBuffers = make(map[int]bool)
		}
		c.burstBuffers[fileIndex] = true
	} else {
		delete(c.burstBuffers, fileIndex)
	}
}

func (c *playbackPriorityCoordinator) update(owner *playbackReader, demand map[int]torrent.PiecePriority, window playbackWindowState) {
	c.mu.Lock()
	defer c.mu.Unlock()
	before := len(c.owners)
	c.windows[owner] = window
	if maps.Equal(c.owners[owner], demand) {
		return
	}
	if len(demand) == 0 {
		delete(c.owners, owner)
	} else {
		copyDemand := make(map[int]torrent.PiecePriority, len(demand))
		for index, priority := range demand {
			copyDemand[index] = priority
		}
		c.owners[owner] = copyDemand
	}
	if len(c.owners) != before {
		c.pendingShareRefresh = true
	}
	c.applyLocked()
}

func (c *playbackPriorityCoordinator) remove(owner *playbackReader) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if _, existed := c.owners[owner]; existed {
		c.pendingShareRefresh = true
	}
	delete(c.owners, owner)
	delete(c.windows, owner)
	c.applyLocked()
}

// flushShareRefresh resizes every reader after the owner count changes.
// The caller must not hold a reader lock: resizing locks each reader in turn.
func (p *playbackReader) flushShareRefresh() {
	p.mu.Lock()
	superseded := p.superseded
	p.superseded = nil
	p.mu.Unlock()
	parkSuperseded(superseded)
	owner := p.priorityOwner
	if owner == nil || owner.server == nil || p.torrent == nil {
		return
	}
	owner.mu.Lock()
	pending := owner.pendingShareRefresh
	owner.pendingShareRefresh = false
	owner.mu.Unlock()
	if pending {
		owner.server.refreshPlaybackWindows(p.torrent)
	}
}

// Snapshot published windows without taking reader locks. Readers acquire the
// coordinator lock while holding their own lock, so observers must not reverse it.
func (s *EngineServer) playbackWindows(t *torrent.Torrent) []playbackWindowState {
	s.playbackMu.Lock()
	c := s.playbackOwners[t]
	s.playbackMu.Unlock()
	if c == nil {
		return nil
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.closed {
		return nil
	}
	windows := make([]playbackWindowState, 0, len(c.windows))
	for _, window := range c.windows {
		windows = append(windows, window)
	}
	return windows
}

func (s *EngineServer) refreshPlaybackWindows(t *torrent.Torrent) {
	s.playbackMu.Lock()
	c := s.playbackOwners[t]
	s.playbackMu.Unlock()
	if c == nil {
		return
	}
	c.mu.Lock()
	readers := make([]*playbackReader, 0, len(c.readers))
	for p := range c.readers {
		readers = append(readers, p)
	}
	c.mu.Unlock()
	for _, p := range readers {
		p.mu.Lock()
		if !p.closed && p.windowApplied {
			p.repositionWindowLocked(p.lastOffset, p.haveOffset)
		}
		p.mu.Unlock()
	}
}

func (c *playbackPriorityCoordinator) setHints(demand map[int]torrent.PiecePriority) {
	c.mu.Lock()
	defer c.mu.Unlock()
	unchanged := maps.Equal(c.hints, demand)
	c.resumeSuspended = len(demand) == 0
	if unchanged && (len(demand) != 0 || len(c.resumeDemand) == 0) {
		return
	}
	if len(demand) == 0 {
		c.clearResumeLocked()
	}
	c.hints = cloneDemand(demand)
	c.applyLocked()
}

func (c *playbackPriorityCoordinator) setFocus(demand map[int]torrent.PiecePriority) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if maps.Equal(c.focus, demand) {
		return
	}
	c.focus = cloneDemand(demand)
	c.applyLocked()
}

func cloneDemand(demand map[int]torrent.PiecePriority) map[int]torrent.PiecePriority {
	if len(demand) == 0 {
		return nil
	}
	out := make(map[int]torrent.PiecePriority, len(demand))
	for index, priority := range demand {
		out[index] = priority
	}
	return out
}

func raiseDemand(dst map[int]torrent.PiecePriority, src map[int]torrent.PiecePriority) {
	for index, priority := range src {
		if current, ok := dst[index]; !ok || priority > current {
			dst[index] = priority
		}
	}
}

func (c *playbackPriorityCoordinator) applyLocked() {
	next := make(map[int]torrent.PiecePriority)
	for _, demand := range c.owners {
		raiseDemand(next, demand)
	}
	// With no player open, an on-screen thumbnail goes first: its few pieces
	// must not queue behind every selected episode's startup head and tail.
	previewFirst := len(c.owners) == 0 && len(c.previewDemand) > 0
	// Background metadata preparation must yield to active playback. Readers
	// retain their own urgent head/tail probes until their open phase ends.
	for index, priority := range c.hints {
		if len(c.owners) > 0 && priority > torrent.PiecePriorityHigh {
			priority = torrent.PiecePriorityHigh
		}
		if previewFirst && priority > torrent.PiecePriorityNext {
			priority = torrent.PiecePriorityNext
		}
		if priority > next[index] {
			next[index] = priority
		}
	}
	raiseDemand(next, c.focus)
	raiseDemand(next, c.resumeDemand)
	for index, priority := range c.previewDemand {
		if previewFirst {
			priority = torrent.PiecePriorityNow
		}
		if priority > next[index] {
			next[index] = priority
		}
	}
	for index := range c.effective {
		if _, ok := next[index]; !ok {
			// Clear the override; the selected file's base priority remains.
			c.torrent.Piece(index).SetPriority(torrent.PiecePriorityNone)
		}
	}
	for index, priority := range next {
		if previous, ok := c.effective[index]; ok && previous == priority {
			continue
		}
		c.torrent.Piece(index).SetPriority(priority)
	}
	if c.storage != nil {
		demand := make(map[int]bool)
		for i := range next {
			if next[i] >= torrent.PiecePriorityReadahead {
				demand[i] = true
			}
		}
		c.storage.setStreamDemand(demand)
	}
	c.effective = next
}

// refocusOnSeek clears every peer's old request queue on an open or a seek;
// a variable so the seek comparison can measure without it.
var refocusOnSeek = true

type playbackWindowState struct {
	urgent, readahead, lookahead    pieceSpan
	openPhase, headBoost, tailBoost bool
}

// Each Android proxy descriptor owns a reader. Its cursor/readahead survive
// successive reads, and closing the descriptor cancels any blocked read.
type playbackReader struct {
	mu               sync.Mutex
	readIO           sync.Mutex
	readAttempts     []time.Duration
	stallRecoveries  int
	reader           torrent.Reader
	torrent          *torrent.Torrent
	file             *torrent.File
	length           int64
	ctx              context.Context
	cancel           context.CancelFunc
	closed           bool
	idleTimer        *time.Timer
	idleGeneration   uint64
	inFlight         int
	adaptive         bool
	forwardBytes     int64
	lastReadTime     time.Time
	readSampleOffset int64
	readRate         float64
	bufferControl    *demandBufferController

	positioned bool
	cursor     int64
	lastOffset int64
	haveOffset bool
	// Keep the first request after open/seek narrow until verified bytes arrive.
	focusPending bool
	// When the latest seek began; cleared once its first bytes are delivered.
	seekStarted time.Time
	// Set by an open or a seek: once the new window's priorities are in,
	// every peer drops its old queue and refills from the playhead.
	refocus bool
	// Readers of the same file this one displaced, parked once p.mu is free.
	superseded []*playbackReader
	// Set when a newer seek displaced this reader mid-read; it parks once
	// the read returns.
	parkWhenIdle bool

	urgent          pieceSpan
	readahead       pieceSpan
	lookahead       pieceSpan
	appliedWindow   playbackWindowState
	windowApplied   bool
	readerReadahead int64

	headBegin, headEnd int
	tailBegin, tailEnd int
	headBoostActive    bool
	tailBoostActive    bool
	// boost selects the larger startup window for this reader only.
	boost            bool
	startupHeadBytes int64
	startupTailBytes int64

	openPhase     bool
	headRead      bool
	tailRead      bool
	scrubbing     bool
	lastScrubTime time.Time
	// unverifiedReads lets reads return blocks before their piece is
	// verified (see applyResponsiveLocked); responsiveReader and
	// responsiveOn record what the current library reader was set to.
	unverifiedReads  bool
	responsiveReader torrent.Reader
	responsiveOn     bool
	scrubTimer       *time.Timer
	nowFn            func() time.Time

	priorityOwner *playbackPriorityCoordinator
	record        *TorrentRecord
	fileIndex     int
	preloadAsked  bool
	directPlay    bool
	storage       *DocumentTorrentStorage
	waitCancel    context.CancelFunc
	readSeq       uint64
	// refreshCompletion is set for Watch/stream storage so discarded cache
	// pieces are re-queued when the playhead returns to them.
	refreshCompletion bool
	// Piece the library reader was last moved to by a stream read.
	streamSeekPiece  int64
	streamSeekReader torrent.Reader
}

func (s *EngineServer) ensurePlaybackCoordinator(t *torrent.Torrent) *playbackPriorityCoordinator {
	s.playbackMu.Lock()
	defer s.playbackMu.Unlock()
	if s.playbackOwners == nil {
		s.playbackOwners = make(map[*torrent.Torrent]*playbackPriorityCoordinator)
	}
	coordinator := s.playbackOwners[t]
	if coordinator == nil {
		coordinator = newPlaybackPriorityCoordinator(t)
		coordinator.server = s
		s.playbackOwners[t] = coordinator
	} else if coordinator.server == nil {
		coordinator.server = s
	}
	return coordinator
}

func (p *playbackReader) now() time.Time {
	if p.nowFn != nil {
		return p.nowFn()
	}
	return time.Now()
}

func (s *EngineServer) OpenPlayback(id string, index int) (*playbackReader, error) {
	return s.openPlaybackAt(id, index, -1)
}

// openPlaybackAt opens a reader whose first read is expected at start, or at
// the saved resume position when start is negative. A player opens a new HTTP
// range on every seek; placing the window at the resume position first sent
// requests there and moved hundreds of piece priorities twice.
func (s *EngineServer) openPlaybackAt(id string, index int, start int64) (*playbackReader, error) {
	s.mu.RLock()
	rec := s.records[id]
	s.mu.RUnlock()
	if rec == nil {
		return nil, fmt.Errorf("torrent not found")
	}
	rec.mu.Lock()
	defer rec.mu.Unlock()
	if rec.removed || !rec.Configured || s.shuttingDown.Load() {
		return nil, fmt.Errorf("torrent is closing")
	}
	files := rec.Torrent.Files()
	selected := false
	for _, i := range rec.Selected {
		if i == index {
			selected = true
		}
	}
	if index < 0 || index >= len(files) || !rec.Configured || (!selected && (rec.DirectPlay || !fileIsVerified(rec.Torrent, files[index]))) {
		return nil, fmt.Errorf("file is not configured for playback")
	}
	file := files[index]
	reader := file.NewReader()
	reader.SetReadaheadFunc(nil)
	reader.SetReadahead(playbackUrgentBytes)
	ctx, cancel := context.WithCancel(context.Background())
	p := &playbackReader{
		record:  rec,
		reader:  reader,
		torrent: rec.Torrent,
		file:    file,
		length:  file.Length(),
		ctx:     ctx,
		cancel:  cancel,
	}
	p.directPlay, p.fileIndex = rec.DirectPlay, index
	if st := s.storage.GetStorage(rec.InfoHash); st != nil {
		p.storage = st
		if st.StreamMode() {
			p.refreshCompletion = true
			st.setStartupFile(index)
		}
	}
	p.adaptive = true
	p.forwardBytes = playbackWindowBytes
	if p.refreshCompletion {
		p.forwardBytes = 4 << 20
	}
	p.priorityOwner = s.ensurePlaybackCoordinator(rec.Torrent)
	p.priorityOwner.mu.Lock()
	boost := p.priorityOwner.playBoost[index]
	adaptiveBuffer := p.priorityOwner.adaptiveBuffers[index]
	burstAwareBuffer := p.priorityOwner.burstBuffers[index]
	if rec.DirectPlay && len(p.priorityOwner.readers) >= 4 {
		p.priorityOwner.mu.Unlock()
		cancel()
		_ = reader.Close()
		return nil, fmt.Errorf("too many simultaneous playback readers")
	}
	focusChanged := (rec.DirectPlay || rec.FileOnly) && isVideoFileName(file.DisplayPath()) && (rec.FocusedFile == nil || *rec.FocusedFile != index)
	if focusChanged {
		focused := index
		rec.FocusedFile = &focused
		// The coordinator lock is already held; compute hints after releasing it.
	}
	p.priorityOwner.storage = p.storage
	p.priorityOwner.readers[p] = struct{}{}
	p.priorityOwner.mu.Unlock()
	// Stay interested in every peer holding part of this file, so a seek
	// there is not left waiting to be unchoked.
	rec.Torrent.SetPlaybackInterest(filePieceRange(file, 0, file.Length()))
	if boost {
		p.forwardBytes = playbackUrgentBytes + playbackBoostReadaheadBytes
	}
	if adaptiveBuffer || burstAwareBuffer {
		p.bufferControl = &demandBufferController{window: p.forwardBytes, burstAware: burstAwareBuffer}
	}
	if rec.FileOnly && focusChanged {
		s.applySavedFilePriorities(rec)
		s.applyBackgroundFocus(rec)
		p.priorityOwner.prepareResume(file, nil)
	}
	if rec.DirectPlay || rec.FileOnly {
		s.applyVideoStartupHints(rec)
	}
	// CACHE-04: reuse a bounded validated resume window before scheduling demand.
	resumeOffset := p.priorityOwner.resumePosition(file)
	// Warm bytes without replacing the current external-player seek intent.
	s.applyResumeWindowToStorage(rec, index)
	headBytes, tailBytes := videoStartupHeadBytes, videoStartupTailBytes
	if boost {
		headBytes, tailBytes = playbackBoostHeadBytes, playbackBoostTailBytes
	} else if p.refreshCompletion {
		headBytes, tailBytes = directStartupHead, directStartupTail
	}
	p.mu.Lock()
	p.boost = boost
	p.startupHeadBytes = headBytes
	p.startupTailBytes = tailBytes
	p.headBegin, p.headEnd = filePieceRange(file, 0, minInt64(file.Length(), headBytes))
	p.headBoostActive = p.headBegin < p.headEnd
	if file.Length() > tailBytes {
		p.tailBegin, p.tailEnd = filePieceRange(file, file.Length()-tailBytes, file.Length())
		p.tailBoostActive = p.tailBegin < p.tailEnd
	}
	p.openPhase = true
	p.focusPending = true
	p.refocus = true
	if start >= 0 && start < p.length {
		resumeOffset = start
	}
	p.updateWindowLocked(resumeOffset)
	p.armIdleLocked()
	p.mu.Unlock()
	p.flushShareRefresh()
	return p, nil
}

func (p *playbackReader) ReadAt(data []byte, offset int64) (int, error) {
	return p.read(data, offset, false)
}

// ReadSome is ReadAt that returns as soon as some bytes are ready instead of
// filling data. After a seek the first block then reaches the player when it
// arrives, not when the next blocks of a large read do. Only for callers
// that accept short reads (HTTP); Android's file proxy can take a short read
// for the end of the file.
func (p *playbackReader) ReadSome(data []byte, offset int64) (int, error) {
	return p.read(data, offset, true)
}

func (p *playbackReader) read(data []byte, offset int64, partial bool) (int, error) {
	if srv := p.engine(); srv != nil && p.directPlay {
		defer srv.beginPlaybackRead(p.record)()
	}
	p.mu.Lock()
	if p.closed {
		p.mu.Unlock()
		return 0, fmt.Errorf("playback closed")
	}
	if offset < 0 {
		p.mu.Unlock()
		return 0, fmt.Errorf("invalid playback offset")
	}
	if offset >= p.length || len(data) == 0 {
		p.mu.Unlock()
		return 0, nil
	}
	data = data[:min(int64(len(data)), p.length-offset)]
	if p.waitCancel != nil {
		p.waitCancel()
		p.waitCancel = nil
	}
	if p.reader == nil {
		p.reader = p.file.NewReader()
		p.reader.SetReadaheadFunc(nil)
		p.positioned, p.windowApplied = false, false
		p.readerReadahead = -1
	}
	if p.idleTimer != nil {
		p.idleTimer.Stop()
	}
	p.inFlight++
	defer func() {
		p.mu.Lock()
		p.inFlight--
		if p.inFlight == 0 && p.parkWhenIdle {
			p.parkWhenIdle = false
			if p.priorityOwner != nil && !p.priorityOwner.isActive(p) {
				p.parkIdleLocked()
			}
		}
		p.armIdleLocked()
		p.mu.Unlock()
	}()
	if p.adaptive {
		now := p.now()
		// Measure a whole run of sequential callbacks. Resetting the clock on
		// every 32 KiB read discarded fast callbacks and measured only the
		// final small read after a player pause, shrinking useful read-ahead.
		if p.lastReadTime.IsZero() || !p.haveOffset || offset < p.lastOffset || offset-p.lastOffset >= playbackScrubJumpBytes {
			p.lastReadTime, p.readSampleOffset = now, offset
		} else if offset > p.readSampleOffset {
			elapsed := now.Sub(p.lastReadTime).Seconds()
			if elapsed >= 0.2 {
				rate := float64(offset-p.readSampleOffset) / elapsed
				if p.readRate == 0 {
					p.readRate = rate
				} else {
					p.readRate = p.readRate*0.8 + rate*0.2
				}
				limit := playbackWindowBytes
				if p.boost {
					limit = playbackUrgentBytes + playbackBoostReadaheadBytes
				}
				p.forwardBytes = max(int64(4<<20), min(limit, int64(p.readRate*8)))
				if p.bufferControl != nil {
					p.forwardBytes = p.bufferControl.observe(rate, limit)
				}
				p.lastReadTime, p.readSampleOffset = now, offset
			}
		}
	}
	p.updateWindowLocked(offset)
	if !p.openPhase && p.priorityOwner != nil {
		p.priorityOwner.setPlaying(p)
		// A read in the body is where the player is. Head and tail probes
		// while opening, and a new descriptor's resume guess, do not
		// displace it. Their locks cannot be taken while holding p.mu;
		// flushShareRefresh parks them.
		if others, became := p.priorityOwner.takeActive(p); became {
			p.superseded = append(p.superseded, others...)
			// Moving to this reader is a seek: apply its window now and
			// clear the peers' queues for the old spot.
			p.windowApplied, p.refocus = false, true
			p.repositionWindowLocked(offset, true)
		}
	}
	p.maybeStartNextPreloadLocked(offset)
	p.readSeq++
	seq := p.readSeq
	st := p.storage
	abs := p.file.Offset() + offset
	stream := p.refreshCompletion
	p.mu.Unlock()
	p.flushShareRefresh()

	if !stream && st != nil && st.RangeComplete(abs, int64(len(data))) {
		if spans, ok := st.byteSpans(abs, int64(len(data))); ok {
			n, err := preadFileSpans(spans, data)
			p.mu.Lock()
			if p.closed || p.readSeq != seq {
				p.mu.Unlock()
				return 0, fmt.Errorf("playback closed")
			}
			if n > 0 {
				p.cursor = offset + int64(n)
				p.positioned = false
				p.noteResumeReadLocked(offset + int64(n) - 1)
				p.finishFocusLocked()
			}
			p.mu.Unlock()
			return n, err
		}
	}

	// A newer seek cancels the previous wait before taking this lock. Its
	// cursor cannot race the old read while the latter unwinds cancellation.
	p.readIO.Lock()
	defer p.readIO.Unlock()
	attempts := p.readAttempts
	claudeSeek := false
	if srv := p.engine(); srv != nil {
		claudeSeek = srv.isClaudeSeekEnabled()
	}
	if len(attempts) == 0 {
		attempts = []time.Duration{8 * time.Second, 12 * time.Second, 25 * time.Second}
		if claudeSeek {
			attempts = claudeSeekReadAttempts
		}
	}
	if claudeSeek {
		defer startUrgentNudger(p.torrent, claudeSeekNudgeEvery)()
	}
	n := 0
	var err error
	for attempt, timeout := range attempts {
		p.mu.Lock()
		if p.closed || p.readSeq != seq {
			p.mu.Unlock()
			return 0, fmt.Errorf("playback closed or superseded")
		}
		ctx, cancel := context.WithTimeout(p.ctx, timeout)
		p.waitCancel = cancel
		p.reader.SetContext(ctx)
		if !p.positioned || offset+int64(n) != p.cursor {
			_, err = p.reader.Seek(offset+int64(n), io.SeekStart)
			if err != nil {
				cancel()
				p.waitCancel = nil
				p.mu.Unlock()
				return n, err
			}
			p.positioned = true
		}
		if !stream {
			p.applyResponsiveLocked(offset + int64(n))
		}
		reader := p.reader
		p.mu.Unlock()
		var got int
		var readErr error
		if stream {
			got, readErr = p.readVerifiedStream(ctx, data[n:], offset+int64(n), seq, partial)
		} else {
			if partial {
				got, readErr = reader.Read(data[n:])
			} else {
				got, readErr = io.ReadFull(reader, data[n:])
			}
		}
		n += got
		timedOut := errors.Is(ctx.Err(), context.DeadlineExceeded)
		cancel()
		p.mu.Lock()
		if p.closed || p.readSeq != seq {
			p.mu.Unlock()
			return 0, fmt.Errorf("playback closed or superseded")
		}
		p.waitCancel = nil
		p.cursor = offset + int64(n)
		if n > 0 {
			p.noteResumeReadLocked(offset + int64(n) - 1)
			p.finishFocusLocked()
		}
		err = readErr
		if err == nil || !timedOut || p.ctx.Err() != nil || attempt == len(attempts)-1 {
			p.mu.Unlock()
			break
		}
		p.stallRecoveries++
		if srv := p.engine(); srv != nil {
			srv.seekStats.recordStall()
		}
		// Refresh only this stalled reader's urgent request. Reopening drops
		// the library's stale reader demand and lets its existing scheduler
		// reconsider peer ownership; no peer is disconnected or duplicate
		// block traffic injected by this layer.
		_ = p.reader.Close()
		if p.priorityOwner != nil {
			p.priorityOwner.refreshUrgent(p, offset+int64(n))
		}
		p.reader = p.file.NewReader()
		p.reader.SetReadaheadFunc(nil)
		p.positioned, p.windowApplied = false, false
		p.readerReadahead = -1
		p.updateWindowLocked(offset + int64(n))
		p.mu.Unlock()
		p.flushShareRefresh()
	}
	return n, err
}

// The library's capped-storage reader retries recursively even after context
// cancellation. Direct Play instead waits on storage verification events and
// advances its existing reader demand one piece at a time. The peer scheduler
// and torrent hash verification remain owned by the library.
func (p *playbackReader) readVerifiedStream(ctx context.Context, b []byte, offset int64, seq uint64, partial bool) (int, error) {
	n := 0
	refreshed := int64(-1)
	for n < len(b) {
		if err := ctx.Err(); err != nil {
			return n, err
		}
		pos := offset + int64(n)
		p.mu.Lock()
		if p.closed || p.readSeq != seq {
			p.mu.Unlock()
			return n, context.Canceled
		}
		p.updateWindowLocked(pos)
		abs := p.file.Offset() + pos
		pieceLen := p.storage.chunkLength
		// Moving the library reader takes the client lock that peers need to
		// deliver blocks. It only matters when the read enters another piece.
		var err error
		if p.streamSeekPiece != abs/pieceLen || p.streamSeekReader != p.reader {
			_, err = p.reader.Seek(pos, io.SeekStart)
			p.streamSeekPiece, p.streamSeekReader = abs/pieceLen, p.reader
		}
		p.mu.Unlock()
		p.flushShareRefresh()
		if err != nil {
			return n, err
		}
		count := min(int64(len(b)-n), pieceLen-abs%pieceLen)
		got, ready, err := p.storage.readVerifiedStream(b[n:n+int(count)], abs)
		n += got
		if err != nil {
			return n, err
		}
		if ready != nil && partial && n > 0 {
			// Hand over what arrived; the caller reads the rest next.
			return n, nil
		}
		if ready != nil {
			// An evicted piece can still have a stale completion bit in the
			// library until the asynchronous eviction callback runs. Once per
			// piece: each delivered block wakes this wait, and the refresh
			// takes the client lock those blocks need.
			if piece := abs / pieceLen; piece != refreshed {
				p.torrent.Piece(int(piece)).UpdateCompletion()
				refreshed = piece
			}
			select {
			case <-ctx.Done():
				return n, ctx.Err()
			case <-ready:
			}
		}
	}
	return n, nil
}

func (c *playbackPriorityCoordinator) refreshUrgent(p *playbackReader, offset int64) {
	c.mu.Lock()
	defer c.mu.Unlock()
	begin, end := filePieceRange(p.file, offset, min(p.length, offset+1))
	for i := begin; i < end; i++ {
		priority := c.effective[i]
		if priority >= torrent.PiecePriorityNow {
			c.torrent.Piece(i).SetPriority(torrent.PiecePriorityHigh)
			c.torrent.Piece(i).SetPriority(priority)
		}
	}
}

// HTTP range reads use the same coordinator, RAM limits and recovery as JNI.
type playbackHTTPReader struct {
	playback *playbackReader
	offset   int64
}

func (r *playbackHTTPReader) Read(b []byte) (int, error) {
	if r.offset >= r.playback.length {
		return 0, io.EOF
	}
	n, err := r.playback.ReadSome(b, r.offset)
	r.offset += int64(n)
	return n, err
}
func (r *playbackHTTPReader) Seek(offset int64, whence int) (int64, error) {
	switch whence {
	case io.SeekStart:
	case io.SeekCurrent:
		offset += r.offset
	case io.SeekEnd:
		offset += r.playback.length
	default:
		return 0, fmt.Errorf("invalid seek")
	}
	if offset < 0 {
		return 0, fmt.Errorf("invalid seek")
	}
	r.offset = offset
	return offset, nil
}

func (s *EngineServer) closePlaybackReaders(t *torrent.Torrent) {
	s.playbackMu.Lock()
	c := s.playbackOwners[t]
	delete(s.playbackOwners, t)
	s.playbackMu.Unlock()
	if c == nil {
		return
	}
	c.mu.Lock()
	c.closed = true
	c.clearResumeLocked()
	readers := make([]*playbackReader, 0, len(c.readers))
	for p := range c.readers {
		readers = append(readers, p)
	}
	c.mu.Unlock()
	for _, p := range readers {
		p.Close()
	}
}

func (p *playbackReader) Close() {
	p.cancel()
	p.mu.Lock()
	if p.waitCancel != nil {
		p.waitCancel()
		p.waitCancel = nil
	}
	if p.closed {
		p.mu.Unlock()
		return
	}
	p.closed = true
	if p.idleTimer != nil {
		p.idleTimer.Stop()
	}
	p.clearPrioritiesLocked()
	var resumeServer *EngineServer
	var resumeFileIndex int
	var resumeOffset int64
	if p.priorityOwner != nil {
		p.priorityOwner.holdReaderResume(p)
		p.priorityOwner.mu.Lock()
		delete(p.priorityOwner.readers, p)
		if p.priorityOwner.playing == p {
			p.priorityOwner.playing = nil
		}
		if p.priorityOwner.active[p.file] == p {
			delete(p.priorityOwner.active, p.file)
		}
		resumeServer = p.priorityOwner.server
		if p.priorityOwner.resumeFile == p.file && p.priorityOwner.resumeOffset > 0 {
			resumeOffset = p.priorityOwner.resumeOffset
			for i, f := range p.torrent.Files() {
				if f == p.file {
					resumeFileIndex = i
					break
				}
			}
		}
		p.priorityOwner.mu.Unlock()
	}
	p.mu.Unlock()
	p.flushShareRefresh()
	if resumeServer != nil && resumeOffset > 0 {
		resumeServer.mu.RLock()
		var resumeRec *TorrentRecord
		for _, rec := range resumeServer.records {
			if rec.Torrent == p.torrent {
				resumeRec = rec
				break
			}
		}
		resumeServer.mu.RUnlock()
		if resumeRec != nil {
			resumeServer.scheduleResumePersist(resumeRec, resumeFileIndex, resumeOffset)
		}
	}
	// torrent.Reader.Close does not synchronize its storage cursor with Read.
	// Cancellation must unwind active I/O before closing that cursor.
	p.readIO.Lock()
	defer p.readIO.Unlock()
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.reader != nil {
		_ = p.reader.Close()
		p.reader = nil
	}
}

// Old probes relinquish both explicit priorities and the library reader's own
// demand. The same descriptor resumes on its next read; active I/O is never parked.
// The playing reader is never parked by the timer.
func (p *playbackReader) armIdleLocked() {
	if p.closed || !p.adaptive || p.inFlight != 0 {
		return
	}
	if p.idleTimer != nil {
		p.idleTimer.Stop()
	}
	p.idleGeneration++
	generation := p.idleGeneration
	p.idleTimer = time.AfterFunc(15*time.Second, func() { p.idleExpired(generation) })
}

func (p *playbackReader) idleExpired(generation uint64) {
	p.mu.Lock()
	if p.idleGeneration != generation || p.closed {
		p.mu.Unlock()
		return
	}
	// The timer fires once. A reader that is still playing has to arm the
	// next one, or it keeps its window after a newer viewer becomes the playhead.
	if p.isPlayingLocked() {
		p.armIdleLocked()
		p.mu.Unlock()
		return
	}
	p.parkIdleLocked()
	p.mu.Unlock()
	p.flushShareRefresh()
}

// isPlayingLocked reports whether p is its torrent's playing reader. Requires p.mu.
func (p *playbackReader) isPlayingLocked() bool {
	if p.priorityOwner == nil {
		return false
	}
	p.priorityOwner.mu.Lock()
	defer p.priorityOwner.mu.Unlock()
	return p.priorityOwner.playing == p
}

func (c *playbackPriorityCoordinator) setPlaying(p *playbackReader) {
	c.mu.Lock()
	c.playing = p
	c.mu.Unlock()
}

func (p *playbackReader) parkIdle() {
	p.mu.Lock()
	p.parkIdleLocked()
	p.mu.Unlock()
	p.flushShareRefresh()
}

func (p *playbackReader) parkIdleLocked() {
	if p.closed || p.inFlight != 0 {
		return
	}
	if p.scrubTimer != nil {
		p.scrubTimer.Stop()
		p.scrubTimer = nil
	}
	p.scrubbing = false
	if p.priorityOwner != nil {
		p.priorityOwner.remove(p)
	}
	if p.reader != nil {
		_ = p.reader.Close()
		p.reader = nil
	}
	p.windowApplied, p.positioned = false, false
}

func (p *playbackReader) clearPrioritiesLocked() {
	if p.scrubTimer != nil {
		p.scrubTimer.Stop()
		p.scrubTimer = nil
	}
	p.urgent, p.readahead, p.lookahead = pieceSpan{}, pieceSpan{}, pieceSpan{}
	p.headBoostActive, p.tailBoostActive = false, false
	p.openPhase = false
	p.scrubbing = false
	if p.priorityOwner != nil {
		p.priorityOwner.remove(p)
	}
}

func (p *playbackReader) scheduleScrubDebounceLocked() {
	if p.scrubTimer != nil {
		p.scrubTimer.Stop()
	}
	p.scrubTimer = time.AfterFunc(playbackScrubDebounce, func() {
		p.mu.Lock()
		if p.closed || !p.scrubbing {
			p.mu.Unlock()
			return
		}
		if p.now().Sub(p.lastScrubTime) >= playbackScrubDebounce {
			p.scrubbing = false
			p.updateWindowLocked(p.lastOffset)
		}
		p.mu.Unlock()
		p.flushShareRefresh()
	})
}

// startupSpanLocked is the head and tail of the file players read for the
// container index.
func (p *playbackReader) startupSpanLocked() (head, tail int64) {
	head, tail = p.startupHeadBytes, p.startupTailBytes
	if head <= 0 {
		head, tail = videoStartupHeadBytes, videoStartupTailBytes
		if p.refreshCompletion {
			head, tail = directStartupHead, directStartupTail
		}
	}
	return head, tail
}

// applyResponsiveLocked lets the library reader return blocks before their
// piece is verified, when unverifiedReads is on and offset is outside the
// start and end of the file. Download playback otherwise got only verified
// pieces: on the phone, 1 and 2 MiB after a seek came at once about 2.2 s in.
// Direct Play has its own unverified path. A bad block is a brief glitch; a
// bad index at the start or end can crash a player, so those stay verified.
func (p *playbackReader) applyResponsiveLocked(offset int64) {
	head, tail := p.startupSpanLocked()
	want := p.unverifiedReads && offset >= head && offset < p.length-tail
	if p.responsiveReader == p.reader && p.responsiveOn == want {
		return
	}
	if r, ok := p.reader.(interface{ WebtorSetResponsive(bool) }); ok {
		r.WebtorSetResponsive(want)
		p.responsiveReader, p.responsiveOn = p.reader, want
	}
}

// updateWindowLocked retargets peer piece priorities around the playhead.
// Seeks immediately retarget the urgent tip and full 24MB forward window.
// The quiet timer only settles seek tracking; it never gates downloading.
// Open phase gates dual head+tail at Now until the body/moov probe completes.
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
	// Peers send the waited-on piece from this offset, not from its start.
	p.torrent.SetRequestFocus(p.file.Offset() + offset)

	now := p.now()

	headBytes, tailBytes := p.startupSpanLocked()
	headLimit := minInt64(p.length, headBytes)
	hasTail := p.length > tailBytes
	var tailLimit int64
	if hasTail {
		tailLimit = p.length - tailBytes
	}

	inHead := offset < headLimit
	inTail := hasTail && offset >= tailLimit
	inBody := !inHead && !inTail

	if p.openPhase {
		if inBody || (p.adaptive && p.haveOffset && offset >= 256<<10 && offset > p.lastOffset && offset-p.lastOffset < playbackScrubJumpBytes) {
			p.openPhase = false
		} else if inHead {
			if p.headRead && p.tailRead {
				p.openPhase = false
			} else {
				p.headRead = true
			}
		} else if inTail {
			p.tailRead = true
		}
	}

	if p.scrubbing && now.Sub(p.lastScrubTime) >= playbackScrubDebounce {
		p.scrubbing = false
		if p.scrubTimer != nil {
			p.scrubTimer.Stop()
			p.scrubTimer = nil
		}
	}

	wasPositioned := p.haveOffset
	if p.haveOffset {
		delta := offset - p.lastOffset
		if !p.openPhase && (delta < 0 || delta >= playbackScrubJumpBytes) {
			p.focusPending = true
			p.refocus = true
			p.seekStarted = now
			p.scrubbing = true
			p.lastScrubTime = now
			p.scheduleScrubDebounceLocked()
		} else if p.scrubbing {
			p.lastScrubTime = now
			p.scheduleScrubDebounceLocked()
		}
	}
	p.lastOffset = offset
	p.haveOffset = true
	p.repositionWindowLocked(offset, wasPositioned)
}

// takeActive makes p its file's active reader. When it was not already, it
// reports so with the other readers of that file. Requires p.mu.
func (c *playbackPriorityCoordinator) takeActive(p *playbackReader) (others []*playbackReader, became bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.closed || c.active[p.file] == p {
		return nil, false
	}
	c.active[p.file] = p
	for r := range c.readers {
		if r != p && r.file == p.file {
			others = append(others, r)
		}
	}
	return others, true
}

func (c *playbackPriorityCoordinator) isActive(p *playbackReader) bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.active[p.file] == p
}

// mayFetch reports whether p's window may be fetched: it is its file's
// active reader, or the file has none yet (the player is still opening).
func (c *playbackPriorityCoordinator) mayFetch(p *playbackReader) bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	active := c.active[p.file]
	return active == nil || active == p
}

// unpublish drops p's demand and window if it has any.
func (c *playbackPriorityCoordinator) unpublish(p *playbackReader) {
	c.mu.Lock()
	defer c.mu.Unlock()
	_, demand := c.owners[p]
	_, window := c.windows[p]
	if !demand && !window {
		return
	}
	if demand {
		c.pendingShareRefresh = true
	}
	delete(c.owners, p)
	delete(c.windows, p)
	c.applyLocked()
}

// parkSuperseded stops fetching for readers left behind by a newer seek of
// the same file. An idle one is parked like an idle probe. A read still
// waiting at the old spot is ended (the player has moved on) and the reader is
// parked when it returns, which also releases the library reader that holds
// the old piece at top priority. A parked reader that reads again takes the
// file back.
func parkSuperseded(readers []*playbackReader) {
	for _, r := range readers {
		r.mu.Lock()
		if r.closed || r.priorityOwner == nil || r.priorityOwner.isActive(r) {
			r.mu.Unlock()
			continue
		}
		if r.inFlight == 0 {
			r.parkIdleLocked()
		} else {
			r.priorityOwner.unpublish(r)
			r.windowApplied = false
			if r.reader != nil && r.readerReadahead != 0 {
				r.reader.SetReadahead(0)
				r.readerReadahead = 0
			}
			if r.waitCancel != nil {
				r.waitCancel()
				r.waitCancel = nil
			}
			r.readSeq++
			r.parkWhenIdle = true
		}
		r.mu.Unlock()
		r.flushShareRefresh()
	}
}

func (p *playbackReader) repositionWindowLocked(offset int64, wasPositioned bool) {
	isGhostPro := p.priorityOwner != nil && p.priorityOwner.server != nil && p.priorityOwner.server.isGhostProEnabled()
	pieceLen := p.torrent.Info().PieceLength
	urgentBytes := playbackUrgentBytes
	lookaheadBytes := int64(0)
	forward := playbackWindowBytes
	if p.adaptive {
		forward = p.forwardBytes
	}
	if isGhostPro {
		forward = max(forward, 64<<20, 6*pieceLen)
		if p.boost {
			forward = max(forward, 96<<20)
		}
		lookaheadBytes = max(32<<20, 4*pieceLen)
		if !p.openPhase && !p.focusPending {
			urgentBytes = max(urgentBytes, 16<<20, 2*pieceLen)
		}
	}
	if p.refreshCompletion && p.storage != nil {
		p.storage.mu.Lock()
		budget := p.storage.maxCacheBytes
		pieceLen := p.storage.chunkLength
		p.storage.mu.Unlock()
		readers := 1
		if p.priorityOwner != nil {
			p.priorityOwner.mu.Lock()
			readers = len(p.priorityOwner.owners)
			if _, published := p.priorityOwner.owners[p]; !published {
				readers++
			}
			if readers < 1 {
				readers = 1
			}
			p.priorityOwner.mu.Unlock()
		}
		// Active readers share about 95% of the cache ahead of their playheads.
		// The rest (at least one piece) holds a new reader's probe and pending
		// startup saves; three pieces per reader cover span rounding.
		// Watched pieces fall out of demand and are evicted first.
		reserve := max(budget/20, pieceLen)
		maxForward := max(pieceLen, (budget-reserve)/int64(readers)-3*pieceLen)
		if isGhostPro {
			// Fill the allowance with all three tiers, a quarter of it lookahead
			// when the cache can hold it.
			lookaheadBytes = min(lookaheadBytes, (maxForward/4/pieceLen)*pieceLen)
			forward = max(pieceLen, maxForward-lookaheadBytes)
		} else {
			forward = min(forward, maxForward)
		}
		urgentBytes = min(urgentBytes, forward)
		if isGhostPro {
			urgentBytes = min(urgentBytes, max(pieceLen, (forward/4/pieceLen)*pieceLen))
		}
	}
	urgentEndOff := min(p.length, offset+urgentBytes)
	var newUrgent, newReadahead, newLookahead pieceSpan
	readerReadahead := forward
	if p.openPhase {
		readerReadahead = min(urgentBytes, forward)
		newUrgent = spanFromFile(p.file, offset, urgentEndOff)
		newReadahead = pieceSpan{}
	} else if urgent, readaheadSpan, ahead, bytes, ok := p.directPlaySpansLocked(offset, isGhostPro, forward, urgentBytes); p.refreshCompletion && ok {
		newUrgent, newReadahead, newLookahead = urgent, readaheadSpan, ahead
		readerReadahead = bytes
	} else {
		forwardEnd := offset + forward
		if forwardEnd > p.length {
			forwardEnd = p.length
		}
		newUrgent = spanFromFile(p.file, offset, urgentEndOff)
		newReadahead = spanFromFile(p.file, urgentEndOff, forwardEnd)
		newLookahead = spanFromFile(p.file, forwardEnd, min(p.length, forwardEnd+lookaheadBytes))
	}

	playhead := unionSpan(newUrgent, newReadahead, newLookahead)
	if !p.openPhase && wasPositioned {
		p.demoteBoostOutsideLocked(playhead)
	}
	p.urgent, p.readahead, p.lookahead = newUrgent, newReadahead, newLookahead
	// Only the file's active reader is fetched for in the body. Any other
	// one keeps no window and no library readahead, whichever path asks to
	// apply one. Head and tail probes while opening are always fetched, so a
	// player reopening the file starts even if an old reader lingers.
	if p.priorityOwner != nil && !p.openPhase && !p.priorityOwner.mayFetch(p) {
		if p.reader != nil && p.readerReadahead != 0 {
			p.reader.SetReadahead(0)
		}
		p.readerReadahead = 0
		p.priorityOwner.unpublish(p)
		p.windowApplied = false
		return
	}
	if p.readerReadahead != readerReadahead {
		if p.reader != nil {
			p.reader.SetReadahead(readerReadahead)
		}
		p.readerReadahead = readerReadahead
	}
	window := playbackWindowState{
		urgent: newUrgent, readahead: newReadahead, lookahead: newLookahead,
		openPhase: p.openPhase, headBoost: p.headBoostActive, tailBoost: p.tailBoostActive,
	}
	// Byte reads within the same piece spans do not change download demand.
	// Each read is about 32 KiB, so redoing the demand here took the client
	// lock hundreds of times per read and slowed both reads and downloads.
	refocus := p.refocus
	p.refocus = false
	if p.windowApplied && window == p.appliedWindow {
		return
	}
	kept := p.currentDemandLocked(newUrgent, newReadahead, newLookahead)
	if p.priorityOwner != nil {
		p.priorityOwner.update(p, kept, window)
	}
	if refocus && refocusOnSeek {
		p.torrent.RefocusRequests()
	}
	if p.refreshCompletion {
		// Stream storage re-queues an evicted piece's completion itself.
		// Only pieces entering the window may still carry a stale bit.
		var previous playbackWindowState
		if p.windowApplied {
			previous = p.appliedWindow
		}
		for i := range kept {
			if !previous.urgent.contains(i) && !previous.readahead.contains(i) &&
				!previous.lookahead.contains(i) {
				p.torrent.Piece(i).UpdateCompletion()
			}
		}
	}

	p.urgent, p.readahead, p.lookahead = newUrgent, newReadahead, newLookahead
	p.appliedWindow, p.windowApplied = window, true
}

// directPlaySpansLocked sizes a Direct Play window in whole pieces. Byte
// lengths spill into a neighboring piece when the playhead is unaligned, and
// that extra piece is what pushes two viewers past the RAM cache. The whole
// share is ahead of the playhead; nothing behind it is fetched.
func (p *playbackReader) directPlaySpansLocked(offset int64, isGhostPro bool, forwardBytes, urgentBytes int64) (urgent, readahead, lookahead pieceSpan, readerReadahead int64, ok bool) {
	if p.storage == nil || p.file == nil {
		return
	}
	p.storage.mu.Lock()
	budget := p.storage.maxCacheBytes
	pieceLen := p.storage.chunkLength
	p.storage.mu.Unlock()
	if pieceLen <= 0 || budget < pieceLen {
		return
	}
	readers := 1
	if p.priorityOwner != nil {
		p.priorityOwner.mu.Lock()
		readers = len(p.priorityOwner.owners)
		if _, published := p.priorityOwner.owners[p]; !published {
			readers++
		}
		if readers < 1 {
			readers = 1
		}
		p.priorityOwner.mu.Unlock()
	}
	reserve := max(budget/20, pieceLen)
	if reserve >= budget {
		reserve = 0
	}
	share := (budget - reserve) / int64(readers)
	if share < pieceLen {
		share = pieceLen
	}
	pieceBudget := int(share / pieceLen)
	if pieceBudget < 1 {
		pieceBudget = 1
	}
	cachePieces := int(budget / pieceLen)
	if cachePieces > 0 && pieceBudget*readers > cachePieces {
		pieceBudget = max(1, cachePieces/readers)
	}
	aheadPieces := pieceBudget
	urgentPieces := 1
	forwardPieces := aheadPieces
	lookaheadPieces := 0
	if isGhostPro && aheadPieces >= 3 {
		lookaheadPieces = max(1, aheadPieces/4)
		forwardPieces = aheadPieces - lookaheadPieces
		urgentPieces = max(1, forwardPieces/4)
		if urgentPieces >= forwardPieces {
			urgentPieces = forwardPieces - 1
		}
	} else if isGhostPro {
		forwardPieces = aheadPieces
		urgentPieces = 1
		if urgentPieces > forwardPieces {
			urgentPieces = forwardPieces
		}
	} else {
		forwardPieces = int((max(forwardBytes, pieceLen) + pieceLen - 1) / pieceLen)
		if forwardPieces > aheadPieces {
			forwardPieces = aheadPieces
		}
		if forwardPieces < 1 {
			forwardPieces = 1
		}
		urgentPieces = int((min(urgentBytes, forwardBytes) + pieceLen - 1) / pieceLen)
		if urgentPieces < 1 {
			urgentPieces = 1
		}
		if urgentPieces > forwardPieces {
			urgentPieces = forwardPieces
		}
		aheadPieces = forwardPieces
	}
	cur := spanFromFile(p.file, offset, offset+1)
	if cur.empty() {
		return
	}
	play := cur.begin
	fileEnd := p.file.EndPieceIndex()
	aheadEnd := play + aheadPieces
	if aheadEnd > fileEnd {
		aheadEnd = fileEnd
	}
	if aheadEnd <= play {
		aheadEnd = min(fileEnd, play+1)
	}
	urgentEnd := min(aheadEnd, play+urgentPieces)
	forwardEnd := min(aheadEnd, play+forwardPieces)
	return pieceSpan{play, urgentEnd}, pieceSpan{urgentEnd, forwardEnd}, pieceSpan{forwardEnd, aheadEnd}, int64(forwardPieces) * pieceLen, true
}

func (p *playbackReader) finishFocusLocked() {
	if !p.focusPending {
		return
	}
	p.focusPending = false
	if !p.seekStarted.IsZero() {
		if srv := p.engine(); srv != nil {
			srv.seekStats.recordSeek(p.now().Sub(p.seekStarted))
		}
		p.seekStarted = time.Time{}
	}
	if !p.openPhase {
		p.repositionWindowLocked(p.lastOffset, true)
	}
}

func (p *playbackReader) demoteBoostOutsideLocked(playhead pieceSpan) {
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

func (p *playbackReader) currentDemandLocked(urgent, readahead, lookahead pieceSpan) map[int]torrent.PiecePriority {
	demand := make(map[int]torrent.PiecePriority)
	// This block, then every next block in order. The window's spans only
	// size it; they are not separate tiers.
	now, _ := splitUrgent(urgent, p.torrent.Info().PieceLength)
	raiseSpan(demand, now, torrent.PiecePriorityNow)
	for _, span := range []pieceSpan{urgent, readahead, lookahead} {
		raiseSpan(demand, span, torrent.PiecePriorityNext)
	}
	if p.openPhase {
		if p.headBoostActive {
			raiseSpan(demand, pieceSpan{p.headBegin, p.headEnd}, torrent.PiecePriorityNow)
		}
		if p.tailBoostActive {
			raiseSpan(demand, pieceSpan{p.tailBegin, p.tailEnd}, torrent.PiecePriorityNow)
		}
	} else {
		if p.headBoostActive {
			raiseSpan(demand, pieceSpan{p.headBegin, p.headEnd}, torrent.PiecePriorityHigh)
		}
		if p.tailBoostActive {
			raiseSpan(demand, pieceSpan{p.tailBegin, p.tailEnd}, torrent.PiecePriorityHigh)
		}
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
	p.updateWindowLocked(offset)
	p.mu.Unlock()
	p.flushShareRefresh()
}

func (p *playbackReader) testSettleScrub() {
	p.mu.Lock()
	p.scrubbing = false
	if p.scrubTimer != nil {
		p.scrubTimer.Stop()
		p.scrubTimer = nil
	}
	p.updateWindowLocked(p.lastOffset)
	p.mu.Unlock()
	p.flushShareRefresh()
}

// testWindow reports active playhead spans for tests.
func (p *playbackReader) testWindow() (urgent, readahead pieceSpan, scrubbing, openPhase, headBoost, tailBoost bool) {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.urgent, p.readahead, p.scrubbing, p.openPhase, p.headBoostActive, p.tailBoostActive
}

// testPiecePriority reports this reader's demand for deterministic window
// tests. The coordinator, rather than this per-reader view, owns the actual
// torrent-global priority when multiple handles are open.
func (p *playbackReader) testPiecePriority(i int) torrent.PiecePriority {
	p.mu.Lock()
	defer p.mu.Unlock()
	if prio, ok := p.currentDemandLocked(p.urgent, p.readahead, p.lookahead)[i]; ok {
		return prio
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
