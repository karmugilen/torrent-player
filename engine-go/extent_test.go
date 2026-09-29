package main

import (
	"bytes"
	"crypto/sha1"
	"os"
	"testing"

	"github.com/anacrolix/torrent/metainfo"
)

func TestHashExtentsMatchesSHA1AndPread(t *testing.T) {
	f, err := os.CreateTemp(t.TempDir(), "extent")
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	data := bytes.Repeat([]byte{9}, 9000)
	if _, err := f.Write(data); err != nil {
		t.Fatal(err)
	}
	span := fileSpan{fd: int32(f.Fd()), offset: 0, length: int64(len(data))}
	got, allHoles, err := hashExtents([]fileSpan{span})
	if err != nil {
		t.Fatal(err)
	}
	if allHoles {
		t.Fatal("dense span reported all holes")
	}
	want := sha1.Sum(data)
	if got != want {
		t.Fatalf("hash=%x want %x", got, want)
	}
	buf := make([]byte, len(data))
	n, err := preadExtents([]fileSpan{span}, buf)
	if err != nil || n != len(data) || !bytes.Equal(buf, data) {
		t.Fatalf("pread n=%d err=%v", n, err)
	}
}

func TestHashExtentsAllHoleMatchesZeroHash(t *testing.T) {
	f, err := os.CreateTemp(t.TempDir(), "hole")
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	const n = 16 * 1024
	if err := f.Truncate(n); err != nil {
		t.Fatal(err)
	}
	span := fileSpan{fd: int32(f.Fd()), offset: 0, length: n}
	got, _, err := hashExtents([]fileSpan{span})
	if err != nil {
		t.Fatal(err)
	}
	want := zeroPieceHash(n)
	if metainfo.Hash(got) != want {
		t.Fatalf("hole hash=%x want %x", got, want)
	}
}

func TestRangeCompleteUsesDurableBits(t *testing.T) {
	info := &metainfo.Info{Name: "one", Length: 32 * 1024, PieceLength: 16 * 1024, Pieces: make([]byte, 40)}
	st := newDocumentTorrentStorage(info, metainfo.Hash{}, t.TempDir())
	t.Cleanup(func() { _ = st.Close() })
	f, err := os.CreateTemp(t.TempDir(), "dest")
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	fd := int(f.Fd())
	if err := st.AttachDescriptors([]*int{&fd}, []int{0}); err != nil {
		t.Fatal(err)
	}
	if st.RangeComplete(0, 16*1024) {
		t.Fatal("empty storage must not report a complete range")
	}
	st.mu.Lock()
	st.completed[0] = true
	st.mu.Unlock()
	if !st.RangeComplete(0, 16*1024) {
		t.Fatal("piece 0 should cover the first range")
	}
	if st.RangeComplete(0, 32*1024) {
		t.Fatal("piece 1 is not complete")
	}
}
