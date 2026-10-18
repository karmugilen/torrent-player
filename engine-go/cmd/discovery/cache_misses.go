package main

import (
	"sync"
	"time"

	"github.com/anacrolix/torrent/metainfo"
)

const metadataMissTTL = 2 * time.Minute
const metadataMissLimit = 8192

type metadataMiss struct {
	hash  metainfo.Hash
	until int64
}

// Confirmed HTTP 404/410 results only. A bounded FIFO avoids repeated requests
// on hot retries, while expiry lets newly cached metadata become discoverable.
// Challenges, timeouts and source outages must never be cached as hash misses.
type metadataMissCache struct {
	mu      sync.Mutex
	entries map[metainfo.Hash]int64
	ring    []metadataMiss
	next    int
}

func (c *metadataMissCache) has(hash metainfo.Hash, now time.Time) bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	until, ok := c.entries[hash]
	if ok && now.UnixNano() >= until {
		delete(c.entries, hash)
		return false
	}
	return ok
}

func (c *metadataMissCache) put(hash metainfo.Hash, now time.Time) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.entries == nil {
		c.entries = make(map[metainfo.Hash]int64)
	}
	if _, exists := c.entries[hash]; exists {
		return
	}
	entry := metadataMiss{hash, now.Add(metadataMissTTL).UnixNano()}
	if len(c.ring) < metadataMissLimit {
		c.ring = append(c.ring, entry)
	} else {
		old := c.ring[c.next]
		if c.entries[old.hash] == old.until {
			delete(c.entries, old.hash)
		}
		c.ring[c.next] = entry
		c.next = (c.next + 1) % metadataMissLimit
	}
	c.entries[hash] = entry.until
}
