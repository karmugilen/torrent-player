package main

import (
	"context"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sync"
	"sync/atomic"
	"syscall"
	"time"

	"github.com/anacrolix/torrent/metainfo"
	"github.com/anacrolix/torrent/storage"
)

// DefaultMaxCacheBytes is the Download RAM piece buffer (write-back). Peers
// land in RAM first; disk flush runs asynchronously so MediaStore I/O does not
// stall receive.
const DefaultMaxCacheBytes = 64 * 1024 * 1024

// DefaultStreamCacheMaxBytes caps Watch-mode pieces kept in RAM only (no
// .stream disk files). Pieces farthest from the playhead are dropped first.
const DefaultStreamCacheMaxBytes = 50 * 1024 * 1024

// Keep at least this much around the playhead when trimming Watch RAM.
const streamCacheKeepBytes int64 = 24 * 1024 * 1024

const pieceDiskFlushDelay = 20 * time.Millisecond

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
		Piece: func(p metainfo.Piece) storage.PieceImpl {
			base := &documentPiece{storage: st, piece: p}
			if !st.RestoreVerifyEnabled() {
				return base
			}
			// Hybrid/v2 pieces need library zero-padding when hashed; keep the
			// plain type so anacrolix owns that path. Pure v1 can SelfHash.
			st.mu.Lock()
			hasV2 := st.info != nil && st.info.HasV2()
			st.mu.Unlock()
			if hasV2 {
				return base
			}
			return &restoreVerifyPiece{documentPiece: base}
		},
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
	// external means Android supplied the descriptor. Boundary files are
	// engine-owned scratch storage used for pieces shared with unselected files.
	external bool
}
type DocumentTorrentStorage struct {
	mu                        sync.Mutex
	info                      *metainfo.Info
	infoHash                  metainfo.Hash
	baseDir                   string
	chunkLength, totalLength  int64
	files                     []FileDescriptor
	attached, closed          bool
	// stream is true when selected files are stored under .stream/ instead of
	// Android document descriptors (Watch mode).
	stream                    bool
	needsVerify               bool
	restoreVerify             atomic.Bool
	// freezeTrusted blocks MarkNotComplete from clearing checkpoint-restored
	// bits until saved-data checking finishes (or UnfreezeTrusted is called).
	freezeTrusted             atomic.Bool
	cache                     map[int][]byte
	cacheDirty                map[int]bool
	cacheOrder                []int // LRU: index 0 is oldest
	cacheBytes, maxCacheBytes int64
	streamMaxBytes            int64
	lastReadOffset            int64
	completed                 map[int]bool
	checkpointDirty           bool
	checkpointFlushing        bool
	checkpointGen             uint64
	pendingSync               map[*os.File]struct{}
	diskFlushing              bool
	identities                []fileIdentity
}

func streamDir(baseDir, infoHashHex string) string {
	return filepath.Join(baseDir, ".stream", infoHashHex)
}

func boundaryDir(baseDir, infoHashHex string) string {
	return filepath.Join(baseDir, ".boundary", infoHashHex)
}

func engineOwnedPartPath(baseDir, infoHashHex string, fileIndex int, streamSelected bool) string {
	root := boundaryDir(baseDir, infoHashHex)
	if streamSelected {
		root = streamDir(baseDir, infoHashHex)
	}
	return filepath.Join(root, fmt.Sprintf("%d.part", fileIndex))
}

// DeleteEngineOwnedStorage removes Watch (.stream) and boundary scratch files
// for one info-hash. Safe when the directories do not exist.
func DeleteEngineOwnedStorage(baseDir, infoHashHex string) error {
	var first error
	for _, dir := range []string{streamDir(baseDir, infoHashHex), boundaryDir(baseDir, infoHashHex)} {
		if err := os.RemoveAll(dir); err != nil && !os.IsNotExist(err) && first == nil {
			first = err
		}
	}
	return first
}

