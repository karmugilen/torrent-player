package main

import (
	"encoding/binary"
	"testing"
	"time"

	"github.com/anacrolix/torrent/metainfo"
)

func TestMetadataMissesExpireAndStayBounded(t *testing.T) {
	var cache metadataMissCache
	var hash metainfo.Hash
	now := time.Now()
	cache.put(hash, now)
	if !cache.has(hash, now.Add(time.Second)) || cache.has(hash, now.Add(metadataMissTTL)) {
		t.Fatal("confirmed miss did not expire")
	}
	for i := 0; i < metadataMissLimit+100; i++ {
		binary.BigEndian.PutUint64(hash[12:], uint64(i))
		cache.put(hash, now)
	}
	if len(cache.entries) > metadataMissLimit {
		t.Fatal("negative cache grew beyond its memory bound")
	}
	if !cache.has(hash, now.Add(time.Second)) {
		t.Fatal("newest confirmed miss was evicted")
	}
	if cache.has(metainfo.Hash{}, now.Add(time.Second)) {
		t.Fatal("oldest miss was not evicted")
	}
}
