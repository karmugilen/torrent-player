package main

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/anacrolix/torrent/metainfo"
)

func TestSelectedFileBoundaryRetainsWholePiece(t *testing.T) {
	info := &metainfo.Info{Name: "bundle", PieceLength: 8, Files: []metainfo.FileInfo{{Length: 4, Path: []string{"a"}}, {Length: 4, Path: []string{"b"}}}, Pieces: make([]byte, 20)}
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, t.TempDir())
	t.Cleanup(func() { _ = st.Close() })
	dest, err := os.CreateTemp(t.TempDir(), "selected")
	if err != nil {
		t.Fatal(err)
	}
	defer dest.Close()
	fd := int(dest.Fd())
	if err := st.AttachDescriptors([]*int{&fd, nil}, []int{0}); err != nil {
		t.Fatal(err)
	}
	piece := &documentPiece{storage: st, piece: info.Piece(0)}
	want := []byte("AAAABBBB")
	if _, err := piece.WriteAt(want, 0); err != nil {
		t.Fatal(err)
	}
	got := make([]byte, len(want))
	if _, err := piece.ReadAt(got, 0); err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(got, want) {
		t.Fatalf("got %q, want %q", got, want)
	}
}

func TestClosedStorageIsNotReused(t *testing.T) {
	info := &metainfo.Info{Name: "one", PieceLength: 4, Length: 4, Pieces: make([]byte, 20)}
	client := NewDocumentStorageClient(t.TempDir())
	hash := metainfo.Hash{}
	first, err := client.OpenTorrent(context.Background(), info, hash)
	if err != nil {
		t.Fatal(err)
	}
	old := client.GetStorage(hash.HexString())
	if err := first.Close(); err != nil {
		t.Fatal(err)
	}
	second, err := client.OpenTorrent(context.Background(), info, hash)
	if err != nil {
		t.Fatal(err)
	}
	defer second.Close()
	if client.GetStorage(hash.HexString()) == old {
		t.Fatal("closed storage was reused")
	}
	if completion := second.Piece(info.Piece(0)).Completion(); completion.Ok {
		t.Fatalf("fresh storage reported known completion: %+v", completion)
	}
}

func TestPreparedCacheLimitIsEnforced(t *testing.T) {
	info := &metainfo.Info{Name: "one", PieceLength: 4, Length: 8, Pieces: make([]byte, 40)}
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, t.TempDir())
	defer st.Close()
	st.maxCacheBytes = 4
	if _, err := (&documentPiece{storage: st, piece: info.Piece(0)}).WriteAt([]byte("DATA"), 0); err != nil {
		t.Fatal(err)
	}
	if _, err := (&documentPiece{storage: st, piece: info.Piece(1)}).WriteAt([]byte("DATA"), 0); err == nil {
		t.Fatal("cache limit was not enforced")
	}
}

func TestAttachDescriptorsRetainsUnselectedExternalFiles(t *testing.T) {
	info := &metainfo.Info{Name: "bundle", PieceLength: 4, Files: []metainfo.FileInfo{
		{Length: 4, Path: []string{"a"}}, {Length: 4, Path: []string{"b"}},
	}, Pieces: make([]byte, 40)}
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, t.TempDir())
	defer st.Close()
	a, err := os.CreateTemp(t.TempDir(), "a")
	if err != nil {
		t.Fatal(err)
	}
	b, err := os.CreateTemp(t.TempDir(), "b")
	if err != nil {
		a.Close()
		t.Fatal(err)
	}
	defer a.Close()
	defer b.Close()
	ad, bd := int(a.Fd()), int(b.Fd())
	if err := st.AttachDescriptors([]*int{&ad, &bd}, []int{0}); err != nil {
		t.Fatal(err)
	}
	if !st.files[1].external || st.files[1].File == nil {
		t.Fatal("unselected external descriptor was not retained")
	}
	if !st.SelectionCanUseExistingFiles([]int{1}) {
		t.Fatal("retained external file cannot be selected later")
	}
	if err := st.UpdateSelection([]*int{nil, nil}, []int{1}); err != nil {
		t.Fatal(err)
	}
	if st.files[1].File == nil || !st.files[1].external {
		t.Fatal("reselection discarded retained external file")
	}
}

