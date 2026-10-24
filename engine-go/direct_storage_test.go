package main

import (
	"bytes"
	"crypto/sha1"
	"errors"
	"github.com/anacrolix/torrent/metainfo"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func directStorageFixture(t *testing.T, base string, pieceLength int64, pieces int) (*DocumentTorrentStorage, []byte) {
	t.Helper()
	data := make([]byte, int(pieceLength)*pieces)
	for i := range data {
		data[i] = byte(i*17 + i/int(pieceLength))
	}
	info := &metainfo.Info{Name: "movie.mp4", Length: int64(len(data)), PieceLength: pieceLength}
	for i := 0; i < len(data); i += int(pieceLength) {
		hash := sha1.Sum(data[i : i+int(pieceLength)])
		info.Pieces = append(info.Pieces, hash[:]...)
	}
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, base)
	st.streamMaxBytes = 4 * pieceLength
	if err := st.AttachDescriptorsMode([]*int{nil}, []int{0}, true); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = st.Close() })
	return st, data
}
func putDirectPiece(t *testing.T, st *DocumentTorrentStorage, data []byte, index int) {
	t.Helper()
	p := &documentPiece{storage: st, piece: st.info.Piece(index)}
	begin := int64(index) * st.chunkLength
	if _, err := p.WriteAt(data[begin:begin+p.piece.Length()], 0); err != nil {
		t.Fatal(err)
	}
	if err := p.MarkComplete(); err != nil {
		t.Fatal(err)
	}
}

func TestDirectPlayPersistsOnlyVerifiedStartupAndRejectsCorruption(t *testing.T) {
	base := t.TempDir()
	st, data := directStorageFixture(t, base, 1<<20, 12)
	for _, index := range []int{0, 6, 11} {
		putDirectPiece(t, st, data, index)
	}
	// A partial opening piece is never persisted.
	p := &documentPiece{storage: st, piece: st.info.Piece(1)}
	if _, err := p.WriteAt(data[1<<20:(1<<20)+1024], 0); err != nil {
		t.Fatal(err)
	}
	if err := st.Close(); err != nil {
		t.Fatal(err)
	}
	for _, index := range []int{1, 6} {
		if _, err := os.Stat(st.startupPiecePath(index)); !os.IsNotExist(err) {
			t.Fatalf("persisted nonverified/body piece %d: %v", index, err)
		}
	}
	// Corruption must not be trusted as a warm start.
	if err := os.WriteFile(st.startupPiecePath(0), bytes.Repeat([]byte{3}, (1<<20)+32), 0600); err != nil {
		t.Fatal(err)
	}
	warm, _ := directStorageFixture(t, base, 1<<20, 12)
	if warm.PieceComplete(0) || warm.PieceComplete(6) || !warm.PieceComplete(11) {
		t.Fatal("wrong restored completion bits")
	}
	got := make([]byte, 97)
	if _, err := (&documentPiece{storage: warm, piece: warm.info.Piece(11)}).ReadAt(got, 31); err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(got, data[(11<<20)+31:(11<<20)+128]) {
		t.Fatal("warm read differs")
	}
	for _, dir := range []string{".stream", ".boundary"} {
		if _, err := os.Stat(filepath.Join(base, dir)); !os.IsNotExist(err) {
			t.Fatalf("full backing created: %s", dir)
		}
	}
}

func TestDirectPlayEvictsPartialWithoutReturningFabricatedBytes(t *testing.T) {
	st, data := directStorageFixture(t, t.TempDir(), 8, 8)
	st.startupPieces = map[int]bool{}
	for i := 0; i < 4; i++ {
		p := &documentPiece{storage: st, piece: st.info.Piece(i)}
		if _, err := p.WriteAt(data[i*8:i*8+4], 0); err != nil {
			t.Fatal(err)
		}
	}
	putDirectPiece(t, st, data, 4)
	p := &documentPiece{storage: st, piece: st.info.Piece(0)}
	if _, err := p.ReadAt(make([]byte, 8), 0); !errors.Is(err, errStreamEvicted) {
		t.Fatalf("lost partial was readable: %v", err)
	}
	if _, err := p.WriteAt(data[4:8], 4); err != nil {
		t.Fatal(err)
	}
	if _, err := p.ReadAt(make([]byte, 8), 0); !errors.Is(err, errStreamEvicted) {
		t.Fatal("zero gaps were exposed")
	}
	if err := p.MarkNotComplete(); err != nil {
		t.Fatal(err)
	}
	putDirectPiece(t, st, data, 0)
	got := make([]byte, 8)
	if _, err := p.ReadAt(got, 0); err != nil || !bytes.Equal(got, data[:8]) {
		t.Fatalf("refetch failed: %v", err)
	}
	if st.cacheBytes > st.maxCacheBytes {
		t.Fatal("RAM budget exceeded")
	}
}