func newDocumentTorrentStorage(info *metainfo.Info, hash metainfo.Hash, baseDir string) *DocumentTorrentStorage {
	st := &DocumentTorrentStorage{
		infoHash:       hash,
		baseDir:        baseDir,
		maxCacheBytes:  DefaultMaxCacheBytes,
		streamMaxBytes: DefaultStreamCacheMaxBytes,
		cache:          make(map[int][]byte),
		cacheDirty:     make(map[int]bool),
		completed:   make(map[int]bool),
		pendingSync: make(map[*os.File]struct{}),
	}
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

// AttachDescriptors commits only after every supplied descriptor and every
// private engine-owned file is ready. Descriptors may be supplied for files
// that are currently unselected: selection controls network demand, while
// keeping the descriptor attached preserves saved bytes and permits later
// reselection or verified local playback after a restart. Boundary files
// retain bytes from files for which Android has not supplied a document
// descriptor. Stream mode stores selected files under .stream/ and rejects
// external descriptors.
func (st *DocumentTorrentStorage) AttachDescriptors(descriptors []*int, selected []int) error {
	return st.attachDescriptors(descriptors, selected, false)
}

// AttachDescriptorsMode is AttachDescriptors with an explicit storage mode.
// stream=true opens selected nil-descriptor files under .stream/.
func (st *DocumentTorrentStorage) AttachDescriptorsMode(descriptors []*int, selected []int, stream bool) error {
	return st.attachDescriptors(descriptors, selected, stream)
}

func (st *DocumentTorrentStorage) attachDescriptors(descriptors []*int, selected []int, stream bool) error {
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
	if len(descriptors) != len(st.files) {
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
		if fd != nil && *fd < 0 {
			return fmt.Errorf("invalid descriptor for file %d", i)
		}
		if stream && fd != nil {
			return fmt.Errorf("stream storage rejects external descriptors for file %d", i)
		}
	}

	opened := make([]*os.File, len(st.files))
	preIdentities := make([]fileIdentity, len(st.files))
	preOK := true
	cleanup := func() {
		for _, f := range opened {
			if f != nil {
				_ = f.Close()
			}
		}
	}
	for i := range st.files {
		var f *os.File
		// A descriptor is a durable document destination independently of
		// current download selection. Selection is applied by the engine's file
		// priorities, not by replacing the backing file with scratch storage.
		if descriptors[i] != nil {
			dup, err := syscall.Dup(*descriptors[i])
			if err != nil {
				cleanup()
				return fmt.Errorf("duplicate descriptor %d: %w", i, err)
			}
			f = os.NewFile(uintptr(dup), fmt.Sprintf("torrent-file-%d", i))
			stat, err := f.Stat()
			if err != nil {
				_ = f.Close()
				cleanup()
				return fmt.Errorf("inspect file %d: %w", i, err)
			}
			if stat.Size() > 0 {
				st.needsVerify = true
			}
			// Identity for checkpoint matching must be sampled before Truncate:
			// some filesystems bump mtime even when the size is unchanged.
			id, ok := identityFromStat(st.files[i], true, stat)
			preIdentities[i] = id
			if !ok || stat.Size() != st.files[i].Length {
				preOK = false
			}
		} else if stream {
			// Watch mode is RAM-only: no .stream / boundary disk files.
			opened[i] = nil
			preOK = false
			continue
		} else {
			path := engineOwnedPartPath(st.baseDir, st.infoHash.HexString(), i, false)
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
			stat, err := f.Stat()
			if err != nil {
				_ = f.Close()
				cleanup()
				return fmt.Errorf("inspect engine-owned file %d: %w", i, err)
			}
			id, ok := identityFromStat(st.files[i], false, stat)
			preIdentities[i] = id
			if !ok || stat.Size() != st.files[i].Length {
				preOK = false
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
		st.files[i].external = descriptors[i] != nil
	}
	st.stream = stream
	st.attached = true
	if stream {
		// Watch: keep prepared pieces in RAM and cap at the stream budget.
		st.maxCacheBytes = st.streamMaxBytes
		if st.maxCacheBytes <= 0 {
			st.maxCacheBytes = DefaultStreamCacheMaxBytes
		}
		st.cacheDirty = make(map[int]bool)
		st.cacheOrder = st.cacheOrder[:0]
		for index := range st.cache {
			st.cacheDirty[index] = false
			st.touchCacheLocked(index)
		}
		for st.cacheBytes > st.maxCacheBytes && len(st.cacheOrder) > 0 {
			st.dropCacheLocked(st.cacheOrder[0])
		}
	} else {
		for index, data := range st.cache {
			if _, err := st.writeToFiles(int64(index)*st.chunkLength, data); err != nil {
				for i := range st.files {
					if st.files[i].File != nil {
						_ = st.files[i].File.Close()
					}
					st.files[i].File = nil
				}
				st.attached = false
				st.stream = false
				return fmt.Errorf("flush prepared data: %w", err)
			}
		}
		// Prepared bytes are on disk; keep them in the hot RAM cache when they fit.
		st.cacheDirty = make(map[int]bool)
		st.cacheOrder = st.cacheOrder[:0]
		for index := range st.cache {
			st.touchCacheLocked(index)
		}
		if st.cacheBytes > st.maxCacheBytes {
			st.cache, st.cacheBytes, st.cacheOrder = make(map[int][]byte), 0, nil
		}
	}
	loaded := false
	if preOK {
		loaded = st.loadCheckpointLocked(preIdentities)
	}
	st.captureIdentitiesLocked()
	if loaded {
		// Rebind the durable identity to the post-truncate view so the next
		// resume still matches even when Truncate bumps mtime.
		st.checkpointDirty = true
		st.scheduleCheckpointFlushLocked()
		// Pre-configure hashing can still fail asynchronously and call
		// MarkNotComplete; keep restored bits until checking publishes them.
		st.freezeTrusted.Store(true)
	}
	return nil
}

// UpdateSelection changes the files that receive torrent data after the
// initial configure call. Android supplies descriptors for newly selected
// files. Existing external files remain open when deselected so their saved
// bytes and playback URI survive; they simply receive no piece priority.
//
// The operation is transactional: descriptors are duplicated and prepared
// before the storage table changes, and verified pieces already present in an
// engine-owned boundary file are copied to each newly selected destination.
func (st *DocumentTorrentStorage) UpdateSelection(descriptors []*int, selected []int) error {
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.closed {
		return fmt.Errorf("storage closed")
	}
	if st.info == nil {
		return fmt.Errorf("metadata not ready")
	}
	if !st.attached {
		return fmt.Errorf("storage is not configured")
	}
	if len(descriptors) != len(st.files) {
		return fmt.Errorf("file descriptors do not match torrent files")
	}
	chosen := make(map[int]bool, len(selected))
	for _, i := range selected {
		if i < 0 || i >= len(st.files) || chosen[i] {
			return fmt.Errorf("invalid selected file index %d", i)
		}
		chosen[i] = true
	}

	old := st.files
	next := append([]FileDescriptor(nil), old...)
	type replacement struct {
		index int
		file  *os.File
	}
	var opened []replacement
	cleanup := func() {
		for _, item := range opened {
			_ = item.file.Close()
		}
	}
	for i := range old {
		// A selected boundary file needs an Android destination (download mode).
		// Stream mode keeps engine-owned handles; a selected external file may
		// omit its descriptor to keep the existing handle.
		if !chosen[i] || (old[i].external || st.stream) && descriptors[i] == nil {
			continue
		}
		if st.stream {
			cleanup()
			return fmt.Errorf("stream storage rejects external descriptors for file %d", i)
		}
		if descriptors[i] == nil || *descriptors[i] < 0 {
			cleanup()
			return fmt.Errorf("descriptor for newly selected file %d is required", i)
		}
		dup, err := syscall.Dup(*descriptors[i])
		if err != nil {
			cleanup()
			return fmt.Errorf("duplicate descriptor %d: %w", i, err)
		}
		f := os.NewFile(uintptr(dup), fmt.Sprintf("torrent-file-%d", i))
		if f == nil {
			_ = syscall.Close(dup)
			cleanup()
			return fmt.Errorf("open descriptor %d", i)
		}
		stat, err := f.Stat()
		if err != nil {
			_ = f.Close()
			cleanup()
			return fmt.Errorf("inspect file %d: %w", i, err)
		}
		if sameFile(old[i].File, f) {
			// The caller may send descriptors for every selected file. Keep the
			// already owned duplicate and close this temporary duplicate.
			_ = f.Close()
			continue
		}
		// A new destination is rebuilt from verified pieces below. This avoids
		// accepting arbitrary stale bytes as torrent data.
		if !sameFile(old[i].File, f) {
			if err := f.Truncate(0); err != nil {
				_ = f.Close()
				cleanup()
				return fmt.Errorf("clear file %d: %w", i, err)
			}
			if err := f.Truncate(old[i].Length); err != nil {
				_ = f.Close()
				cleanup()
				return fmt.Errorf("size file %d: %w", i, err)
			}
		}
		_ = stat // stat is intentionally obtained to validate the descriptor.
		opened = append(opened, replacement{index: i, file: f})
		next[i].File = f
		next[i].external = true
	}

	// Copy only pieces already known complete. Partial data is deliberately
	// discarded; it must be fetched and hash-checked again after reselection.
	for _, item := range opened {
		if sameFile(old[item.index].File, item.file) {
			continue
		}
		for pieceIndex := range st.completed {
			piece := st.info.Piece(pieceIndex)
			pieceStart := int64(pieceIndex) * st.chunkLength
			from := maxInt64(pieceStart, old[item.index].Offset)
			to := minInt64(pieceStart+int64(piece.Length()), old[item.index].Offset+old[item.index].Length)
			if from >= to {
				continue
			}
			data := make([]byte, to-from)
			if _, err := readFromFilesOn(old, from, data); err != nil {
				cleanup()
				return fmt.Errorf("copy saved piece %d: %w", pieceIndex, err)
			}
			if _, err := writeToFilesOn(next, from, data); err != nil {
				cleanup()
				return fmt.Errorf("restore saved piece %d: %w", pieceIndex, err)
			}
		}
	}

	st.files = next
	for _, item := range opened {
		if old[item.index].File != nil && !sameFile(old[item.index].File, item.file) {
			_ = old[item.index].File.Close()
		}
		// New destination bytes are rebuilt only for pieces already known
		// complete; drop completion for pieces that touch replaced files so a
		// stale checkpoint cannot skip hashing them later.
		if !sameFile(old[item.index].File, item.file) {
			st.clearCompletedOverlappingLocked(item.index)
		}
	}
	st.captureIdentitiesLocked()
	st.checkpointDirty = true
	st.scheduleCheckpointFlushLocked()
	return nil
}

func sameFile(a, b *os.File) bool {
	if a == nil || b == nil {
		return false
	}
	sa, ea := a.Stat()
	sb, eb := b.Stat()
	if ea != nil || eb != nil {
		return false
	}
	// Device/inode identifies the same document provider backing file on
	// normal Android/Linux descriptors. Fall back to pointer identity.
	if ia, ok := sa.Sys().(*syscall.Stat_t); ok {
		if ib, ok := sb.Sys().(*syscall.Stat_t); ok {
			return ia.Dev == ib.Dev && ia.Ino == ib.Ino
		}
	}
	return a == b
}

func readFromFilesOn(files []FileDescriptor, start int64, buf []byte) (int, error) {
	end, done := start+int64(len(buf)), 0
	for _, f := range files {
		from, to := maxInt64(start, f.Offset), minInt64(end, f.Offset+f.Length)
		if from >= to {
			continue
		}
		if f.File == nil {
			return done, fmt.Errorf("storage for %s is not configured", f.Path)
		}
		n, err := f.File.ReadAt(buf[from-start:to-start], from-f.Offset)
		done += n
		if err != nil && !(err == io.EOF && n == int(to-from)) {
			return done, err
		}
		if n != int(to-from) {
			return done, io.ErrUnexpectedEOF
		}
	}
	if done != len(buf) {
		return done, io.ErrUnexpectedEOF
	}
	return done, nil
}

func writeToFilesOn(files []FileDescriptor, start int64, data []byte) (int, error) {
	end, done := start+int64(len(data)), 0
	for _, f := range files {
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

func (st *DocumentTorrentStorage) NeedsVerify() bool {
	st.mu.Lock()
	defer st.mu.Unlock()
	return st.needsVerify
}

// MarkVerified records that the currently attached destinations have been
// checked. A later failed check intentionally leaves this flag set so a
// resume can retry; successful completion is the only point at which the
// initial restore obligation is consumed.
func (st *DocumentTorrentStorage) MarkVerified() {
	st.mu.Lock()
	for st.checkpointFlushing {
		st.mu.Unlock()
		time.Sleep(5 * time.Millisecond)
		st.mu.Lock()
	}
	st.needsVerify = false
	st.checkpointDirty = true
	st.checkpointGen++
	st.checkpointFlushing = true
	st.mu.Unlock()
	st.drainSyncAndCheckpoint()
}

// PieceComplete reports whether storage already trusts a piece (including bits
// restored from a valid checkpoint). Used to skip re-hashing on resume.
func (st *DocumentTorrentStorage) PieceComplete(index int) bool {
	st.mu.Lock()
	defer st.mu.Unlock()
	return st.completed[index]
}

func (st *DocumentTorrentStorage) DeleteCheckpoint() error {
	st.mu.Lock()
	defer st.mu.Unlock()
	st.checkpointDirty = false
	st.identities = nil
	return deletePieceCheckpoint(st.baseDir, st.infoHash.HexString())
}

func (c *DocumentStorageClient) DeleteCheckpoint(infoHashHex string) error {
	c.mu.Lock()
	base := c.baseDir
	st := c.torrents[infoHashHex]
	c.mu.Unlock()
	if st != nil {
		return st.DeleteCheckpoint()
	}
	return deletePieceCheckpoint(base, infoHashHex)
}

func (st *DocumentTorrentStorage) SetRestoreVerify(enabled bool) {
	st.restoreVerify.Store(enabled)
}

func (st *DocumentTorrentStorage) RestoreVerifyEnabled() bool {
	return st.restoreVerify.Load()
}

// SelectionCanUseExistingFiles reports whether every requested file already
// has a usable destination. Download mode requires Android-supplied
// descriptors. Stream mode accepts any already-open engine-owned file so
// /select can change demand without new FDs.
func (st *DocumentTorrentStorage) SelectionCanUseExistingFiles(selected []int) bool {
	st.mu.Lock()
	defer st.mu.Unlock()
	if !st.attached {
		return false
	}
	for _, i := range selected {
		if i < 0 || i >= len(st.files) || st.files[i].File == nil {
			return false
		}
		if !st.stream && !st.files[i].external {
			return false
		}
	}
	return true
}

// StreamMode reports whether this torrent was configured for Watch/temp cache.
func (st *DocumentTorrentStorage) StreamMode() bool {
	st.mu.Lock()
	defer st.mu.Unlock()
	return st.stream
}

// UnfreezeTrusted allows MarkNotComplete to clear pieces again after restore
// checking has published checkpoint bits (or when no check is needed).
func (st *DocumentTorrentStorage) UnfreezeTrusted() {
	st.freezeTrusted.Store(false)
}

func (st *DocumentTorrentStorage) occupiedBytesLocked() int64 {
	var total int64
	for _, f := range st.files {
		n, err := fileOccupiedBytes(f.File)
		if err != nil {
			continue
		}
		total += n
	}
	return total
}

// trimStreamCacheLocked discards completed Watch pieces farthest from the
// latest read until occupied disk use is at or under streamMaxBytes. Caller
// holds st.mu.
func (st *DocumentTorrentStorage) trimStreamCacheLocked() {
	if !st.stream || !st.attached || st.info == nil || st.chunkLength <= 0 {
		return
	}
	maxBytes := st.streamMaxBytes
	if maxBytes <= 0 {
		maxBytes = DefaultStreamCacheMaxBytes
	}
	keep := streamCacheKeepBytes
	if keep > maxBytes {
		keep = maxBytes
	}
	anchor := st.lastReadOffset
	if anchor < 0 {
		anchor = 0
	}
	keepStart := anchor - keep/4
	if keepStart < 0 {
		keepStart = 0
	}
	keepEnd := keepStart + keep
	if keepEnd > st.totalLength {
		keepEnd = st.totalLength
		keepStart = keepEnd - keep
		if keepStart < 0 {
			keepStart = 0
		}
	}

	for st.occupiedBytesLocked() > maxBytes {
		victim := -1
		victimDist := int64(-1)
		for index, ok := range st.completed {
			if !ok {
				continue
			}
			start := int64(index) * st.chunkLength
			end := start + st.info.Piece(index).Length()
			if end > keepStart && start < keepEnd {
				continue
			}
			mid := start + (end-start)/2
			dist := mid - anchor
			if dist < 0 {
				dist = -dist
			}
			if dist > victimDist {
				victim, victimDist = index, dist
			}
		}
		if victim < 0 {
			return
		}
		if err := st.discardPieceLocked(victim); err != nil {
			return
		}
	}
}

func (st *DocumentTorrentStorage) discardPieceLocked(index int) error {
	if st.info == nil || st.chunkLength <= 0 {
		return nil
	}
	piece := st.info.Piece(index)
	start := int64(index) * st.chunkLength
	end := start + piece.Length()
	for _, f := range st.files {
		if f.File == nil || f.external {
			continue
		}
		from, to := maxInt64(start, f.Offset), minInt64(end, f.Offset+f.Length)
		if from >= to {
			continue
		}
		if ok, err := punchHole(f.File, from-f.Offset, to-from); err != nil {
			return err
		} else if !ok {
			// Fallback: overwrite with zeros so occupied space can shrink on
			// filesystems that coalesce zeros into holes later.
			zeros := make([]byte, to-from)
			if _, err := f.File.WriteAt(zeros, from-f.Offset); err != nil {
				return err
			}
		}
	}
	// Keep an explicit incomplete mark so UpdateCompletion can re-queue download.
	st.completed[index] = false
	st.dropCacheLocked(index)
	st.checkpointDirty = true
	st.scheduleCheckpointFlushLocked()
	return nil
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
	if st.closed {
		st.mu.Unlock()
		return nil
	}
	// Drain an in-flight coalesced flush so it cannot rewrite the checkpoint
	// after this Close observes checkpointDirty=false.
	for st.checkpointFlushing {
		st.mu.Unlock()
		time.Sleep(5 * time.Millisecond)
		st.mu.Lock()
	}
	if st.attached {
		_ = st.flushAllDirtyLocked()
		st.checkpointDirty = true
		st.checkpointGen++
		st.checkpointFlushing = true
		st.mu.Unlock()
		st.drainSyncAndCheckpoint()
		st.mu.Lock()
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
	st.attached, st.cache, st.cacheDirty, st.cacheOrder, st.completed, st.cacheBytes = false, nil, nil, nil, nil, 0
	st.pendingSync = nil
	st.identities = nil
	st.freezeTrusted.Store(false)
	st.mu.Unlock()
	return first
}

func (st *DocumentTorrentStorage) touchCacheLocked(index int) {
	for i, v := range st.cacheOrder {
		if v == index {
			st.cacheOrder = append(st.cacheOrder[:i], st.cacheOrder[i+1:]...)
			break
		}
	}
	st.cacheOrder = append(st.cacheOrder, index)
}

func (st *DocumentTorrentStorage) dropCacheLocked(index int) {
	if data, ok := st.cache[index]; ok {
		st.cacheBytes -= int64(len(data))
		if st.cacheBytes < 0 {
			st.cacheBytes = 0
		}
		delete(st.cache, index)
	}
	delete(st.cacheDirty, index)
	for i, v := range st.cacheOrder {
		if v == index {
			st.cacheOrder = append(st.cacheOrder[:i], st.cacheOrder[i+1:]...)
			break
		}
	}
}

func (st *DocumentTorrentStorage) ensureCacheSlotLocked(index int, expected int64) error {
	if _, ok := st.cache[index]; ok {
		return nil
	}
	for st.cacheBytes+expected > st.maxCacheBytes {
		if len(st.cacheOrder) == 0 {
			return fmt.Errorf("piece cache is full")
		}
		victim := st.cacheOrder[0]
		if victim == index {
			if len(st.cacheOrder) == 1 {
				return fmt.Errorf("piece cache is full")
			}
			victim = st.cacheOrder[1]
		}
		if st.cacheDirty[victim] {
			if err := st.flushPieceLocked(victim); err != nil {
				return err
			}
		}
		st.dropCacheLocked(victim)
	}
	st.cache[index] = make([]byte, expected)
	st.cacheBytes += expected
	st.touchCacheLocked(index)
	return nil
}

func (st *DocumentTorrentStorage) flushPieceLocked(index int) error {
	data, ok := st.cache[index]
	if !ok || !st.cacheDirty[index] {
		return nil
	}
	if !st.attached {
		return fmt.Errorf("download storage is not configured")
	}
	if _, err := st.writeToFiles(int64(index)*st.chunkLength, data); err != nil {
		return err
	}
	st.cacheDirty[index] = false
	return nil
}

func (st *DocumentTorrentStorage) flushAllDirtyLocked() error {
	var first error
	for index, dirty := range st.cacheDirty {
		if !dirty {
			continue
		}
		if err := st.flushPieceLocked(index); err != nil && first == nil {
			first = err
		}
	}
	return first
}

func (st *DocumentTorrentStorage) scheduleDiskFlushLocked() {
	if st.diskFlushing || st.closed || !st.attached {
		return
	}
	st.diskFlushing = true
	go func() {
		time.Sleep(pieceDiskFlushDelay)
		st.mu.Lock()
		defer st.mu.Unlock()
		st.diskFlushing = false
		if st.closed || !st.attached {
			return
		}
		_ = st.flushAllDirtyLocked()
		for _, dirty := range st.cacheDirty {
			if dirty {
				st.scheduleDiskFlushLocked()
				return
			}
		}
	}()
}

func (st *DocumentTorrentStorage) notePieceFilesNeedSyncLocked(index int) {
	if st.info == nil || st.chunkLength <= 0 {
		return
	}
	if st.pendingSync == nil {
		st.pendingSync = make(map[*os.File]struct{})
	}
	start := int64(index) * st.chunkLength
	end := start + st.info.Piece(index).Length()
	for _, f := range st.files {
		if f.File == nil {
			continue
		}
		if end <= f.Offset || start >= f.Offset+f.Length {
			continue
		}
		st.pendingSync[f.File] = struct{}{}
	}
}

func (st *DocumentTorrentStorage) takePendingSyncLocked() []*os.File {
	if len(st.pendingSync) == 0 {
		return nil
	}
	out := make([]*os.File, 0, len(st.pendingSync))
	for f := range st.pendingSync {
		out = append(out, f)
	}
	st.pendingSync = make(map[*os.File]struct{})
	return out
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
		st.touchCacheLocked(index)
		if off >= int64(len(data)) {
			return 0, io.EOF
		}
		n := copy(b, data[off:])
		if n != len(b) {
			return n, io.EOF
		}
		if st.attached {
			st.lastReadOffset = int64(index)*st.chunkLength + off
		}
		return n, nil
	}
	if !st.attached {
		return 0, fmt.Errorf("download storage is not configured")
	}
	abs := int64(index)*st.chunkLength + off
	st.lastReadOffset = abs
	// Populate the hot cache from disk for repeated playhead reads.
	expected := int64(p.piece.Length())
	if err := st.ensureCacheSlotLocked(index, expected); err == nil {
		if data := st.cache[index]; data != nil {
			if _, err := st.readFromFiles(int64(index)*st.chunkLength, data); err == nil {
				st.cacheDirty[index] = false
				st.touchCacheLocked(index)
				if off >= int64(len(data)) {
					return 0, io.EOF
				}
				n := copy(b, data[off:])
				if n != len(b) {
					return n, io.EOF
				}
				return n, nil
			}
			st.dropCacheLocked(index)
		}
	}
	return st.readFromFiles(abs, b)
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
	if !st.attached {
		data, ok := st.cache[index]
		if !ok {
			if st.cacheBytes+expected > st.maxCacheBytes {
				return 0, fmt.Errorf("prepared data cache is full")
			}
			data = make([]byte, expected)
			st.cache[index] = data
			st.cacheBytes += expected
			st.touchCacheLocked(index)
		}
		copy(data[off:], b)
		st.cacheDirty[index] = true
		return len(b), nil
	}
	// Attached: RAM first (write-back). Disk flush is asynchronous so peer
	// receives are not blocked on storage I/O.
	if err := st.ensureCacheSlotLocked(index, expected); err != nil {
		// Cache full of dirty pieces that could not be flushed: fall back to disk.
		return st.writeToFiles(int64(index)*st.chunkLength+off, b)
	}
	copy(st.cache[index][off:], b)
	st.cacheDirty[index] = true
	st.touchCacheLocked(index)
	st.scheduleDiskFlushLocked()
	return len(b), nil
}
func (p *documentPiece) MarkComplete() error {
	p.storage.mu.Lock()
	defer p.storage.mu.Unlock()
	if p.storage.closed {
		return fmt.Errorf("storage closed")
	}
	index := p.piece.Index()
	// Durable complete bits require the piece bytes on disk first. fsync is
	// coalesced into drainSyncAndCheckpoint so MediaStore Sync is not on the
	// per-piece hot path.
	if err := p.storage.flushPieceLocked(index); err != nil {
		return err
	}
	p.storage.completed[index] = true
	p.storage.notePieceFilesNeedSyncLocked(index)
	p.storage.checkpointDirty = true
	p.storage.checkpointGen++
	p.storage.scheduleCheckpointFlushLocked()
	if p.storage.stream {
		p.storage.trimStreamCacheLocked()
	}
	return nil
}
func (p *documentPiece) MarkNotComplete() error {
	p.storage.mu.Lock()
	defer p.storage.mu.Unlock()
	if p.storage.closed || !p.storage.attached {
		return nil
	}
	index := p.piece.Index()
	// Ignore late/stale hash failures once the piece is trusted (checkpoint
	// restore or a successful MarkComplete). Intentional invalidation goes
	// through discardPieceLocked, which clears the bit first.
	if p.storage.completed[index] {
		return nil
	}
	delete(p.storage.completed, index)
	p.storage.dropCacheLocked(index)
	p.storage.checkpointDirty = true
	p.storage.scheduleCheckpointFlushLocked()
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

func (st *DocumentTorrentStorage) captureIdentitiesLocked() {
	ids := make([]fileIdentity, len(st.files))
	for i, f := range st.files {
		id, ok := identityForFile(f)
		if !ok {
			// Unreadable identity keeps checkpoint from loading until every
			// destination can be bound stably.
			st.identities = nil
			return
		}
		ids[i] = id
	}
	st.identities = ids
}

func (st *DocumentTorrentStorage) loadCheckpointLocked(identities []fileIdentity) bool {
	if st.info == nil || len(identities) != len(st.files) {
		return false
	}
	cp, err := readPieceCheckpoint(checkpointPath(st.baseDir, st.infoHash.HexString()))
	if err != nil {
		return false
	}
	var hash [20]byte
	copy(hash[:], st.infoHash[:])
	if cp.InfoHash != hash || cp.PieceLength != st.chunkLength || cp.NumPieces != st.info.NumPieces() {
		return false
	}
	if !identitiesMatch(cp.Files, identities) {
		return false
	}
	for i, ok := range cp.Complete {
		if ok {
			st.completed[i] = true
		}
	}
	return true
}

func (st *DocumentTorrentStorage) clearCompletedOverlappingLocked(fileIndex int) {
	if st.info == nil || fileIndex < 0 || fileIndex >= len(st.files) {
		return
	}
	f := st.files[fileIndex]
	for index := range st.completed {
		piece := st.info.Piece(index)
		start := int64(index) * st.chunkLength
		end := start + piece.Length()
		if end <= f.Offset || start >= f.Offset+f.Length {
			continue
		}
		delete(st.completed, index)
	}
}

func (st *DocumentTorrentStorage) scheduleCheckpointFlushLocked() {
	if st.checkpointFlushing || st.closed || !st.attached {
		return
	}
	if !st.checkpointDirty && len(st.pendingSync) == 0 {
		return
	}
	st.checkpointFlushing = true
	go func() {
		time.Sleep(coalesceFlushDelay())
		st.drainSyncAndCheckpoint()
	}()
}

// drainSyncAndCheckpoint fsyncs pending files off-lock, refreshes file
// identities after Sync (mtime), then writes the checkpoint. checkpointFlushing
// stays set until this returns so Close/tests cannot race .chk writers.
func (st *DocumentTorrentStorage) drainSyncAndCheckpoint() {
	st.mu.Lock()
	if st.closed || !st.attached {
		st.checkpointFlushing = false
		st.mu.Unlock()
		return
	}
	syncFiles := st.takePendingSyncLocked()
	needCP := st.checkpointDirty
	gen := st.checkpointGen
	hashHex := st.infoHash.HexString()
	st.mu.Unlock()

	for _, f := range syncFiles {
		_ = f.Sync()
	}

	var cp *pieceCheckpoint
	var path string
	if needCP {
		st.mu.Lock()
		if !st.closed && st.attached && st.info != nil {
			// Capture identities after Sync so ModNano matches resume checks.
			st.captureIdentitiesLocked()
			if len(st.identities) == len(st.files) {
				numPieces := st.info.NumPieces()
				complete := make([]bool, numPieces)
				for i := 0; i < numPieces; i++ {
					complete[i] = st.completed[i]
				}
				var hash [20]byte
				copy(hash[:], st.infoHash[:])
				cp = &pieceCheckpoint{
					InfoHash:    hash,
					PieceLength: st.chunkLength,
					NumPieces:   numPieces,
					Files:       append([]fileIdentity(nil), st.identities...),
					Complete:    complete,
				}
				path = checkpointPath(st.baseDir, hashHex)
				gen = st.checkpointGen
			}
		}
		st.mu.Unlock()
	}
	if cp != nil && path != "" {
		_ = writePieceCheckpoint(path, cp)
	}

	st.mu.Lock()
	defer st.mu.Unlock()
	st.checkpointFlushing = false
	if st.closed {
		return
	}
	if cp != nil && st.checkpointGen == gen {
		st.checkpointDirty = false
	}
	if st.checkpointDirty || len(st.pendingSync) > 0 {
		st.scheduleCheckpointFlushLocked()
	}
}

// Caller holds st.mu. Used by tests that force an immediate checkpoint write.
func (st *DocumentTorrentStorage) flushCheckpointLocked() error {
	for _, f := range st.takePendingSyncLocked() {
		if err := f.Sync(); err != nil {
			if st.pendingSync == nil {
				st.pendingSync = make(map[*os.File]struct{})
			}
			st.pendingSync[f] = struct{}{}
			return err
		}
	}
	if !st.checkpointDirty {
		return nil
	}
	if st.info == nil || !st.attached || st.closed {
		return nil
	}
	st.captureIdentitiesLocked()
	if len(st.identities) != len(st.files) {
		return nil
	}
	numPieces := st.info.NumPieces()
	complete := make([]bool, numPieces)
	for i := 0; i < numPieces; i++ {
		complete[i] = st.completed[i]
	}
	var hash [20]byte
	copy(hash[:], st.infoHash[:])
	cp := &pieceCheckpoint{
		InfoHash:    hash,
		PieceLength: st.chunkLength,
		NumPieces:   numPieces,
		Files:       append([]fileIdentity(nil), st.identities...),
		Complete:    complete,
	}
	if err := writePieceCheckpoint(checkpointPath(st.baseDir, st.infoHash.HexString()), cp); err != nil {
		return err
	}
	st.checkpointDirty = false
	return nil
}