func TestWriteBackCacheServesFromRAMBeforeDisk(t *testing.T) {
	base := t.TempDir()
	info := &metainfo.Info{Name: "one", PieceLength: 8, Length: 16, Pieces: make([]byte, 40)}
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, base)
	st.maxCacheBytes = 64
	t.Cleanup(func() { _ = st.Close() })
	dest, err := os.CreateTemp(base, "dest")
	if err != nil {
		t.Fatal(err)
	}
	defer dest.Close()
	fd := int(dest.Fd())
	if err := st.AttachDescriptors([]*int{&fd}, []int{0}); err != nil {
		t.Fatal(err)
	}
	p0 := &documentPiece{storage: st, piece: info.Piece(0)}
	want := []byte("ABCDEFGH")
	if _, err := p0.WriteAt(want, 0); err != nil {
		t.Fatal(err)
	}
	st.mu.Lock()
	if !st.cacheDirty[0] {
		st.mu.Unlock()
		t.Fatal("write-back cache should be dirty before flush")
	}
	if _, ok := st.cache[0]; !ok {
		st.mu.Unlock()
		t.Fatal("piece missing from RAM cache")
	}
	st.mu.Unlock()
	got := make([]byte, 8)
	if _, err := p0.ReadAt(got, 0); err != nil {
		t.Fatal(err)
	}
	if string(got) != string(want) {
		t.Fatalf("RAM read got %q want %q", got, want)
	}
	if err := p0.MarkComplete(); err != nil {
		t.Fatal(err)
	}
	st.mu.Lock()
	dirty := st.cacheDirty[0]
	st.mu.Unlock()
	if dirty {
		t.Fatal("MarkComplete should flush dirty piece to disk")
	}
}

// A flush writes only the bytes received since the last flush. Rewriting the
// whole piece on every 20 ms flush held the storage lock for a full piece
// write (4–16 MiB) per block that arrived, stalling peer writes and reads.
func TestFlushWritesOnlyBytesReceivedSinceLastFlush(t *testing.T) {
	base := t.TempDir()
	info := &metainfo.Info{Name: "one", PieceLength: 12, Length: 12, Pieces: make([]byte, 20)}
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, base)
	t.Cleanup(func() { _ = st.Close() })
	dest, err := os.CreateTemp(base, "dest")
	if err != nil {
		t.Fatal(err)
	}
	defer dest.Close()
	fd := int(dest.Fd())
	if err := st.AttachDescriptors([]*int{&fd}, []int{0}); err != nil {
		t.Fatal(err)
	}
	flush := func() {
		st.mu.Lock()
		defer st.mu.Unlock()
		if err := st.flushAllDirtyLocked(); err != nil {
			t.Fatal(err)
		}
	}
	p0 := &documentPiece{storage: st, piece: info.Piece(0)}
	if _, err := p0.WriteAt([]byte("AAAA"), 0); err != nil {
		t.Fatal(err)
	}
	flush()
	// Mark the flushed bytes on disk: a whole-piece rewrite would restore them.
	if _, err := dest.WriteAt([]byte("zzzz"), 0); err != nil {
		t.Fatal(err)
	}
	// Out of order, so the dirty bytes are two separate ranges.
	if _, err := p0.WriteAt([]byte("CCCC"), 8); err != nil {
		t.Fatal(err)
	}
	if _, err := p0.WriteAt([]byte("BB"), 4); err != nil {
		t.Fatal(err)
	}
	flush()
	got := make([]byte, 12)
	if _, err := dest.ReadAt(got, 0); err != nil {
		t.Fatal(err)
	}
	if want := "zzzzBB\x00\x00CCCC"; string(got) != want {
		t.Fatalf("disk got %q want %q", got, want)
	}
	st.mu.Lock()
	dirty, spans := st.cacheDirty[0], len(st.dirtySpans[0])
	st.mu.Unlock()
	if dirty || spans != 0 {
		t.Fatalf("after flush dirty=%v spans=%d", dirty, spans)
	}
}

