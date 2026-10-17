//go:build rustdump

package main

/*
#cgo CFLAGS: -I${SRCDIR}/../../../engine-rust/discovery
#include "discovery.h"
#include "search.h"
#include <stdlib.h>
*/
import "C"

import (
	"encoding/hex"
	"runtime"
	"sync"
	"unsafe"
)

type rustSearchIndex struct {
	mu     sync.Mutex
	handle unsafe.Pointer
	batch  []byte
}

func newNativeSearch() nativeSearchIndex {
	index := &rustSearchIndex{handle: C.webtor_search_new()}
	if index.handle == nil {
		return nil
	}
	runtime.SetFinalizer(index, func(index *rustSearchIndex) { C.webtor_search_free(index.handle) })
	return index
}

func (index *rustSearchIndex) flushLocked() {
	if len(index.batch) == 0 {
		return
	}
	C.webtor_search_add(index.handle, (*C.uint8_t)(unsafe.Pointer(&index.batch[0])), C.uintptr_t(len(index.batch)))
	index.batch = index.batch[:0]
	runtime.KeepAlive(index)
}

func (index *rustSearchIndex) add(hash, name string) {
	index.mu.Lock()
	defer index.mu.Unlock()
	index.batch = append(index.batch, hash...)
	index.batch = append(index.batch, '\t')
	index.batch = append(index.batch, normalizeSearchText(name)...)
	index.batch = append(index.batch, '\n')
	if len(index.batch) >= 128*1024 {
		index.flushLocked()
	}
}

func (index *rustSearchIndex) candidates(query string) ([]string, bool) {
	index.mu.Lock()
	defer index.mu.Unlock()
	index.flushLocked()
	text := C.CString(normalizeSearchText(query))
	defer C.free(unsafe.Pointer(text))
	packed := make([]byte, 2048*20)
	count := int(C.webtor_search_query(index.handle, text, (*C.uint8_t)(unsafe.Pointer(&packed[0])), 2048))
	if count < 0 {
		return nil, false
	}
	if count > 2048 {
		packed = make([]byte, count*20)
		count = int(C.webtor_search_query(index.handle, text, (*C.uint8_t)(unsafe.Pointer(&packed[0])), C.uintptr_t(count)))
	}
	runtime.KeepAlive(index)
	if count < 0 {
		return nil, false
	}
	hashes := make([]string, 0, count)
	for i := 0; i < count; i++ {
		hashes = append(hashes, hex.EncodeToString(packed[i*20:(i+1)*20]))
	}
	return hashes, true
}
