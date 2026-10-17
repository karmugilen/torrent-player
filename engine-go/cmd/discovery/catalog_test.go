package main

import (
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

func TestCatalogIngestNamesNewTorrents(t *testing.T) {
	d := testDashboard(t)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_ = json.NewEncoder(w).Encode(catalogPage{Torrents: []catalogTorrent{{
			InfoHash:  strings.Repeat("ab", 20),
			Name:      "Example.Show.S01E01.1080p.WEB-DL.mkv",
			SizeBytes: 1500,
			Seeders:   12,
			Leechers:  3,
		}}})
	}))
	defer server.Close()
	client := server.Client()
	// Point the helper at the test server by temporarily swapping endpoint usage
	// through a direct apply after a fetch against the test client URL.
	started := time.Now()
	req, err := http.NewRequest(http.MethodGet, server.URL, nil)
	if err != nil {
		t.Fatal(err)
	}
	resp, err := client.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	var page catalogPage
	if json.NewDecoder(resp.Body).Decode(&page) != nil {
		t.Fatal("decode")
	}
	if n := d.applyCatalogTorrents(page.Torrents, time.Since(started)); n != 1 {
		t.Fatalf("added %d", n)
	}
	snap := d.snapshot()
	if snap.Successes != 1 || snap.CatalogHits != 1 || snap.Resolved != 1 {
		t.Fatalf("catalog counters: %+v", snap)
	}
	got := snap.Records[0]
	if got.Name == "" || got.Source != "catalog" || got.Kind != "Video" || got.Peers != 15 || got.Seeds == nil || *got.Seeds != 12 {
		t.Fatalf("catalog observation: %+v", got)
	}
	// Second ingest of the same hash must not double-count successes.
	if n := d.applyCatalogTorrents(page.Torrents, time.Millisecond); n != 0 || d.snapshot().Successes != 1 {
		t.Fatal("duplicate catalog row counted twice")
	}
}

func TestCatalogKeepsIngestingWithUnlimitedStore(t *testing.T) {
	d := testDashboard(t)
	now := time.Now()
	for i := 0; i < 100; i++ {
		h := fmt.Sprintf("%040x", i)
		d.records[h] = observation{Hash: h, Name: "named", Resolved: true, LastSeen: now}
	}
	if !d.catalogShouldIngest() {
		t.Fatal("catalog should keep pacing with an unlimited JSON store")
	}
}
