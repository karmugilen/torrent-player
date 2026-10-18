package main

import (
	"time"

	"github.com/anacrolix/dht/v2/krpc"
	"github.com/anacrolix/torrent/metainfo"
)

type announcedPeer struct {
	hash string
	peer krpc.NodeAddr
}

// Reuse the same bounded, expiring RAM peer hints when serving normal DHT
// get_peers requests. The DHT server validates announcement tokens itself.
type announcementPeers struct{ hints *peerHintCache }

func (p announcementPeers) AddPeer(hash metainfo.Hash, peer krpc.NodeAddr) {
	p.hints.put(hash.HexString(), []krpc.NodeAddr{peer}, time.Now())
}
func (p announcementPeers) GetPeers(hash metainfo.Hash) []krpc.NodeAddr {
	return p.hints.get(hash.HexString(), time.Now())
}

func (s *sampler) acceptAnnouncement(a announcedPeer, now time.Time) {
	a.hash = normalizeInfoHash(a.hash)
	if a.hash == "" || !publicIP(a.peer.IP) || a.peer.Port <= 0 || a.peer.Port > 65535 {
		return
	}
	s.hints.put(a.hash, []krpc.NodeAddr{a.peer}, now)
	s.d.mu.Lock()
	s.d.state.Announcements++
	if !s.d.upsertSampleLocked(a.hash, now) {
		s.d.mu.Unlock()
		return
	}
	rec := s.d.records[a.hash]
	rec.PeerSeenAt = now
	rec.Peers = max(rec.Peers, 1)
	rec.NextAttempt = now
	s.d.records[a.hash] = rec
	s.d.mu.Unlock()
	// A fresh endpoint should go straight to peer workers, even during mirror
	// cooldown. The in-flight guard still allows only one job for this hash.
	s.mu.Lock()
	if s.cacheTried == nil {
		s.cacheTried = map[string]bool{}
	}
	s.cacheTried[a.hash] = true
	s.mu.Unlock()
	if s.d.tryResolveHashFromDump(a.hash) {
		s.clearCacheTried(a.hash)
	}
}
