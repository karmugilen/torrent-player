package main

import (
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"syscall"
	"testing"
	"time"

	"github.com/anacrolix/torrent/metainfo"
)

func getPeakRSSMiB() float64 {
	var r syscall.Rusage
	if err := syscall.Getrusage(syscall.RUSAGE_SELF, &r); err == nil {
		// On Linux, ru_maxrss is in KiB
		return float64(r.Maxrss) / 1024.0
	}
	return 0
}

const (
	benchTorrentBytes = 256 * 1024 * 1024 // 256 MiB
	benchPieceLength  = 1024 * 1024       // 1 MiB
	benchNumPieces    = 256
	benchChunkSize    = 16 * 1024 // 16 KiB
	benchChunksPiece  = benchPieceLength / benchChunkSize
)

func createBenchInfo() *metainfo.Info {
	return &metainfo.Info{
		Name:        "bench-torrent.bin",
		PieceLength: benchPieceLength,
		Length:      benchTorrentBytes,
		Pieces:      make([]byte, benchNumPieces*20),
	}
}

// 1. Download Write-Back Storage
func runGoDownloadWriteSeq(baseDir string) (time.Duration, error) {
	info := createBenchInfo()
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, baseDir)
	defer st.Close()

	destPath := filepath.Join(baseDir, "download-dest-seq.bin")
	dest, err := os.OpenFile(destPath, os.O_RDWR|os.O_CREATE|os.O_TRUNC, 0600)
	if err != nil {
		return 0, err
	}
	defer dest.Close()
	defer os.Remove(destPath)

	fd := int(dest.Fd())
	if err := st.AttachDescriptors([]*int{&fd}, []int{0}); err != nil {
		return 0, err
	}

	chunk := make([]byte, benchChunkSize)
	for i := range chunk {
		chunk[i] = byte(i & 0xFF)
	}

	start := time.Now()
	for p := 0; p < benchNumPieces; p++ {
		piece := &documentPiece{storage: st, piece: info.Piece(p)}
		for c := 0; c < benchChunksPiece; c++ {
			if _, err := piece.WriteAt(chunk, int64(c*benchChunkSize)); err != nil {
				return 0, err
			}
		}
		if err := piece.MarkComplete(); err != nil {
			return 0, err
		}
	}
	// Close to ensure all write-back buffer is drained and flushed to disk
	if err := st.Close(); err != nil {
		return 0, err
	}
	return time.Since(start), nil
}

func runGoDownloadReadSeq(baseDir string) (time.Duration, error) {
	info := createBenchInfo()
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, baseDir)

	destPath := filepath.Join(baseDir, "download-dest-readseq.bin")
	dest, err := os.OpenFile(destPath, os.O_RDWR|os.O_CREATE|os.O_TRUNC, 0600)
	if err != nil {
		return 0, err
	}
	defer dest.Close()
	defer os.Remove(destPath)

	// Pre-fill file with 256 MiB so we can read it back
	data := make([]byte, 1024*1024)
	for i := range data { data[i] = 0xAA }
	for i := 0; i < 256; i++ { if _, err := dest.Write(data); err != nil { return 0, err } }
	fd := int(dest.Fd())
	if err := st.AttachDescriptors([]*int{&fd}, []int{0}); err != nil {
		return 0, err
	}

	buf := make([]byte, benchChunkSize)
	start := time.Now()
	for p := 0; p < benchNumPieces; p++ {
		piece := &documentPiece{storage: st, piece: info.Piece(p)}
		for c := 0; c < benchChunksPiece; c++ {
			if _, err := piece.ReadAt(buf, int64(c*benchChunkSize)); err != nil {
				return 0, err
			}
		}
	}
	st.Close()
	return time.Since(start), nil
}

func runGoDownloadWriteRandom(baseDir string) (time.Duration, error) {
	info := createBenchInfo()
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, baseDir)

	destPath := filepath.Join(baseDir, "download-dest-rand.bin")
	dest, err := os.OpenFile(destPath, os.O_RDWR|os.O_CREATE|os.O_TRUNC, 0600)
	if err != nil {
		return 0, err
	}
	defer dest.Close()
	defer os.Remove(destPath)

	fd := int(dest.Fd())
	if err := st.AttachDescriptors([]*int{&fd}, []int{0}); err != nil {
		return 0, err
	}

	chunk := make([]byte, benchChunkSize)
	for i := range chunk {
		chunk[i] = byte(i & 0xFF)
	}

	perm := benchPermutation(benchNumPieces, 42)

	start := time.Now()
	for _, p := range perm {
		piece := &documentPiece{storage: st, piece: info.Piece(p)}
		for c := 0; c < benchChunksPiece; c++ {
			if _, err := piece.WriteAt(chunk, int64(c*benchChunkSize)); err != nil {
				return 0, err
			}
		}
		if err := piece.MarkComplete(); err != nil {
			return 0, err
		}
	}
	if err := st.Close(); err != nil {
		return 0, err
	}
	return time.Since(start), nil
}

