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
