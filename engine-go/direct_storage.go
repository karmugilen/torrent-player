package main

import (
	"bytes"
	"crypto/sha1"
	"crypto/sha256"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"
)

const (
	directStartupHead        = int64(2 << 20)
	directStartupTail        = int64(1 << 20)
	directStartupBudget      = int64(8 << 20)
	directStartupTotalBudget = int64(32 << 20)
)

// Direct Play has no full-sized backing files. Only verified opening/container
// pieces survive a session; all other bytes live in the bounded RAM cache.
func (st *DocumentTorrentStorage) attachDirectPlay(descriptors []*int, selected []int) error {
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.closed || st.info == nil {
		return fmt.Errorf("storage is closed, configured, or missing metadata")
	}
	if st.previewTemp {
		st.releasePreviewTempLocked()
	}
	if st.attached {
		return fmt.Errorf("storage is closed, configured, or missing metadata")
	}
	if len(descriptors) != len(st.files) {
		return fmt.Errorf("file descriptors do not match torrent files")
	}
	for _, fd := range descriptors {
		if fd != nil {
			return fmt.Errorf("direct play rejects external descriptors")
		}
	}
	chosen := make(map[int]bool)
	for _, i := range selected {
		if i < 0 || i >= len(st.files) || chosen[i] {
			return fmt.Errorf("invalid selected file index %d", i)
		}
		chosen[i] = true
	}
	st.stream, st.attached = true, true
	st.maxCacheBytes = st.streamMaxBytes
	if st.maxCacheBytes <= 0 {
		st.maxCacheBytes = DefaultStreamCacheMaxBytes
	}
	// A piece must fit completely for hash verification; reject pathological
	// torrents rather than silently allocating beyond the configured RAM budget.
	if st.chunkLength > st.maxCacheBytes/4 {
		st.stream, st.attached = false, false
		return fmt.Errorf("torrent piece exceeds direct play memory budget; use Download")
	}
	st.startupPieces, st.startupSaved, st.streamDemand = make(map[int]bool), make(map[int]bool), make(map[int]bool)
	st.streamLost, st.streamWhole = make(map[int]bool), make(map[int]bool)
	st.streamRanges = make(map[int][]streamByteRange)
	st.streamReady = make(chan struct{})
	registerStartupCache(st.baseDir, st.infoHash.HexString())
	budget := int64(0)
	for _, i := range selected {
		f := st.files[i]
		for _, span := range [][2]int64{{f.Offset, f.Offset + min(f.Length, directStartupHead)}, {f.Offset + max(int64(0), f.Length-directStartupTail), f.Offset + f.Length}} {
			if span[1] <= span[0] {
				continue
			}
			for p := int(span[0] / st.chunkLength); p <= int((span[1]-1)/st.chunkLength); p++ {
				if st.startupPieces[p] {
					continue
				}
				n := st.info.Piece(p).Length()
				if budget+n > directStartupBudget {
					continue
				}
				st.startupPieces[p], budget = true, budget+n
			}
		}
	}
	st.validateStartupPiecesLocked(st.startupPieces)
	// Prepared transfers do not own payload data. Discard any unverified
	// preparation buffers rather than trusting their old write bookkeeping.
	st.cache, st.cacheDirty, st.cacheOrder, st.cacheBytes = make(map[int][]byte), make(map[int]bool), nil, 0
	pruneStartupCaches(st.baseDir, st.infoHash.HexString())
	return nil
}

func (st *DocumentTorrentStorage) startupPiecePath(index int) string {
	return filepath.Join(st.baseDir, ".startup", st.infoHash.HexString(), strconv.Itoa(index)+".piece")
}

func (st *DocumentTorrentStorage) validStartupPiece(index int, data []byte) bool {
	if len(data) != sha256.Size+int(st.info.Piece(index).Length()) {
		return false
	}
	payload := data[sha256.Size:]
	sum := sha256.Sum256(payload)
	if !bytes.Equal(sum[:], data[:sha256.Size]) {
		return false
	}
	if len(st.info.Pieces) >= (index+1)*sha1.Size {
		v1 := sha1.Sum(payload)
		return bytes.Equal(v1[:], st.info.Pieces[index*sha1.Size:(index+1)*sha1.Size])
	}
	// For v2 the bytes were torrent-verified before saving. The checksum detects
	// local corruption, just as the download checkpoint binds saved file identities.
	return true
}

