package main

import (
	"fmt"
	"testing"
	"time"
)

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
