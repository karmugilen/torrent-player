package main

import (
	"compress/gzip"
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestAdditionalCatalogStreamsUnsortedRowsAndPreservesVerifiedNames(t *testing.T) {
	d := testDashboard(t)
	verified, pending := strings.Repeat("a", 40), strings.Repeat("b", 40)
	d.applyNameLocked(namePatch{Hash: verified, Name: "Verified title.mkv", Source: "peer"}, time.Now())
	path := filepath.Join(t.TempDir(), "extra.csv.gz")
	f, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	w := gzip.NewWriter(f)
	_, _ = fmt.Fprintf(w, "hash,title,size\n%s,Fresh indexed episode.mkv,42\n%s,Wrong catalog name,99\n%s,Duplicate title,0\ninvalid,Not a hash,0\n", pending, verified, pending)
	_ = w.Close()
	_ = f.Close()
	rows, err := d.importNamedCSV(context.Background(), path)
	if err != nil || rows != 3 {
		t.Fatalf("import: rows=%d err=%v", rows, err)
	}
	if d.records[verified].Name != "Verified title.mkv" || d.records[pending].Name != "Fresh indexed episode.mkv" {
		t.Fatal("import overwrote a known name")
	}
	if len(d.snapshotForAPI(viewQuery{Q: "Fresh indexed", Limit: 10}).Records) != 1 {
		t.Fatal("imported name not searchable")
	}
	if d.snapshot().VerifiedNamesPerMinute != 1 || d.snapshot().IndexNamesPerMinute != 1 {
		t.Fatal("imports inflated verification or duplicate count")
	}
}

func BenchmarkImportNames100K(b *testing.B) {
	path := filepath.Join(b.TempDir(), "input.csv")
	f, err := os.Create(path)
	if err != nil {
		b.Fatal(err)
	}
	_, _ = fmt.Fprintln(f, "infohash,name,size_bytes")
	for i := 0; i < 100_000; i++ {
		_, _ = fmt.Fprintf(f, "%040x,Example movie %d.mkv,1234\n", i+1, i)
	}
	_ = f.Close()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		d := newDashboard(filepath.Join(b.TempDir(), "empty.json"), nil)
		if _, err := d.importNamedCSV(context.Background(), path); err != nil {
			b.Fatal(err)
		}
		if len(d.records) != 100_000 || d.state.CatalogHits != 100_000 {
			b.Fatal("did not import 100,000 distinct names")
		}
	}
	b.ReportMetric(float64(100_000)*float64(b.N)/b.Elapsed().Minutes(), "new-catalog-names/min")
}