// Use the runtime CPU allowance, including all available cores on Android.
// The torrent client also caps aggregate active hashers across all torrents.
func backgroundWorkers() int { return max(1, runtime.GOMAXPROCS(0)) }

func (st *DocumentTorrentStorage) saveStartupPieceLocked(index int) {
	if !st.startupPieces[index] || st.startupSaved[index] {
		return
	}
	data := st.cache[index]
	if int64(len(data)) != st.info.Piece(index).Length() {
		return
	}
	if st.startupPending == nil {
		st.startupPending = make(map[int][]byte)
	}
	if _, pending := st.startupPending[index]; pending {
		return
	}
	// An episode switch can replace the plan while older writes are draining.
	// Bound both generations together rather than accumulating queued payload.
	pendingBytes := int64(len(data))
	for _, pending := range st.startupPending {
		pendingBytes += int64(len(pending))
	}
	if pendingBytes > directStartupBudget {
		return
	}
	st.startupPending[index] = bytes.Clone(data)
	if st.startupWriting {
		return
	}
	st.startupWriting = true
	go st.drainStartupPieces()
}

func (st *DocumentTorrentStorage) drainStartupPieces() {
	for {
		st.mu.Lock()
		index := -1
		var data []byte
		for p, payload := range st.startupPending {
			index, data = p, payload
			break
		}
		if index < 0 {
			st.startupWriting = false
			st.mu.Unlock()
			return
		}
		path := st.startupPiecePath(index)
		st.mu.Unlock()
		sum := sha256.Sum256(data)

		err := os.MkdirAll(filepath.Dir(path), 0700)
		if err == nil {
			var out *os.File
			out, err = os.OpenFile(path+".tmp", os.O_CREATE|os.O_TRUNC|os.O_WRONLY, 0600)
			if err == nil {
				_, err = out.Write(sum[:])
				if err == nil {
					_, err = out.Write(data)
				}
				closeErr := out.Close()
				if err == nil {
					err = closeErr
				}
			}
		}
		st.mu.Lock()
		// Commit under the plan lock so an episode switch cannot resurrect
		// a file it just evicted from the startup budget.
		if err == nil && st.startupPieces[index] && !st.closed {
			err = os.Rename(path+".tmp", path)
		} else {
			_ = os.Remove(path + ".tmp")
			if err == nil {
				err = errStreamEvicted
			}
		}
		delete(st.startupPending, index)
		if err == nil && !st.closed {
			st.startupSaved[index], st.completed[index] = true, true
			st.queueCompletionLocked(index)
			st.notifyStreamReadyLocked()
		}
		st.mu.Unlock()
	}
}

// Wake all blocked playback descriptors without a polling goroutine. The
// channel is captured under the same lock as completion, so a verification
// between testing the bit and waiting cannot lose its wakeup.
func (st *DocumentTorrentStorage) notifyStreamReadyLocked() {
	if st.streamReady != nil {
		close(st.streamReady)
		st.streamReady = make(chan struct{})
	}
}

func (st *DocumentTorrentStorage) readVerifiedStream(b []byte, abs int64) (int, <-chan struct{}, error) {
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.closed || !st.stream || !st.attached {
		return 0, nil, fmt.Errorf("direct play storage is closed")
	}
	index := int(abs / st.chunkLength)
	if !st.completed[index] {
		return 0, st.streamReady, nil
	}
	p := &documentPiece{storage: st, piece: st.info.Piece(index)}
	// Keep verification and the byte copy under one lock: eviction cannot
	// replace trusted bytes with a partial buffer between these operations.
	n, err := p.readAtLocked(b, abs%st.chunkLength)
	return n, nil, err
}