// A piece put in RAM for a write is not first read from disk. A read of
// bytes it has not received loads the saved ones, after flushing its own.
func TestWriteSlotLoadsSavedBytesOnlyWhenRead(t *testing.T) {
	base := t.TempDir()
	info := &metainfo.Info{Name: "one", PieceLength: 8, Length: 8, Pieces: make([]byte, 20)}
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, base)
	t.Cleanup(func() { _ = st.Close() })
	dest, err := os.CreateTemp(base, "dest")
	if err != nil {
		t.Fatal(err)
	}
	defer dest.Close()
	fd := int(dest.Fd())
	if err := st.AttachDescriptors([]*int{&fd}, []int{0}); err != nil {
		t.Fatal(err)
	}
	// A block flushed before the piece left RAM.
	if _, err := dest.WriteAt([]byte("xxxxYYYY"), 0); err != nil {
		t.Fatal(err)
	}
	p0 := &documentPiece{storage: st, piece: info.Piece(0)}
	if _, err := p0.WriteAt([]byte("AAAA"), 0); err != nil {
		t.Fatal(err)
	}
	st.mu.Lock()
	_, partial := st.partialSlots[0]
	st.mu.Unlock()
	if !partial {
		t.Fatal("write slot was loaded from disk")
	}
	got := make([]byte, 4)
	if _, err := p0.ReadAt(got, 0); err != nil || string(got) != "AAAA" {
		t.Fatalf("written bytes: %q %v", got, err)
	}
	all := make([]byte, 8)
	if _, err := p0.ReadAt(all, 0); err != nil || string(all) != "AAAAYYYY" {
		t.Fatalf("whole piece: %q %v", all, err)
	}
	disk := make([]byte, 8)
	if _, err := dest.ReadAt(disk, 0); err != nil || string(disk) != "AAAAYYYY" {
		t.Fatalf("disk: %q %v", disk, err)
	}
	st.mu.Lock()
	_, partial = st.partialSlots[0]
	st.mu.Unlock()
	if partial {
		t.Fatal("slot still partial after loading")
	}
}

// A piece fully written in RAM never needs its saved bytes.
func TestFullyWrittenSlotIsNotPartial(t *testing.T) {
	base := t.TempDir()
	info := &metainfo.Info{Name: "one", PieceLength: 8, Length: 8, Pieces: make([]byte, 20)}
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, base)
	t.Cleanup(func() { _ = st.Close() })
	dest, err := os.CreateTemp(base, "dest")
	if err != nil {
		t.Fatal(err)
	}
	defer dest.Close()
	fd := int(dest.Fd())
	if err := st.AttachDescriptors([]*int{&fd}, []int{0}); err != nil {
		t.Fatal(err)
	}
	p0 := &documentPiece{storage: st, piece: info.Piece(0)}
	for _, w := range []struct {
		data string
		off  int64
	}{{"BBBB", 4}, {"AAAA", 0}} {
		if _, err := p0.WriteAt([]byte(w.data), w.off); err != nil {
			t.Fatal(err)
		}
	}
	st.mu.Lock()
	_, partial := st.partialSlots[0]
	st.mu.Unlock()
	if partial {
		t.Fatal("fully written slot still partial")
	}
}

