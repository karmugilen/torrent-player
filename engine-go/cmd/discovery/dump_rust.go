//go:build rustdump

package main

/*
#cgo CFLAGS: -I${SRCDIR}/../../../engine-rust/discovery
#cgo LDFLAGS: ${SRCDIR}/../../../engine-rust/discovery/target/release/libwebtor_discovery.a -ldl -lm -lpthread
#include "discovery.h"
#include <stdlib.h>

extern int goWebtorSeedCb(char* hash, WebtorDumpRow* row, void* ctx);

static int webtor_seed_trampoline(const char* hash, const WebtorDumpRow* row, void* ctx) {
	return goWebtorSeedCb((char*)hash, (WebtorDumpRow*)row, ctx);
}

static int webtor_call_seed_all(void* ctx) {
	return webtor_dump_for_each_seeded(webtor_seed_trampoline, ctx);
}
*/
import "C"

import (
	"encoding/json"
	"errors"
	"sync/atomic"
	"unsafe"
)

// seedDash holds the dashboard for the C→Go seed callback. Passing a Go
// pointer-to-pointer through cgo is forbidden; a process-wide slot is safe
// because only one rustSeedAll runs at a time.
var seedDash atomic.Pointer[dashboard]

func rustDumpEnabled() bool { return true }

func rustEnsureDump(path string, force bool) error {
	cpath := C.CString(path)
	defer C.free(unsafe.Pointer(cpath))
	forceI := C.int(0)
	if force {
		forceI = 1
	}
	if C.webtor_dump_ensure(cpath, forceI) != 0 {
		return errors.New("rust dump download failed")
	}
	return nil
}

func rustLoadDump(path string) (*dumpIndex, error) {
	cpath := C.CString(path)
	defer C.free(unsafe.Pointer(cpath))
	n := C.webtor_dump_load(cpath)
	if n < 0 {
		return nil, errors.New("rust dump load failed")
	}
	return &dumpIndex{rust: true, rustRows: int(n)}, nil
}

func rustLookup(hash string) (dumpRow, bool) {
	chash := C.CString(hash)
	defer C.free(unsafe.Pointer(chash))
	var row C.WebtorDumpRow
	if C.webtor_dump_lookup(chash, &row) != 1 {
		return dumpRow{}, false
	}
	return dumpRow{
		Name:     C.GoString(&row.name[0]),
		Size:     int64(row.size),
		Seeders:  int(row.seeders),
		Leechers: int(row.leechers),
	}, true
}

func rustDumpLen() int { return int(C.webtor_dump_len()) }

var seedAdded atomic.Int64
var seedSeen atomic.Int64

//export goWebtorSeedCb
func goWebtorSeedCb(hash *C.char, row *C.WebtorDumpRow, ctx unsafe.Pointer) C.int {
	_ = ctx
	d := seedDash.Load()
	if d == nil {
		return 0
	}
	h := C.GoString(hash)
	r := dumpRow{
		Name:     C.GoString(&row.name[0]),
		Size:     int64(row.size),
		Seeders:  int(row.seeders),
		Leechers: int(row.leechers),
	}
	seen := seedSeen.Add(1)
	if d.applyDumpRow(h, r, true) {
		seedAdded.Add(1)
	}
	if seen%int64(dumpSeedChunk) == 0 {
		d.setStatus("Local dump seeding (rust)… " + itoa(int(seedAdded.Load())) + " new names")
	}
	return 1
}

func rustSeedAll(d *dashboard) int {
	seedDash.Store(d)
	seedAdded.Store(0)
	seedSeen.Store(0)
	defer seedDash.Store(nil)
	// nil ctx: no Go pointers cross the cgo boundary.
	C.webtor_call_seed_all(nil)
	return int(seedAdded.Load())
}

func rustFetchCatalogPage(query string, size, after int) (*catalogPage, error) {
	cq := C.CString(query)
	defer C.free(unsafe.Pointer(cq))
	var jsonPtr *C.char
	var next C.int = -1
	n := C.webtor_catalog_fetch(cq, C.int(size), C.int(after), &jsonPtr, &next)
	if n < 0 {
		return nil, errors.New("rust catalog fetch failed")
	}
	defer C.webtor_string_free(jsonPtr)
	raw := C.GoString(jsonPtr)
	var torrents []catalogTorrent
	if err := json.Unmarshal([]byte(raw), &torrents); err != nil {
		return nil, err
	}
	page := &catalogPage{Torrents: torrents}
	if next >= 0 {
		v := int(next)
		page.Next = &v
	}
	return page, nil
}

func itoa(n int) string {
	if n == 0 {
		return "0"
	}
	var buf [20]byte
	i := len(buf)
	for n > 0 {
		i--
		buf[i] = byte('0' + n%10)
		n /= 10
	}
	return string(buf[i:])
}
