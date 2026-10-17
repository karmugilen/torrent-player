package main

import (
	"fmt"
	"testing"
	"time"
)

func TestSearchIndexFindsIndexedNames(t *testing.T) {
	d := testDashboard(t)
	now := time.Now()
	for i := 0; i < 500; i++ {
		h := fmt.Sprintf("%040x", i)
		name := fmt.Sprintf("Movie Title %d 1080p BluRay", i)
		d.records[h] = observation{
			Hash: h, Name: name, Resolved: true,
			LastSeen: now, FirstSeen: now,
		}
		d.indexNameLocked(h, name)
	}
	hashes, ok := d.searchCandidateHashes("bluray")
	if !ok || len(hashes) == 0 {
		t.Fatalf("indexed bluray ok=%v n=%d", ok, len(hashes))
	}
	s := d.snapshotForAPI(viewQuery{Limit: 20, Sort: "name", Q: "BluRay"})
	if len(s.Records) == 0 || s.MatchCount == 0 {
		t.Fatal("search via index returned empty")
	}
	if len(s.Records) > 20 {
		t.Fatalf("limit ignored: %d", len(s.Records))
	}
}

func TestSearchIndexSupportsTokenAndHashLookup(t *testing.T) {
	d := testDashboard(t)
	now := time.Now()
	for i := 0; i < 200; i++ {
		h := fmt.Sprintf("%040x", i+1)
		name := fmt.Sprintf("Show Episode %d x265", i)
		d.records[h] = observation{Hash: h, Name: name, Resolved: true, LastSeen: now, FirstSeen: now}
		d.indexNameLocked(h, name)
	}
	hashes, ok := d.searchCandidateHashes("x265")
	if !ok || len(hashes) == 0 {
		t.Fatalf("x265 should stay indexed ok=%v n=%d", ok, len(hashes))
	}
	target := fmt.Sprintf("%040x", 7)
	got, ok := d.searchCandidateHashes(target)
	if !ok || len(got) != 1 || got[0] != target {
		t.Fatalf("hash lookup got=%v ok=%v", got, ok)
	}
}

func TestSnapshotSearchWorksWithoutPrebuiltIndex(t *testing.T) {
	// Records inserted without indexNameLocked must still be findable via scan.
	d := testDashboard(t)
	now := time.Now()
	h := fmt.Sprintf("%040x", 42)
	d.records[h] = observation{
		Hash: h, Name: "Unindexed Rare Title ZZ9", Resolved: true,
		LastSeen: now, FirstSeen: now,
	}
	s := d.snapshotForAPI(viewQuery{Limit: 10, Sort: "name", Q: "ZZ9"})
	if len(s.Records) != 1 || s.MatchCount != 1 {
		t.Fatalf("scan fallback failed records=%d match=%d", len(s.Records), s.MatchCount)
	}
}

func TestLoadMoreWindowUsesOffsetAndLimit(t *testing.T) {
	d := testDashboard(t)
	now := time.Now()
	for i := 0; i < 400; i++ {
		h := fmt.Sprintf("%040x", i)
		seeds := i
		d.records[h] = observation{
			Hash: h, Name: fmt.Sprintf("Title %04d", i), Resolved: true,
			Peers: 1, Seeds: &seeds, LastSeen: now, FirstSeen: now,
		}
	}
	first := d.snapshotForAPI(viewQuery{Limit: 150, Sort: "seeds"})
	if len(first.Records) != 150 || first.MatchCount != 400 {
		t.Fatalf("first window=%d match=%d", len(first.Records), first.MatchCount)
	}
	second := d.snapshotForAPI(viewQuery{Limit: 300, Sort: "seeds"})
	if len(second.Records) != 300 {
		t.Fatalf("load-more window=%d", len(second.Records))
	}
	if second.MatchCount != 400 {
		t.Fatalf("matchCount=%d", second.MatchCount)
	}
}
