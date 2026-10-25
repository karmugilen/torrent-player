package main

import (
	"bytes"
	"os"
	"path/filepath"
	"testing"
	"time"
)

// After a seek into the middle of a video, the player must get the first
// block peers deliver, not wait for the whole piece and its hash check.
func TestSeekServesDeliveredBlocksBeforeThePieceIsVerified(t *testing.T) {
	st, info, payload := metadataEpisodes(t, t.TempDir())
	body := 5 // inside episode 1, outside its saved start and end
	p := &documentPiece{storage: st, piece: info.Piece(body)}
	if _, err := p.WriteAt(payload[:64<<10], 0); err != nil {
		t.Fatal(err)
	}
	abs := int64(body) << 20
	got := make([]byte, 128<<10)
	n, ready, err := st.readVerifiedStream(got, abs+16<<10)
	if err != nil || ready != nil || n != 48<<10 || !bytes.Equal(got[:n], payload[16<<10:64<<10]) {
		t.Fatalf("delivered block not served: n=%d ready=%v err=%v", n, ready != nil, err)
	}

	// A read past the delivered bytes waits, and the next block wakes it.
	n, ready, err = st.readVerifiedStream(got, abs+64<<10)
	if err != nil || n != 0 || ready == nil {
		t.Fatalf("undelivered bytes must wait: n=%d err=%v", n, err)
	}
	if _, err := p.WriteAt(payload[64<<10:128<<10], 64<<10); err != nil {
		t.Fatal(err)
	}
	select {
	case <-ready:
	case <-time.After(time.Second):
		t.Fatal("a delivered block did not wake the waiting read")
	}
	if n, _, _ := st.readVerifiedStream(got, abs+64<<10); n != 64<<10 || !bytes.Equal(got[:n], payload[64<<10:128<<10]) {
		t.Fatalf("woken read got %d bytes", n)
	}

	// A video's start and end hold the index players parse; only verified
	// bytes are served there.
	head := &documentPiece{storage: st, piece: info.Piece(12)}
	if _, err := head.WriteAt(payload[:64<<10], 0); err != nil {
		t.Fatal(err)
	}
	if n, ready, _ := st.readVerifiedStream(got, 12<<20); n != 0 || ready == nil {
		t.Fatalf("unverified metadata piece was served: %d bytes", n)
	}
}

// A player reopens the file for every seek. Replanning, rescanning the cache
// folder and rehashing saved pieces each time held the storage lock that
// incoming blocks need.
func TestReopeningTheSameFileDoesNotReplanMetadata(t *testing.T) {
	st, _, _ := metadataEpisodes(t, t.TempDir())
	st.setStartupFile(1)
	st.mu.Lock()
	st.startupOrder = []int{-1}
	st.mu.Unlock()
	st.setStartupFile(1)
	st.mu.Lock()
	order := st.startupOrder
	st.mu.Unlock()
	if len(order) != 1 || order[0] != -1 {
		t.Fatalf("reopening the same file replanned: %v", order)
	}
	st.setStartupFile(2)
	st.mu.Lock()
	order = st.startupOrder
	st.mu.Unlock()
	if len(order) == 0 || order[0] != 24 {
		t.Fatalf("another file must be planned first: %v", order)
	}
}

// Every seek reopens the file and re-injects the saved resume window. Pieces
// already in RAM were allocated again and counted twice, so the cache filled
// with phantom bytes and evicted the pieces the player was waiting for.
func TestReinjectingResumeBytesDoesNotLeakCacheAccounting(t *testing.T) {
	st, info, payload := metadataEpisodes(t, t.TempDir())
	counted := func() (int64, int64) {
		st.mu.Lock()
		defer st.mu.Unlock()
		var real int64
		for _, data := range st.cache {
			real += int64(len(data))
		}
		return st.cacheBytes, real
	}
	data := bytes.Repeat(payload, 2)
	for i := 0; i < 5; i++ {
		if !st.injectVerifiedBytes(5<<20, data) && i == 0 {
			t.Fatal("resume bytes not installed")
		}
		if got, real := counted(); got != real || real != 2<<20 {
			t.Fatalf("seek %d: counted %d bytes, cache holds %d", i, got, real)
		}
	}
	// A partial buffer already in RAM is replaced, not counted twice.
	p := &documentPiece{storage: st, piece: info.Piece(8)}
	if _, err := p.WriteAt(payload[:16<<10], 0); err != nil {
		t.Fatal(err)
	}
	if !st.injectVerifiedBytes(8<<20, payload) {
		t.Fatal("verified bytes not installed over a partial piece")
	}
	if got, real := counted(); got != real || real != 3<<20 {
		t.Fatalf("partial piece: counted %d bytes, cache holds %d", got, real)
	}
}