func TestDirectPlayProtectsFullyReceivedPiecesUntilVerification(t *testing.T) {
	st, data := directStorageFixture(t, t.TempDir(), 8, 8)
	st.startupPieces = map[int]bool{}
	for i := 0; i < 4; i++ {
		if _, err := (&documentPiece{storage: st, piece: st.info.Piece(i)}).WriteAt(data[i*8:i*8+8], 0); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := (&documentPiece{storage: st, piece: st.info.Piece(4)}).WriteAt(data[32:40], 0); !errors.Is(err, errStreamBusy) {
		t.Fatalf("hashing piece evicted: %v", err)
	}
	if err := (&documentPiece{storage: st, piece: st.info.Piece(0)}).MarkComplete(); err != nil {
		t.Fatal(err)
	}
	putDirectPiece(t, st, data, 4)
	if st.PieceComplete(0) {
		t.Fatal("evicted RAM piece still complete")
	}
}

func TestDirectPlayCloseDoesNotJoinBlockedCompletionCallback(t *testing.T) {
	st, data := directStorageFixture(t, t.TempDir(), 8, 8)
	entered, release := make(chan struct{}, 1), make(chan struct{})
	st.onStreamEvict = func(int) {
		select {
		case entered <- struct{}{}:
		default:
		}
		<-release
	}
	defer close(release)
	putDirectPiece(t, st, data, 0)
	select {
	case <-entered:
	case <-time.After(time.Second):
		t.Fatal("no completion callback")
	}
	done := make(chan error, 1)
	go func() { done <- st.Close() }()
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(time.Second):
		t.Fatal("close deadlocked")
	}
}

func TestStartupPruningKeepsActiveReadersCache(t *testing.T) {
	base := t.TempDir()
	active, old := "active", "old"
	for _, name := range []string{active, old} {
		dir := filepath.Join(base, ".startup", name)
		if err := os.MkdirAll(dir, 0700); err != nil {
			t.Fatal(err)
		}
		f, err := os.Create(filepath.Join(dir, "0.piece"))
		if err != nil {
			t.Fatal(err)
		}
		if err := f.Truncate(20 << 20); err != nil {
			t.Fatal(err)
		}
		_ = f.Close()
	}
	registerStartupCache(base, active)
	defer unregisterStartupCache(base, active)
	pruneStartupCaches(base, "")
	if _, err := os.Stat(filepath.Join(base, ".startup", active)); err != nil {
		t.Fatal("active cache removed")
	}
	if _, err := os.Stat(filepath.Join(base, ".startup", old)); !os.IsNotExist(err) {
		t.Fatal("inactive cache retained above budget")
	}
}

