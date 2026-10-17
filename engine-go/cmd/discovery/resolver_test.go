package main

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/anacrolix/torrent/bencode"
	"github.com/anacrolix/torrent/metainfo"
)

func torrentFixture(t *testing.T, private bool) ([]byte, metainfo.Hash) {
	t.Helper()
	info := metainfo.Info{Name: "Example episode.mkv", Length: 42, PieceLength: 16384, Pieces: []byte(strings.Repeat("x", 20))}
	if private {
		info.Private = &private
	}
	raw, err := bencode.Marshal(info)
	if err != nil {
		t.Fatal(err)
	}
	mi := metainfo.MetaInfo{InfoBytes: raw}
	var buf bytes.Buffer
	if err := mi.Write(&buf); err != nil {
		t.Fatal(err)
	}
	return buf.Bytes(), mi.HashInfoBytes()
}

func TestCacheMetadataRequiresMatchingHashAndPublicTorrent(t *testing.T) {
	blob, hash := torrentFixture(t, false)
	got, err := parseCacheMetadata(blob, hash)
	if err != nil || got.BestName() != "Example episode.mkv" {
		t.Fatalf("valid metadata: %v", err)
	}
	hash[0] ^= 1
	if _, err := parseCacheMetadata(blob, hash); err == nil {
		t.Fatal("accepted another torrent's name")
	}
	blob, hash = torrentFixture(t, true)
	if _, err := parseCacheMetadata(blob, hash); err == nil {
		t.Fatal("accepted private torrent")
	}
	if _, err := parseCacheMetadata([]byte("<html>cloud challenge</html>"), hash); err == nil {
		t.Fatal("accepted HTML")
	}
}

func TestCacheRaceDoesNotWaitForSlowMirrorAndCancelsIt(t *testing.T) {
	blob, hash := torrentFixture(t, false)
	started, cancelled := make(chan struct{}), make(chan struct{})
	slow := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		close(started)
		<-r.Context().Done()
		close(cancelled)
	}))
	defer slow.Close()
	fast := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		<-started
		_, _ = w.Write(blob)
	}))
	defer fast.Close()
	c := newMetadataCaches([]string{slow.URL + "/{hash}", fast.URL + "/{hash}"})
	defer c.close()
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	begin := time.Now()
	if got := c.fetch(ctx, hash); got == nil {
		t.Fatal("fast valid mirror did not win")
	}
	if time.Since(begin) > time.Second {
		t.Fatal("waited for stalled mirror")
	}
	select {
	case <-cancelled:
	case <-ctx.Done():
		t.Fatal("losing request leaked")
	}
}

func TestCacheMissDoesNotDisableSourceAndOutageBacksOff(t *testing.T) {
	status, calls := http.StatusNotFound, 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { calls++; w.WriteHeader(status) }))
	defer server.Close()
	c := newMetadataCaches([]string{server.URL + "/{hash}"})
	defer c.close()
	_, hash := torrentFixture(t, false)
	c.fetch(context.Background(), hash)
	c.fetch(context.Background(), hash)
	if calls != 2 {
		t.Fatal("ordinary miss suppressed healthy source")
	}
	status = http.StatusTooManyRequests
	c.fetch(context.Background(), hash)
	c.fetch(context.Background(), hash)
	if calls != 3 {
		t.Fatal("rate limited source was hammered")
	}
}

func TestRetryRunsWithoutNewSampleAndNeverDuplicatesActiveHash(t *testing.T) {
	d := testDashboard(t)
	h := strings.Repeat("e", 40)
	now := time.Now()
	d.records[h] = observation{Hash: h, FirstSeen: now.Add(-time.Hour), LastSeen: now, ResolveAttempts: 1, NextAttempt: now.Add(-time.Second)}
	s := &sampler{d: d, inflight: map[string]bool{}}
	j, ok := s.nextJob(now)
	if !ok || j.hash != h {
		t.Fatal("due retry needed another announcement")
	}
	if _, ok := s.nextJob(now); ok {
		t.Fatal("active hash scheduled twice")
	}
	s.releaseJob(h)
	d.records[h] = observation{Hash: h, NextAttempt: now.Add(time.Hour)}
	if _, ok := s.nextJob(now); ok {
		t.Fatal("retry backoff ignored")
	}
}

func TestRetryReservationSurvivesEarlyFailureAndFreshWorkCannotStarveRetries(t *testing.T) {
	d := testDashboard(t)
	now := time.Now()
	retryHash := strings.Repeat("a", 40)
	freshHash := strings.Repeat("b", 40)
	d.records[retryHash] = observation{Hash: retryHash, ResolveAttempts: 1, FirstSeen: now.Add(-time.Hour), NextAttempt: now.Add(-time.Second)}
	d.records[freshHash] = observation{Hash: freshHash, FirstSeen: now.Add(-time.Minute)}
	s := &sampler{d: d}
	j, ok := s.nextJob(now)
	if !ok || j.hash != retryHash {
		t.Fatal("new samples starved a due retry")
	}
	s.releaseJob(j.hash) // Simulate network/client failure before publishing.
	j, ok = s.nextJob(now)
	if !ok || j.hash != freshHash {
		t.Fatal("early failure caused immediate retry instead of fresh work")
	}
	s.releaseJob(j.hash)
	if _, ok := s.nextJob(now); ok {
		t.Fatal("failed jobs ignored their reserved backoff")
	}
	if d.state.Active != 0 {
		t.Fatal("active job slot leaked")
	}
}

func TestRetentionKeepsReachableUnresolvedSwarmsForRetry(t *testing.T) {
	d := testDashboard(t)
	for i := 0; i < retainedTorrentLimit; i++ {
		h := strings.Repeat("0", 36) + fmt.Sprintf("%04x", i)
		d.records[h] = observation{Hash: h, LastSeen: time.Now()}
	}
	h := strings.Repeat("d", 40)
	d.records[h] = observation{Hash: h, Peers: 3, LastSeen: time.Now().Add(-time.Hour)}
	d.pruneLocked()
	if _, ok := d.records[h]; !ok {
		t.Fatal("new sample evicted a reachable pending torrent before its retry")
	}
}

func TestRepeatedSamplesReceiveNameLookupPriority(t *testing.T) {
	d := testDashboard(t)
	now := time.Now()
	single, repeated := strings.Repeat("1", 40), strings.Repeat("2", 40)
	d.records[single] = observation{Hash: single, FirstSeen: now.Add(-time.Hour), SampleHits: 1}
	d.records[repeated] = observation{Hash: repeated, FirstSeen: now.Add(-time.Minute), SampleHits: 3}
	s := &sampler{d: d}
	job, ok := s.nextJob(now)
	if !ok || job.hash != repeated {
		t.Fatal("repeatedly observed candidate lost priority to an older single sample")
	}
	s.releaseJob(job.hash)
}

func TestCacheRejectsOversizedBodiesAndDishonestLengths(t *testing.T) {
	_, hash := torrentFixture(t, false)
	if _, err := parseCacheMetadata([]byte("d4:info999999999999999999999:"), hash); err == nil {
		t.Fatal("accepted malicious bencode length")
	}
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Chunked encoding forces the streaming limit instead of Content-Length.
		w.(http.Flusher).Flush()
		_, _ = io.CopyN(w, strings.NewReader(strings.Repeat("x", maxMetadataBytes+100)), maxMetadataBytes+100)
	}))
	defer server.Close()
	c := newMetadataCaches([]string{server.URL + "/{hash}"})
	defer c.close()
	if c.fetch(context.Background(), hash) != nil {
		t.Fatal("oversized response accepted")
	}
}
