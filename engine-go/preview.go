package main

import (
	"context"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sync"
	"time"

	"github.com/anacrolix/torrent"
)

const (
	previewBudgetBytes    = int64(8 << 20)
	previewSessionTimeout = 10 * time.Second
)

var errPreviewBudget = fmt.Errorf("preview byte budget exceeded")

// PreviewReader supplies seekable verified bytes for thumbnails without
// mutating playback focus, resume history, or startup ownership.
type PreviewReader interface {
	ReadAt(p []byte, off int64) (int, error)
	Close() error
	Length() int64
}

type previewReader struct {
	mu         sync.Mutex
	readIO     sync.Mutex
	server     *EngineServer
	rec        *TorrentRecord
	torrent    *torrent.Torrent
	file       *torrent.File
	fileIndex  int
	reader     torrent.Reader
	storage    *DocumentTorrentStorage
	owner      *playbackPriorityCoordinator
	ctx        context.Context
	cancel     context.CancelFunc
	length     int64
	budget     int64
	admitted   map[int]bool
	admittedN  int64
	closed     bool
	cachedOnly bool
	directPlay bool
	temp       bool
	waitCancel context.CancelFunc
}

func previewDir(baseDir, infoHashHex string) string {
	return filepath.Join(baseDir, ".preview", infoHashHex)
}

func (s *EngineServer) closeActivePreview() {
	s.mu.Lock()
	p := s.previewActive
	s.mu.Unlock()
	if p != nil {
		_ = p.Close()
	}
}

// closePreviewFor closes the active preview only when it belongs to rec.
func (s *EngineServer) closePreviewFor(rec *TorrentRecord) {
	if rec == nil {
		return
	}
	s.mu.Lock()
	p := s.previewActive
	if p == nil || p.rec != rec {
		s.mu.Unlock()
		return
	}
	s.mu.Unlock()
	_ = p.closeWithRecordLocked()
}

func (s *EngineServer) OpenPreview(id string, index int) (PreviewReader, error) {
	s.mu.Lock()
	if s.previewActive != nil {
		s.mu.Unlock()
		return nil, fmt.Errorf("preview already active")
	}
	rec := s.records[id]
	if rec == nil {
		s.mu.Unlock()
		return nil, fmt.Errorf("torrent not found")
	}
	// Reserve the actual object, including its record identity. Configuration,
	// pause and shutdown can cancel setup as well as an established reader.
	p := &previewReader{server: s, rec: rec}
	s.previewActive = p
	s.mu.Unlock()
	rec.mu.Lock()
	defer rec.mu.Unlock()
	fail := func(err error) (PreviewReader, error) {
		_ = p.closeWithRecordLocked()
		return nil, err
	}
	if p.closed || rec.removed || s.shuttingDown.Load() || rec.Paused {
		return fail(fmt.Errorf("torrent unavailable for preview"))
	}
	if rec.Torrent.Info() == nil {
		return fail(fmt.Errorf("metadata not ready"))
	}
	files := rec.Torrent.Files()
	if index < 0 || index >= len(files) {
		return fail(fmt.Errorf("invalid file index"))
	}
	file := files[index]
	st := s.storage.GetStorage(rec.InfoHash)
	if st == nil {
		return fail(fmt.Errorf("storage unavailable"))
	}
	p.storage, p.torrent = st, rec.Torrent
	if !rec.Configured {
		if err := st.attachPreviewTemp(index); err != nil {
			return fail(err)
		}
		p.temp = true
		if s.payloadTransfersAllowed.Load() {
			rec.Torrent.AllowDataDownload()
		}
	} else {
		selected := false
		for _, i := range rec.Selected {
			if i == index {
				selected = true
				break
			}
		}
		if !selected && !fileIsVerified(rec.Torrent, file) {
			return fail(fmt.Errorf("file is not selected for preview"))
		}
	}
	p.ctx, p.cancel = context.WithTimeout(context.Background(), previewSessionTimeout)
	p.file, p.fileIndex, p.length = file, index, file.Length()
	p.budget, p.admitted = previewBudgetBytes, make(map[int]bool)
	p.reader = file.NewReader()
	p.reader.SetReadaheadFunc(nil)
	p.reader.SetReadahead(0)
	p.owner = s.ensurePlaybackCoordinator(rec.Torrent)
	p.owner.mu.Lock()
	p.owner.storage = st
	p.cachedOnly = rec.DirectPlay && len(p.owner.readers) > 0
	p.directPlay = rec.DirectPlay
	p.owner.mu.Unlock()
	if s.shuttingDown.Load() {
		return fail(fmt.Errorf("engine is closing"))
	}
	// Admit only actual decoder requests. Pre-admitting an entire head window
	// consumes the budget before MP4 tail metadata can be requested.
	context.AfterFunc(p.ctx, func() { _ = p.Close() })
	return p, nil
}

func (p *previewReader) Length() int64 { return p.length }