// A peer's request for a piece not in RAM reads only the requested bytes and
// leaves the RAM cache to the pieces being downloaded.
func TestReadMissReadsOnlyRequestedBytes(t *testing.T) {
	base := t.TempDir()
	info := &metainfo.Info{Name: "one", PieceLength: 8, Length: 12, Pieces: make([]byte, 40)}
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, base)
	t.Cleanup(func() { _ = st.Close() })
	dest, err := os.CreateTemp(base, "dest")
	if err != nil {
		t.Fatal(err)
	}
	defer dest.Close()
	fd := int(dest.Fd())
	if err := st.AttachDescriptors([]*int{&fd}, []int{0}); err != nil {
		t.Fatal(err)
	}
	if _, err := dest.WriteAt([]byte("ABCDEFGHIJKL"), 0); err != nil {
		t.Fatal(err)
	}
	p0 := &documentPiece{storage: st, piece: info.Piece(0)}
	got := make([]byte, 3)
	if n, err := p0.ReadAt(got, 2); err != nil || n != 3 || string(got) != "CDE" {
		t.Fatalf("got %q n=%d err=%v", got, n, err)
	}
	// The last piece is 4 bytes: a longer read stops at its end.
	p1 := &documentPiece{storage: st, piece: info.Piece(1)}
	tail := make([]byte, 6)
	if n, err := p1.ReadAt(tail, 1); err != io.EOF || n != 3 || string(tail[:n]) != "JKL" {
		t.Fatalf("tail %q n=%d err=%v", tail[:n], n, err)
	}
	st.mu.Lock()
	cached := len(st.cache)
	st.mu.Unlock()
	if cached != 0 {
		t.Fatalf("read miss cached %d pieces", cached)
	}
}

// Bytes prepared before a destination is attached are written alone: the
// rest of their piece in RAM is zeros, not the file's existing bytes.
func TestAttachWritesOnlyPreparedBytes(t *testing.T) {
	base := t.TempDir()
	info := &metainfo.Info{Name: "one", PieceLength: 8, Length: 8, Pieces: make([]byte, 20)}
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, base)
	t.Cleanup(func() { _ = st.Close() })
	p0 := &documentPiece{storage: st, piece: info.Piece(0)}
	if _, err := p0.WriteAt([]byte("AA"), 0); err != nil {
		t.Fatal(err)
	}
	dest, err := os.CreateTemp(base, "dest")
	if err != nil {
		t.Fatal(err)
	}
	defer dest.Close()
	if _, err := dest.WriteAt([]byte("12345678"), 0); err != nil {
		t.Fatal(err)
	}
	fd := int(dest.Fd())
	if err := st.AttachDescriptors([]*int{&fd}, []int{0}); err != nil {
		t.Fatal(err)
	}
	disk := make([]byte, 8)
	if _, err := dest.ReadAt(disk, 0); err != nil || string(disk) != "AA345678" {
		t.Fatalf("disk %q %v", disk, err)
	}
	// RAM does not serve the zeros in place of the saved bytes either.
	got := make([]byte, 8)
	if _, err := p0.ReadAt(got, 0); err != nil || string(got) != "AA345678" {
		t.Fatalf("read %q %v", got, err)
	}
}

// Close reports a write-back that failed instead of dropping it silently.
func TestCloseReportsFailedWriteBack(t *testing.T) {
	base := t.TempDir()
	info := &metainfo.Info{Name: "one", PieceLength: 8, Length: 8, Pieces: make([]byte, 20)}
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, base)
	dest, err := os.CreateTemp(base, "dest")
	if err != nil {
		t.Fatal(err)
	}
	defer dest.Close()
	fd := int(dest.Fd())
	if err := st.AttachDescriptors([]*int{&fd}, []int{0}); err != nil {
		t.Fatal(err)
	}
	readOnly, err := os.Open(dest.Name()) // every write fails
	if err != nil {
		t.Fatal(err)
	}
	st.mu.Lock()
	st.files[0].File = readOnly
	st.mu.Unlock()
	p0 := &documentPiece{storage: st, piece: info.Piece(0)}
	if _, err := p0.WriteAt([]byte("LOST"), 0); err != nil {
		t.Fatal(err)
	}
	if err := st.Close(); err == nil {
		t.Fatal("Close hid a failed write-back")
	}
}

