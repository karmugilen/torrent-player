package main

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestDumpIndexResolvesPendingHashes(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "torrents.csv")
	hash := strings.Repeat("ab", 20)
	body := "infohash,name,size_bytes,created_unix,seeders,leechers,completed,scraped_date,published\n" +
		hash + ",\"Example.Show.S01E01.1080p.WEB-DL.mkv\",1500,1,12,3,0,1,1\n"
	if err := os.WriteFile(path, []byte(body), 0600); err != nil {
		t.Fatal(err)
	}
	idx, err := loadDumpIndex(path)
	if err != nil {
		t.Fatal(err)
	}
	if idx.len() != 1 {
		t.Fatalf("rows=%d rust=%v", idx.len(), idx.rust)
	}
	d := testDashboard(t)
	d.dumpPath = path
	now := time.Now()
	d.records[hash] = observation{Hash: hash, FirstSeen: now, LastSeen: now, Countries: map[string]int{}}
	d.pendingSet[hash] = struct{}{}
	other := strings.Repeat("cd", 20)
	d.records[other] = observation{Hash: other, FirstSeen: now, LastSeen: now, Countries: map[string]int{}}
	d.pendingSet[other] = struct{}{}
	d.setDumpIndex(idx)
	if n := d.resolvePendingFromDump(idx); n != 1 {
		t.Fatalf("resolved %d", n)
	}
	got := d.records[hash]
	if !got.Resolved || got.Name == "" || got.Source != "dump" || got.Peers != 15 || got.Seeds == nil || *got.Seeds != 12 {
		t.Fatalf("dump observation: %+v", got)
	}
	if d.snapshot().DumpHits != 1 || d.snapshot().Successes != 1 {
		t.Fatalf("counters: %+v", d.snapshot())
	}
	if d.records[other].Resolved {
		t.Fatal("unknown hash should stay unresolved")
	}
	if n := d.resolvePendingFromDump(idx); n != 0 || d.snapshot().DumpHits != 1 {
		t.Fatal("dump hits double-counted")
	}
	seedHash := strings.Repeat("11", 20)
	if !d.applyDumpRow(seedHash, dumpRow{Name: "Seeded.Show.1080p.mkv", Size: 9, Seeders: 4, Leechers: 1}, true) {
		t.Fatal("dump seed should insert a new named row")
	}
	if d.records[seedHash].Source != "dump" || !d.records[seedHash].Resolved {
		t.Fatal("seeded row missing")
	}
	if d.applyDumpRow(seedHash, dumpRow{Name: "Seeded.Show.1080p.mkv", Size: 9, Seeders: 4}, true) {
		t.Fatal("seed-only must skip hashes already present")
	}
}

func TestTryResolveHashFromDumpOnSample(t *testing.T) {
	d := testDashboard(t)
	hash := strings.Repeat("ab", 20)
	d.setDumpIndex(&dumpIndex{rows: map[string]dumpRow{
		hash: {Name: "Instant.Lookup.mkv", Size: 42, Seeders: 7, Leechers: 2},
	}})
	now := time.Now()
	d.records[hash] = observation{Hash: hash, FirstSeen: now, LastSeen: now, Countries: map[string]int{}}
	d.pendingSet[hash] = struct{}{}
	if !d.tryResolveHashFromDump(hash) {
		t.Fatal("dump should resolve sampled hash immediately")
	}
	if !d.records[hash].Resolved || d.snapshot().DumpHits != 1 {
		t.Fatal("instant dump resolve failed")
	}
}

func TestUnlimitedStoreKeepsGrowing(t *testing.T) {
	d := testDashboard(t)
	now := time.Now()
	for i := 0; i < 12_000; i++ {
		h := fmt.Sprintf("%040x", i)
		d.records[h] = observation{Hash: h, Name: "n", Resolved: true, LastSeen: now}
	}
	d.pruneLocked()
	if len(d.records) < 12_000 {
		t.Fatalf("store still capped at %d", len(d.records))
	}
	if d.snapshot().RetentionLimit != 0 {
		t.Fatal("retention limit should report unlimited (0)")
	}
}