func (st *DocumentTorrentStorage) ensureStreamSlotLocked(index int, expected int64) (bool, error) {
	for st.cacheBytes+expected > st.maxCacheBytes {
		victim := -1
		// Prefer old verified pieces outside current demand. A partial piece
		// may also be discarded, but reads then explicitly fail until the
		// library finishes its current chunk cycle and resets that piece.
		// Fully received pieces awaiting verification must remain intact.
		for pass := 0; pass < 2 && victim < 0; pass++ {
			for _, p := range st.cacheOrder {
				if p == index || (!st.completed[p] && st.streamWhole[p]) {
					continue
				}
				if pass == 0 && st.streamDemand[p] {
					continue
				}
				victim = p
				break
			}
		}
		if victim < 0 {
			return false, errStreamBusy
		}
		if !st.completed[victim] {
			st.streamLost[victim] = true
		}
		st.dropCacheLocked(victim)
		delete(st.streamRanges, victim)
		delete(st.streamWhole, victim)
		if !st.startupSaved[victim] {
			delete(st.completed, victim)
			st.queueCompletionLocked(victim)
		}
	}
	st.cache[index] = make([]byte, expected)
	st.cacheBytes += expected
	st.touchCacheLocked(index)
	return true, nil
}

func (st *DocumentTorrentStorage) setStreamDemand(demand map[int]bool) {
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.stream {
		st.streamDemand = demand
	}
}

// Keep only a few recent warm starts. Ordinary downloaded files are untouched.
func pruneStartupCaches(base, keep string) {
	startupRegistry.Lock()
	defer startupRegistry.Unlock()
	root := filepath.Join(base, ".startup")
	dirs, _ := os.ReadDir(root)
	type cacheDir struct {
		path     string
		size     int64
		modified int64
	}
	var caches []cacheDir
	var total int64
	for _, d := range dirs {
		if !d.IsDir() {
			continue
		}
		path := filepath.Join(root, d.Name())
		files, _ := os.ReadDir(path)
		var size, modified int64
		for _, f := range files {
			if info, err := f.Info(); err == nil {
				size += info.Size()
				modified = max(modified, info.ModTime().UnixNano())
			}
		}
		total += size
		if d.Name() != keep && startupRegistry.active[filepath.Join(base, d.Name())] == 0 {
			caches = append(caches, cacheDir{path, size, modified})
		}
	}
	sort.Slice(caches, func(i, j int) bool { return caches[i].modified < caches[j].modified })
	// Reserve one torrent's startup budget for the session being configured.
	for _, c := range caches {
		if total <= directStartupTotalBudget-directStartupBudget {
			break
		}
		if os.RemoveAll(c.path) == nil {
			total -= c.size
		}
	}
}

var errStreamEvicted = errors.New("direct play bytes were evicted; piece must be fetched again")
var errStreamBusy = errors.New("direct play pieces are being verified")

type streamByteRange struct{ start, end int64 }

func (st *DocumentTorrentStorage) recordStreamWriteLocked(index int, start, end, length int64) {
	ranges := append(st.streamRanges[index], streamByteRange{start, end})
	sort.Slice(ranges, func(i, j int) bool { return ranges[i].start < ranges[j].start })
	merged := ranges[:0]
	for _, r := range ranges {
		if len(merged) != 0 && r.start <= merged[len(merged)-1].end {
			merged[len(merged)-1].end = max(merged[len(merged)-1].end, r.end)
		} else {
			merged = append(merged, r)
		}
	}
	st.streamRanges[index] = merged
	st.streamWhole[index] = !st.streamLost[index] && len(merged) == 1 && merged[0].start == 0 && merged[0].end == length
}

// One coalescing completion worker per storage. Close never joins a callback
// which may be waiting for the torrent client's lock while it drops storage.
func (st *DocumentTorrentStorage) queueCompletionLocked(index int) {
	if st.closed || st.onStreamEvict == nil {
		return
	}
	if st.completionWake == nil {
		st.completionWake = make(chan struct{}, 1)
		st.completionPending = make(map[int]bool)
		go st.drainCompletionUpdates()
	}
	st.completionPending[index] = true
	select {
	case st.completionWake <- struct{}{}:
	default:
	}
}

func (st *DocumentTorrentStorage) drainCompletionUpdates() {
	for range st.completionWake {
		for {
			st.mu.Lock()
			if st.closed {
				st.mu.Unlock()
				return
			}
			index := -1
			for p := range st.completionPending {
				index = p
				delete(st.completionPending, p)
				break
			}
			callback := st.onStreamEvict
			st.mu.Unlock()
			if index < 0 {
				break
			}
			callback(index)
		}
	}
}