// An empty write before attaching prepares nothing, so attaching leaves the
// existing file alone.
func TestEmptyPreparedWriteKeepsExistingFile(t *testing.T) {
	base := t.TempDir()
	info := &metainfo.Info{Name: "one", PieceLength: 8, Length: 8, Pieces: make([]byte, 20)}
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, base)
	t.Cleanup(func() { _ = st.Close() })
	p0 := &documentPiece{storage: st, piece: info.Piece(0)}
	if n, err := p0.WriteAt(nil, 0); n != 0 || err != nil {
		t.Fatalf("empty write n=%d err=%v", n, err)
	}
	dest, err := os.CreateTemp(base, "dest")
	if err != nil {
		t.Fatal(err)
	}
	defer dest.Close()
	if _, err := dest.WriteAt([]byte("12345678"), 0); err != nil {
		t.Fatal(err)
	}
	fd := int(dest.Fd())
	if err := st.AttachDescriptors([]*int{&fd}, []int{0}); err != nil {
		t.Fatal(err)
	}
	disk := make([]byte, 8)
	if _, err := dest.ReadAt(disk, 0); err != nil || string(disk) != "12345678" {
		t.Fatalf("disk %q %v", disk, err)
	}
}

// A background save that keeps failing is reported and retried with backoff,
// not every 20 ms; once the disk accepts it again the error clears.
func TestFailedWriteBackIsReportedAndBacksOff(t *testing.T) {
	base := t.TempDir()
	info := &metainfo.Info{Name: "one", PieceLength: 8, Length: 8, Pieces: make([]byte, 20)}
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, base)
	t.Cleanup(func() { _ = st.Close() })
	dest, err := os.CreateTemp(base, "dest")
	if err != nil {
		t.Fatal(err)
	}
	defer dest.Close()
	fd := int(dest.Fd())
	if err := st.AttachDescriptors([]*int{&fd}, []int{0}); err != nil {
		t.Fatal(err)
	}
	readOnly, err := os.Open(dest.Name())
	if err != nil {
		t.Fatal(err)
	}
	defer readOnly.Close()
	st.mu.Lock()
	writable := st.files[0].File
	st.files[0].File = readOnly
	st.mu.Unlock()
	p0 := &documentPiece{storage: st, piece: info.Piece(0)}
	if _, err := p0.WriteAt([]byte("SAVE"), 0); err != nil {
		t.Fatal(err)
	}
	waitFor := func(what string, ok func() bool) {
		t.Helper()
		for deadline := time.Now().Add(10 * time.Second); !ok(); {
			if time.Now().After(deadline) {
				t.Fatal(what)
			}
			time.Sleep(5 * time.Millisecond)
		}
	}
	waitFor("failed save not reported", func() bool { return st.WriteBackError() != nil })
	st.mu.Lock()
	retry := st.flushRetry
	st.mu.Unlock()
	if retry < 100*time.Millisecond {
		t.Fatalf("retry after failure %v", retry)
	}
	st.mu.Lock()
	st.files[0].File = writable
	st.mu.Unlock()
	waitFor("error not cleared after a successful save", func() bool { return st.WriteBackError() == nil })
	disk := make([]byte, 4)
	if _, err := dest.ReadAt(disk, 0); err != nil || string(disk) != "SAVE" {
		t.Fatalf("disk %q %v", disk, err)
	}
}