// admitRange unions whole pieces covering [offset, offset+length) into the
// session-admitted set. Already-admitted or already-verified pieces reuse the
// budget without inventing completion. Fails when a new piece cannot fit.
func (p *previewReader) admitRange(offset, length int64) error {
	if length <= 0 {
		return nil
	}
	info := p.torrent.Info()
	if info == nil {
		return fmt.Errorf("metadata not ready")
	}
	begin, end := filePieceRange(p.file, offset, min(p.length, offset+length))
	var need int64
	newPieces := make([]int, 0, end-begin)
	for i := begin; i < end; i++ {
		if p.admitted[i] {
			continue
		}
		if p.storage != nil && p.storage.PieceComplete(i) {
			// Cached verified bytes require no network admission.
			continue
		}
		if p.server != nil && !p.server.payloadTransfersAllowed.Load() {
			return fmt.Errorf("payload transfers are paused")
		}
		n := info.Piece(i).Length()
		if p.admittedN+need+n > p.budget {
			return errPreviewBudget
		}
		need += n
		newPieces = append(newPieces, i)
	}
	for _, i := range newPieces {
		p.admitted[i] = true
		p.admittedN += info.Piece(i).Length()
	}
	return nil
}

func (p *previewReader) publishDemand() {
	if p.owner == nil {
		return
	}
	demand := make(map[int]torrent.PiecePriority, len(p.admitted))
	for i := range p.admitted {
		if p.storage != nil && p.storage.PieceComplete(i) {
			continue
		}
		demand[i] = torrent.PiecePriorityHigh
	}
	p.owner.setPreviewDemand(demand)
}

func (p *previewReader) ReadAt(data []byte, offset int64) (int, error) {
	p.readIO.Lock()
	defer p.readIO.Unlock()
	p.mu.Lock()
	if p.closed {
		p.mu.Unlock()
		return -1, fmt.Errorf("preview closed")
	}
	if err := p.ctx.Err(); err != nil {
		p.mu.Unlock()
		return -1, err
	}
	if offset < 0 {
		p.mu.Unlock()
		return -1, fmt.Errorf("invalid preview offset")
	}
	if offset >= p.length || len(data) == 0 {
		p.mu.Unlock()
		return 0, nil
	}
	data = data[:min(int64(len(data)), p.length-offset)]
	if p.cachedOnly {
		st, abs := p.storage, p.file.Offset()+offset
		p.mu.Unlock()
		if n, _ := st.readVerifiedPreview(data, abs); n > 0 {
			return n, nil
		}
		return -1, fmt.Errorf("preview range is not cached yet")
	}
	if err := p.admitRange(offset, int64(len(data))); err != nil {
		p.mu.Unlock()
		return -1, err
	}
	p.publishDemand()
	ctx, cancel := context.WithCancel(p.ctx)
	p.waitCancel = cancel
	abs, st := p.file.Offset()+offset, p.storage
	p.mu.Unlock()
	defer func() {
		cancel()
		p.mu.Lock()
		p.waitCancel = nil
		p.mu.Unlock()
	}()
	if st != nil && st.RangeComplete(abs, int64(len(data))) {
		if spans, ok := st.byteSpans(abs, int64(len(data))); ok {
			return preadFileSpans(spans, data)
		}
		if n, ok := st.readVerifiedPreview(data, abs); ok {
			return n, nil
		}
	}
	if p.directPlay && p.server != nil && p.rec != nil {
		// A thumbnail that needs the network wakes an idle-rested session.
		p.rec.playbackIdle.touch(time.Now())
		p.server.wakeDirectPlayFromRead(p.rec)
	}
	p.reader.SetContext(ctx)
	if _, err := p.reader.Seek(offset, io.SeekStart); err != nil {
		return -1, err
	}
	n, err := io.ReadFull(p.reader, data)
	if err == io.EOF || err == io.ErrUnexpectedEOF {
		return n, nil
	}
	if err != nil {
		return -1, err
	}
	return n, nil
}

func (p *previewReader) Close() error {
	if p.rec != nil {
		p.rec.mu.Lock()
		defer p.rec.mu.Unlock()
	}
	return p.closeWithRecordLocked()
}

// Configuration/pause/remove already hold rec.mu; exported Close acquires it.
func (p *previewReader) closeWithRecordLocked() error {
	p.mu.Lock()
	if p.closed {
		p.mu.Unlock()
		return nil
	}
	p.closed = true
	if p.waitCancel != nil {
		p.waitCancel()
		p.waitCancel = nil
	}
	if p.cancel != nil {
		p.cancel()
	}
	reader := p.reader
	owner := p.owner
	temp := p.temp
	st := p.storage
	rec := p.rec
	server := p.server
	p.mu.Unlock()

	// Join active I/O before closing the torrent reader / releasing temp storage,
	// matching playback Close semantics for blocked reads.
	p.readIO.Lock()
	if reader != nil {
		_ = reader.Close()
	}
	p.readIO.Unlock()

	if owner != nil {
		owner.setPreviewDemand(nil)
	}
	if temp && st != nil {
		st.mu.Lock()
		complete := make([]int, 0, len(st.completed))
		for index, verified := range st.completed {
			if verified {
				complete = append(complete, index)
			}
		}
		st.mu.Unlock()
		st.releasePreviewTemp()
		// The library caches completion independently of storage. Discarding the
		// draft's RAM must invalidate those flags before a later configuration.
		for _, index := range complete {
			p.torrent.Piece(index).UpdateCompletion()
		}
		// Only revoke the temporary allow when the torrent is still a draft.
		// Configure/pause hold rec.mu and restore transfer policy themselves;
		// do not DisallowDataDownload over a newly configured Direct Play session.
		if rec != nil && !rec.Configured && !rec.removed && !rec.DirectPlay {
			rec.Torrent.DisallowDataDownload()
		}
	}
	if server != nil {
		server.mu.Lock()
		if server.previewActive == p {
			server.previewActive = nil
		}
		server.mu.Unlock()
	}
	return nil
}

