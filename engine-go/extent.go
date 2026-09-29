package main

import (
	"fmt"
	"io"

	"github.com/anacrolix/torrent/metainfo"
)

// fileSpan is one contiguous region of a piece (or read) on a single fd.
// Layout matches WebtorFileSpan in engine-rust/pieces.h.
type fileSpan struct {
	fd     int32
	_pad   int32
	offset int64
	length int64
}

func (st *DocumentTorrentStorage) pieceSpans(piece metainfo.Piece) ([]fileSpan, bool) {
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.closed || !st.attached || st.chunkLength <= 0 {
		return nil, false
	}
	start := int64(piece.Index()) * st.chunkLength
	return st.byteSpansLocked(start, start+piece.Length())
}

func (st *DocumentTorrentStorage) byteSpans(absStart, length int64) ([]fileSpan, bool) {
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.closed || !st.attached || length <= 0 {
		return nil, false
	}
	return st.byteSpansLocked(absStart, absStart+length)
}

func (st *DocumentTorrentStorage) byteSpansLocked(start, end int64) ([]fileSpan, bool) {
	var spans []fileSpan
	for _, f := range st.files {
		from, to := maxInt64(start, f.Offset), minInt64(end, f.Offset+f.Length)
		if from >= to {
			continue
		}
		if f.File == nil {
			return nil, false
		}
		spans = append(spans, fileSpan{
			fd:     int32(f.File.Fd()),
			offset: from - f.Offset,
			length: to - from,
		})
	}
	if len(spans) == 0 {
		return nil, false
	}
	return spans, true
}

// RangeComplete reports whether every piece covering [absStart, absStart+length)
// already has a durable complete bit.
func (st *DocumentTorrentStorage) RangeComplete(absStart, length int64) bool {
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.closed || !st.attached || st.chunkLength <= 0 || length <= 0 || st.info == nil {
		return false
	}
	begin := int(absStart / st.chunkLength)
	last := int((absStart + length - 1) / st.chunkLength)
	n := st.info.NumPieces()
	if begin < 0 || last >= n {
		return false
	}
	for i := begin; i <= last; i++ {
		if !st.completed[i] {
			return false
		}
	}
	return true
}

func (st *DocumentTorrentStorage) AllSelectedComplete(selected []int) bool {
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.closed || !st.attached || st.info == nil || st.chunkLength <= 0 {
		return false
	}
	for _, index := range selected {
		if index < 0 || index >= len(st.files) {
			return false
		}
		f := st.files[index]
		if f.Length <= 0 {
			continue
		}
		begin := int(f.Offset / st.chunkLength)
		last := int((f.Offset + f.Length - 1) / st.chunkLength)
		n := st.info.NumPieces()
		if last >= n {
			last = n - 1
		}
		for p := begin; p <= last; p++ {
			if !st.completed[p] {
				return false
			}
		}
	}
	return true
}

func hashFileSpans(spans []fileSpan) (sum [20]byte, allHoles bool, err error) {
	if len(spans) == 0 {
		return sum, true, fmt.Errorf("no file spans")
	}
	return hashExtents(spans)
}

func preadFileSpans(spans []fileSpan, buf []byte) (int, error) {
	if len(spans) == 0 {
		return 0, io.EOF
	}
	return preadExtents(spans, buf)
}
