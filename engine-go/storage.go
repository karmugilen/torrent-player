package main

import (
	"context"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sync"
	"syscall"

	"github.com/anacrolix/torrent/metainfo"
	"github.com/anacrolix/torrent/storage"
)

const DefaultMaxCacheBytes = 48 * 1024 * 1024

type DocumentStorageClient struct {
	mu       sync.Mutex
	baseDir  string
	torrents map[string]*DocumentTorrentStorage
}

func NewDocumentStorageClient(baseDir string) *DocumentStorageClient {
	return &DocumentStorageClient{baseDir: baseDir, torrents: make(map[string]*DocumentTorrentStorage)}
}

func (c *DocumentStorageClient) OpenTorrent(_ context.Context, info *metainfo.Info, hash metainfo.Hash) (storage.TorrentImpl, error) {
	c.mu.Lock()
	key := hash.HexString()
	st := c.torrents[key]
	if st == nil || st.isClosed() {
		st = newDocumentTorrentStorage(info, hash, c.baseDir)
		c.torrents[key] = st
	} else if info != nil {
		st.updateInfo(info)
	}
	c.mu.Unlock()
	return storage.TorrentImpl{
		Piece: func(p metainfo.Piece) storage.PieceImpl { return &documentPiece{storage: st, piece: p} },
		Close: func() error {
			err := st.Close()
			c.mu.Lock()
			if c.torrents[key] == st {
				delete(c.torrents, key)
			}
			c.mu.Unlock()
			return err
		},
	}, nil
}

func (c *DocumentStorageClient) GetStorage(key string) *DocumentTorrentStorage {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.torrents[key]
}

type FileDescriptor struct {
	File           *os.File
	Offset, Length int64
	Path           string
}
type DocumentTorrentStorage struct {
	mu                        sync.Mutex
	info                      *metainfo.Info
	infoHash                  metainfo.Hash
	baseDir                   string
	chunkLength, totalLength  int64
	files                     []FileDescriptor
	attached, closed          bool
	needsVerify               bool
	cache                     map[int][]byte
	cacheBytes, maxCacheBytes int64
	completed                 map[int]bool
}

func newDocumentTorrentStorage(info *metainfo.Info, hash metainfo.Hash, baseDir string) *DocumentTorrentStorage {
	st := &DocumentTorrentStorage{infoHash: hash, baseDir: baseDir, maxCacheBytes: DefaultMaxCacheBytes, cache: make(map[int][]byte), completed: make(map[int]bool)}
	if info != nil {
		st.updateInfo(info)
	}
	return st
}

func (st *DocumentTorrentStorage) isClosed() bool {
	st.mu.Lock()
	defer st.mu.Unlock()
	return st.closed
}
func (st *DocumentTorrentStorage) updateInfo(info *metainfo.Info) {
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.info != nil || info == nil || st.closed {
		return
	}
	st.info, st.chunkLength, st.totalLength = info, info.PieceLength, info.TotalLength()
	st.files = make([]FileDescriptor, len(info.UpvertedFiles()))
	var offset int64
	for i, f := range info.UpvertedFiles() {
		st.files[i] = FileDescriptor{Offset: offset, Length: f.Length, Path: filepath.Join(f.Path...)}
		offset += f.Length
	}
}

// AttachDescriptors commits only after every selected descriptor and every
// private boundary file is ready. Boundary files retain bytes from unselected
// files when a torrent piece crosses a file boundary.
func (st *DocumentTorrentStorage) AttachDescriptors(descriptors []*int, selected []int) error {
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.closed {
		return fmt.Errorf("storage closed")
	}
	if st.info == nil {
		return fmt.Errorf("metadata not ready")
	}
	if st.attached {
		return fmt.Errorf("storage already configured")
	}
	if len(descriptors) != len(st.files) || len(selected) == 0 {
		return fmt.Errorf("file descriptors do not match torrent selection")
	}
	chosen := make(map[int]bool, len(selected))
	for _, i := range selected {
		if i < 0 || i >= len(st.files) || chosen[i] {
			return fmt.Errorf("invalid selected file index %d", i)
		}
		chosen[i] = true
	}
	for i, fd := range descriptors {
		if chosen[i] != (fd != nil) || (fd != nil && *fd < 0) {
			return fmt.Errorf("descriptor for file %d does not match selection", i)
		}
	}

	opened := make([]*os.File, len(st.files))
	cleanup := func() {
		for _, f := range opened {
			if f != nil {
				_ = f.Close()
			}
		}
	}
	for i := range st.files {
		var f *os.File
		if chosen[i] {
			dup, err := syscall.Dup(*descriptors[i])
			if err != nil {
				cleanup()
				return fmt.Errorf("duplicate descriptor %d: %w", i, err)
			}
			f = os.NewFile(uintptr(dup), fmt.Sprintf("torrent-file-%d", i))
			if stat, err := f.Stat(); err != nil {
				_ = f.Close()
				cleanup()
				return fmt.Errorf("inspect file %d: %w", i, err)
			} else if stat.Size() > 0 {
				st.needsVerify = true
			}
		} else {
			path := filepath.Join(st.baseDir, ".boundary", st.infoHash.HexString(), fmt.Sprintf("%d.part", i))
			if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
				cleanup()
				return err
			}
			var err error
			f, err = os.OpenFile(path, os.O_RDWR|os.O_CREATE, 0600)
			if err != nil {
				cleanup()
				return fmt.Errorf("open boundary storage: %w", err)
			}
		}
		if err := f.Truncate(st.files[i].Length); err != nil {
			_ = f.Close()
			cleanup()
			return fmt.Errorf("size file %d: %w", i, err)
		}
		opened[i] = f
	}
	for i := range st.files {
		st.files[i].File = opened[i]
	}
	st.attached = true
	for index, data := range st.cache {
		if _, err := st.writeToFiles(int64(index)*st.chunkLength, data); err != nil {
			for i := range st.files {
				_ = st.files[i].File.Close()
				st.files[i].File = nil
			}
			st.attached = false
			return fmt.Errorf("flush prepared data: %w", err)
		}
	}
	st.cache, st.cacheBytes = make(map[int][]byte), 0
	return nil
}

