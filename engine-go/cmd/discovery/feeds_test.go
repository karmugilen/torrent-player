package main

import (
	"context"
	"github.com/anacrolix/torrent/metainfo"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

func TestFeedHintRemainsPendingAndSearchableUntilVerified(t *testing.T) {
	xml := `<rss><channel><item><title>Example fresh episode.mkv</title><link>magnet:?xt=urn:btih:C55XSN2P7NXXG6UKY6CFQVSF3LHVJKVL&amp;tr=udp%3A%2F%2Ftracker.example%3A6969%2Fannounce</link></item><item><title>Duplicate</title><link>magnet:?xt=urn:btih:C55XSN2P7NXXG6UKY6CFQVSF3LHVJKVL</link></item></channel></rss>`
	items, err := parseNamedFeed(strings.NewReader(xml))
	if err != nil || len(items) != 1 {
		t.Fatalf("parse: %v %+v", err, items)
	}
	d := testDashboard(t)
	tr := newTrackerResolver()
	defer tr.close()
	s := &sampler{d: d, trackers: tr}
	now := time.Now()
	if s.applyFeedEntries(items, now) != 1 || s.applyFeedEntries(items, now) != 0 {
		t.Fatal("feed duplicates counted as new hints")
	}
	h := items[0].hash
	r := d.records[h]
	if r.Resolved || r.Name != "" || r.NameHint != "Example fresh episode.mkv" {
		t.Fatalf("hint claimed verification: %+v", r)
	}
	got := d.snapshotForAPI(viewQuery{Q: "fresh episode", Source: "feed", Limit: 10})
	if len(got.Records) != 1 || got.VerifiedNamesPerMinute != 0 || got.LiveNamesPerMinute != 0 {
		t.Fatalf("unsearchable or inflated hint: %+v", got)
	}
	if _, ok := d.pendingSet[h]; !ok {
		t.Fatal("provisional name skipped network verification")
	}
	d.mu.Lock()
	d.applyNameLocked(namePatch{Hash: h, Name: "Confirmed title.mkv", Source: "peer"}, now)
	d.mu.Unlock()
	r = d.records[h]
	if !r.Resolved || r.NameHint != "" || r.Name != "Confirmed title.mkv" || d.snapshot().FeedHints != 0 {
		t.Fatalf("verification did not replace hint: %+v", r)
	}
	if d.snapshot().VerifiedNamesPerMinute != 1 {
		t.Fatal("verified result not counted once")
	}
	d.persist()
	restored := newDashboard(d.cachePath, nil)
	if !restored.records[h].Resolved {
		t.Fatal("verified metadata lost on restart")
	}
}

func TestFeedConditionalRefreshAndMalformedResponse(t *testing.T) {
	requests := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		requests++
		if requests == 2 {
			if r.Header.Get("If-None-Match") != "feed-v1" {
				t.Error("missing conditional refresh")
			}
			w.WriteHeader(304)
			return
		}
		w.Header().Set("ETag", "feed-v1")
		_, _ = w.Write([]byte(`<feed><entry><title>Atom title</title><link href="magnet:?xt=urn:btih:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"/></entry></feed>`))
	}))
	defer server.Close()
	cursor := feedCursor{url: server.URL}
	items, err := fetchNamedFeed(context.Background(), server.Client(), &cursor)
	if err != nil || len(items) != 1 {
		t.Fatalf("initial refresh: %v %+v", err, items)
	}
	items, err = fetchNamedFeed(context.Background(), server.Client(), &cursor)
	if err != nil || len(items) != 0 {
		t.Fatal("304 counted as more names")
	}
	if _, err := parseNamedFeed(strings.NewReader(`<rss><item>`)); err == nil {
		t.Fatal("accepted malformed feed")
	}
}

func TestPublisherMetadataMustMatchFeedHash(t *testing.T) {
	blob, hash := torrentFixture(t, false)
	server := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { _, _ = w.Write(blob) }))
	defer server.Close()
	r := newTrackerResolver()
	defer r.close()
	r.http = server.Client()
	r.hints[hash.HexString()] = trackerHint{metadataURL: server.URL, at: time.Now()}
	if info := r.fetchMetadata(context.Background(), hash); info == nil || info.BestName() != "Example episode.mkv" {
		t.Fatal("publisher metadata was not verified")
	}
	wrong := metainfo.Hash(hash)
	wrong[0] ^= 1
	r.hints[wrong.HexString()] = trackerHint{metadataURL: server.URL, at: time.Now()}
	if r.fetchMetadata(context.Background(), wrong) != nil {
		t.Fatal("accepted a publisher's mismatched name")
	}
	stats := r.metadataMeter.snapshot("publisher")
	if stats.Hits != 1 || stats.Failures != 1 {
		t.Fatalf("incorrect verified accounting: %+v", stats)
	}
}

func TestNamespacedFeedHashAndMetadataLink(t *testing.T) {
	items, err := parseNamedFeed(strings.NewReader(`<rss xmlns:nyaa="https://nyaa.si/xmlns/nyaa"><channel><item><title>New movie.mkv</title><link>https://nyaa.si/download/123.torrent</link><nyaa:infoHash>AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA</nyaa:infoHash></item></channel></rss>`))
	if err != nil || len(items) != 1 || items[0].metadataURL != "https://nyaa.si/download/123.torrent" || items[0].hash != strings.Repeat("a", 40) {
		t.Fatalf("namespaced feed: %v %+v", err, items)
	}
}
