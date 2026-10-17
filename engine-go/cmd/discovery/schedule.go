package main

import (
	"time"

	"github.com/anacrolix/dht/v2/krpc"
)

const resolverWorkers = 12

// Keep source hints separate from persistent observations. Peer/node addresses
// never enter the webpage or its JSON cache.
func (s *sampler) rememberSource(hash string, node krpc.NodeInfo) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.sources == nil {
		s.sources = make(map[string][]krpc.NodeInfo)
	}
	if len(s.sources) >= retainedTorrentLimit*2 {
		// Hints are opportunistic; full DHT traversal works without them.
		for key := range s.sources {
			delete(s.sources, key)
			if len(s.sources) < retainedTorrentLimit*3/2 {
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
		if r.Resolved {
			return r.CheckedAt.Add(30 * time.Minute)
		}
		return r.CheckedAt.Add(30 * time.Second)
	}
	return r.FirstSeen
}

func (s *sampler) nextJob(now time.Time) (discoveryJob, bool) {
	s.d.mu.Lock()
	defer s.d.mu.Unlock()
	s.mu.Lock()
	defer s.mu.Unlock()
	var fresh, retry *observation
	pending := 0
	better := func(a, b observation) bool {
		if (a.Peers > 0) != (b.Peers > 0) {
			return a.Peers > 0
		}
		// Repeatedly sampled hashes are useful candidates, without implying a
		// worldwide popularity estimate.
		if (a.SampleHits > 1) != (b.SampleHits > 1) {
			return a.SampleHits > 1
		}
		if !dueAt(a).Equal(dueAt(b)) {
			return dueAt(a).Before(dueAt(b))
		}
		return a.Hash < b.Hash
	}
	for _, r := range s.d.records {
		if s.inflight[r.Hash] || now.Before(dueAt(r)) {
			continue
		}
		pending++
		if r.ResolveAttempts == 0 && !r.Resolved && r.CheckedAt.IsZero() {
			if fresh == nil || better(r, *fresh) {
				copy := r
				fresh = &copy
			}
		} else if retry == nil || better(r, *retry) {
			copy := r
			retry = &copy
		}
	}
	s.d.state.Pending = pending
	chosen := fresh
	// Reserve one out of four dispatches for retries so a steady flow of new
	// samples cannot starve previously observed torrents with available peers.
	if retry != nil && (fresh == nil || s.turn%4 == 0) {
		chosen = retry
	}
	if chosen == nil {
		return discoveryJob{}, false
	}
	s.turn++
	if s.inflight == nil {
		s.inflight = make(map[string]bool)
	}
	s.inflight[chosen.Hash] = true
	// Reserve the retry deadline before network work. Even a malformed/private
	// response or a client setup failure must not cause a hot retry loop.
	chosen.ResolveAttempts++
	chosen.NextAttempt = now.Add(retryDelay(chosen.ResolveAttempts))
	s.d.records[chosen.Hash] = *chosen
	s.d.state.Active++
	s.d.state.Pending--
	return discoveryJob{hash: chosen.Hash, sources: append([]krpc.NodeInfo(nil), s.sources[chosen.Hash]...)}, true
}

func (s *sampler) releaseJob(hash string) {
	s.d.mu.Lock()
	defer s.d.mu.Unlock()
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.inflight[hash] {
		delete(s.inflight, hash)
		s.d.state.Active--
	}
}

func retryDelay(attempt int) time.Duration {
	return min(15*time.Minute, (30*time.Second)*time.Duration(1<<min(max(attempt-1, 0), 5)))
}
