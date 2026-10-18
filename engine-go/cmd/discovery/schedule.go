package main

import (
	"time"

	"github.com/anacrolix/dht/v2/krpc"
)

// Cache workers only hit HTTP .torrent mirrors. Peer workers finish BEP 9 for
// hashes the mirrors miss. Keep both pools small so naming leans on
// torrents-csv and does not flood the uplink with peer handshakes.
const cacheWorkers = 48
const peerWorkers = 8
const resolverWorkers = cacheWorkers + peerWorkers

type jobKind int

const (
	// Zero-value jobs keep the full peer path so unit tests remain valid.
	jobPeer jobKind = iota
	jobCache
)

// Keep source hints separate from persistent observations. Peer/node addresses
// never enter the webpage or its JSON cache.
func (s *sampler) rememberSource(hash string, node krpc.NodeInfo) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.sources == nil {
		s.sources = make(map[string][]krpc.NodeInfo)
	}
	if len(s.sources) >= sourceHintLimit {
		// Hints are opportunistic; full DHT traversal works without them.
		for key := range s.sources {
			delete(s.sources, key)
			if len(s.sources) < sourceHintLimit*3/4 {
				break
			}
		}
	}
	for _, existing := range s.sources[hash] {
		if existing.Addr.String() == node.Addr.String() {
			return
		}
	}
	s.sources[hash] = append(s.sources[hash], node)
	if len(s.sources[hash]) > 3 {
		s.sources[hash] = s.sources[hash][1:]
	}
}

func dueAt(r observation) time.Time {
	if !r.NextAttempt.IsZero() {
		return r.NextAttempt
	}
	if !r.CheckedAt.IsZero() {
		return r.CheckedAt.Add(30 * time.Second)
	}
	return r.FirstSeen
}

// resolvePriority ranks unresolved hashes for naming. Higher scores go first:
// seed/peer signal, repeated DHT samples, fewer failed attempts, and overdue retries.
func resolvePriority(r observation, now time.Time) int64 {
	score := int64(0)
	if r.Seeds != nil && *r.Seeds > 0 {
		score += int64(min(*r.Seeds, 100)) * 100
	}
	if r.Peers > 0 {
		score += int64(min(r.Peers, 64)) * 40
		if !r.CheckedAt.IsZero() && now.Sub(r.CheckedAt) < peerHintTTL {
			score += 10_000
		}
	}
	hits := r.SampleHits
	if hits > 20 {
		hits = 20
	}
	score += int64(hits) * 25
	score -= int64(r.ResolveAttempts) * 20
	// Overdue bonus only for real retries. Fresh rows use FirstSeen as dueAt, and
	// aging that would wrongly beat peer-rich newer samples.
	if r.ResolveAttempts > 0 || !r.CheckedAt.IsZero() {
		if overdue := now.Sub(dueAt(r)); overdue > 0 {
			secs := int(overdue / time.Second)
			if secs > 60 {
				secs = 60
			}
			score += int64(secs)
		}
	}
	return score
}

func betterResolveCandidate(a, b observation, now time.Time) bool {
	pa, pb := resolvePriority(a, now), resolvePriority(b, now)
	if pa != pb {
		return pa > pb
	}
	da, db := dueAt(a), dueAt(b)
	if !da.Equal(db) {
		return da.Before(db)
	}
	return a.Hash < b.Hash
}

func (s *sampler) nextJob(now time.Time) (discoveryJob, bool) {
	return s.nextJobOf(now, jobPeer)
}

func (s *sampler) nextJobOf(now time.Time, kind jobKind) (discoveryJob, bool) {
	s.d.mu.Lock()
	defer s.d.mu.Unlock()
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.cacheTried == nil {
		s.cacheTried = map[string]bool{}
	}
	batch := &s.dispatch[kind]
	if !now.Before(batch.refreshAt) {
		s.refillDispatchLocked(now, kind)
	}
	fresh, hasFresh := s.readyCandidateLocked(&batch.fresh, now, kind)
	retry, hasRetry := s.readyCandidateLocked(&batch.retry, now, kind)
	chosen := fresh
	queue := &batch.fresh
	// Reserve one out of four dispatches for retries so a steady flow of new
	// samples cannot starve previously observed torrents with available peers.
	if hasRetry && (!hasFresh || batch.turn%4 == 0) {
		chosen = retry
		queue = &batch.retry
	}
	if !hasFresh && !hasRetry {
		return discoveryJob{}, false
	}
	*queue = (*queue)[:len(*queue)-1]
	batch.turn++
	if s.inflight == nil {
		s.inflight = make(map[string]bool)
	}
	s.inflight[chosen.Hash] = true
	// Reserve the retry deadline before network work. Even a malformed/private
	// response or a client setup failure must not cause a hot retry loop.
	// Cache miss -> peer is one attempt through two sources, not two retry
	// failures. Double counting used to skip the initial 30-second retry.
	if kind != jobPeer || !s.cacheTried[chosen.Hash] || chosen.ResolveAttempts == 0 {
		chosen.ResolveAttempts++
	}
	chosen.NextAttempt = now.Add(retryDelay(chosen.ResolveAttempts))
	s.d.records[chosen.Hash] = chosen
	s.d.state.Active++
	if kind == jobPeer {
		s.d.state.Pending--
	}
	return discoveryJob{hash: chosen.Hash, sources: append([]krpc.NodeInfo(nil), s.sources[chosen.Hash]...), kind: kind}, true
}

func (s *sampler) releaseJob(hash string) {
	s.d.mu.Lock()
	defer s.d.mu.Unlock()
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.inflight[hash] {
		delete(s.inflight, hash)
		s.d.state.Active--
		if rec, ok := s.d.records[hash]; ok && rec.Resolved {
			delete(s.cacheTried, hash)
			return
		}
		// A completed cache miss can go straight to the peer batch, including
		// when that batch was empty. No backlog rescan is needed for handoff.
		if rec, ok := s.d.records[hash]; ok && s.cacheTried[hash] && s.eligibleLocked(rec, time.Now(), jobPeer) {
			s.dispatch[jobPeer].offer(rec, time.Now())
			s.d.state.Pending++
		}
	}
}

func retryDelay(attempt int) time.Duration {
	return min(15*time.Minute, (30*time.Second)*time.Duration(1<<min(max(attempt-1, 0), 5)))
}
