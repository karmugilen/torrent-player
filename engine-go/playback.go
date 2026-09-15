package main

import (
	"context"
	"fmt"
	"io"
	"sync"
	"time"

	"github.com/anacrolix/torrent"
)

// Each Android proxy descriptor owns a reader. Its cursor/readahead survive
// successive reads, and closing the descriptor cancels any blocked read.
type playbackReader struct {
	mu     sync.Mutex
	reader torrent.Reader
	length int64
	ctx    context.Context
	cancel context.CancelFunc
	closed bool
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
	if !rec.Configured || !selected || index < 0 || index >= len(files) {
		return nil, fmt.Errorf("file is not configured for playback")
	}
	reader := files[index].NewReader()
	reader.SetReadaheadFunc(nil)
	reader.SetReadahead(20 * 1024 * 1024)
	ctx, cancel := context.WithCancel(context.Background())
	return &playbackReader{reader: reader, length: files[index].Length(), ctx: ctx, cancel: cancel}, nil
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
	ctx, cancel := context.WithTimeout(p.ctx, 45*time.Second)
	defer cancel()
	p.reader.SetContext(ctx)
	if _, err := p.reader.Seek(offset, io.SeekStart); err != nil {
		return 0, err
	}
	// Android's proxy callback requires the requested length except at EOF.
	// Never translate missing torrent bytes into zeros or premature EOF.
	return io.ReadFull(p.reader, data)
}

func (p *playbackReader) Close() {
	p.cancel()
	p.mu.Lock()
	defer p.mu.Unlock()
	if !p.closed {
		p.closed = true
		_ = p.reader.Close()
	}
}
