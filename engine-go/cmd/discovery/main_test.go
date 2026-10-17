package main

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/anacrolix/dht/v2/krpc"
	"github.com/anacrolix/torrent/metainfo"
)

func testDashboard(t *testing.T) *dashboard {
	t.Helper()
	return newDashboard(filepath.Join(t.TempDir(), "observations.json"), nil)
}
func TestMetadataOnlyStorageRejectsPayload(t *testing.T) {
	impl, e := (metadataStorage{}).OpenTorrent(context.Background(), nil, metainfo.Hash{})
	if e != nil {
		t.Fatal(e)
	}
	piece := impl.Piece(metainfo.Piece{})
	if n, e := piece.WriteAt([]byte("must never be saved"), 0); n != 0 || e == nil {
		t.Fatal("payload write was accepted")
	}
	if _, e := piece.ReadAt(make([]byte, 1), 0); e == nil {
		t.Fatal("payload read was accepted")
	}
	if piece.Completion().Complete {
		t.Fatal("fabricated completion")
	}
}
func TestSnapshotPersistsNamesAndKeepsCallerChangesIsolated(t *testing.T) {
	d := testDashboard(t)
	h := strings.Repeat("a", 40)
	d.records[h] = observation{Hash: h, Name: "Example episode.mkv", Resolved: true, Peers: 4, Files: []string{"episode.mkv"}, Countries: map[string]int{"IN": 4}, FirstSeen: time.Now(), LastSeen: time.Now()}
	s := d.snapshot()
	s.Records[0].Countries["IN"] = 999
	s.Records[0].Files[0] = "changed"
	d.persist()
	restored := newDashboard(d.cachePath, nil)
	got := restored.snapshot()
	if got.Resolved != 1 || got.Records[0].Countries["IN"] != 4 || got.Records[0].Files[0] != "episode.mkv" {
		t.Fatalf("lost or aliased observation: %+v", got)
	}
}
func TestPageEscapesUntrustedMetadataAndControlRejectsForeignOrigins(t *testing.T) {
	d := testDashboard(t)
	h := strings.Repeat("b", 40)
	d.records[h] = observation{Hash: h, Name: "</script><script>alert('torrent')</script>", Resolved: true}
	handler := d.handler([]byte("[]"))
	w := httptest.NewRecorder()
	handler.ServeHTTP(w, httptest.NewRequest("GET", "http://127.0.0.1:18181/", nil))
	if w.Code != 200 || strings.Contains(w.Body.String(), "</script><script>alert") {
		t.Fatal("untrusted title escaped script data")
	}
	request := httptest.NewRequest("POST", "http://127.0.0.1:18181/api/control", bytes.NewBufferString(`{"running":false}`))
	request.Header.Set("Origin", "https://unrelated.example")
	w = httptest.NewRecorder()
	handler.ServeHTTP(w, request)
	if w.Code != http.StatusForbidden {
		t.Fatalf("foreign origin: %d", w.Code)
	}
}
func TestStateEndpointAndInvalidControl(t *testing.T) {
	d := testDashboard(t)
	handler := d.handler([]byte("[]"))
	w := httptest.NewRecorder()
	handler.ServeHTTP(w, httptest.NewRequest("GET", "http://127.0.0.1:18181/api/state", nil))
	var s state
	if json.Unmarshal(w.Body.Bytes(), &s) != nil || s.Records == nil {
		t.Fatal("invalid state response")
	}
	for _, body := range []string{`{}`, `{"running":"yes"}`, `not JSON`} {
		w = httptest.NewRecorder()
		handler.ServeHTTP(w, httptest.NewRequest("POST", "http://127.0.0.1:18181/api/control", strings.NewReader(body)))
		if w.Code != 400 {
			t.Fatalf("invalid control accepted: %s", body)
		}
	}
}
func TestBoundedCacheRetainsResolvedNamesAndDropsInvalidHashes(t *testing.T) {
	d := testDashboard(t)
	old := time.Now().Add(-time.Hour)
	for i := 0; i < 800; i++ {
		h := fmt.Sprintf("%040x", i)
		d.records[h] = observation{Hash: h, LastSeen: time.Now()}
	}
	h := strings.Repeat("c", 40)
	d.records[h] = observation{Hash: h, Name: "Known video", Resolved: true, LastSeen: old}
	d.pruneLocked()
	if len(d.records) != 600 || !d.records[h].Resolved {
		t.Fatal("cache evicted useful names or exceeded bound")
	}
	d.persist()
	if len(newDashboard(d.cachePath, nil).records) != 600 {
		t.Fatal("persisted cache changed")
	}
}
func TestEmptyBloomIsZeroAndUnsupportedLocationsRemainUnknown(t *testing.T) {
	var b krpc.ScrapeBloomFilter
	n := countBloom(&b)
	if n == nil || *n != 0 {
		t.Fatal("empty bloom invented seeders")
	}
	d := testDashboard(t)
	if d.country(nil) != "ZZ" {
		t.Fatal("invented a country")
	}
}
