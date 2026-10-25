package main

import (
	"bytes"
	"crypto/sha1"
	"os"
	"slices"
	"testing"
	"time"

	"github.com/anacrolix/torrent/metainfo"
)

// Four 12 MiB episodes of 1 MiB pieces: each has head pieces 12k, 12k+1 and
// tail pieces 12k+10, 12k+11.
func metadataEpisodes(t *testing.T, base string) (*DocumentTorrentStorage, *metainfo.Info, []byte) {
	t.Helper()
	payload := bytes.Repeat([]byte("abcdefgh"), (1<<20)/8)
	hash := sha1.Sum(payload)
	info := &metainfo.Info{Name: "season", PieceLength: 1 << 20}
	for _, name := range []string{"e1.mkv", "e2.mkv", "e3.mkv", "e4.mkv", "notes.txt"} {
		info.Files = append(info.Files, metainfo.FileInfo{Length: 12 << 20, Path: []string{name}})
	}
	for i := 0; i < 60; i++ {
		info.Pieces = append(info.Pieces, hash[:]...)
	}
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, base)
	st.streamMaxBytes = 16 << 20
	if err := st.AttachDescriptorsMode(make([]*int, 5), []int{0, 1, 2, 3, 4}, true); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = st.Close() })
	return st, info, payload
}

func episodeMetadata(k int) []int { return []int{12 * k, 12*k + 1, 12*k + 10, 12*k + 11} }

func waitStartupSaved(t *testing.T, st *DocumentTorrentStorage, pieces []int) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for {
		st.mu.Lock()
		all := true
		for _, p := range pieces {
			all = all && st.startupSaved[p]
		}
		st.mu.Unlock()
		if all {
			return
		}
		if time.Now().After(deadline) {
			t.Fatalf("pieces %v not saved", pieces)
		}
		time.Sleep(10 * time.Millisecond)
	}
}

func TestDirectPlaySavesEveryEpisodesMetadataOnce(t *testing.T) {
	base := t.TempDir()
	st, info, payload := metadataEpisodes(t, base)
	put := func(index int) {
		p := &documentPiece{storage: st, piece: info.Piece(index)}
		if _, err := p.WriteAt(payload, 0); err != nil {
			t.Fatal(err)
		}
		if err := p.MarkComplete(); err != nil {
			t.Fatal(err)
		}
	}
	var all []int
	for k := 0; k < 4; k++ {
		all = append(all, episodeMetadata(k)...)
	}
	if !slices.Equal(st.startupOrder, all) {
		t.Fatalf("plan should cover every video's head and tail only: %v", st.startupOrder)
	}

	// Batches fit a quarter of the 16 MiB cache with no player open.
	if got := st.startupFetchPieces(false); !slices.Equal(got, episodeMetadata(0)) {
		t.Fatalf("first batch: %v", got)
	}
	for _, p := range episodeMetadata(0) {
		put(p)
	}
	waitStartupSaved(t, st, episodeMetadata(0))
	if got := st.startupFetchPieces(false); !slices.Equal(got, episodeMetadata(1)) {
		t.Fatalf("second batch: %v", got)
	}
	// With a player open the batch shrinks to the playback reserve.
	if got := st.startupFetchPieces(true); len(got) != 1 {
		t.Fatalf("batch beside playback: %v", got)
	}

	// Switching episodes writes nothing again and deletes nothing.
	stat, err := os.Stat(st.startupPiecePath(0))
	if err != nil {
		t.Fatal(err)
	}
	st.setStartupFile(3)
	if !slices.Equal(st.startupOrder[:4], episodeMetadata(3)) {
		t.Fatalf("opened episode not first: %v", st.startupOrder)
	}
	for _, p := range episodeMetadata(0) {
		again, err := os.Stat(st.startupPiecePath(p))
		if err != nil {
			t.Fatalf("episode switch removed saved metadata %d: %v", p, err)
		}
		if p == 0 && !again.ModTime().Equal(stat.ModTime()) {
			t.Fatal("episode switch rewrote saved metadata")
		}
	}
	for _, p := range episodeMetadata(2) {
		put(p)
	}
	waitStartupSaved(t, st, episodeMetadata(2))
	// Body pieces are never written to disk.
	put(5)
	if _, err := os.Stat(st.startupPiecePath(5)); !os.IsNotExist(err) {
		t.Fatalf("body piece written to disk: %v", err)
	}
	if err := st.Close(); err != nil {
		t.Fatal(err)
	}

	// After a restart the first 8 MiB of the plan is validated at once; the
	// third episode is found on disk, not fetched again, and validated when
	// it opens.
	warm := newDocumentTorrentStorage(info, metainfo.Hash{}, base)
	warm.streamMaxBytes = 16 << 20
	if err := warm.AttachDescriptorsMode(make([]*int, 5), []int{0, 1, 2, 3, 4}, true); err != nil {
		t.Fatal(err)
	}
	defer warm.Close()
	if !warm.PieceComplete(0) || warm.PieceComplete(24) {
		t.Fatal("expected eager validation of the first episodes only")
	}
	if got := warm.startupFetchPieces(false); !slices.Equal(got, episodeMetadata(1)) {
		t.Fatalf("restart refetched saved metadata: %v", got)
	}
	warm.setStartupFile(2)
	for _, p := range episodeMetadata(2) {
		if !warm.PieceComplete(p) {
			t.Fatalf("opened episode missed saved piece %d", p)
		}
	}
}

