package main

import (
	"crypto/sha1"
	"fmt"
	"io"

	"github.com/anacrolix/torrent/metainfo"
	"github.com/anacrolix/torrent/storage"
)

// restoreVerifyPiece is only returned while a saved-data check is running.
// SelfHash lets restore skip hashing filesystem holes; peer hashing keeps the
// plain documentPiece type so smart banning still works.
type restoreVerifyPiece struct {
	*documentPiece
}

var _ storage.SelfHashing = (*restoreVerifyPiece)(nil)

func (p *restoreVerifyPiece) SelfHash() (metainfo.Hash, error) {
	length := p.piece.Length()
	// Write-back may hold the only copy in RAM; never treat that as a hole.
	p.storage.mu.Lock()
	index := p.piece.Index()
	inRAM := false
	if _, ok := p.storage.cache[index]; ok {
		inRAM = true
	}
	p.storage.mu.Unlock()
	if inRAM {
		return hashPieceContents(p.documentPiece)
	}
	allHoles, supported := p.storage.pieceExtentAllHoles(p.piece)
	if supported && allHoles {
		return zeroPieceHash(length), nil
	}
	return hashPieceContents(p.documentPiece)
}

func hashPieceContents(p *documentPiece) (metainfo.Hash, error) {
	length := p.piece.Length()
	h := sha1.New()
	buf := make([]byte, 64*1024)
	var off int64
	for off < length {
		n := int64(len(buf))
		if off+n > length {
			n = length - off
		}
		got, err := p.ReadAt(buf[:n], off)
		if got > 0 {
			_, _ = h.Write(buf[:got])
			off += int64(got)
		}
		if err == io.EOF {
			break
		}
		if err != nil {
			return metainfo.Hash{}, err
		}
		if got == 0 {
			return metainfo.Hash{}, fmt.Errorf("short piece read at offset %d", off)
		}
	}
	if off != length {
		return metainfo.Hash{}, io.ErrUnexpectedEOF
	}
	var out metainfo.Hash
	copy(out[:], h.Sum(nil))
	return out, nil
}

// pieceExtentAllHoles reports whether every byte of the piece is a confirmed
// sparse hole. supported=false means the caller must hash normally.
func (st *DocumentTorrentStorage) pieceExtentAllHoles(piece metainfo.Piece) (allHoles bool, supported bool) {
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.closed || !st.attached || st.chunkLength <= 0 {
		return false, false
	}
	// Dirty/clean RAM cache means the piece is not a filesystem hole.
	if _, ok := st.cache[piece.Index()]; ok {
		return false, true
	}
	start := int64(piece.Index()) * st.chunkLength
	end := start + piece.Length()
	if end <= start {
		return false, false
	}
	sawSpan := false
	for _, f := range st.files {
		from, to := maxInt64(start, f.Offset), minInt64(end, f.Offset+f.Length)
		if from >= to {
			continue
		}
		if f.File == nil {
			return false, false
		}
		sawSpan = true
		fileStart, fileEnd := from-f.Offset, to-f.Offset
		pos, ok, err := seekData(f.File, fileStart)
		if !ok {
			return false, false
		}
		if err == io.EOF {
			continue
		}
		if err != nil {
			return false, false
		}
		if pos < fileEnd {
			// Real data overlaps this piece extent.
			return false, true
		}
	}
	if !sawSpan {
		return false, false
	}
	return true, true
}
