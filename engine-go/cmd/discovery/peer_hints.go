package main

import (
	"net"
	"sync"
	"time"

	"github.com/anacrolix/dht/v2/krpc"
)

const peerHintTTL = 5 * time.Minute
const maxHintSwarms = 2048

type hintedPeer struct {
	ip   [4]byte
	port int
}
type peerHintEntry struct {
	peers    []hintedPeer
	observed time.Time
}

// Short-lived, bounded RAM hints let retries contact recently observed peers
// immediately. Hints are never persisted, exposed, or counted as fresh sightings.
type peerHintCache struct {
	mu      sync.Mutex
	entries map[string]peerHintEntry
}

// Snapshot only warm hashes for dispatch, avoiding a cache mutex acquisition
// for every row in the pending backlog. Endpoint addresses stay in this cache.
func (h *peerHintCache) readyHashes(now time.Time) map[string]bool {
	out := make(map[string]bool)
	if h == nil {
		return out
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	for hash, entry := range h.entries {
		if now.Sub(entry.observed) < peerHintTTL && len(entry.peers) > 0 {
			out[hash] = true
		}
	}
	return out
}

func (h *peerHintCache) get(hash string, now time.Time) []krpc.NodeAddr {
	if h == nil {
		return nil
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	entry, ok := h.entries[hash]
	if !ok {
		return nil
	}
	if now.Sub(entry.observed) >= peerHintTTL {
		delete(h.entries, hash)
		return nil
	}
	peers := make([]krpc.NodeAddr, 0, len(entry.peers))
	for _, p := range entry.peers {
		peers = append(peers, krpc.NodeAddr{IP: net.IPv4(p.ip[0], p.ip[1], p.ip[2], p.ip[3]), Port: p.port})
	}
	return peers
}

func (h *peerHintCache) put(hash string, peers []krpc.NodeAddr, now time.Time) {
	if h == nil {
		return
	}
	valid := make([]hintedPeer, 0, 24)
	seen := make(map[hintedPeer]bool)
	for _, p := range peers {
		if !publicIP(p.IP) || p.Port <= 0 || p.Port > 65535 {
			continue
		}
		x := hintedPeer{port: p.Port}
		copy(x.ip[:], p.IP.To4())
		if !seen[x] {
			seen[x] = true
			valid = append(valid, x)
		}
		if len(valid) == 24 {
			break
		}
	}
	if len(valid) == 0 {
		return
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.entries == nil {
		h.entries = make(map[string]peerHintEntry)
	}
	if len(h.entries) >= maxHintSwarms {
		oldestHash := ""
		var oldest time.Time
		for key, entry := range h.entries {
			if now.Sub(entry.observed) >= peerHintTTL {
				delete(h.entries, key)
				continue
			}
			if oldestHash == "" || entry.observed.Before(oldest) {
				oldestHash, oldest = key, entry.observed
			}
		}
		if len(h.entries) >= maxHintSwarms {
			delete(h.entries, oldestHash)
		}
	}
	old := h.entries[hash]
	if now.Sub(old.observed) < peerHintTTL {
		for _, p := range old.peers {
			if !seen[p] && len(valid) < 24 {
				valid = append(valid, p)
				seen[p] = true
			}
		}
	}
	h.entries[hash] = peerHintEntry{peers: valid, observed: now}
}