func TestDirectPlayFetchesEveryEpisodesMetadataInBackground(t *testing.T) {
	if testing.Short() {
		t.Skip("local peer transfer")
	}
	const episode = int64(8 << 20)
	_, newPeer := localFilesSwarm(t, "Season", []int64{episode, episode, episode, episode})
	seed, _ := newPeer(true)
	leech, rec := newPeer(false)
	if rec.Torrent.AddClientPeer(seed.client) == 0 {
		t.Fatal("no seed address")
	}
	st := leech.storage.GetStorage(rec.InfoHash)
	st.mu.Lock()
	planned := slices.Clone(st.startupOrder)
	st.mu.Unlock()
	if len(planned) != 4*int((directStartupHead+directStartupTail)/(64<<10)) {
		t.Fatalf("plan size: %d", len(planned))
	}
	// No player is open: the metadata of every episode is saved anyway.
	deadline := time.Now().Add(30 * time.Second)
	for {
		st.mu.Lock()
		saved := 0
		for _, p := range planned {
			if st.startupSaved[p] {
				saved++
			}
		}
		st.mu.Unlock()
		if saved == len(planned) {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("saved %d of %d metadata pieces", saved, len(planned))
		}
		time.Sleep(50 * time.Millisecond)
	}
	files := rec.Torrent.Files()
	for _, f := range files {
		mid := f.BeginPieceIndex() + (f.EndPieceIndex()-f.BeginPieceIndex())/2
		if rec.Torrent.Piece(mid).State().Complete {
			t.Fatalf("fetched body piece %d", mid)
		}
	}
}

// Play raises the opened episode's head and tail before the player opens it,
// so a peer may have written part of a piece whose verified copy is on disk.
// Validating the saved copy must replace the partial RAM bytes, or the player
// reads zeros inside its moov, and must tell the library the piece is done.
func TestValidatedSavedMetadataReplacesPartialRAMPiece(t *testing.T) {
	base := t.TempDir()
	st, info, payload := metadataEpisodes(t, base)
	for _, p := range episodeMetadata(2) {
		piece := &documentPiece{storage: st, piece: info.Piece(p)}
		if _, err := piece.WriteAt(payload, 0); err != nil {
			t.Fatal(err)
		}
		if err := piece.MarkComplete(); err != nil {
			t.Fatal(err)
		}
	}
	waitStartupSaved(t, st, episodeMetadata(2))
	if err := st.Close(); err != nil {
		t.Fatal(err)
	}

	warm := newDocumentTorrentStorage(info, metainfo.Hash{}, base)
	warm.streamMaxBytes = 16 << 20
	completions := make(chan int, 64)
	warm.onStreamEvict = func(index int) { completions <- index }
	if err := warm.AttachDescriptorsMode(make([]*int, 5), []int{0, 1, 2, 3, 4}, true); err != nil {
		t.Fatal(err)
	}
	defer warm.Close()
	// A peer delivers the first KiB of piece 24 before the episode opens.
	partial := &documentPiece{storage: warm, piece: info.Piece(24)}
	if _, err := partial.WriteAt(payload[:1024], 0); err != nil {
		t.Fatal(err)
	}
	warm.setStartupFile(2)
	got := make([]byte, 4096)
	n, wait, err := warm.readVerifiedStream(got, 24<<20+8192)
	if err != nil || wait != nil || n != len(got) {
		t.Fatalf("read saved metadata: n=%d wait=%v err=%v", n, wait != nil, err)
	}
	if !bytes.Equal(got, payload[8192:8192+len(got)]) {
		t.Fatal("player read partial RAM bytes instead of the verified saved piece")
	}
	deadline := time.After(5 * time.Second)
	for {
		select {
		case index := <-completions:
			if index == 24 {
				return
			}
		case <-deadline:
			t.Fatal("library was not told the saved piece is complete")
		}
	}
}