// A player closes its stream on every seek. The resume window is saved once
// seeking settles, not hashed and fsynced on each close; a reopen with the
// window already in RAM skips the disk.
func TestResumeWindowSavesOnceSeekingSettlesAndReopensFromRAM(t *testing.T) {
	s, rec, data := playbackFixtureMode(t, true)
	for index := 0; index < rec.Torrent.NumPieces(); index++ {
		writeVerifiedPiece(t, s, rec, data, index)
	}
	window := filepath.Join(resumeWindowDir(s.downloadDir, rec.InfoHash), "window.bin")
	for _, offset := range []int64{3, 11, 19} {
		s.scheduleResumePersist(rec, 1, offset)
	}
	if _, err := os.Stat(window); !os.IsNotExist(err) {
		t.Fatalf("a seek's close wrote the resume window at once: %v", err)
	}
	s.flushResumeSaves()
	meta, _, ok := s.loadResumeWindow(rec.InfoHash)
	if !ok || meta.FileIndex != 1 || meta.Offset != 19 {
		t.Fatalf("latest window not saved: ok=%v %+v", ok, meta)
	}

	// The window's pieces are in RAM, so a reopen must not need the file.
	if err := os.WriteFile(window, []byte("x"), 0600); err != nil {
		t.Fatal(err)
	}
	if offset, ok := s.applyResumeWindowToStorage(rec, 1); !ok || offset != 19 {
		t.Fatalf("reopen read the disk instead of RAM: ok=%v offset=%d", ok, offset)
	}

	// Deleting the window cancels a pending save.
	s.scheduleResumePersist(rec, 1, 11)
	s.deleteResumeWindow(rec.InfoHash)
	s.flushResumeSaves()
	if _, err := os.Stat(window); !os.IsNotExist(err) {
		t.Fatalf("a pending save brought back a deleted window: %v", err)
	}
}

// An HTTP read after a seek returns the block that arrived, instead of
// waiting until the rest of its buffer has arrived too. A full read (Android's
// file proxy) still waits.
func TestHTTPReadHandsOverTheFirstBlockAtOnce(t *testing.T) {
	s, rec := ghostProPlaybackFixture(t, 1<<20, 256, true)
	st := s.storage.GetStorage(rec.InfoHash)
	p, err := s.OpenPlayback(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer p.Close()
	const body = 100
	piece := &documentPiece{storage: st, piece: rec.Torrent.Info().Piece(body)}
	block := bytes.Repeat([]byte{7}, 16<<10)
	if _, err := piece.WriteAt(block, 0); err != nil {
		t.Fatal(err)
	}
	buf := make([]byte, 64<<10)
	start := time.Now()
	n, err := p.ReadSome(buf, int64(body)<<20)
	if err != nil || n != len(block) || !bytes.Equal(buf[:n], block) || time.Since(start) > time.Second {
		t.Fatalf("partial read: n=%d err=%v after %v", n, err, time.Since(start))
	}
	full := make(chan int, 1)
	go func() {
		n, _ := p.ReadAt(buf, int64(body)<<20)
		full <- n
	}()
	select {
	case n := <-full:
		t.Fatalf("a full read returned %d bytes before the rest arrived", n)
	case <-time.After(200 * time.Millisecond):
	}
	p.Close()
}
