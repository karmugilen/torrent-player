package main

import (
	"math/rand"
	"slices"
	"sort"
	"testing"
)

// The incremental merge (Rust with the rust tag) must give what sorting every
// range and merging them did.
func TestReceivedRangesMergeLikeASortedMerge(t *testing.T) {
	reference := func(ranges []streamByteRange) []streamByteRange {
		ranges = slices.Clone(ranges)
		sort.Slice(ranges, func(i, j int) bool { return ranges[i].start < ranges[j].start })
		var merged []streamByteRange
		for _, r := range ranges {
			if len(merged) != 0 && r.start <= merged[len(merged)-1].end {
				merged[len(merged)-1].end = max(merged[len(merged)-1].end, r.end)
			} else {
				merged = append(merged, r)
			}
		}
		return merged
	}
	rng := rand.New(rand.NewSource(1))
	for round := 0; round < 2000; round++ {
		var got, all []streamByteRange
		for k := 0; k < 1+rng.Intn(40); k++ {
			start := int64(rng.Intn(64)) * 1024
			if rng.Intn(3) == 0 && len(all) != 0 {
				// Blocks usually arrive next to the last one.
				start = got[len(got)-1].end
			}
			r := streamByteRange{start, start + int64(1+rng.Intn(4))*1024}
			all = append(all, r)
			got = mergeReceivedRange(got, r)
			if want := reference(all); !slices.Equal(got, want) {
				t.Fatalf("after %v: got %v, want %v", all, got, want)
			}
		}
	}
}
