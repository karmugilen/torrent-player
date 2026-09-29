//go:build !rust && !android

package main

import "testing"

func TestAggregatePieceBucketsRejectsInvalidInput(t *testing.T) {
	valid := []pieceRun{{length: 4, flags: 0}}
	cases := []struct {
		name   string
		runs   []pieceRun
		ranges []pieceRange
		total  int
		max    int
	}{
		{name: "inverted range", runs: valid, ranges: []pieceRange{{start: 3, end: 1}}, total: 4, max: 4},
		{name: "end past total", runs: valid, ranges: []pieceRange{{start: 0, end: 5}}, total: 4, max: 4},
		{name: "total zero with nonzero run", runs: []pieceRun{{length: 1, flags: 1}}, total: 0, max: 256},
		{name: "zero-length run", runs: []pieceRun{{length: 0, flags: 1}}, total: 0, max: 256},
		{name: "run coverage short", runs: []pieceRun{{length: 3, flags: 1}}, total: 4, max: 4},
		{name: "run coverage long", runs: []pieceRun{{length: 5, flags: 1}}, total: 4, max: 4},
		{name: "maxBuckets low", runs: valid, total: 4, max: 0},
		{name: "maxBuckets high", runs: valid, total: 4, max: 1025},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if _, err := aggregatePieceBuckets(tc.runs, tc.ranges, tc.total, tc.max); err == nil {
				t.Fatal("accepted invalid input")
			}
		})
	}

	buckets, err := aggregatePieceBuckets(nil, nil, 0, 256)
	if err != nil || len(buckets) != 0 {
		t.Fatalf("empty coverage: buckets=%+v err=%v", buckets, err)
	}
	if _, err := aggregatePieceBuckets(nil, []pieceRange{{start: 0, end: 0}}, 0, 1); err != nil {
		t.Fatal(err)
	}
}

func TestAggregatePieceBucketsSweepMatchesCompressedRuns(t *testing.T) {
	// Adjacent equal flags are split on purpose so the sweep must coalesce them.
	// Overlapping copies of [1, 5) plus [4, 8) cover seven pieces, once each.
	runs := []pieceRun{
		{length: 1, flags: 3},
		{length: 2, flags: 3},
		{length: 1, flags: 2},
		{length: 2, flags: 2},
		{length: 4, flags: 0},
	}
	ranges := []pieceRange{{start: 4, end: 8}, {start: 1, end: 5}, {start: 1, end: 5}}
	buckets, err := aggregatePieceBuckets(runs, ranges, 10, 4)
	if err != nil {
		t.Fatal(err)
	}
	if len(buckets) != 4 {
		t.Fatalf("buckets=%d %+v", len(buckets), buckets)
	}
	var selected, selectedVerified, selectedReceiving int
	covered := 0
	for _, b := range buckets {
		if b.End < b.Start || b.Total != b.End-b.Start+1 {
			t.Fatalf("inclusive bucket=%+v", b)
		}
		covered += b.Total
		selected += b.Selected
		selectedVerified += b.SelectedVerified
		selectedReceiving += b.SelectedReceiving
	}
	if covered != 10 || buckets[0].Start != 0 || buckets[3].Start != 9 || buckets[3].End != 9 || buckets[3].Total != 1 {
		t.Fatalf("partition=%+v", buckets)
	}
	if selected != 7 || selectedVerified != 2 || selectedReceiving != 3 {
		t.Fatalf("selected=%d selectedVerified=%d selectedReceiving=%d buckets=%+v", selected, selectedVerified, selectedReceiving, buckets)
	}
	if buckets[0].Verified != 3 || buckets[0].Receiving != 0 {
		t.Fatalf("verified must preempt receiving: %+v", buckets[0])
	}
}
