package main

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"
)

func TestStreamedSnapshotKeepsEachRecordConsistentDuringUpdates(t *testing.T) {
	d := testDashboard(t)
	for i := 0; i < 4096; i++ {
		h := fmt.Sprintf("%040x", i+1)
		d.records[h] = observation{Hash: h, Name: "Example.mkv", Resolved: true, LastSeen: time.Now(), Countries: map[string]int{"IN": 0}}
	}
	done := make(chan struct{})
	var work sync.WaitGroup
	work.Add(1)
	go func() {
		defer work.Done()
		for i := 0; ; i++ {
			select {
			case <-done:
				return
			default:
			}
			hash := fmt.Sprintf("%040x", i%4096+1)
			d.mu.Lock()
			r := d.records[hash]
			r.Peers++
			r.Countries["IN"] = r.Peers
			d.records[hash] = r
			d.mu.Unlock()
		}
	}()
	d.persist()
	close(done)
	work.Wait()
	b, err := os.ReadFile(d.cachePath)
	if err != nil {
		t.Fatal(err)
	}
	var saved []observation
	if err := json.Unmarshal(b, &saved); err != nil {
		t.Fatal(err)
	}
	if len(saved) != 4096 {
		t.Fatal("snapshot lost records")
	}
	for _, r := range saved {
		if r.Peers != r.Countries["IN"] {
			t.Fatal("snapshot aliased mutable country totals")
		}
	}
}

func TestFailedSnapshotPreservesPreviousFile(t *testing.T) {
	d := testDashboard(t)
	d.persist()
	before, err := os.ReadFile(d.cachePath)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.Mkdir(d.cachePath+".tmp", 0700); err != nil {
		t.Fatal(err)
	}
	d.persist()
	after, err := os.ReadFile(d.cachePath)
	if err != nil || string(before) != string(after) {
		t.Fatal("failed write damaged the previous snapshot")
	}
}

func BenchmarkPersist100K(b *testing.B) {
	d := newDashboard(filepath.Join(b.TempDir(), "observations.json"), nil)
	for i := 0; i < 100_000; i++ {
		h := fmt.Sprintf("%040x", i+1)
		d.records[h] = observation{Hash: h, Name: fmt.Sprintf("Example episode %d.mkv", i), Resolved: true, Source: "peer", LastSeen: time.Now(), Files: []string{"episode.mkv", "subtitles.srt"}, Countries: map[string]int{"IN": 2, "US": 1}}
	}
	b.ReportAllocs()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		d.persist()
	}
}
