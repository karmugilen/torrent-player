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
//   - Tip (urgent): PiecePriorityNow — peers focus here for resume-after-seek
//   - Forward readahead: PiecePriorityReadahead — sequential within the window
//   - Lookbehind: PiecePriorityHigh — warm recent timestamps
//   - Open phase: Head (~5MB) and Tail (~3MB) at PiecePriorityNow, hold fat readahead
//   - Scrubbing: Tip (~2MB) at PiecePriorityNow + full forward readahead immediately
const (
	playbackUrgentBytes     = int64(2 * 1024 * 1024)
	playbackReadaheadBytes  = int64(24 * 1024 * 1024)
	playbackLookbehindBytes = int64(512 * 1024)
	playbackScrubJumpBytes  = int64(2 * 1024 * 1024)
	playbackScrubDebounce   = 250 * time.Millisecond

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
}

func newPlaybackPriorityCoordinator(t *torrent.Torrent) *playbackPriorityCoordinator {
	return &playbackPriorityCoordinator{
		torrent:   t,
		readers:   make(map[*playbackReader]struct{}),
		owners:    make(map[*playbackReader]map[int]torrent.PiecePriority),
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

func (c *playbackPriorityCoordinator) update(owner *playbackReader, demand map[int]torrent.PiecePriority) {
	c.mu.Lock()
	defer c.mu.Unlock()
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
	c.applyLocked()
}

func (c *playbackPriorityCoordinator) remove(owner *playbackReader) {
	c.mu.Lock()
	defer c.mu.Unlock()
	delete(c.owners, owner)
	c.applyLocked()
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
	// Background metadata preparation must yield to active playback. Readers
	// retain their own urgent head/tail probes until their open phase ends.
	for index, priority := range c.hints {
		if len(c.owners) > 0 && priority > torrent.PiecePriorityHigh {
			priority = torrent.PiecePriorityHigh
		}
		if priority > next[index] {
			next[index] = priority
		}
	}
	raiseDemand(next, c.focus)
	raiseDemand(next, c.resumeDemand)
	raiseDemand(next, c.previewDemand)
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

type playbackWindowState struct {
	urgent, readahead, lookbehind   pieceSpan
	openPhase, headBoost, tailBoost bool
}

// Each Android proxy descriptor owns a reader. Its cursor/readahead survive
// successive reads, and closing the descriptor cancels any blocked read.
type playbackReader struct {
	mu              sync.Mutex
	readIO          sync.Mutex
	readAttempts    []time.Duration
	stallRecoveries int
	reader          torrent.Reader
	torrent         *torrent.Torrent
	file            *torrent.File
	length          int64
	ctx             context.Context
	cancel          context.CancelFunc
	closed          bool
	idleTimer       *time.Timer
	idleGeneration  uint64
	inFlight        int
	adaptive        bool
	forwardBytes    int64
	lastReadTime    time.Time
	readRate        float64
	bufferControl   *demandBufferController

	positioned bool
	cursor     int64
	lastOffset int64
	haveOffset bool

	urgent          pieceSpan
	readahead       pieceSpan
	lookbehind      pieceSpan
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
	scrubTimer    *time.Timer
	nowFn         func() time.Time

	priorityOwner *playbackPriorityCoordinator
	storage       *DocumentTorrentStorage
	waitCancel    context.CancelFunc
	readSeq       uint64
	// refreshCompletion is set for Watch/stream storage so discarded cache
	// pieces are re-queued when the playhead returns to them.
	refreshCompletion bool
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
		reader:  reader,
		torrent: rec.Torrent,
		file:    file,
		length:  file.Length(),
		ctx:     ctx,
		cancel:  cancel,
	}
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
	p.updateWindowLocked(resumeOffset)
	p.armIdleLocked()
	return p, nil
}

func (p *playbackReader) ReadAt(data []byte, offset int64) (int, error) {
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
	defer func() { p.mu.Lock(); p.inFlight--; p.armIdleLocked(); p.mu.Unlock() }()
	if p.adaptive {
		now := p.now()
		if p.haveOffset && offset > p.lastOffset && offset-p.lastOffset < playbackScrubJumpBytes && !p.lastReadTime.IsZero() {
			elapsed := now.Sub(p.lastReadTime).Seconds()
			if elapsed >= 0.02 {
				rate := float64(offset-p.lastOffset) / elapsed
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
			}
		}
		p.lastReadTime = now
	}
	p.updateWindowLocked(offset)
	p.readSeq++
	seq := p.readSeq
	st := p.storage
	abs := p.file.Offset() + offset
	stream := p.refreshCompletion
	p.mu.Unlock()

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
	if len(attempts) == 0 {
		attempts = []time.Duration{8 * time.Second, 12 * time.Second, 25 * time.Second}
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
		reader := p.reader
		p.mu.Unlock()
		var got int
		var readErr error
		if stream {
			got, readErr = p.readVerifiedStream(ctx, data[n:], offset+int64(n), seq)
		} else {
			got, readErr = io.ReadFull(reader, data[n:])
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
		}
		err = readErr
		if err == nil || !timedOut || p.ctx.Err() != nil || attempt == len(attempts)-1 {
			p.mu.Unlock()
			break
		}
		p.stallRecoveries++
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
	}
	return n, err
}

// The library's capped-storage reader retries recursively even after context
// cancellation. Direct Play instead waits on storage verification events and
// advances its existing reader demand one piece at a time. The peer scheduler
// and torrent hash verification remain owned by the library.
func (p *playbackReader) readVerifiedStream(ctx context.Context, b []byte, offset int64, seq uint64) (int, error) {
	n := 0
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
		_, err := p.reader.Seek(pos, io.SeekStart)
		p.mu.Unlock()
		if err != nil {
			return n, err
		}
		abs := p.file.Offset() + pos
		pieceLen := p.storage.chunkLength
		count := min(int64(len(b)-n), pieceLen-abs%pieceLen)
		got, ready, err := p.storage.readVerifiedStream(b[n:n+int(count)], abs)
		n += got
		if err != nil {
			return n, err
		}
		if ready != nil {
			// An evicted piece can still have a stale completion bit in the
			// library until the asynchronous eviction callback runs.
			p.torrent.Piece(int(abs / pieceLen)).UpdateCompletion()
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
	n, err := r.playback.ReadAt(b, r.offset)
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
			resumeServer.persistVerifiedResumeWindow(resumeRec, resumeFileIndex, resumeOffset)
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
func (p *playbackReader) armIdleLocked() {
	if p.closed || !p.adaptive || p.inFlight != 0 {
		return
	}
	if p.idleTimer != nil {
		p.idleTimer.Stop()
	}
	p.idleGeneration++
	generation := p.idleGeneration
	p.idleTimer = time.AfterFunc(15*time.Second, func() {
		p.mu.Lock()
		defer p.mu.Unlock()
		if p.idleGeneration == generation {
			p.parkIdleLocked()
		}
	})
}

func (p *playbackReader) parkIdle() {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.parkIdleLocked()
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
	p.urgent, p.readahead, p.lookbehind = pieceSpan{}, pieceSpan{}, pieceSpan{}
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
		defer p.mu.Unlock()
		if p.closed || !p.scrubbing {
			return
		}
		if p.now().Sub(p.lastScrubTime) >= playbackScrubDebounce {
			p.scrubbing = false
			p.updateWindowLocked(p.lastOffset)
		}
	})
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

	now := p.now()

	headBytes, tailBytes := p.startupHeadBytes, p.startupTailBytes
	if headBytes <= 0 {
		headBytes, tailBytes = videoStartupHeadBytes, videoStartupTailBytes
		if p.refreshCompletion {
			headBytes, tailBytes = directStartupHead, directStartupTail
		}
	}
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

	urgentEndOff := offset + playbackUrgentBytes
	if urgentEndOff > p.length {
		urgentEndOff = p.length
	}

	var newUrgent, newReadahead, newLookbehind pieceSpan
	forward := playbackWindowBytes
	if p.adaptive {
		forward = p.forwardBytes
	}
	if p.refreshCompletion && p.storage != nil {
		p.storage.mu.Lock()
		budget := p.storage.maxCacheBytes
		pieceLen := p.storage.chunkLength
		p.storage.mu.Unlock()
		readers := 1
		if p.priorityOwner != nil {
			p.priorityOwner.mu.Lock()
			readers = max(1, len(p.priorityOwner.owners))
			p.priorityOwner.mu.Unlock()
		}
		// Reserve room for startup probes, lookbehind, rounding and in-flight
		// pieces. Four reader slots prevent a new probe overcommitting RAM.
		readers = max(4, readers)
		forward = min(forward, max(pieceLen, (budget-(4<<20))/int64(readers)-pieceLen))
		urgentEndOff = min(p.length, offset+min(playbackUrgentBytes, forward))
	}
	readerReadahead := forward
	if p.openPhase {
		readerReadahead = min(playbackUrgentBytes, forward)
		newUrgent = spanFromFile(p.file, offset, urgentEndOff)
		newReadahead = pieceSpan{}
		newLookbehind = pieceSpan{}
	} else {
		forwardEnd := offset + forward
		if forwardEnd > p.length {
			forwardEnd = p.length
		}
		lookStart := offset - playbackLookbehindBytes
		if lookStart < 0 {
			lookStart = 0
		}
		newUrgent = spanFromFile(p.file, offset, urgentEndOff)
		newReadahead = spanFromFile(p.file, urgentEndOff, forwardEnd)
		newLookbehind = spanFromFile(p.file, lookStart, offset)
	}

	playhead := unionSpan(newUrgent, newReadahead, newLookbehind)
	if !p.openPhase && wasPositioned {
		p.demoteBoostOutsideLocked(playhead)
	}
	if p.readerReadahead != readerReadahead {
		if p.reader != nil {
			p.reader.SetReadahead(readerReadahead)
		}
		p.readerReadahead = readerReadahead
	}
	window := playbackWindowState{
		urgent: newUrgent, readahead: newReadahead, lookbehind: newLookbehind,
		openPhase: p.openPhase, headBoost: p.headBoostActive, tailBoost: p.tailBoostActive,
	}
	// Byte reads within the same piece spans do not change download demand.
	// Stream storage still needs its completion refresh after cache eviction.
	if p.windowApplied && window == p.appliedWindow && !p.refreshCompletion {
		return
	}
	kept := p.currentDemandLocked(newUrgent, newReadahead, newLookbehind)
	if p.priorityOwner != nil {
		p.priorityOwner.update(p, kept)
	}
	if p.refreshCompletion {
		for i := range kept {
			p.torrent.Piece(i).UpdateCompletion()
		}
	}

	p.urgent, p.readahead, p.lookbehind = newUrgent, newReadahead, newLookbehind
	p.appliedWindow, p.windowApplied = window, true
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

func (p *playbackReader) currentDemandLocked(urgent, readahead, lookbehind pieceSpan) map[int]torrent.PiecePriority {
	demand := make(map[int]torrent.PiecePriority)
	raiseSpan(demand, urgent, torrent.PiecePriorityNow)
	raiseSpan(demand, readahead, torrent.PiecePriorityReadahead)
	raiseSpan(demand, lookbehind, torrent.PiecePriorityHigh)
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
	defer p.mu.Unlock()
	p.updateWindowLocked(offset)
}

func (p *playbackReader) testSettleScrub() {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.scrubbing = false
	if p.scrubTimer != nil {
		p.scrubTimer.Stop()
		p.scrubTimer = nil
	}
	p.updateWindowLocked(p.lastOffset)
}

// testWindow reports active playhead spans for tests.
func (p *playbackReader) testWindow() (urgent, readahead, lookbehind pieceSpan, scrubbing, openPhase, headBoost, tailBoost bool) {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.urgent, p.readahead, p.lookbehind, p.scrubbing, p.openPhase, p.headBoostActive, p.tailBoostActive
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
	if p.openPhase {
		if p.headBoostActive && (i >= p.headBegin && i < p.headEnd) {
			return torrent.PiecePriorityNow
		}
		if p.tailBoostActive && (i >= p.tailBegin && i < p.tailEnd) {
			return torrent.PiecePriorityNow
		}
	}
	if p.readahead.contains(i) {
		return torrent.PiecePriorityReadahead
	}
	if p.lookbehind.contains(i) {
		return torrent.PiecePriorityHigh
	}
	if p.headBoostActive && (i >= p.headBegin && i < p.headEnd) {
		return torrent.PiecePriorityHigh
	}
	if p.tailBoostActive && (i >= p.tailBegin && i < p.tailEnd) {
		return torrent.PiecePriorityHigh
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
