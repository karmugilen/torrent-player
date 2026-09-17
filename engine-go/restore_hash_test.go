package main

import (
	"bytes"
	"crypto/sha1"
	"os"
	"testing"

	"github.com/anacrolix/torrent/metainfo"
	"github.com/anacrolix/torrent/storage"
)

func TestZeroPieceHashMatchesSHA1(t *testing.T) {
	for _, length := range []int64{0, 1, 20, 16 * 1024, 256*1024 + 7} {
		got := zeroPieceHash(length)
		h := sha1.New()
		_, _ = h.Write(bytes.Repeat([]byte{0}, int(length)))
		var want metainfo.Hash
		copy(want[:], h.Sum(nil))
		if got != want {
			t.Fatalf("length %d: got %x want %x", length, got, want)
		}
		if zeroPieceHash(length) != got {
			t.Fatalf("length %d: cache returned a different hash", length)
		}
	}
}

func TestSparsePieceExtentAllHoles(t *testing.T) {
	info := &metainfo.Info{Name: "sparse.bin", Length: 32 * 1024, PieceLength: 16 * 1024, Pieces: make([]byte, 40)}
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

	all, supported := st.pieceExtentAllHoles(info.Piece(0))
	if !supported {
		t.Skip("filesystem does not expose sparse holes")
	}
	if !all {
		t.Fatal("truncated file should report all-hole pieces")
	}

	if _, err := f.WriteAt([]byte("not-a-hole"), 0); err != nil {
		t.Fatal(err)
	}
	all, supported = st.pieceExtentAllHoles(info.Piece(0))
	if !supported {
		t.Fatal("sparse probing became unsupported after write")
	}
	if all {
		t.Fatal("piece with written data was treated as all holes")
	}
	// Some filesystems (notably tmpfs) allocate the whole truncated file on the
	// first write, so later pieces may no longer be holes. That must fall back
	// to normal hashing rather than a false all-hole result.
	all, supported = st.pieceExtentAllHoles(info.Piece(1))
	if !supported {
		t.Fatal("sparse probing became unsupported for later piece")
	}
	if all {
		sum, err := (&restoreVerifyPiece{documentPiece: &documentPiece{storage: st, piece: info.Piece(1)}}).SelfHash()
		if err != nil {
			t.Fatal(err)
		}
		if sum != zeroPieceHash(info.PieceLength) {
			t.Fatalf("all-hole piece hash=%x want %x", sum, zeroPieceHash(info.PieceLength))
		}
	}
}

func TestRestoreVerifySelfHashUsesZeroHashForHoles(t *testing.T) {
	info := &metainfo.Info{Name: "sparse.bin", Length: 16 * 1024, PieceLength: 16 * 1024, Pieces: make([]byte, 20)}
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
	st.SetRestoreVerify(true)
	piece := &restoreVerifyPiece{documentPiece: &documentPiece{storage: st, piece: info.Piece(0)}}
	sum, err := piece.SelfHash()
	if err != nil {
		t.Fatal(err)
	}
	all, supported := st.pieceExtentAllHoles(info.Piece(0))
	if supported && all && sum != zeroPieceHash(info.PieceLength) {
		t.Fatalf("all-hole SelfHash=%x want zero hash %x", sum, zeroPieceHash(info.PieceLength))
	}
	if !supported {
		// Fallback hashed the zeros the hard way; result must still match.
		if sum != zeroPieceHash(info.PieceLength) {
			t.Fatalf("fallback SelfHash=%x want %x", sum, zeroPieceHash(info.PieceLength))
		}
	}
}

func TestRestoreVerifyPieceTypeOnlyWhileEnabled(t *testing.T) {
	info := &metainfo.Info{Name: "one", Length: 4, PieceLength: 4, Pieces: make([]byte, 20)}
	client := NewDocumentStorageClient(t.TempDir())
	hash := metainfo.Hash{1, 2, 3}
	impl, err := client.OpenTorrent(t.Context(), info, hash)
	if err != nil {
		t.Fatal(err)
	}
	defer impl.Close()
	st := client.GetStorage(hash.HexString())
	plain := impl.Piece(info.Piece(0))
	if _, ok := plain.(storage.SelfHashing); ok {
		t.Fatal("plain piece unexpectedly implements SelfHashing")
	}
	st.SetRestoreVerify(true)
	restoring := impl.Piece(info.Piece(0))
	if _, ok := restoring.(storage.SelfHashing); !ok {
		t.Fatal("restore piece should implement SelfHashing")
	}
	st.SetRestoreVerify(false)
	again := impl.Piece(info.Piece(0))
	if _, ok := again.(storage.SelfHashing); ok {
		t.Fatal("SelfHashing remained after restore verify ended")
	}
}

func BenchmarkSparseVersusDenseSelfHash(b *testing.B) {
	const pieceLen = 1024 * 1024
	info := &metainfo.Info{Name: "bench.bin", Length: pieceLen, PieceLength: pieceLen, Pieces: make([]byte, 20)}

	makeStorage := func(b *testing.B, fill bool) *restoreVerifyPiece {
		b.Helper()
		st := newDocumentTorrentStorage(info, metainfo.Hash{}, b.TempDir())
		b.Cleanup(func() { _ = st.Close() })
		f, err := os.CreateTemp(b.TempDir(), "bench")
		if err != nil {
			b.Fatal(err)
		}
		b.Cleanup(func() { _ = f.Close() })
		if fill {
			buf := bytes.Repeat([]byte{1}, 64*1024)
			var written int64
			for written < pieceLen {
				n, err := f.Write(buf)
				written += int64(n)
				if err != nil {
					b.Fatal(err)
				}
			}
		}
		fd := int(f.Fd())
		if err := st.AttachDescriptors([]*int{&fd}, []int{0}); err != nil {
			b.Fatal(err)
		}
		st.SetRestoreVerify(true)
		return &restoreVerifyPiece{documentPiece: &documentPiece{storage: st, piece: info.Piece(0)}}
	}

	b.Run("sparse", func(b *testing.B) {
		piece := makeStorage(b, false)
		all, supported := piece.storage.pieceExtentAllHoles(info.Piece(0))
		if !supported || !all {
			b.Skip("filesystem does not expose sparse holes")
		}
		b.SetBytes(pieceLen)
		b.ResetTimer()
		for i := 0; i < b.N; i++ {
			if _, err := piece.SelfHash(); err != nil {
				b.Fatal(err)
			}
		}
	})
	b.Run("dense", func(b *testing.B) {
		piece := makeStorage(b, true)
		b.SetBytes(pieceLen)
		b.ResetTimer()
		for i := 0; i < b.N; i++ {
			if _, err := piece.SelfHash(); err != nil {
				b.Fatal(err)
			}
		}
	})
}
