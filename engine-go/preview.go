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
	previewBudgetBytes = int64(8 << 20)
	previewAttempt     = 10 * time.Second
)

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
	reader     torrent.Reader
	storage    *DocumentTorrentStorage
	owner      *playbackPriorityCoordinator
	ctx        context.Context
	cancel     context.CancelFunc
	length     int64
	budget     int64
	admitted   map[int]bool
	closed     bool
	temp       bool
	waitCancel context.CancelFunc
	readSeq    uint64
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

func (s *EngineServer) OpenPreview(id string, index int) (PreviewReader, error) {
	s.mu.Lock()
	if s.previewActive != nil {
		s.mu.Unlock()
		return nil, fmt.Errorf("preview already active")
	}
	rec := s.records[id]
	s.mu.Unlock()
	if rec == nil {
		return nil, fmt.Errorf("torrent not found")
	}
	rec.mu.Lock()
	defer rec.mu.Unlock()
	if rec.removed || s.shuttingDown.Load() || rec.Paused {
		return nil, fmt.Errorf("torrent unavailable for preview")
	}
	if rec.Torrent.Info() == nil {
		return nil, fmt.Errorf("metadata not ready")
	}
	files := rec.Torrent.Files()
	if index < 0 || index >= len(files) {
		return nil, fmt.Errorf("invalid file index")
	}
	file := files[index]
	st := s.storage.GetStorage(rec.InfoHash)
	if st == nil {
		return nil, fmt.Errorf("storage unavailable")
	}

	temp := false
	if !rec.Configured {
		if err := st.attachPreviewTemp(index); err != nil {
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
			return nil, fmt.Errorf("file is not selected for preview")
		}
	}

	ctx, cancel := context.WithCancel(context.Background())
	reader := file.NewReader()
	reader.SetReadaheadFunc(nil)
	reader.SetReadahead(min(previewBudgetBytes, file.Length()))
	owner := s.ensurePlaybackCoordinator(rec.Torrent)
	p := &previewReader{
		server:   s,
		rec:      rec,
		torrent:  rec.Torrent,
		file:     file,
		reader:   reader,
		storage:  st,
		owner:    owner,
		ctx:      ctx,
		cancel:   cancel,
		length:   file.Length(),
		budget:   previewBudgetBytes,
		admitted: make(map[int]bool),
		temp:     temp,
	}
	begin, end := filePieceRange(file, 0, min(file.Length(), previewBudgetBytes))
	demand := make(map[int]torrent.PiecePriority, end-begin)
	info := rec.Torrent.Info()
	var admitted int64
	for i := begin; i < end; i++ {
		n := info.Piece(i).Length()
		if admitted+n > previewBudgetBytes {
			break
		}
		demand[i] = torrent.PiecePriorityHigh
		p.admitted[i] = true
		admitted += n
	}
	owner.setPreviewDemand(demand)
	s.mu.Lock()
	s.previewActive = p
	s.mu.Unlock()
	return p, nil
}

func (p *previewReader) Length() int64 { return p.length }

func (p *previewReader) ReadAt(data []byte, offset int64) (int, error) {
	p.mu.Lock()
	if p.closed {
		p.mu.Unlock()
		return -1, fmt.Errorf("preview closed")
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
	if p.waitCancel != nil {
		p.waitCancel()
		p.waitCancel = nil
	}
	p.readSeq++
	seq := p.readSeq
	abs := p.file.Offset() + offset
	st := p.storage
	p.mu.Unlock()

	if st != nil && st.RangeComplete(abs, int64(len(data))) {
		if spans, ok := st.byteSpans(abs, int64(len(data))); ok {
			n, err := preadFileSpans(spans, data)
			if err != nil {
				return -1, err
			}
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
	ctx, cancel := context.WithTimeout(p.ctx, previewAttempt)
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
	p.cancel()
	reader := p.reader
	owner := p.owner
	temp := p.temp
	st := p.storage
	rec := p.rec
	server := p.server
	p.mu.Unlock()

	if reader != nil {
		_ = reader.Close()
	}
	if owner != nil {
		owner.setPreviewDemand(nil)
	}
	if temp && st != nil {
		st.releasePreviewTemp()
		// Draft previews temporarily allow payload. Callers that hold rec.mu
		// (pause/configure) restore transfer policy via applyTransferState.
		if rec != nil {
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
