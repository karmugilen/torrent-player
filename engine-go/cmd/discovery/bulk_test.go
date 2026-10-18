package main

import (
	"bytes"
	"encoding/base32"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"
)

func TestBulkResolveUsesStoredNamesAndDumpWithoutInflatingLiveRates(t *testing.T) {
	d := testDashboard(t)
	stored, indexed, missing := strings.Repeat("a", 40), strings.Repeat("b", 40), strings.Repeat("c", 40)
	d.records[stored] = observation{Hash: stored, Name: "Verified episode.mkv", Source: "peer", Resolved: true}
	d.setDumpIndex(&dumpIndex{rows: map[string]dumpRow{indexed: {Name: "Indexed movie.mkv", Size: 123}}})
	raw, _ := hex.DecodeString(indexed)
	magnet := "magnet:?xt=urn:btih:" + base32.StdEncoding.WithPadding(base32.NoPadding).EncodeToString(raw)
	body, _ := json.Marshal(map[string]any{"hashes": []string{strings.ToUpper(stored), magnet, missing, stored, "invalid"}})
	w := httptest.NewRecorder()
	d.handler(nil).ServeHTTP(w, httptest.NewRequest("POST", "http://127.0.0.1:18181/api/resolve", bytes.NewReader(body)))
	if w.Code != 200 {
		t.Fatalf("bulk resolver unavailable: status=%d body=%s", w.Code, w.Body.String())
	}
	var got struct {
		Requested, Unique, Matched, Invalid, Duplicates int
		Results                                         []struct {
			Hash, Name, Source string
			Verified           bool
		}
		Missing []string
	}
	if err := json.Unmarshal(w.Body.Bytes(), &got); err != nil {
		t.Fatal(err)
	}
	if got.Requested != 5 || got.Unique != 3 || got.Matched != 2 || got.Invalid != 1 || got.Duplicates != 1 || len(got.Missing) != 1 || got.Missing[0] != missing {
		t.Fatalf("incorrect accounting: %+v", got)
	}
	if len(got.Results) != 2 || !got.Results[0].Verified || got.Results[1].Verified || got.Results[1].Name != "Indexed movie.mkv" {
		t.Fatalf("lost provenance: %+v", got.Results)
	}
	if s := d.snapshot(); s.LiveNamesPerMinute != 0 || s.VerifiedNamesPerMinute != 0 || s.Completed != 0 {
		t.Fatalf("read-only batch inflated live naming: %+v", s)
	}
}

func TestBulkResolveBoundsAndOrigin(t *testing.T) {
	d := testDashboard(t)
	for _, tc := range []struct {
		body, origin string
		status       int
	}{
		{`{"hashes":[]}`, "", 400}, {`{"hashes":["bad"]} trailing`, "", 400},
		{`{"hashes":["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]}`, "https://foreign.example", 403},
	} {
		r := httptest.NewRequest("POST", "http://127.0.0.1:18181/api/resolve", strings.NewReader(tc.body))
		r.Header.Set("Origin", tc.origin)
		w := httptest.NewRecorder()
		d.handler(nil).ServeHTTP(w, r)
		if w.Code != tc.status {
			t.Fatalf("status=%d expected=%d", w.Code, tc.status)
		}
	}
}

func BenchmarkBulkResolve100K(b *testing.B) {
	d := newDashboard(filepath.Join(b.TempDir(), "empty.json"), nil)
	hashes := make([]string, 100_000)
	for i := range hashes {
		hashes[i] = fmt.Sprintf("%040x", i+1)
		d.records[hashes[i]] = observation{Hash: hashes[i], Name: fmt.Sprintf("Episode %d.mkv", i), Resolved: true, Source: "dump"}
	}
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		for start := 0; start < len(hashes); start += 50_000 {
			got := d.resolveHashes(hashes[start : start+50_000])
			if got.Matched != 50_000 {
				b.Fatal(got.Matched)
			}
		}
	}
	b.ReportMetric(float64(100_000)*float64(b.N)/b.Elapsed().Minutes(), "matches/min")
}
