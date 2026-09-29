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

var _ [C.sizeof_WebtorFileSpan - unsafe.Sizeof(fileSpan{})]byte
var _ [unsafe.Sizeof(fileSpan{}) - C.sizeof_WebtorFileSpan]byte

func hashExtents(spans []fileSpan) ([20]byte, bool, error) {
	var out [20]byte
	n := int(C.webtor_hash_extents(
		(*C.WebtorFileSpan)(unsafe.Pointer(unsafe.SliceData(spans))),
		C.size_t(len(spans)),
		(*C.uint8_t)(unsafe.Pointer(&out[0])),
	))
	if n < 0 {
		return out, false, fmt.Errorf("extent hash failed")
	}
	return out, n == 1, nil
}

func preadExtents(spans []fileSpan, buf []byte) (int, error) {
	n := int(C.webtor_pread_extents(
		(*C.WebtorFileSpan)(unsafe.Pointer(unsafe.SliceData(spans))),
		C.size_t(len(spans)),
		(*C.uint8_t)(unsafe.Pointer(unsafe.SliceData(buf))),
		C.size_t(len(buf)),
	))
	if n < 0 {
		return 0, fmt.Errorf("extent pread failed")
	}
	return n, nil
}
