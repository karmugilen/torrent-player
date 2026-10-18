package main

import (
	"container/heap"
	"sort"
	"time"
)

// A bounded batch amortizes the backlog walk across many workers. New samples
// and changed priorities become visible on the next 250ms refresh. Dispatch checks
// the live record, so an import or another worker cannot cause duplicate work.
const dispatchBatchSize = 128
const dispatchRefresh = 250 * time.Millisecond

type resolveCandidate struct {
	hash     string
	priority int64
	due      time.Time
}

type resolveCandidates []resolveCandidate

func (c resolveCandidates) Len() int { return len(c) }
func (c resolveCandidates) Less(i, j int) bool {
	if c[i].priority != c[j].priority {
		return c[i].priority < c[j].priority
	}
	if !c[i].due.Equal(c[j].due) {
		return c[i].due.After(c[j].due)
	}
	return c[i].hash > c[j].hash
}
func (c resolveCandidates) Swap(i, j int) { c[i], c[j] = c[j], c[i] }
func (c *resolveCandidates) Push(v any)   { *c = append(*c, v.(resolveCandidate)) }
func (c *resolveCandidates) Pop() any {
	last := (*c)[len(*c)-1]
	*c = (*c)[:len(*c)-1]
	return last
}

func (c *resolveCandidates) offer(rec observation, now time.Time) {
	c.offerWithBonus(rec, now, 0)
}

func (c *resolveCandidates) offerWithBonus(rec observation, now time.Time, bonus int64) {
	candidate := resolveCandidate{rec.Hash, resolvePriority(rec, now) + bonus, dueAt(rec)}
	if len(*c) < dispatchBatchSize {
		heap.Push(c, candidate)
	} else if (resolveCandidates{(*c)[0], candidate}).Less(0, 1) {
		(*c)[0] = candidate
		heap.Fix(c, 0)
	}
}

type resolverBatch struct {
	fresh, retry resolveCandidates
	refreshAt    time.Time
	turn         int
}

func (b *resolverBatch) offer(rec observation, now time.Time) {
	queue := &b.retry
	if rec.ResolveAttempts == 0 && rec.CheckedAt.IsZero() {
		queue = &b.fresh
	}
	// Batches are sorted between refills. Heapify only this tiny handoff batch.
	heap.Init(queue)
	queue.offer(rec, now)
	sort.Sort(*queue)
}

// Both dashboard.mu and sampler.mu are held by the caller throughout dispatch.
func (s *sampler) eligibleLocked(rec observation, now time.Time, kind jobKind) bool {
	return s.workDueLocked(rec, now, kind) && !s.coldAdmissionBlocked(rec, kind)
}

func (s *sampler) coldAdmissionBlocked(rec observation, kind jobKind) bool {
	return kind == jobPeer && s.coldJobs != nil && len(s.coldJobs) >= cap(s.coldJobs) && !s.readyPeers[rec.Hash] && rec.NameHint == ""
}

func (s *sampler) workDueLocked(rec observation, now time.Time, kind jobKind) bool {
	if rec.Resolved || s.inflight[rec.Hash] || now.Before(dueAt(rec)) {
		return false
	}
	if kind == jobCache {
		return !s.cacheTried[rec.Hash]
	}
	return s.cacheTried[rec.Hash] || rec.Peers > 0 || rec.ResolveAttempts > 0 || s.cacheOffline
}

func (s *sampler) refillDispatchLocked(now time.Time, kind jobKind) {
	batch := &s.dispatch[kind]
	if kind == jobPeer {
		s.readyPeers = s.hints.readyHashes(now)
	}
	batch.fresh, batch.retry = batch.fresh[:0], batch.retry[:0]
	pending := 0
	for hash := range s.d.pendingSet {
		rec, ok := s.d.records[hash]
		if !ok || rec.Resolved {
			s.d.markResolvedLocked(hash)
			continue
		}
		if !s.workDueLocked(rec, now, kind) {
			continue
		}
		pending++
		if s.coldAdmissionBlocked(rec, kind) {
			continue
		}
		queue := &batch.retry
		if rec.ResolveAttempts == 0 && rec.CheckedAt.IsZero() {
			queue = &batch.fresh
		}
		var bonus int64
		if kind == jobPeer && s.readyPeers[rec.Hash] {
			bonus = 30_000
		} else if kind == jobPeer && len(s.sources[rec.Hash]) > 0 && now.Sub(rec.LastSeen) < peerHintTTL {
			// Fresh samples have a known reporting node we can ask directly;
			// old blind backlog hashes need an entire traversal first.
			bonus = 5_000
		}
		queue.offerWithBonus(rec, now, bonus)
	}
	sort.Sort(batch.fresh)
	sort.Sort(batch.retry)
	batch.refreshAt = now.Add(dispatchRefresh)
	if kind == jobPeer {
		s.d.state.Pending = pending
	}
}

func (s *sampler) readyCandidateLocked(queue *resolveCandidates, now time.Time, kind jobKind) (observation, bool) {
	for len(*queue) > 0 {
		candidate := (*queue)[len(*queue)-1]
		rec, ok := s.d.records[candidate.hash]
		if ok && s.eligibleLocked(rec, now, kind) {
			return rec, true
		}
		*queue = (*queue)[:len(*queue)-1]
	}
	return observation{}, false
}
