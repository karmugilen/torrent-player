package main

import (
	"fmt"
	"strings"
	"testing"
	"time"
)

func TestApplyNameLockedUnifiesDumpAndCatalogCounters(t *testing.T) {
	d := testDashboard(t)
	now := time.Now()
	hash := strings.Repeat("ab", 20)
	d.mu.Lock()
	ok := d.applyNameLocked(namePatch{Hash: hash, Name: "Show.S01.1080p.mkv", Source: "dump", Size: 9, PeerBump: 5}, now)
	d.mu.Unlock()
	if !ok {
		t.Fatal("first apply should resolve")
	}
	if !d.records[hash].Resolved || d.snapshot().DumpHits != 1 || d.snapshot().Successes != 1 {
		t.Fatalf("dump apply: %+v state=%+v", d.records[hash], d.snapshot())
	}
	if _, pending := d.pendingSet[hash]; pending {
		t.Fatal("resolved hash must leave pendingSet")
	}
	d.mu.Lock()
	again := d.applyNameLocked(namePatch{Hash: hash, Name: "Show.S01.1080p.mkv", Source: "catalog"}, now)
	d.mu.Unlock()
	if again {
		t.Fatal("second apply must not double-count")
	}
	if d.snapshot().CatalogHits != 0 || d.snapshot().Successes != 1 {
		t.Fatal("already-named row counted again")
	}
}

func TestSchedulerScansPendingSetNotWholeStore(t *testing.T) {
	d := testDashboard(t)
	now := time.Now()
	for i := 0; i < 500; i++ {
		h := fmt.Sprintf("%040x", i)
		d.records[h] = observation{Hash: h, Name: "named", Resolved: true, LastSeen: now}
	}
	pending := strings.Repeat("cd", 20)
	d.records[pending] = observation{Hash: pending, FirstSeen: now, LastSeen: now, Countries: map[string]int{}}
	d.pendingSet[pending] = struct{}{}
	s := &sampler{d: d, cacheTried: map[string]bool{}}
	job, ok := s.nextJobOf(now, jobCache)
	if !ok || job.hash != pending {
		t.Fatalf("expected pending hash, got %#v ok=%v", job, ok)
	}
}

func TestCacheMissCannotBeReclaimedBeforePeerHandoff(t *testing.T) {
	d := testDashboard(t)
	now := time.Now()
	h := strings.Repeat("d", 40)
	d.records[h] = observation{Hash: h, FirstSeen: now, NextAttempt: now.Add(-time.Second), ResolveAttempts: 1}
	d.pendingSet[h] = struct{}{}
	s := &sampler{d: d, cacheTried: map[string]bool{h: true}}
	if _, ok := s.nextJobOf(now, jobCache); ok {
		t.Fatal("cache worker reclaimed its own miss and starved peer lookup")
	}
	job, ok := s.nextJobOf(now, jobPeer)
	if !ok || job.hash != h {
		t.Fatal("cache miss did not reach peer worker")
	}
}

func TestVerifiedMetadataCorrectsCatalogNameWithoutDoubleCounting(t *testing.T) {
	d := testDashboard(t)
	hash := strings.Repeat("ef", 20)
	now := time.Now()
	d.mu.Lock()
	d.applyNameLocked(namePatch{Hash: hash, Name: "Catalog title", Source: "dump", Size: 10}, now)
	added := d.applyNameLocked(namePatch{Hash: hash, Name: "Exact.Metadata.Title.mkv", Source: "peer",
		Size: 42, Files: []string{"episode.mkv"}}, now.Add(time.Second))
	d.applyNameLocked(namePatch{Hash: hash, Name: "Catalog title", Source: "catalog", Size: 10}, now.Add(2*time.Second))
	d.mu.Unlock()
	rec := d.records[hash]
	if added || rec.Name != "Exact.Metadata.Title.mkv" || rec.Source != "peer" || rec.Size != 42 || len(rec.Files) != 1 {
		t.Fatalf("verified metadata must correct the catalog without counting a second name: %+v", rec)
	}
	if d.state.Successes != 1 {
		t.Fatal("verification counted the same hash twice")
	}
	view := d.snapshotForAPI(viewQuery{Q: "exact metadata", Limit: 10})
	if view.MatchCount != 1 {
		t.Fatal("corrected name missing from search")
	}
}
