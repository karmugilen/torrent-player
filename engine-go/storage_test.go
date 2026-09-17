package main

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"testing"

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