func TestEpisodeSwitchMovesStartupBudgetToOpenedEpisode(t *testing.T) {
	payload := bytes.Repeat([]byte("abcdefgh"), (1<<20)/8)
	hash := sha1.Sum(payload)
	info := &metainfo.Info{Name: "episodes", PieceLength: 1 << 20}
	for _, name := range []string{"episode1.mp4", "episode2.mp4", "episode3.mp4", "episode4.mp4"} {
		info.Files = append(info.Files, metainfo.FileInfo{Length: 12 << 20, Path: []string{name}})
	}
	for i := 0; i < 48; i++ {
		info.Pieces = append(info.Pieces, hash[:]...)
	}
	base := t.TempDir()
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, base)
	st.streamMaxBytes = 16 << 20
	if err := st.AttachDescriptorsMode([]*int{nil, nil, nil, nil}, []int{0, 1, 2, 3}, true); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = st.Close() })
	put := func(index int) {
		p := &documentPiece{storage: st, piece: info.Piece(index)}
		if _, err := p.WriteAt(payload, 0); err != nil {
			t.Fatal(err)
		}
		if err := p.MarkComplete(); err != nil {
			t.Fatal(err)
		}
	}
	put(0)
	put(1)
	put(11)
	st.setStartupFile(3)
	for _, index := range []int{36, 37, 47} {
		if !st.startupPieces[index] {
			t.Fatalf("opened episode missing startup piece %d", index)
		}
		put(index)
	}
	if err := st.Close(); err != nil {
		t.Fatal(err)
	}
	files, err := os.ReadDir(filepath.Join(base, ".startup", st.infoHash.HexString()))
	if err != nil {
		t.Fatal(err)
	}
	var size int64
	for _, file := range files {
		stat, err := file.Info()
		if err != nil {
			t.Fatal(err)
		}
		size += stat.Size()
	}
	if size > directStartupBudget+int64(len(files))*32 {
		t.Fatalf("startup disk budget exceeded: %d", size)
	}
	for _, index := range []int{36, 37, 47} {
		raw, err := os.ReadFile(st.startupPiecePath(index))
		if err != nil || !st.validStartupPiece(index, raw) {
			t.Fatalf("current episode not durable: %d %v", index, err)
		}
	}
	warm := newDocumentTorrentStorage(info, metainfo.Hash{}, base)
	warm.streamMaxBytes = 16 << 20
	if err := warm.AttachDescriptorsMode([]*int{nil, nil, nil, nil}, []int{0, 1, 2, 3}, true); err != nil {
		t.Fatal(err)
	}
	defer warm.Close()
	warm.setStartupFile(3)
	for _, index := range []int{36, 37, 47} {
		if !warm.PieceComplete(index) {
			t.Fatalf("opened episode missed warm piece %d after restart", index)
		}
	}

}

func TestNextEpisodePreloadHasItsOwnStartupBudget(t *testing.T) {
	// 4 MiB pieces: the current episode's head and tail fill the whole budget.
	payload := bytes.Repeat([]byte("abcdefgh"), (4<<20)/8)
	hash := sha1.Sum(payload)
	info := &metainfo.Info{Name: "episodes", PieceLength: 4 << 20}
	for _, name := range []string{"episode1.mkv", "episode2.mkv"} {
		info.Files = append(info.Files, metainfo.FileInfo{Length: 12 << 20, Path: []string{name}})
	}
	for i := 0; i < 6; i++ {
		info.Pieces = append(info.Pieces, hash[:]...)
	}
	base := t.TempDir()
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, base)
	st.streamMaxBytes = 16 << 20
	if err := st.AttachDescriptorsMode([]*int{nil, nil}, []int{0, 1}, true); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = st.Close() })
	put := func(index int) {
		p := &documentPiece{storage: st, piece: info.Piece(index)}
		if _, err := p.WriteAt(payload, 0); err != nil {
			t.Fatal(err)
		}
		if err := p.MarkComplete(); err != nil {
			t.Fatal(err)
		}
	}
	saved := func(index int) bool {
		deadline := time.Now().Add(5 * time.Second)
		for time.Now().Before(deadline) {
			st.mu.Lock()
			ok := st.startupSaved[index]
			st.mu.Unlock()
			if ok {
				return true
			}
			time.Sleep(10 * time.Millisecond)
		}
		return false
	}
	st.setStartupFile(0)
	put(0)
	put(2)
	planned := st.setPreloadFile(1)
	if len(planned) != 2 || !planned[3] || !planned[5] {
		t.Fatalf("next episode plan: %v", planned)
	}
	put(3)
	put(5)
	// A player reopening the current episode must not evict the warmed one.
	st.setStartupFile(0)
	for _, index := range []int{0, 2, 3, 5} {
		if !saved(index) {
			t.Fatalf("startup piece %d not saved", index)
		}
	}
	for _, index := range []int{3, 5} {
		raw, err := os.ReadFile(st.startupPiecePath(index))
		if err != nil || !st.validStartupPiece(index, raw) {
			t.Fatalf("warmed piece %d not durable: %v", index, err)
		}
	}
	// Playing the next episode keeps its pieces and ends the separate budget.
	st.setStartupFile(1)
	if st.preloadPieces != nil {
		t.Fatal("preload outlived the episode switch")
	}
	for _, index := range []int{3, 5} {
		if !st.PieceComplete(index) {
			t.Fatalf("next episode lost warm piece %d", index)
		}
	}
	if st.setPreloadFile(-1) != nil || st.preloadPieces != nil {
		t.Fatal("cancel left a preload plan")
	}
}