// A piece evicted while downloading is loaded from disk as soon as it is
// written again, while its other blocks still arrive, not when it completes:
// that is when a player waits for it after a seek.
func TestEvictedPartialPieceLoadsOnNextWrite(t *testing.T) {
	base := t.TempDir()
	info := &metainfo.Info{Name: "one", PieceLength: 8, Length: 16, Pieces: make([]byte, 40)}
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, base)
	st.maxCacheBytes = 8 // one piece in RAM
	t.Cleanup(func() { _ = st.Close() })
	dest, err := os.CreateTemp(base, "dest")
	if err != nil {
		t.Fatal(err)
	}
	defer dest.Close()
	fd := int(dest.Fd())
	if err := st.AttachDescriptors([]*int{&fd}, []int{0}); err != nil {
		t.Fatal(err)
	}
	p0 := &documentPiece{storage: st, piece: info.Piece(0)}
	p1 := &documentPiece{storage: st, piece: info.Piece(1)}
	if _, err := p0.WriteAt([]byte("AAAA"), 0); err != nil {
		t.Fatal(err)
	}
	if _, err := p1.WriteAt([]byte("CCCC"), 0); err != nil { // evicts piece 0
		t.Fatal(err)
	}
	if _, err := p0.WriteAt([]byte("BBBB"), 4); err != nil {
		t.Fatal(err)
	}
	st.mu.Lock()
	_, partial := st.partialSlots[0]
	st.mu.Unlock()
	if partial {
		t.Fatal("evicted piece was not loaded when written again")
	}
	got := make([]byte, 8)
	if _, err := p0.ReadAt(got, 0); err != nil || string(got) != "AAAABBBB" {
		t.Fatalf("read %q %v", got, err)
	}
}

func TestMarkCompletePersistsBytesAndClearsDirty(t *testing.T) {
	base := t.TempDir()
	info := &metainfo.Info{Name: "one", PieceLength: 8, Length: 8, Pieces: make([]byte, 20)}
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, base)
	t.Cleanup(func() { _ = st.Close() })
	dest, err := os.CreateTemp(base, "dest")
	if err != nil {
		t.Fatal(err)
	}
	defer dest.Close()
	fd := int(dest.Fd())
	if err := st.AttachDescriptors([]*int{&fd}, []int{0}); err != nil {
		t.Fatal(err)
	}
	p0 := &documentPiece{storage: st, piece: info.Piece(0)}
	want := []byte("PERSIST!")
	if _, err := p0.WriteAt(want, 0); err != nil {
		t.Fatal(err)
	}
	if err := p0.MarkComplete(); err != nil {
		t.Fatal(err)
	}
	st.mu.Lock()
	dirty := st.cacheDirty[0]
	complete := st.completed[0]
	st.mu.Unlock()
	if dirty {
		t.Fatal("MarkComplete left piece dirty")
	}
	if !complete {
		t.Fatal("MarkComplete did not mark piece complete")
	}
	got := make([]byte, len(want))
	if _, err := dest.ReadAt(got, 0); err != nil {
		t.Fatal(err)
	}
	if string(got) != string(want) {
		t.Fatalf("disk got %q want %q", got, want)
	}
}

func TestWriteBackCacheEvictsLRUAfterFlush(t *testing.T) {
	base := t.TempDir()
	info := &metainfo.Info{Name: "one", PieceLength: 4, Length: 12, Pieces: make([]byte, 60)}
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, base)
	st.maxCacheBytes = 8 // two pieces
	t.Cleanup(func() { _ = st.Close() })
	dest, err := os.CreateTemp(base, "dest")
	if err != nil {
		t.Fatal(err)
	}
	defer dest.Close()
	fd := int(dest.Fd())
	if err := st.AttachDescriptors([]*int{&fd}, []int{0}); err != nil {
		t.Fatal(err)
	}
	for i, payload := range [][]byte{[]byte("AAAA"), []byte("BBBB"), []byte("CCCC")} {
		p := &documentPiece{storage: st, piece: info.Piece(i)}
		if _, err := p.WriteAt(payload, 0); err != nil {
			t.Fatalf("piece %d: %v", i, err)
		}
	}
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.cacheBytes > st.maxCacheBytes {
		t.Fatalf("cache bytes %d over cap %d", st.cacheBytes, st.maxCacheBytes)
	}
	if _, ok := st.cache[0]; ok {
		t.Fatal("oldest piece should have been evicted from RAM")
	}
	if _, ok := st.cache[2]; !ok {
		t.Fatal("newest piece should remain in RAM")
	}
}