func runGoDownloadReadRandom(baseDir string) (time.Duration, error) {
	info := createBenchInfo()
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, baseDir)

	destPath := filepath.Join(baseDir, "download-dest-readrand.bin")
	dest, err := os.OpenFile(destPath, os.O_RDWR|os.O_CREATE|os.O_TRUNC, 0600)
	if err != nil {
		return 0, err
	}
	defer dest.Close()
	defer os.Remove(destPath)

	data := make([]byte, 1024*1024)
	for i := range data { data[i] = 0xAA }
	for i := 0; i < 256; i++ { if _, err := dest.Write(data); err != nil { return 0, err } }
	fd := int(dest.Fd())
	if err := st.AttachDescriptors([]*int{&fd}, []int{0}); err != nil {
		return 0, err
	}

	perm := benchPermutation(benchNumPieces, 42)

	buf := make([]byte, benchChunkSize)
	start := time.Now()
	for _, p := range perm {
		piece := &documentPiece{storage: st, piece: info.Piece(p)}
		for c := 0; c < benchChunksPiece; c++ {
			if _, err := piece.ReadAt(buf, int64(c*benchChunkSize)); err != nil {
				return 0, err
			}
		}
	}
	st.Close()
	return time.Since(start), nil
}

// 2. Direct Play RAM Cache with 64 MiB limit
func runGoDirectPlaySeq(baseDir string) (time.Duration, error) {
	info := createBenchInfo()
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, baseDir)
	defer st.Close()

	st.setCacheLimit(64 * 1024 * 1024)
	descriptors := make([]*int, len(st.files))
	if err := st.AttachDescriptorsMode(descriptors, []int{0}, true); err != nil {
		return 0, err
	}

	chunk := make([]byte, benchChunkSize)
	readBuf := make([]byte, benchChunkSize)
	start := time.Now()
	for p := 0; p < benchNumPieces; p++ {
		piece := &documentPiece{storage: st, piece: info.Piece(p)}
		for c := 0; c < benchChunksPiece; c++ {
			if _, err := piece.WriteAt(chunk, int64(c*benchChunkSize)); err != nil {
				return 0, err
			}
		}
		if err := piece.MarkComplete(); err != nil { return 0, err }
		for c := 0; c < benchChunksPiece; c++ {
			if _, err := piece.ReadAt(readBuf, int64(c*benchChunkSize)); err != nil {
				return 0, err
			}
		}
	}
	return time.Since(start), nil
}

func runGoDirectPlayRandomSeeks(baseDir string) (time.Duration, error) {
	info := createBenchInfo()
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, baseDir)
	defer st.Close()

	st.setCacheLimit(64 * 1024 * 1024)
	descriptors := make([]*int, len(st.files))
	if err := st.AttachDescriptorsMode(descriptors, []int{0}, true); err != nil {
		return 0, err
	}

	chunk := make([]byte, benchChunkSize)
	readBuf := make([]byte, benchChunkSize)

	state := uint64(12345)
	// 32 seeks, 8 pieces per seek = 256 pieces = 256 MiB total streamed
	numSeeks := 32
	piecesPerSeek := 8

	start := time.Now()
	for s := 0; s < numSeeks; s++ {
		state = state*6364136223846793005 + 1
		startPiece := int(state >> 33) % (benchNumPieces - piecesPerSeek + 1)
		for p := startPiece; p < startPiece+piecesPerSeek; p++ {
			piece := &documentPiece{storage: st, piece: info.Piece(p)}
			for c := 0; c < benchChunksPiece; c++ {
				if _, err := piece.WriteAt(chunk, int64(c*benchChunkSize)); err != nil {
					return 0, err
				}
			}
			if err := piece.MarkComplete(); err != nil { return 0, err }
			for c := 0; c < benchChunksPiece; c++ {
				if _, err := piece.ReadAt(readBuf, int64(c*benchChunkSize)); err != nil {
					return 0, err
				}
			}
		}
	}
	return time.Since(start), nil
}

// 3. Checkpoint Save/Restore of 256 MiB torrent
func runGoCheckpointSave(baseDir string, iters int) (time.Duration, error) {
	chkPath := filepath.Join(baseDir, "test.chk")
	defer os.Remove(chkPath)

	cp := &pieceCheckpoint{
		InfoHash:    [20]byte{1, 2, 3, 4, 5},
		PieceLength: benchPieceLength,
		NumPieces:   benchNumPieces,
		Files: []fileIdentity{
			{External: true, Dev: 100, Ino: 200, Length: benchTorrentBytes, ModNano: time.Now().UnixNano(), Path: "bench-torrent.bin"},
		},
		Complete: make([]bool, benchNumPieces),
	}
	for i := 0; i < benchNumPieces; i += 2 {
		cp.Complete[i] = true
	}

	start := time.Now()
	for i := 0; i < iters; i++ {
		if err := writePieceCheckpoint(chkPath, cp); err != nil {
			return 0, err
		}
	}
	return time.Since(start), nil
}

