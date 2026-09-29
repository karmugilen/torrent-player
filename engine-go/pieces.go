package main

import (
	"fmt"
	"math"

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
func compressPieceRuns(states []torrent.PieceStateRun, total int) ([]pieceRun, error) {
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
		} else if state.Partial || state.Checking || state.Hashing {
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

func pieceBuckets(t *torrent.Torrent, selected []int, maxBuckets int) ([]PieceBucket, error) {
	runs, err := compressPieceRuns(t.PieceStateRuns(), t.NumPieces())
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