func (st *DocumentTorrentStorage) attachPreviewTemp(fileIndex int) error {
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.closed {
		return fmt.Errorf("storage closed")
	}
	if st.info == nil {
		return fmt.Errorf("metadata not ready")
	}
	if st.attached {
		return nil
	}
	if fileIndex < 0 || fileIndex >= len(st.files) {
		return fmt.Errorf("invalid preview file index")
	}
	dir := previewDir(st.baseDir, st.infoHash.HexString())
	if err := os.MkdirAll(dir, 0700); err != nil {
		return err
	}
	// Preview temp keeps pieces in the existing RAM cache only; no full-sized
	// destination video file is created. Mark attached so completion/reads work.
	st.stream = true
	st.attached = true
	st.previewTemp = true
	st.maxCacheBytes = previewBudgetBytes
	st.startupPieces = make(map[int]bool)
	st.startupSaved = make(map[int]bool)
	st.streamDemand = make(map[int]bool)
	st.streamLost = make(map[int]bool)
	st.streamWhole = make(map[int]bool)
	st.streamRanges = make(map[int][]streamByteRange)
	st.streamReady = make(chan struct{})
	file := st.files[fileIndex]
	var budget int64
	for _, span := range [][2]int64{
		{file.Offset, file.Offset + min(file.Length, previewBudgetBytes)},
	} {
		if span[1] <= span[0] {
			continue
		}
		for p := int(span[0] / st.chunkLength); p <= int((span[1]-1)/st.chunkLength); p++ {
			n := st.info.Piece(p).Length()
			if budget+n > previewBudgetBytes {
				break
			}
			st.startupPieces[p] = true
			budget += n
		}
	}
	return nil
}

func (st *DocumentTorrentStorage) releasePreviewTemp() {
	st.mu.Lock()
	defer st.mu.Unlock()
	st.releasePreviewTempLocked()
}

func (st *DocumentTorrentStorage) releasePreviewTempLocked() {
	if !st.previewTemp {
		return
	}
	st.previewTemp = false
	st.maxCacheBytes = st.downloadMaxBytes
	st.attached = false
	st.stream = false
	st.cache, st.cacheDirty, st.cacheOrder, st.cacheBytes = make(map[int][]byte), make(map[int]bool), nil, 0
	st.completed = make(map[int]bool)
	st.startupPieces, st.startupSaved = nil, nil
	st.streamDemand, st.streamLost, st.streamWhole, st.streamRanges = nil, nil, nil, nil
	if st.streamReady != nil {
		st.notifyStreamReadyLocked()
		st.streamReady = nil
	}
	_ = os.RemoveAll(previewDir(st.baseDir, st.infoHash.HexString()))
}

// readVerifiedPreview copies already-complete stream/RAM cache bytes.
func (st *DocumentTorrentStorage) readVerifiedPreview(b []byte, abs int64) (int, bool) {
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.closed || !st.attached || st.chunkLength <= 0 || len(b) == 0 {
		return 0, false
	}
	n := 0
	for n < len(b) {
		index := int((abs + int64(n)) / st.chunkLength)
		if !st.completed[index] {
			return n, n > 0
		}
		pieceOff := (abs + int64(n)) % st.chunkLength
		want := min(int64(len(b)-n), st.info.Piece(index).Length()-pieceOff)
		if want <= 0 {
			break
		}
		p := &documentPiece{storage: st, piece: st.info.Piece(index)}
		got, err := p.readAtLocked(b[n:n+int(want)], pieceOff)
		n += got
		if err != nil || got == 0 {
			return n, n > 0
		}
	}
	return n, n == len(b)
}

func (c *playbackPriorityCoordinator) setPreviewDemand(demand map[int]torrent.PiecePriority) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if mapsEqualDemand(c.previewDemand, demand) {
		return
	}
	c.previewDemand = cloneDemand(demand)
	c.applyLocked()
}

func mapsEqualDemand(a, b map[int]torrent.PiecePriority) bool {
	if len(a) != len(b) {
		return false
	}
	for k, v := range a {
		if b[k] != v {
			return false
		}
	}
	return true
}
