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
	previewSessionTimeout = 30 * time.Second
	previewOverfetchBytes = int64(256 << 10) // optional slack within the same session budget
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
	temp       bool
	waitCancel context.CancelFunc
	readSeq    uint64
	reserveGen uint64
}

func previewDir(baseDir, infoHashHex string) string {
	return filepath.Join(baseDir, ".preview", infoHashHex)
}

func (s *EngineServer) closeActivePreview() {
	s.mu.Lock()
	p := s.previewActive
	s.previewActive = nil
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
	s.previewActive = nil
	s.mu.Unlock()
	_ = p.Close()
}

func (s *EngineServer) OpenPreview(id string, index int) (PreviewReader, error) {
	// Atomically reserve the single preview slot before any setup work so
	// concurrent opens cannot both proceed and leak priority/storage.
	s.mu.Lock()
	if s.previewActive != nil {
		s.mu.Unlock()
		return nil, fmt.Errorf("preview already active")
	}
	s.previewReserveGen++
	reserveGen := s.previewReserveGen
	placeholder := &previewReader{reserveGen: reserveGen}
	s.previewActive = placeholder
	rec := s.records[id]
	s.mu.Unlock()

	rollback := func() {
		s.mu.Lock()
		if s.previewActive == placeholder {
			s.previewActive = nil
		}
		s.mu.Unlock()
	}

	if rec == nil {
		rollback()
		return nil, fmt.Errorf("torrent not found")
	}
	rec.mu.Lock()
	if rec.removed || s.shuttingDown.Load() || rec.Paused {
		rec.mu.Unlock()
		rollback()
		return nil, fmt.Errorf("torrent unavailable for preview")
	}
	if rec.Torrent.Info() == nil {
		rec.mu.Unlock()
		rollback()
		return nil, fmt.Errorf("metadata not ready")
	}
	files := rec.Torrent.Files()
	if index < 0 || index >= len(files) {
		rec.mu.Unlock()
		rollback()
		return nil, fmt.Errorf("invalid file index")
	}
	file := files[index]
	st := s.storage.GetStorage(rec.InfoHash)
	if st == nil {
		rec.mu.Unlock()
		rollback()
		return nil, fmt.Errorf("storage unavailable")
	}

	temp := false
	if !rec.Configured {
		if err := st.attachPreviewTemp(index); err != nil {
			rec.mu.Unlock()
			rollback()
			return nil, err
		}
		temp = true
		// Allow bounded payload only for this preview; do not mark Configured.
		rec.Torrent.AllowDataDownload()
	} else {
		selected := false
		for _, i := range rec.Selected {
			if i == index {
				selected = true
				break
			}
		}
		if !selected && !fileIsVerified(rec.Torrent, file) && !rec.DirectPlay {
			rec.mu.Unlock()
			rollback()
			return nil, fmt.Errorf("file is not selected for preview")
		}
	}
	rec.mu.Unlock()

	ctx, cancel := context.WithTimeout(context.Background(), previewSessionTimeout)
	reader := file.NewReader()
	reader.SetReadaheadFunc(nil)
	reader.SetReadahead(0) // demand is coordinator-driven within the session budget
	owner := s.ensurePlaybackCoordinator(rec.Torrent)
	p := &previewReader{
		server:     s,
		rec:        rec,
		torrent:    rec.Torrent,
		file:       file,
		fileIndex:  index,
		reader:     reader,
		storage:    st,
		owner:      owner,
		ctx:        ctx,
		cancel:     cancel,
		length:     file.Length(),
		budget:     previewBudgetBytes,
		admitted:   make(map[int]bool),
		temp:       temp,
		reserveGen: reserveGen,
	}
	// Seed the head window within budget so the first metadata probe has demand.
	if err := p.admitRange(0, min(file.Length(), previewBudgetBytes)); err != nil {
		cancel()
		_ = reader.Close()
		if temp {
			st.releasePreviewTemp()
			rec.Torrent.DisallowDataDownload()
		}
		rollback()
		return nil, err
	}
	p.publishDemand()

	s.mu.Lock()
	if s.previewActive != placeholder {
		s.mu.Unlock()
		cancel()
		_ = reader.Close()
		owner.setPreviewDemand(nil)
		if temp {
			st.releasePreviewTemp()
			rec.Torrent.DisallowDataDownload()
		}
		return nil, fmt.Errorf("preview reservation lost")
	}
	s.previewActive = p
	s.mu.Unlock()
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
			// Verified bytes already present: count toward admitted union so
			// readahead cannot expand past the session budget, but do not fetch.
			n := info.Piece(i).Length()
			if p.admittedN+need+n > p.budget+previewOverfetchBytes {
				return errPreviewBudget
			}
			p.admitted[i] = true
			p.admittedN += n
			continue
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
	p.mu.Lock()
	if p.closed {
		p.mu.Unlock()
		return -1, fmt.Errorf("preview closed")
	}
	if err := p.ctx.Err(); err != nil {
		p.mu.Unlock()
		return -1, fmt.Errorf("preview session expired: %w", err)
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
	if err := p.admitRange(offset, int64(len(data))); err != nil {
		p.mu.Unlock()
		return -1, err
	}
	p.publishDemand()
	if p.waitCancel != nil {
		p.waitCancel()
		p.waitCancel = nil
	}
	p.readSeq++
	seq := p.readSeq
	abs := p.file.Offset() + offset
	st := p.storage
	sessionCtx := p.ctx
	p.mu.Unlock()

	if st != nil && st.RangeComplete(abs, int64(len(data))) {
		if spans, ok := st.byteSpans(abs, int64(len(data))); ok {
			n, err := preadFileSpans(spans, data)
			if err != nil {
				return -1, err
			}
			return n, nil
		}
		// Stream/RAM path: copy from verified cache without torrent.Reader.
		if n, ok := st.readVerifiedPreview(data, abs); ok {
			return n, nil
		}
	}

	p.readIO.Lock()
	defer p.readIO.Unlock()
	p.mu.Lock()
	if p.closed || p.readSeq != seq {
		p.mu.Unlock()
		return -1, fmt.Errorf("preview closed")
	}
	if err := sessionCtx.Err(); err != nil {
		p.mu.Unlock()
		return -1, fmt.Errorf("preview session expired: %w", err)
	}
	ctx, cancel := context.WithCancel(sessionCtx)
	p.waitCancel = cancel
	p.reader.SetContext(ctx)
	if _, err := p.reader.Seek(offset, io.SeekStart); err != nil {
		cancel()
		p.waitCancel = nil
		p.mu.Unlock()
		return -1, err
	}
	p.mu.Unlock()

	n, err := io.ReadFull(p.reader, data)
	p.mu.Lock()
	if p.waitCancel != nil {
		p.waitCancel()
		p.waitCancel = nil
	}
	closed := p.closed || p.readSeq != seq
	p.mu.Unlock()
	if closed {
		return -1, fmt.Errorf("preview closed")
	}
	if err == io.EOF || err == io.ErrUnexpectedEOF {
		return n, nil
	}
	if err != nil {
		return -1, err
	}
	return n, nil
}

func (p *previewReader) Close() error {
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
		st.releasePreviewTemp()
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
