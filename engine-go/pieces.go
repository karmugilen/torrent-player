package main

import (
	"fmt"
	"math"
	"sort"

	"github.com/anacrolix/torrent"
)

// Pointer-free C ABI buffers, borrowed only for one synchronous Rust call.
type pieceRun struct{ length, flags uint32 }
type pieceRange struct{ start, end uint32 }
type rawPieceBucket struct {
	start, end, total, selected, verified, receiving, selectedVerified, selectedReceiving uint32
}

// compressPieceRuns copies anacrolix runs into ABI runs using the heat-map
// class only: verified (1) wins over receiving (2). Adjacent equal classes are
// merged so priority-only splits from PieceStateRuns do not survive. The output
// cap stays small so it is not sized to the fat anacrolix run list.
// While a video plays, a partial piece counts as receiving only if playback
// wants it now (Next or above); a part-filled piece left at an earlier seek
// spot is not being fetched.
func compressPieceRuns(states []torrent.PieceStateRun, total int, playing bool) ([]pieceRun, error) {
	if total < 0 {
		return nil, fmt.Errorf("invalid piece run")
	}
	runs := make([]pieceRun, 0, 64)
	var sum uint64
	limit := uint64(math.MaxUint32)
	total64 := uint64(total)
	for _, state := range states {
		if state.Length <= 0 {
			return nil, fmt.Errorf("invalid piece run")
		}
		length := uint64(state.Length)
		next := sum + length
		if next > limit || next > total64 {
			return nil, fmt.Errorf("invalid piece run")
		}
		sum = next
		// Collapse to the class the aggregator counts; Complete never counts as receiving.
		flags := uint32(0)
		if state.Complete {
			flags = 1
		} else if state.Checking || state.Hashing || (state.Partial && (!playing || state.Priority >= torrent.PiecePriorityNext)) {
			flags = 2
		}
		n := uint32(length)
		if last := len(runs) - 1; last >= 0 && runs[last].flags == flags {
			runs[last].length += n
			continue
		}
		runs = append(runs, pieceRun{length: n, flags: flags})
	}
	if sum != total64 {
		return nil, fmt.Errorf("invalid piece run")
	}
	return runs, nil
}

// streamPieceRuns describes only payload currently held in RAM. Torrent
// completion can lag eviction or include startup pieces saved only on disk.
// Retain one immutable compressed snapshot until cache contents change; grid
// refreshes do not repeatedly scan torrent state or copy cached video bytes.
func (st *DocumentTorrentStorage) streamPieceRuns(total int) ([]pieceRun, error) {
	if total < 0 || uint64(total) > math.MaxUint32 {
		return nil, fmt.Errorf("invalid piece count")
	}
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.streamGridRuns != nil && st.streamGridRevision == st.streamRevision && st.streamGridTotal == total {
		return st.streamGridRuns, nil
	}
	indices := make([]int, 0, len(st.cache))
	if !st.closed {
		for index, data := range st.cache {
			if index >= 0 && index < total && len(data) > 0 {
				indices = append(indices, index)
			}
		}
	}
	sort.Ints(indices)
	runs := make([]pieceRun, 0, 2*len(indices)+1)
	appendRun := func(length int, flags uint32) {
		if length == 0 {
			return
		}
		if last := len(runs) - 1; last >= 0 && runs[last].flags == flags {
			runs[last].length += uint32(length)
		} else {
			runs = append(runs, pieceRun{length: uint32(length), flags: flags})
		}
	}
	next := 0
	for _, index := range indices {
		appendRun(index-next, 0)
		flags := uint32(0)
		if !st.streamLost[index] {
			if st.completed[index] {
				flags = 1
			} else if len(st.streamRanges[index]) > 0 && st.streamDemand[index] {
				// Receiving means fetched now. A part-filled piece left at
				// an earlier seek spot is not.
				flags = 2
			}
		}
		appendRun(1, flags)
		next = index + 1
	}
	appendRun(total-next, 0)
	st.streamGridRuns, st.streamGridRevision, st.streamGridTotal = runs, st.streamRevision, total
	return runs, nil
}

func pieceBuckets(t *torrent.Torrent, selected []int, maxBuckets int, streamStorage *DocumentTorrentStorage, playing bool) ([]PieceBucket, error) {
	var runs []pieceRun
	var err error
	if streamStorage != nil {
		runs, err = streamStorage.streamPieceRuns(t.NumPieces())
	} else {
		runs, err = compressPieceRuns(t.PieceStateRuns(), t.NumPieces(), playing)
	}
	if err != nil {
		return nil, err
	}
	files := t.Files()
	ranges := make([]pieceRange, 0, len(selected))
	for _, index := range selected {
		if index < 0 || index >= len(files) || files[index].Length() == 0 {
			continue
		}
		f := files[index]
		ranges = append(ranges, pieceRange{uint32(f.BeginPieceIndex()), uint32(f.EndPieceIndex())})
	}
	return aggregatePieceBuckets(runs, ranges, t.NumPieces(), maxBuckets)
}