var startupRegistry = struct {
	sync.Mutex
	active map[string]int
}{active: make(map[string]int)}

func registerStartupCache(base, hash string) {
	startupRegistry.Lock()
	defer startupRegistry.Unlock()
	startupRegistry.active[filepath.Join(base, hash)]++
	_ = os.Chtimes(filepath.Join(base, ".startup", hash), time.Now(), time.Now())
}
func unregisterStartupCache(base, hash string) {
	startupRegistry.Lock()
	key := filepath.Join(base, hash)
	startupRegistry.active[key]--
	if startupRegistry.active[key] <= 0 {
		delete(startupRegistry.active, key)
	}
	startupRegistry.Unlock()
	pruneStartupCaches(base, "")
}

// Move the durable startup budget to the episode actually opened by the
// external player. Keep older warm pieces while they still fit in the budget.
func (st *DocumentTorrentStorage) setStartupFile(index int) {
	st.mu.Lock()
	defer st.mu.Unlock()
	if !st.stream || st.closed || index < 0 || index >= len(st.files) {
		return
	}
	file := st.files[index]
	next := make(map[int]bool)
	var budget int64
	add := func(p int) {
		if next[p] {
			return
		}
		length := st.info.Piece(p).Length()
		if budget+length > directStartupBudget {
			return
		}
		next[p] = true
		budget += length
	}
	for _, span := range [][2]int64{
		{file.Offset, file.Offset + min(file.Length, directStartupHead)},
		{file.Offset + max(int64(0), file.Length-directStartupTail), file.Offset + file.Length},
	} {
		if span[1] <= span[0] {
			continue
		}
		for p := int(span[0] / st.chunkLength); p <= int((span[1]-1)/st.chunkLength); p++ {
			add(p)
		}
	}
	for p := range st.startupPieces {
		add(p)
	}
	for p := range st.startupPieces {
		if next[p] {
			continue
		}
		delete(st.startupSaved, p)
		_ = os.Remove(st.startupPiecePath(p))
		if st.cache[p] == nil {
			delete(st.completed, p)
			st.queueCompletionLocked(p)
		}
	}
	st.startupPieces = next
	missing := make(map[int]bool)
	for p := range next {
		if !st.startupSaved[p] && !st.completed[p] {
			missing[p] = true
		}
	}
	if len(missing) != 0 {
		st.validateStartupPiecesLocked(missing)
	}
	// Files from a previous process may belong to an episode outside the
	// initial plan. Once the current episode is known, enforce the plan on disk.
	entries, _ := os.ReadDir(filepath.Dir(st.startupPiecePath(0)))
	for _, entry := range entries {
		if !strings.HasSuffix(entry.Name(), ".piece") {
			continue
		}
		p, err := strconv.Atoi(strings.TrimSuffix(entry.Name(), ".piece"))
		if err == nil && !next[p] {
			_ = os.Remove(filepath.Join(filepath.Dir(st.startupPiecePath(0)), entry.Name()))
		}
	}
	for p := range next {
		if st.completed[p] {
			st.saveStartupPieceLocked(p)
		}
	}
}

func (st *DocumentTorrentStorage) validateStartupPiecesLocked(planned map[int]bool) {
	// Independent disk reads and hash checks use a small CPU-sized worker pool.
	jobs := make(chan int, len(planned))
	valid := make(chan int, len(planned))
	for p := range planned {
		jobs <- p
	}
	close(jobs)
	var workers sync.WaitGroup
	for w := 0; w < min(backgroundWorkers(), len(planned)); w++ {
		workers.Add(1)
		go func() {
			defer workers.Done()
			for p := range jobs {
				data, err := st.readStartupPiece(p)
				if err != nil {
					if !os.IsNotExist(err) {
						_ = os.Remove(st.startupPiecePath(p))
					}
					continue
				}
				if st.validStartupPiece(p, data) {
					valid <- p
				} else {
					_ = os.Remove(st.startupPiecePath(p))
				}
			}
		}()
	}
	workers.Wait()
	close(valid)
	for p := range valid {
		st.startupSaved[p], st.completed[p] = true, true
		delete(st.streamLost, p)
	}
	st.notifyStreamReadyLocked()
}