func runGoCheckpointRestore(baseDir string, iters int) (time.Duration, error) {
	chkPath := filepath.Join(baseDir, "test.chk")
	defer os.Remove(chkPath)

	cp := &pieceCheckpoint{
		InfoHash:    [20]byte{1, 2, 3, 4, 5},
		PieceLength: benchPieceLength,
		NumPieces:   benchNumPieces,
		Files: []fileIdentity{
			{External: true, Dev: 100, Ino: 200, Length: benchTorrentBytes, ModNano: time.Now().UnixNano(), Path: "bench-torrent.bin"},
		},
		Complete: make([]bool, benchNumPieces),
	}
	for i := 0; i < benchNumPieces; i += 2 {
		cp.Complete[i] = true
	}
	if err := writePieceCheckpoint(chkPath, cp); err != nil {
		return 0, err
	}

	start := time.Now()
	for i := 0; i < iters; i++ {
		if _, err := readPieceCheckpoint(chkPath); err != nil {
			return 0, err
		}
	}
	return time.Since(start), nil
}

// 4. Piece Aggregation for /pieces heat grid on 10,000 pieces
func runGoPieceAggregation(iters int) (time.Duration, error) {
	const totalPieces = 10000
	const maxBuckets = 100

	// Create 500 alternating piece runs covering 10,000 pieces
	runs := make([]pieceRun, 500)
	perRun := uint32(totalPieces / len(runs))
	for i := range runs {
		flags := uint32(i % 3) // 0: empty, 1: complete, 2: receiving
		runs[i] = pieceRun{length: perRun, flags: flags}
	}

	// 100 selection ranges
	ranges := make([]pieceRange, 100)
	for i := range ranges {
		start := uint32(i * 100)
		ranges[i] = pieceRange{start: start, end: start + 80}
	}

	start := time.Now()
	for i := 0; i < iters; i++ {
		if _, err := aggregatePieceBuckets(runs, ranges, totalPieces, maxBuckets); err != nil {
			return 0, err
		}
	}
	return time.Since(start), nil
}

func TestRunBenchmarks(t *testing.T) {
	baseDir := "/home/kar/.cache/wt-tmp/perf/storage-go"
	_ = os.RemoveAll(baseDir)
	_ = os.MkdirAll(baseDir, 0700)
	defer os.RemoveAll(baseDir)

	fmt.Println("=== GO STORAGE BENCHMARKS ===")

	measure := func(name string, bytes int64, fn func() (time.Duration, error)) {
		runtime.GC()
		var m1, m2 runtime.MemStats
		runtime.ReadMemStats(&m1)
		rssBefore := getPeakRSSMiB()

		dur, err := fn()
		if err != nil {
			t.Fatalf("%s failed: %v", name, err)
		}

		runtime.ReadMemStats(&m2)
		rssAfter := getPeakRSSMiB()

		sec := dur.Seconds()
		mibs := (float64(bytes) / (1024.0 * 1024.0)) / sec
		allocKB := float64(m2.TotalAlloc-m1.TotalAlloc) / 1024.0
		numAllocs := m2.Mallocs - m1.Mallocs

		fmt.Printf("[GO BENCH] %-30s | %8.2f MiB/s | %8.2f ms | %8.1f KB alloc | %6d allocs | RSS: %6.1f MiB\n",
			name, mibs, sec*1000.0, allocKB, numAllocs, rssAfter)
		_ = rssBefore
	}

	// 1. Download Write-Back
	measure("Download_Write_Seq_256MiB", benchTorrentBytes, func() (time.Duration, error) {
		return runGoDownloadWriteSeq(baseDir)
	})
	measure("Download_Read_Seq_256MiB", benchTorrentBytes, func() (time.Duration, error) {
		return runGoDownloadReadSeq(baseDir)
	})
	measure("Download_Write_Rand_256MiB", benchTorrentBytes, func() (time.Duration, error) {
		return runGoDownloadWriteRandom(baseDir)
	})
	measure("Download_Read_Rand_256MiB", benchTorrentBytes, func() (time.Duration, error) {
		return runGoDownloadReadRandom(baseDir)
	})

	// 2. Direct Play
	measure("DirectPlay_Seq_256MiB", benchTorrentBytes, func() (time.Duration, error) {
		return runGoDirectPlaySeq(baseDir)
	})
	measure("DirectPlay_RandSeek_256MiB", benchTorrentBytes, func() (time.Duration, error) {
		return runGoDirectPlayRandomSeeks(baseDir)
	})

	// 3. Checkpoint (1000 iters)
	chkIters := 2000
	measure("Checkpoint_Save_2k", int64(chkIters*1024), func() (time.Duration, error) {
		return runGoCheckpointSave(baseDir, chkIters)
	})
	measure("Checkpoint_Restore_2k", int64(chkIters*1024), func() (time.Duration, error) {
		return runGoCheckpointRestore(baseDir, chkIters)
	})

	// 4. Piece Aggregation (5000 iters)
	aggIters := 5000
	measure("Piece_Aggregation_5k", int64(aggIters*10000), func() (time.Duration, error) {
		return runGoPieceAggregation(aggIters)
	})
}

func benchPermutation(n int, state uint64) []int {
 p := make([]int,n); for i := range p { p[i] = i }; for i := n-1; i > 0; i-- { state = state*6364136223846793005+1; j := int(state >> 33) % (i+1); p[i], p[j] = p[j], p[i] }; return p
}
