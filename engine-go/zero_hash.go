package main

import (
	"crypto/sha1"
	"sync"

	"github.com/anacrolix/torrent/metainfo"
)

var (
	zeroHashMu    sync.Mutex
	zeroHashCache = map[int64]metainfo.Hash{}
)

// SHA-1 of length zero bytes. Computed once per distinct piece length.
func zeroPieceHash(length int64) metainfo.Hash {
	if length < 0 {
		panic("negative zero hash length")
	}
	zeroHashMu.Lock()
	defer zeroHashMu.Unlock()
	if hash, ok := zeroHashCache[length]; ok {
		return hash
	}
	h := sha1.New()
	var zeros [32 * 1024]byte
	remaining := length
	for remaining > 0 {
		chunk := int64(len(zeros))
		if chunk > remaining {
			chunk = remaining
		}
		_, _ = h.Write(zeros[:chunk])
		remaining -= chunk
	}
	var out metainfo.Hash
	copy(out[:], h.Sum(nil))
	zeroHashCache[length] = out
	return out
}
