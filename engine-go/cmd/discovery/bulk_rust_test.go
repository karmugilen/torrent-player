//go:build rustdump

package main

import (
	"encoding/csv"
	"os"
	"testing"
)

// Opt-in benchmark using 100,000 distinct real rows, not one repeated hash.
// ATLAS_BENCH_CSV=/path/torrents.csv go test ... -bench BenchmarkRustDump100K
func BenchmarkRustDump100K(b *testing.B) {
	path := os.Getenv("ATLAS_BENCH_CSV")
	if path == "" {
		b.Skip("set ATLAS_BENCH_CSV to a real dump")
	}
	idx, err := rustLoadDump(path)
	if err != nil {
		b.Fatal(err)
	}
	_ = idx
	f, err := os.Open(path)
	if err != nil {
		b.Fatal(err)
	}
	defer f.Close()
	r := csv.NewReader(f)
	r.FieldsPerRecord = -1
	r.LazyQuotes = true
	_, _ = r.Read()
	hashes := make([]string, 0, 100_000)
	for len(hashes) < 100_000 {
		row, err := r.Read()
		if err != nil {
			b.Fatal(err)
		}
		if len(row) > 1 && validHash(row[0]) {
			hashes = append(hashes, row[0])
		}
	}
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		for start := 0; start < len(hashes); start += 50_000 {
			if hits := rustLookupMany(hashes[start : start+50_000]); len(hits) != 50_000 {
				b.Fatalf("matched %d", len(hits))
			}
		}
	}
	b.ReportMetric(float64(100_000)*float64(b.N)/b.Elapsed().Minutes(), "matches/min")
}
