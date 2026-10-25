package main

import (
	"bytes"
	"crypto/sha1"
	"crypto/sha256"
	"errors"
	"fmt"
	"io"
	"maps"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"
)

// Direct Play saves only video metadata to disk: the head and tail of every
// selected video (MP4 moov, MKV header and cues), written once and kept.
// Everything else stays in RAM to spare internal storage from wear.
const (
	directStartupHead = int64(2 << 20)
	directStartupTail = int64(2 << 20)
	// Saved pieces validated when a session attaches; the rest are validated
	// when their episode opens, so a large cache does not delay the start.
	directStartupBudget = int64(8 << 20)
	// Overall disk budget for saved metadata across all torrents.
	directStartupTotalBudget = int64(1 << 30)
	// The next episode's warm start has its own budget so preloading never
	// displaces the current episode's head and tail.
	directPreloadBudget = int64(8 << 20)
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
	// Only pieces too large for any allowed cache are refused. A smaller RAM
	// setting is raised to the minimum this torrent can play from.
	if directPlayMinPieces*st.chunkLength > MaxRamCacheBytes {
		st.stream, st.attached = false, false
		return fmt.Errorf("torrent piece exceeds direct play memory budget; use Download")
	}
	st.maxCacheBytes = max(st.maxCacheBytes, directPlayMinPieces*st.chunkLength)
	st.startupPieces, st.startupSaved, st.streamDemand = make(map[int]bool), make(map[int]bool), make(map[int]bool)
	st.streamLost, st.streamWhole = make(map[int]bool), make(map[int]bool)
	st.streamRanges = make(map[int][]streamByteRange)
	st.streamReady = make(chan struct{})
	registerStartupCache(st.baseDir, st.infoHash.HexString())
	st.directSelected, st.startupOnDisk = slices.Clone(selected), make(map[int]bool)
	st.startupOrder, st.startupPlanned = st.startupPlanLocked(-1), false
	st.streamWatch = make(map[int]bool)
	quick := make(map[int]bool)
	var quickBytes int64
	for _, p := range st.startupOrder {
		st.startupPieces[p] = true
		if n := st.info.Piece(p).Length(); quickBytes+n <= directStartupBudget {
			quick[p], quickBytes = true, quickBytes+n
		} else if st.startupFileOnDisk(p) {
			st.startupOnDisk[p] = true
		}
	}
	st.validateStartupPiecesLocked(quick)
	// Prepared transfers do not own payload data. Discard any unverified
	// preparation buffers rather than trusting their old write bookkeeping.
	st.cache, st.cacheDirty, st.cacheBytes = make(map[int][]byte), make(map[int]bool), 0
	st.cacheOrder.reset()
	st.streamRevision++
	pruneStartupCaches(st.baseDir, st.infoHash.HexString())
	return nil
}

// startupFilePieces lists the pieces of a file's head and tail.
func startupFilePieces(f FileDescriptor, chunk int64) []int {
	var pieces []int
	for _, span := range [][2]int64{
		{f.Offset, f.Offset + min(f.Length, directStartupHead)},
		{f.Offset + max(int64(0), f.Length-directStartupTail), f.Offset + f.Length},
	} {
		if span[1] <= span[0] {
			continue
		}
		for p := int(span[0] / chunk); p <= int((span[1]-1)/chunk); p++ {
			if !slices.Contains(pieces, p) {
				pieces = append(pieces, p)
			}
		}
	}
	return pieces
}

// startupPlanLocked orders the head and tail pieces of every selected video,
// starting with first (which may be any file) and continuing in file order,
// within the overall disk budget.
func (st *DocumentTorrentStorage) startupPlanLocked(first int) []int {
	files := slices.Sorted(slices.Values(st.directSelected))
	if at := slices.Index(files, first); at > 0 {
		files = slices.Concat(files[at:], files[:at])
	}
	seen := make(map[int]bool)
	var order []int
	var budget int64
	for _, i := range files {
		if i < 0 || i >= len(st.files) {
			continue
		}
		name := st.files[i].Path
		if name == "" {
			name = st.info.Name
		}
		if i != first && !isVideoFileName(name) {
			continue
		}
		for _, p := range startupFilePieces(st.files[i], st.chunkLength) {
			if seen[p] {
				continue
			}
			n := st.info.Piece(p).Length()
			if budget+n > directStartupTotalBudget {
				return order
			}
			seen[p], budget = true, budget+n
			order = append(order, p)
		}
	}
	return order
}