// The startup writer keeps a verified clone while the RAM copy can be evicted
// and replaced. Committing that clone must not serve the new zero-padded buffer.
func TestSavedMetadataCommitDropsPartialRAM(t *testing.T) {
	base := t.TempDir()
	st, info, payload := metadataEpisodes(t, base)
	st.mu.Lock()
	st.startupWriting = true
	st.mu.Unlock()
	t.Cleanup(func() {
		st.mu.Lock()
		st.startupWriting = false
		st.mu.Unlock()
	})
	piece := &documentPiece{storage: st, piece: info.Piece(0)}
	if _, err := piece.WriteAt(payload, 0); err != nil {
		t.Fatal(err)
	}
	if err := piece.MarkComplete(); err != nil {
		t.Fatal(err)
	}
	demand := make(map[int]bool)
	for i := 1; i <= 15; i++ {
		p := &documentPiece{storage: st, piece: info.Piece(i)}
		if _, err := p.WriteAt(payload, 0); err != nil {
			t.Fatal(err)
		}
		demand[i] = true
	}
	st.setStreamDemand(demand)
	extra := &documentPiece{storage: st, piece: info.Piece(16)}
	if _, err := extra.WriteAt(payload[:1024], 0); err != nil {
		t.Fatal(err)
	}
	again := &documentPiece{storage: st, piece: info.Piece(0)}
	if _, err := again.WriteAt(payload[:1024], 0); err != nil {
		t.Fatal(err)
	}
	st.mu.Lock()
	cached := st.cache[0]
	if len(cached) <= 8192 || cached[8192] != 0 {
		st.mu.Unlock()
		t.Fatal("setup did not leave a zero-padded buffer over the queued piece")
	}
	if !bytes.Equal(st.startupPending[0], payload) {
		st.mu.Unlock()
		t.Fatal("queued metadata is not the verified piece")
	}
	st.startupWriting = false
	st.mu.Unlock()

	st.drainStartupPieces()
	got := make([]byte, 4096)
	n, wait, err := st.readVerifiedStream(got, 8192)
	if err != nil || wait != nil || n != len(got) {
		t.Fatalf("read saved metadata: n=%d wait=%v err=%v", n, wait != nil, err)
	}
	if !bytes.Equal(got, payload[8192:8192+len(got)]) {
		t.Fatal("player read zeros from a piece whose verified copy was saved")
	}
}

// Evicting the replacement buffer marks the piece lost. The commit has the
// verified file and must clear that flag, or the player errors instead of reading it.
func TestSavedMetadataCommitClearsEvictedFlag(t *testing.T) {
	base := t.TempDir()
	st, info, payload := metadataEpisodes(t, base)
	st.mu.Lock()
	st.startupWriting = true
	st.mu.Unlock()
	t.Cleanup(func() {
		st.mu.Lock()
		st.startupWriting = false
		st.mu.Unlock()
	})
	piece := &documentPiece{storage: st, piece: info.Piece(0)}
	if _, err := piece.WriteAt(payload, 0); err != nil {
		t.Fatal(err)
	}
	if err := piece.MarkComplete(); err != nil {
		t.Fatal(err)
	}
	demand := make(map[int]bool)
	for i := 1; i <= 15; i++ {
		p := &documentPiece{storage: st, piece: info.Piece(i)}
		if _, err := p.WriteAt(payload, 0); err != nil {
			t.Fatal(err)
		}
		demand[i] = true
	}
	st.setStreamDemand(demand)
	if _, err := (&documentPiece{storage: st, piece: info.Piece(16)}).WriteAt(payload[:1024], 0); err != nil {
		t.Fatal(err)
	}
	if _, err := (&documentPiece{storage: st, piece: info.Piece(0)}).WriteAt(payload[:16], 0); err != nil {
		t.Fatal(err)
	}
	// The partial piece is no longer protected, so the next arrival drops it
	// and records that its RAM bytes were lost.
	if _, err := (&documentPiece{storage: st, piece: info.Piece(17)}).WriteAt(payload[:16], 0); err != nil {
		t.Fatal(err)
	}
	st.mu.Lock()
	lost := st.streamLost[0]
	st.startupWriting = false
	st.mu.Unlock()
	if !lost {
		t.Fatal("setup did not mark the replaced piece evicted")
	}
	st.drainStartupPieces()
	got := make([]byte, 64)
	n, wait, err := st.readVerifiedStream(got, 8192)
	if err != nil || wait != nil || n != len(got) || !bytes.Equal(got, payload[8192:8192+len(got)]) {
		t.Fatalf("evicted flag hid the saved piece: n=%d wait=%v err=%v", n, wait != nil, err)
	}
}