func (st *DocumentTorrentStorage) NeedsVerify() bool {
	st.mu.Lock()
	defer st.mu.Unlock()
	return st.needsVerify
}

func (st *DocumentTorrentStorage) writeToFiles(start int64, data []byte) (int, error) {
	end, done := start+int64(len(data)), 0
	for _, f := range st.files {
		from, to := maxInt64(start, f.Offset), minInt64(end, f.Offset+f.Length)
		if from >= to {
			continue
		}
		if f.File == nil {
			return done, fmt.Errorf("storage for %s is not configured", f.Path)
		}
		chunk := data[from-start : to-start]
		n, err := f.File.WriteAt(chunk, from-f.Offset)
		done += n
		if err != nil {
			return done, err
		}
		if n != len(chunk) {
			return done, io.ErrShortWrite
		}
	}
	if done != len(data) {
		return done, io.ErrShortWrite
	}
	return done, nil
}

func (st *DocumentTorrentStorage) readFromFiles(start int64, buf []byte) (int, error) {
	end, done := start+int64(len(buf)), 0
	for _, f := range st.files {
		from, to := maxInt64(start, f.Offset), minInt64(end, f.Offset+f.Length)
		if from >= to {
			continue
		}
		if f.File == nil {
			return done, fmt.Errorf("storage for %s is not configured", f.Path)
		}
		chunk := buf[from-start : to-start]
		n, err := f.File.ReadAt(chunk, from-f.Offset)
		done += n
		if err != nil {
			return done, err
		}
		if n != len(chunk) {
			return done, io.ErrUnexpectedEOF
		}
	}
	if done != len(buf) {
		return done, io.ErrUnexpectedEOF
	}
	return done, nil
}

func (st *DocumentTorrentStorage) Close() error {
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.closed {
		return nil
	}
	st.closed = true
	var first error
	for i := range st.files {
		if st.files[i].File != nil {
			if err := st.files[i].File.Close(); err != nil && first == nil {
				first = err
			}
			st.files[i].File = nil
		}
	}
	st.attached, st.cache, st.completed, st.cacheBytes = false, nil, nil, 0
	return first
}

type documentPiece struct {
	storage *DocumentTorrentStorage
	piece   metainfo.Piece
}

func (p *documentPiece) ReadAt(b []byte, off int64) (int, error) {
	st := p.storage
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.closed {
		return 0, fmt.Errorf("storage closed")
	}
	index := p.piece.Index()
	if data, ok := st.cache[index]; ok {
		if off >= int64(len(data)) {
			return 0, io.EOF
		}
		n := copy(b, data[off:])
		if n != len(b) {
			return n, io.EOF
		}
		return n, nil
	}
	if !st.attached {
		return 0, fmt.Errorf("download storage is not configured")
	}
	return st.readFromFiles(int64(index)*st.chunkLength+off, b)
}
func (p *documentPiece) WriteAt(b []byte, off int64) (int, error) {
	st := p.storage
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.closed {
		return 0, fmt.Errorf("storage closed")
	}
	index, expected := p.piece.Index(), int64(p.piece.Length())
	if off < 0 || off+int64(len(b)) > expected {
		return 0, fmt.Errorf("invalid piece write")
	}
	if st.attached {
		return st.writeToFiles(int64(index)*st.chunkLength+off, b)
	}
	data, ok := st.cache[index]
	if !ok {
		if st.cacheBytes+expected > st.maxCacheBytes {
			return 0, fmt.Errorf("prepared data cache is full")
		}
		data = make([]byte, expected)
		st.cache[index] = data
		st.cacheBytes += expected
	}
	copy(data[off:], b)
	return len(b), nil
}
func (p *documentPiece) MarkComplete() error {
	p.storage.mu.Lock()
	defer p.storage.mu.Unlock()
	if p.storage.closed {
		return fmt.Errorf("storage closed")
	}
	p.storage.completed[p.piece.Index()] = true
	return nil
}
func (p *documentPiece) MarkNotComplete() error {
	p.storage.mu.Lock()
	defer p.storage.mu.Unlock()
	if !p.storage.closed {
		delete(p.storage.completed, p.piece.Index())
	}
	return nil
}
func (p *documentPiece) Completion() storage.Completion {
	p.storage.mu.Lock()
	defer p.storage.mu.Unlock()
	if p.storage.closed {
		return storage.Completion{Err: fmt.Errorf("storage closed")}
	}
	value, known := p.storage.completed[p.piece.Index()]
	return storage.Completion{Complete: value, Ok: known}
}
func maxInt64(a, b int64) int64 {
	if a > b {
		return a
	}
	return b
}
func minInt64(a, b int64) int64 {
	if a < b {
		return a
	}
	return b
}
