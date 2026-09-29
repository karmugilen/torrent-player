//go:build !rust && !android

package main

import (
	"fmt"
	"sort"
)

// Host fallback used when the Rust piece aggregator is not linked.
// Android builds always set the rust tag and do not compile this file.
// Selection ranges and the bucket slice are the only scratch; runs stay compressed.
func aggregatePieceBuckets(runs []pieceRun, ranges []pieceRange, total, maxBuckets int) ([]PieceBucket, error) {
	if total < 0 || uint64(total) > uint64(^uint32(0)) || maxBuckets < 1 || maxBuckets > 1024 {
		return nil, fmt.Errorf("invalid piece telemetry bounds")
	}
	var covered uint64
	for _, run := range runs {
		if run.length == 0 {
			return nil, fmt.Errorf("invalid compressed piece states")
		}
		covered += uint64(run.length)
	}
	if covered != uint64(total) {
		return nil, fmt.Errorf("invalid compressed piece states")
	}
	for _, r := range ranges {
		if r.start > r.end || uint64(r.end) > uint64(total) {
			return nil, fmt.Errorf("invalid compressed piece states")
		}
	}
	if total == 0 {
		return []PieceBucket{}, nil
	}

	merged := append([]pieceRange(nil), ranges...)
	sort.Slice(merged, func(i, j int) bool {
		if merged[i].start != merged[j].start {
			return merged[i].start < merged[j].start
		}
		return merged[i].end < merged[j].end
	})
	n := 0
	for _, r := range merged {
		if r.start == r.end {
			continue
		}
		if n > 0 && r.start <= merged[n-1].end {
			if r.end > merged[n-1].end {
				merged[n-1].end = r.end
			}
			continue
		}
		merged[n] = r
		n++
	}
	merged = merged[:n]

	size := total / maxBuckets
	if total%maxBuckets != 0 {
		size++
	}
	count := total / size
	if total%size != 0 {
		count++
	}
	buckets := make([]PieceBucket, count)
	for i := range buckets {
		start := i * size
		end := start + size
		if end > total {
			end = total
		}
		buckets[i] = PieceBucket{Start: start, End: end - 1, Total: end - start}
	}

	// Coalesce equal heat classes, then split only on bucket and selection edges.
	// Verified (bit 1) wins over receiving (bit 2). Merged ranges count once.
	cursor := 0
	bucketIndex := 0
	bucketLimit := min(size, total)
	selectedIndex := 0
	for i := 0; i < len(runs); {
		flags := runs[i].flags
		verified := flags&1 != 0
		receiving := !verified && flags&2 != 0
		runEnd := cursor + int(runs[i].length)
		i++
		for i < len(runs) {
			next := runs[i].flags
			nextVerified := next&1 != 0
			nextReceiving := !nextVerified && next&2 != 0
			if nextVerified != verified || nextReceiving != receiving {
				break
			}
			runEnd += int(runs[i].length)
			i++
		}
		for cursor < runEnd {
			if cursor == bucketLimit {
				bucketIndex++
				next := cursor + size
				if next > total {
					next = total
				}
				bucketLimit = next
			}
			for selectedIndex < len(merged) && int(merged[selectedIndex].end) <= cursor {
				selectedIndex++
			}
			selected := false
			selectionEnd := total
			if selectedIndex < len(merged) {
				r := merged[selectedIndex]
				if int(r.start) <= cursor {
					selected = true
					selectionEnd = int(r.end)
				} else {
					selectionEnd = int(r.start)
				}
			}
			end := min(runEnd, bucketLimit, selectionEnd)
			span := end - cursor
			b := &buckets[bucketIndex]
			if selected {
				b.Selected += span
			}
			if verified {
				b.Verified += span
				if selected {
					b.SelectedVerified += span
				}
			} else if receiving {
				b.Receiving += span
				if selected {
					b.SelectedReceiving += span
				}
			}
			cursor = end
		}
	}
	return buckets, nil
}
