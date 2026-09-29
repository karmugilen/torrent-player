//go:build rust

package main

/*
#cgo CFLAGS: -I${SRCDIR}/../engine-rust
#cgo android,arm64 LDFLAGS: -L${SRCDIR}/../engine-rust/target/aarch64-linux-android/release -lwebtor_pieces -ldl -lm
#cgo linux,amd64 LDFLAGS: -L${SRCDIR}/../engine-rust/target/release -lwebtor_pieces -ldl -lm
#include "pieces.h"
*/
import "C"

import (
	"fmt"
	"unsafe"
)

// Fail compilation if either side of the FFI changes layout.
var _ [C.sizeof_WebtorPieceRun - unsafe.Sizeof(pieceRun{})]byte
var _ [unsafe.Sizeof(pieceRun{}) - C.sizeof_WebtorPieceRun]byte
var _ [C.sizeof_WebtorPieceRange - unsafe.Sizeof(pieceRange{})]byte
var _ [unsafe.Sizeof(pieceRange{}) - C.sizeof_WebtorPieceRange]byte
var _ [C.sizeof_WebtorPieceBucket - unsafe.Sizeof(rawPieceBucket{})]byte
var _ [unsafe.Sizeof(rawPieceBucket{}) - C.sizeof_WebtorPieceBucket]byte

func aggregatePieceBuckets(runs []pieceRun, ranges []pieceRange, total, maxBuckets int) ([]PieceBucket, error) {
	if total < 0 || uint64(total) > uint64(^uint32(0)) || maxBuckets < 1 || maxBuckets > 1024 {
		return nil, fmt.Errorf("invalid piece telemetry bounds")
	}
	output := make([]rawPieceBucket, min(total, maxBuckets))
	n := int(C.webtor_piece_buckets(
		(*C.WebtorPieceRun)(unsafe.Pointer(unsafe.SliceData(runs))), C.size_t(len(runs)),
		(*C.WebtorPieceRange)(unsafe.Pointer(unsafe.SliceData(ranges))), C.size_t(len(ranges)),
		C.uint32_t(total), C.uint32_t(maxBuckets),
		(*C.WebtorPieceBucket)(unsafe.Pointer(unsafe.SliceData(output))), C.size_t(len(output)),
	))
	if n < 0 {
		return nil, fmt.Errorf("invalid compressed piece states")
	}
	buckets := make([]PieceBucket, n)
	for i, b := range output[:n] {
		buckets[i] = PieceBucket{int(b.start), int(b.end), int(b.total), int(b.selected), int(b.verified), int(b.receiving), int(b.selectedVerified), int(b.selectedReceiving)}
	}
	return buckets, nil
}
