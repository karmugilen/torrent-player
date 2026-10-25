//go:build rust

package main

/*
#cgo CFLAGS: -I${SRCDIR}/../engine-rust
#include "pieces.h"
*/
import "C"

import "unsafe"

var _ [C.sizeof_WebtorByteRange - unsafe.Sizeof(streamByteRange{})]byte
var _ [unsafe.Sizeof(streamByteRange{}) - C.sizeof_WebtorByteRange]byte
var _ [C.sizeof_WebtorByteRange - unsafe.Offsetof(streamByteRange{}.end) - unsafe.Sizeof(int64(0))]byte

func mergeReceivedRangeOutOfOrder(ranges []streamByteRange, incoming streamByteRange) []streamByteRange {
	// Reserve the extra entry in Go. Rust only rearranges caller-owned bytes
	// during this call; it never stores a Go pointer or allocates a buffer.
	ranges = append(ranges, incoming)
	n := int(C.webtor_merge_received_range(
		(*C.WebtorByteRange)(unsafe.Pointer(unsafe.SliceData(ranges))), C.size_t(len(ranges)),
	))
	if n < 0 || n > len(ranges) {
		panic("invalid received byte range")
	}
	return ranges[:n]
}