// Corrupt files must not expand validation memory beyond the planned piece.
func (st *DocumentTorrentStorage) readStartupPiece(index int) ([]byte, error) {
	file, err := os.Open(st.startupPiecePath(index))
	if err != nil {
		return nil, err
	}
	defer file.Close()
	expected := st.info.Piece(index).Length() + sha256.Size
	stat, err := file.Stat()
	if err != nil {
		return nil, err
	}
	if stat.Size() != expected {
		return nil, fmt.Errorf("invalid startup piece length")
	}
	data := make([]byte, int(expected))
	_, err = io.ReadFull(file, data)
	return data, err
}

// injectVerifiedBytes places already-validated resume bytes into the RAM/stream
// cache as complete whole-piece spans. Partial edge pieces are copied only when
// the injected range covers them entirely within the provided buffer; pieces
// that are only partially covered are left incomplete so completion is never
// invented. Returns false when nothing usable could be installed.
func (st *DocumentTorrentStorage) injectVerifiedBytes(abs int64, data []byte) bool {
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.closed || !st.attached || !st.stream || st.info == nil || st.chunkLength <= 0 || len(data) == 0 || abs < 0 || abs+int64(len(data)) > st.info.TotalLength() {
		return false
	}
	installed := false
	end := abs + int64(len(data))
	begin := int(abs / st.chunkLength)
	last := int((end - 1) / st.chunkLength)
	for index := begin; index <= last; index++ {
		piece := st.info.Piece(index)
		pieceStart := int64(index) * st.chunkLength
		pieceLen := piece.Length()
		pieceEnd := pieceStart + pieceLen
		if abs > pieceStart || end < pieceEnd {
			// Only install fully covered pieces to avoid inventing completion.
			continue
		}
		off := int(pieceStart - abs)
		buf := make([]byte, int(pieceLen))
		copy(buf, data[off:off+int(pieceLen)])
		// Cache checksums detect torn files; torrent hashes establish byte trust.
		expected := piece.V1Hash()
		if !expected.Ok || sha1.Sum(buf) != expected.Value {
			continue
		}
		if st.stream {
			if _, err := st.ensureStreamSlotLocked(index, pieceLen); err != nil {
				continue
			}
		} else {
			if _, err := st.ensureCacheSlotLocked(index, pieceLen); err != nil {
				continue
			}
		}
		if st.cache[index] == nil || int64(len(st.cache[index])) < pieceLen {
			continue
		}
		copy(st.cache[index], buf)
		st.cacheDirty[index] = false
		st.completed[index] = true
		if st.stream {
			st.streamWhole[index] = true
			st.streamRanges[index] = []streamByteRange{{start: 0, end: pieceLen}}
		}
		st.touchCacheLocked(index)
		installed = true
	}
	if installed && st.streamReady != nil {
		st.notifyStreamReadyLocked()
	}
	return installed
}

// copyVerifiedRange returns a durable snapshot of already-complete bytes from
// RAM cache and/or attached files. It never starts fetching.
func (st *DocumentTorrentStorage) copyVerifiedRange(abs, length int64) ([]byte, bool) {
	if length <= 0 || length > resumeWindowMaxBytes {
		return nil, false
	}
	if !st.RangeComplete(abs, length) {
		return nil, false
	}
	buf := make([]byte, int(length))
	if spans, ok := st.byteSpans(abs, length); ok {
		n, err := preadFileSpans(spans, buf)
		if err == nil && int64(n) == length {
			return buf, true
		}
	}
	st.mu.Lock()
	defer st.mu.Unlock()
	n := 0
	for n < len(buf) {
		index := int((abs + int64(n)) / st.chunkLength)
		if !st.completed[index] || st.cache[index] == nil {
			return nil, false
		}
		pieceOff := (abs + int64(n)) % st.chunkLength
		want := min(int64(len(buf)-n), st.info.Piece(index).Length()-pieceOff)
		copy(buf[n:n+int(want)], st.cache[index][pieceOff:pieceOff+want])
		n += int(want)
	}
	return buf, true
}