// Regression: partial chunk write after RAM eviction must merge with disk, not
// flush a zero-filled hole over previously saved chunks (progress loop while playing).
func TestWriteBackPartialWriteAfterEvictReloadsDisk(t *testing.T) {
	base := t.TempDir()
	info := &metainfo.Info{Name: "one", PieceLength: 8, Length: 16, Pieces: make([]byte, 40)}
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, base)
	st.maxCacheBytes = 8 // one piece in RAM
	t.Cleanup(func() { _ = st.Close() })
	dest, err := os.CreateTemp(base, "dest")
	if err != nil {
		t.Fatal(err)
	}
	defer dest.Close()
	fd := int(dest.Fd())
	if err := st.AttachDescriptors([]*int{&fd}, []int{0}); err != nil {
		t.Fatal(err)
	}

	p0 := &documentPiece{storage: st, piece: info.Piece(0)}
	p1 := &documentPiece{storage: st, piece: info.Piece(1)}
	if _, err := p0.WriteAt([]byte("AAAAxxxx"), 0); err != nil {
		t.Fatal(err)
	}
	// Force piece 0 to disk and out of RAM by filling the single-slot cache with piece 1.
	if _, err := p1.WriteAt([]byte("BBBBBBBB"), 0); err != nil {
		t.Fatal(err)
	}
	st.mu.Lock()
	if _, ok := st.cache[0]; ok {
		st.mu.Unlock()
		t.Fatal("piece 0 should have been evicted")
	}
	st.mu.Unlock()

	// Resume piece 0 with only the second half — must not zero-clobber "AAAA".
	if _, err := p0.WriteAt([]byte("YYYY"), 4); err != nil {
		t.Fatal(err)
	}
	if err := p0.MarkComplete(); err != nil {
		t.Fatal(err)
	}

	got := make([]byte, 8)
	if _, err := dest.ReadAt(got, 0); err != nil {
		t.Fatal(err)
	}
	want := []byte("AAAAYYYY")
	if string(got) != string(want) {
		t.Fatalf("disk got %q want %q (zero-clobber after evict)", got, want)
	}
}

func TestDeleteEngineOwnedStorageWipesStreamAndBoundary(t *testing.T) {
	base := t.TempDir()
	hash := "aabbccddeeff00112233445566778899aabbccdd"
	stream := streamDir(base, hash)
	boundary := boundaryDir(base, hash)
	if err := os.MkdirAll(stream, 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(boundary, 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(stream, "0.part"), []byte("x"), 0600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(boundary, "1.part"), []byte("y"), 0600); err != nil {
		t.Fatal(err)
	}
	if err := DeleteEngineOwnedStorage(base, hash); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(stream); !os.IsNotExist(err) {
		t.Fatalf("stream dir still present: %v", err)
	}
	if _, err := os.Stat(boundary); !os.IsNotExist(err) {
		t.Fatalf("boundary dir still present: %v", err)
	}
}

func TestFilesOverlappingFindsOnlyFilesInRange(t *testing.T) {
	// Lengths 4, 0, 3, 0, 5: offsets 0, 4, 4, 7, 7.
	var files []FileDescriptor
	var offset int64
	for i, length := range []int64{4, 0, 3, 0, 5} {
		files = append(files, FileDescriptor{Offset: offset, Length: length, Path: fmt.Sprint(i)})
		offset += length
	}
	names := func(start, end int64) string {
		out := ""
		for _, f := range filesOverlapping(files, start, end) {
			if f.Length > 0 {
				out += f.Path
			}
		}
		return out
	}
	for _, c := range []struct {
		start, end int64
		want       string
	}{{0, 4, "0"}, {3, 5, "02"}, {4, 7, "2"}, {6, 12, "24"}, {0, 12, "024"}, {11, 12, "4"}, {12, 13, ""}} {
		if got := names(c.start, c.end); got != c.want {
			t.Errorf("[%d,%d) got %q want %q", c.start, c.end, got, c.want)
		}
	}
}