// startupFileOnDisk reports whether a saved piece of the right size exists.
// Its contents are validated when its episode opens.
func (st *DocumentTorrentStorage) startupFileOnDisk(index int) bool {
	info, err := os.Stat(st.startupPiecePath(index))
	return err == nil && info.Size() == st.info.Piece(index).Length()+sha256.Size
}

// startupFetchPieces returns planned metadata pieces still to download, in
// plan order. The batch fits in spare RAM: a quarter of the cache with no
// player open, otherwise the 5% playback reserve, and at least one piece.
// Verified pieces waiting to be written count against it.
func (st *DocumentTorrentStorage) startupFetchPieces(playing bool) []int {
	st.mu.Lock()
	defer st.mu.Unlock()
	if !st.stream || st.closed || st.previewTemp {
		return nil
	}
	limit := st.maxCacheBytes / 4
	if playing {
		limit = st.maxCacheBytes / 20
	}
	var used int64
	for _, p := range st.startupOrder {
		if st.completed[p] && !st.startupSaved[p] && st.cache[p] != nil {
			used += st.info.Piece(p).Length()
		}
	}
	var out []int
	for _, p := range st.startupOrder {
		if st.completed[p] || st.startupSaved[p] || st.startupOnDisk[p] {
			continue
		}
		n := st.info.Piece(p).Length()
		if (len(out) > 0 || used > 0) && used+n > limit {
			break
		}
		out, used = append(out, p), used+n
	}
	return out
}

