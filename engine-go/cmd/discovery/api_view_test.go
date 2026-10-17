package main

import (
	"fmt"
	"testing"
	"time"
)

func BenchmarkBrowseWindow(b *testing.B) {
	d := newDashboard("", nil)
	now := time.Now()
	for i := 0; i < 100_000; i++ {
		h := fmt.Sprintf("%040x", i)
		d.records[h] = observation{Hash: h, Name: "Example.1080p.mkv", Resolved: true, Peers: i % 101, LastSeen: now, Countries: map[string]int{"US": 1}, Files: []string{"movie.mkv"}}
	}
	b.ReportAllocs()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		d.snapshotForAPI(viewQuery{Limit: 150, Sort: "peers"})
	}
}

func TestSearchMatchesReorderedWordsSeparatorsAccentsAndMagnet(t *testing.T) {
	d := testDashboard(t)
	h := fmt.Sprintf("%040x", 99)
	d.applyNameLocked(namePatch{Hash: h, Name: "Café.Movie.2026.1080p.mkv", Source: "dump"}, time.Now())
	for _, q := range []string{"2026 cafe movie", "café movie", "magnet:?xt=urn:btih:" + h + "&dn=Example"} {
		s := d.snapshotForAPI(viewQuery{Limit: 10, Q: q})
		if len(s.Records) != 1 || s.Records[0].Hash != h {
			t.Fatalf("search %q lost match", q)
		}
	}
}

func TestSnapshotForAPIReturnsBoundedWindow(t *testing.T) {
	d := testDashboard(t)
	now := time.Now()
	for i := 0; i < 1000; i++ {
		h := fmt.Sprintf("%040x", i)
		seeds := i % 50
		d.records[h] = observation{
			Hash: h, Name: fmt.Sprintf("Title %d", i), Resolved: true,
			Peers: i % 20, Seeds: &seeds, LastSeen: now, FirstSeen: now,
			Countries: map[string]int{"US": 1},
		}
	}
	s := d.snapshotForAPI(viewQuery{Limit: 25, Sort: "trending"})
	if s.Hashes != 1000 || s.Resolved != 1000 || s.Unresolved != 0 {
		t.Fatalf("meta hashes=%d resolved=%d unresolved=%d", s.Hashes, s.Resolved, s.Unresolved)
	}
	if len(s.Records) != 25 {
		t.Fatalf("window=%d", len(s.Records))
	}
	if s.MatchCount != 1000 {
		t.Fatalf("matchCount=%d", s.MatchCount)
	}
	if s.CountryTotals["US"] != 1000 {
		t.Fatalf("country totals %+v", s.CountryTotals)
	}
	s = d.snapshotForAPI(viewQuery{Limit: 10, Sort: "name", Q: "Title 9"})
	if len(s.Records) == 0 || s.MatchCount == 0 {
		t.Fatal("search window empty")
	}
}
