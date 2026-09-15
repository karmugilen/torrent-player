package main

import (
	"bytes"
	"context"
	"os"
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