// queueUnsavedStartupLocked queues verified planned pieces whose save was
// skipped while the writer queue was full. Reports whether any were queued.
func (st *DocumentTorrentStorage) queueUnsavedStartupLocked() bool {
	queued := false
	for _, p := range st.startupOrder {
		if !st.completed[p] || st.startupSaved[p] || int64(len(st.cache[p])) != st.info.Piece(p).Length() {
			continue
		}
		if _, pending := st.startupPending[p]; pending {
			continue
		}
		st.saveStartupPieceLocked(p)
		_, pending := st.startupPending[p]
		queued = queued || pending
	}
	return queued
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
	if pendingBytes > directStartupBudget+directPreloadBudget {
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
	progress, savedAny := false, false
	for {
		st.mu.Lock()
		index := -1
		var data []byte
		for p, payload := range st.startupPending {
			index, data = p, payload
			break
		}
		if index < 0 && progress && !st.closed {
			// Retry pieces skipped while the queue was full, only after a
			// successful write so a failing disk cannot loop.
			progress = false
			if st.queueUnsavedStartupLocked() {
				st.mu.Unlock()
				continue
			}
		}
		if index < 0 {
			st.startupWriting = false
			base, hash, saved := st.baseDir, st.infoHash.HexString(), st.onStartupSaved
			st.mu.Unlock()
			pruneStartupCaches(base, hash)
			if savedAny && saved != nil {
				saved()
			}
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
			// The queued clone is the verified piece. RAM may have been evicted
			// and replaced with a short zero-padded write while this save ran.
			if cached, ok := st.cache[index]; !ok || !bytes.Equal(cached, data) {
				st.dropCacheLocked(index)
				delete(st.streamRanges, index)
				delete(st.streamWhole, index)
			}
			delete(st.streamLost, index)
			progress, savedAny = true, true
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
	clear(st.streamWatch)
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
		if n := st.readReceivedLocked(b, index, abs%st.chunkLength); n > 0 {
			return n, nil, nil
		}
		if st.streamWatch != nil {
			st.streamWatch[index] = true
		}
		return 0, st.streamReady, nil
	}
	p := &documentPiece{storage: st, piece: st.info.Piece(index)}
	// Keep verification and the byte copy under one lock: eviction cannot
	// replace trusted bytes with a partial buffer between these operations.
	n, err := p.readAtLocked(b, abs%st.chunkLength)
	return n, nil, err
}

// readReceivedLocked copies bytes of an unverified piece that peers have
// already delivered, starting at off. After a seek the player then starts on
// the first block that arrives instead of waiting for the whole piece (up to
// 16 MiB) and its hash check. The start and end of each video, where players
// read the index, are only served verified: a bad index crashes players, while
// a bad block elsewhere is a brief glitch, and a failed hash is rare.
func (st *DocumentTorrentStorage) readReceivedLocked(b []byte, index int, off int64) int {
	if st.streamLost[index] || st.startupPieces[index] || st.preloadPieces[index] {
		return 0
	}
	data := st.cache[index]
	if data == nil {
		return 0
	}
	ranges := st.streamRanges[index]
	i := sort.Search(len(ranges), func(i int) bool { return ranges[i].end > off })
	if i < len(ranges) && ranges[i].start <= off {
		n := copy(b[:min(int64(len(b)), ranges[i].end-off)], data[off:])
		st.touchCacheLocked(index)
		return n
	}
	return 0
}

func (st *DocumentTorrentStorage) ensureStreamSlotLocked(index int, expected int64) (bool, error) {
	// Replacing a cached buffer without uncounting it inflated cacheBytes on
	// every seek until the cache looked full and evicted the pieces playback
	// was waiting for.
	if data, ok := st.cache[index]; ok {
		if int64(len(data)) == expected {
			st.touchCacheLocked(index)
			return false, nil
		}
		st.dropCacheLocked(index)
	}
	for st.cacheBytes+expected > st.maxCacheBytes {
		victim := -1
		// Prefer old verified pieces outside current demand. A partial piece
		// may also be discarded, but reads then explicitly fail until the
		// library finishes its current chunk cycle and resets that piece.
		// Fully received pieces awaiting verification must remain intact.
		for pass := 0; pass < 2 && victim < 0; pass++ {
			st.cacheOrder.each(func(p int) bool {
				if p == index || (!st.completed[p] && st.streamWhole[p]) {
					return true
				}
				// Verified metadata waiting to be written is also kept.
				if pass == 0 && (st.streamDemand[p] || (st.preloadPieces[p] && !st.startupSaved[p]) ||
					(st.startupPieces[p] && st.completed[p] && !st.startupSaved[p])) {
					return true
				}
				victim = p
				return false
			})
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
	st.streamRevision++
	st.touchCacheLocked(index)
	return true, nil
}

func (st *DocumentTorrentStorage) setStreamDemand(demand map[int]bool) {
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.stream {
		if !maps.Equal(st.streamDemand, demand) {
			// The grid shows only wanted partial pieces as receiving.
			st.streamRevision++
		}
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

// Ordered delivery needs no native call or range scan. Only out-of-order
// arrivals need the insertion/merge core (Rust on Android).
func mergeReceivedRange(ranges []streamByteRange, incoming streamByteRange) []streamByteRange {
	if incoming.start < 0 || incoming.end <= incoming.start {
		panic("invalid received byte range")
	}
	if len(ranges) == 0 {
		return append(ranges, incoming)
	}
	last := &ranges[len(ranges)-1]
	if incoming.start >= last.start {
		if incoming.start <= last.end {
			last.end = max(last.end, incoming.end)
			return ranges
		}
		return append(ranges, incoming)
	}
	return mergeReceivedRangeOutOfOrder(ranges, incoming)
}

func (st *DocumentTorrentStorage) recordStreamWriteLocked(index int, start, end, length int64) {
	if start == end {
		return
	}
	merged := mergeReceivedRange(st.streamRanges[index], streamByteRange{start, end})
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

// setStartupFile puts the episode opened by the external player first in the
// metadata plan and validates its saved pieces. Other episodes' saved
// metadata stays on disk; it is written once, not on every episode switch.
func (st *DocumentTorrentStorage) setStartupFile(index int) {
	st.mu.Lock()
	defer st.mu.Unlock()
	if !st.stream || st.closed || index < 0 || index >= len(st.files) {
		return
	}
	if st.preloadPieces != nil && st.preloadFile == index {
		st.preloadPieces = nil
	} else if st.startupPlanned && st.startupFile == index {
		// A player opens the file again for every seek. The plan, its disk
		// check and the hashing were done on the first open.
		return
	}
	st.startupFile, st.startupPlanned = index, true
	st.startupOrder = st.startupPlanLocked(index)
	next := make(map[int]bool, len(st.startupOrder))
	for _, p := range st.startupOrder {
		next[p] = true
	}
	for p := range st.preloadPieces {
		next[p] = true
	}
	for p := range st.startupPieces {
		if next[p] {
			continue
		}
		delete(st.startupSaved, p)
		delete(st.startupOnDisk, p)
		_ = os.Remove(st.startupPiecePath(p))
		if st.cache[p] == nil {
			delete(st.completed, p)
			st.queueCompletionLocked(p)
		}
	}
	st.startupPieces = next
	missing := make(map[int]bool)
	for _, p := range startupFilePieces(st.files[index], st.chunkLength) {
		if next[p] && !st.startupSaved[p] && !st.completed[p] {
			missing[p] = true
		}
	}
	if len(missing) != 0 {
		st.validateStartupPiecesLocked(missing)
	}
	// Files from a previous process may belong to a file no longer selected.
	// Once the plan is rebuilt, enforce it on disk.
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

// setPreloadFile plans the head and tail of the episode expected to play next
// as durable startup pieces in a separate budget. Returns the planned pieces.
// A negative index cancels the preload; saved pieces stay until the next plan.
func (st *DocumentTorrentStorage) setPreloadFile(index int) map[int]bool {
	st.mu.Lock()
	defer st.mu.Unlock()
	st.preloadPieces = nil
	if !st.stream || st.closed || st.previewTemp || st.startupPieces == nil || index < 0 || index >= len(st.files) {
		return nil
	}
	file := st.files[index]
	planned := make(map[int]bool)
	var budget int64
	for _, span := range [][2]int64{
		{file.Offset, file.Offset + min(file.Length, directStartupHead)},
		{file.Offset + max(int64(0), file.Length-directStartupTail), file.Offset + file.Length},
	} {
		if span[1] <= span[0] {
			continue
		}
		for p := int(span[0] / st.chunkLength); p <= int((span[1]-1)/st.chunkLength); p++ {
			if planned[p] {
				continue
			}
			// Pieces shared with the current plan are already budgeted there.
			if !st.startupPieces[p] {
				length := st.info.Piece(p).Length()
				if budget+length > directPreloadBudget {
					continue
				}
				budget += length
			}
			planned[p] = true
		}
	}
	st.preloadFile, st.preloadPieces = index, planned
	missing := make(map[int]bool)
	for p := range planned {
		st.startupPieces[p] = true
		if !st.startupSaved[p] && !st.completed[p] {
			missing[p] = true
		}
	}
	if len(missing) != 0 {
		st.validateStartupPiecesLocked(missing)
	}
	for p := range planned {
		if st.completed[p] {
			st.saveStartupPieceLocked(p)
		}
	}
	return maps.Clone(planned)
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
	for p := range planned {
		delete(st.startupOnDisk, p)
	}
	for p := range valid {
		// A peer may have written part of this piece into RAM. Reads prefer
		// RAM, so the verified file must replace it, and the library must
		// learn the piece is complete to stop fetching it.
		st.dropCacheLocked(p)
		delete(st.streamRanges, p)
		delete(st.streamWhole, p)
		delete(st.streamLost, p)
		st.startupSaved[p], st.completed[p] = true, true
		st.queueCompletionLocked(p)
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
		if st.completed[index] && int64(len(st.cache[index])) == pieceLen {
			// A player reopens the file on every seek; the piece is already here.
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
		delete(st.streamLost, index)
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
