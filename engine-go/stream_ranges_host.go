//go:build !rust

package main

import "sort"

// Keep this equivalent to the optional Rust core. Only the
// new interval is inserted; the prefix is already sorted and normalized.
func mergeReceivedRangeOutOfOrder(ranges []streamByteRange, incoming streamByteRange) []streamByteRange {
	existing := len(ranges)
	begin := sort.Search(existing, func(i int) bool { return ranges[i].end >= incoming.start })
	end := begin
	for end < existing && ranges[end].start <= incoming.end {
		incoming.start = min(incoming.start, ranges[end].start)
		incoming.end = max(incoming.end, ranges[end].end)
		end++
	}
	ranges = append(ranges, streamByteRange{})
	copy(ranges[begin+1:], ranges[end:existing])
	ranges[begin] = incoming
	return ranges[:existing-(end-begin)+1]
}
