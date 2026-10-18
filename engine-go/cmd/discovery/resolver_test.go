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
	hash[0] ^= 1 // Another hash must still reach this healthy source.
	c.fetch(context.Background(), hash)
	if calls != 2 {
		t.Fatal("ordinary miss suppressed healthy source")
	}
	status = http.StatusTooManyRequests
	hash[0] ^= 2
	c.fetch(context.Background(), hash)
	c.fetch(context.Background(), hash)
	if calls != 3 {
		t.Fatal("rate limited source was hammered")
	}
}

func TestHTMLCacheBodyBacksOffWithoutCachingFalseAbsence(t *testing.T) {
	calls := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls++
		_, _ = w.Write([]byte("<!DOCTYPE html><html><body>challenge</body></html>"))
	}))
	defer server.Close()
	c := newMetadataCaches([]string{server.URL + "/{hash}"})
	defer c.close()
	_, hash := torrentFixture(t, false)
	if c.fetch(context.Background(), hash) != nil {
		t.Fatal("HTML body accepted as metadata")
	}
	if c.fetch(context.Background(), hash) != nil {
		t.Fatal("HTML body accepted on retry")
	}
	if calls != 1 || c.knownMissing(hash) {
		t.Fatal("challenge was retried immediately or stored as a confirmed missing hash")
	}
	c.sources[0].mu.Lock()
	c.sources[0].next = time.Now().Add(-time.Second)
	c.sources[0].mu.Unlock()
	c.fetch(context.Background(), hash)
	stats := c.sourceStats()[0]
	if calls != 2 || stats.Failures != 2 || stats.Misses != 0 {
		t.Fatalf("recovery probe/accounting failed: %+v", stats)
	}
}

func TestCacheWorkerResolvesWithoutPeerPath(t *testing.T) {
	blob, hash := torrentFixture(t, false)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { _, _ = w.Write(blob) }))
	defer server.Close()
	d := testDashboard(t)
	h := hash.HexString()
	d.records[h] = observation{Hash: h, FirstSeen: time.Now(), LastSeen: time.Now()}
	s := &sampler{d: d, caches: newMetadataCaches([]string{server.URL + "/{hash}"}), cacheTried: map[string]bool{}}
	defer s.caches.close()
	start := time.Now()
	s.resolve(context.Background(), discoveryJob{hash: h, kind: jobCache})
	if time.Since(start) > time.Second {
		t.Fatal("cache worker waited on peer path")
	}
	snap := d.snapshot()
	if snap.Successes != 1 || snap.CacheHits != 1 || snap.Resolved != 1 {
		t.Fatalf("cache worker missed the name: %+v", snap)
	}
	if !s.cacheTried[h] {
		t.Fatal("cache attempt was not recorded")
	}
}

func TestRetryRunsWithoutNewSampleAndNeverDuplicatesActiveHash(t *testing.T) {
	d := testDashboard(t)
	h := strings.Repeat("e", 40)
	now := time.Now()
	d.records[h] = observation{Hash: h, FirstSeen: now.Add(-time.Hour), LastSeen: now, ResolveAttempts: 1, NextAttempt: now.Add(-time.Second)}
	d.pendingSet[h] = struct{}{}
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

func TestResolvedHashesAreNeverRescheduled(t *testing.T) {
	d := testDashboard(t)
	now := time.Now()
	named := strings.Repeat("c", 40)
	pending := strings.Repeat("d", 40)
	d.records[named] = observation{
		Hash: named, Name: "Already named.mkv", Resolved: true, Source: "catalog",
		Peers: 40, Seeds: intPtr(30), CheckedAt: now.Add(-time.Hour),
		NextAttempt: now.Add(-time.Second), FirstSeen: now.Add(-2 * time.Hour), LastSeen: now,
	}
	d.records[pending] = observation{Hash: pending, FirstSeen: now.Add(-time.Minute), LastSeen: now}
	d.pendingSet[pending] = struct{}{}
	s := &sampler{d: d, cacheTried: map[string]bool{}}
	job, ok := s.nextJobOf(now, jobCache)
	if !ok || job.hash != pending {
		t.Fatalf("cache workers should only take unresolved hashes, got %#v ok=%v", job, ok)
	}
	s.releaseJob(job.hash)
	job, ok = s.nextJobOf(now, jobPeer)
	if ok {
		t.Fatalf("peer workers must not recheck named torrents, got %s", job.hash)
	}
}

func intPtr(n int) *int { return &n }

func TestRetryReservationSurvivesEarlyFailureAndFreshWorkCannotStarveRetries(t *testing.T) {
	d := testDashboard(t)
	now := time.Now()
	retryHash := strings.Repeat("a", 40)
	freshHash := strings.Repeat("b", 40)
	d.records[retryHash] = observation{Hash: retryHash, ResolveAttempts: 1, FirstSeen: now.Add(-time.Hour), NextAttempt: now.Add(-time.Second)}
	d.records[freshHash] = observation{Hash: freshHash, FirstSeen: now.Add(-time.Minute)}
	d.pendingSet[retryHash] = struct{}{}
	d.pendingSet[freshHash] = struct{}{}
	s := &sampler{d: d, cacheTried: map[string]bool{}}
	j, ok := s.nextJobOf(now, jobPeer)
	if !ok || j.hash != retryHash {
		t.Fatal("new samples starved a due retry")
	}
	s.releaseJob(j.hash) // Simulate network/client failure before publishing.
	j, ok = s.nextJobOf(now, jobCache)
	if !ok || j.hash != freshHash {
		t.Fatal("early failure caused immediate retry instead of fresh work")
	}
	s.releaseJob(j.hash)
	if _, ok := s.nextJobOf(now, jobPeer); ok {
		t.Fatal("failed jobs ignored their reserved backoff")
	}
	if d.state.Active != 0 {
		t.Fatal("active job slot leaked")
	}
}

func TestRetentionKeepsReachableUnresolvedSwarmsForRetry(t *testing.T) {
	d := testDashboard(t)
	for i := 0; i < 1000; i++ {
		h := fmt.Sprintf("%040x", i)
		d.records[h] = observation{Hash: h, LastSeen: time.Now()}
	}
	h := strings.Repeat("d", 40)
	d.records[h] = observation{Hash: h, Peers: 3, LastSeen: time.Now().Add(-time.Hour)}
	d.pruneLocked()
	if _, ok := d.records[h]; !ok {
		t.Fatal("reachable pending torrent was dropped")
	}
}

func TestResolvePriorityPrefersSeedsPeersAndRepeatedSamples(t *testing.T) {
	now := time.Now()
	low := observation{Hash: strings.Repeat("1", 40), FirstSeen: now.Add(-time.Hour), SampleHits: 1}
	high := observation{Hash: strings.Repeat("2", 40), FirstSeen: now.Add(-time.Minute), SampleHits: 5, Peers: 20}
	seeds := 50
	hot := observation{Hash: strings.Repeat("3", 40), FirstSeen: now.Add(-time.Minute), SampleHits: 2, Peers: 5, Seeds: &seeds}
	if !betterResolveCandidate(high, low, now) {
		t.Fatal("repeated/peer-rich hash should outrank a quiet sample")
	}
	if !betterResolveCandidate(hot, high, now) {
		t.Fatal("seed-rich hash should outrank peer-only signal")
	}
}

func TestRecentlyReachableSwarmOutranksOverdueEmptyLookup(t *testing.T) {
	now := time.Now()
	stale := observation{Hash: strings.Repeat("1", 40), ResolveAttempts: 12,
		FirstSeen: now.Add(-time.Hour), LastSeen: now.Add(-time.Hour),
		CheckedAt: now.Add(-time.Hour), NextAttempt: now.Add(-time.Hour)}
	reachable := observation{Hash: strings.Repeat("2", 40), ResolveAttempts: 1,
		FirstSeen: now.Add(-time.Minute), LastSeen: now, CheckedAt: now.Add(-time.Second),
		NextAttempt: now.Add(-time.Second), Peers: 2, SampleHits: 3}
	if !betterResolveCandidate(reachable, stale, now) {
		t.Fatal("old peerless retries outrank a recently reachable swarm")
	}
}

func TestCacheMissWakesEmptyPeerBatchWithoutDoublingRetryAttempt(t *testing.T) {
	d := testDashboard(t)
	hash := strings.Repeat("ed", 20)
	d.upsertSampleLocked(hash, time.Now())
	cache := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusNotFound)
	}))
	defer cache.Close()
	s := &sampler{d: d, caches: newMetadataCaches([]string{cache.URL + "/{hash}"})}
	defer s.caches.close()
	if _, ok := s.nextJobOf(time.Now(), jobPeer); ok {
		t.Fatal("untested hash bypassed the cache pool")
	}
	job, ok := s.nextJobOf(time.Now(), jobCache)
	if !ok {
		t.Fatal("cache job missing")
	}
	s.resolve(context.Background(), job)
	s.releaseJob(hash)
	job, ok = s.nextJobOf(time.Now(), jobPeer)
	if !ok || job.hash != hash {
		t.Fatal("cache handoff waited for another backlog scan")
	}
	if d.records[hash].ResolveAttempts != 1 {
		t.Fatal("one cache/peer attempt counted twice, doubling retry backoff")
	}
}

func TestDispatchSeesNewSamplesAfterBoundedRefresh(t *testing.T) {
	d := testDashboard(t)
	s := &sampler{d: d}
	now := time.Now()
	if _, ok := s.nextJobOf(now, jobCache); ok {
		t.Fatal("empty queue returned a job")
	}
	hash := strings.Repeat("cb", 20)
	d.upsertSampleLocked(hash, now)
	job, ok := s.nextJobOf(now.Add(dispatchRefresh), jobCache)
	if !ok || job.hash != hash {
		t.Fatal("new sample was invisible after queue refresh")
	}
}

func TestCoolingMirrorsDoNotConsumeAttemptsOrCreatePeerHandoffs(t *testing.T) {
	d := testDashboard(t)
	hash := strings.Repeat("ac", 20)
	now := time.Now()
	d.upsertSampleLocked(hash, now)
	caches := newMetadataCaches([]string{"https://example.invalid/{hash}"})
	defer caches.close()
	caches.sources[0].next = now.Add(time.Minute)
	s := &sampler{d: d, caches: caches}
	if _, ok := s.nextJobOf(now, jobCache); ok {
		t.Fatal("scheduled cache work although no mirror can accept a request")
	}
	if d.records[hash].ResolveAttempts != 0 || d.state.Active != 0 {
		t.Fatal("source cooldown consumed a hash retry attempt")
	}
}

func TestMirrorCooldownAfterDispatchIsNotCountedAsNetworkMiss(t *testing.T) {
	d := testDashboard(t)
	hash := strings.Repeat("ad", 20)
	d.upsertSampleLocked(hash, time.Now())
	caches := newMetadataCaches([]string{"https://example.invalid/{hash}"})
	defer caches.close()
	s := &sampler{d: d, caches: caches}
	job, ok := s.nextJobOf(time.Now(), jobCache)
	if !ok {
		t.Fatal("ready cache job missing")
	}
	caches.sources[0].next = time.Now().Add(time.Minute)
	s.resolve(context.Background(), job)
	s.releaseJob(hash)
	if d.state.Completed != 0 || d.records[hash].ResolveAttempts != 0 || s.cacheTried[hash] {
		t.Fatal("cooldown was counted as a network lookup or hash failure")
	}
	if _, ok := s.nextJobOf(time.Now(), jobPeer); ok {
		t.Fatal("cooldown filled the peer backlog without a cache request")
	}
}

func TestRepeatedSamplesReceiveNameLookupPriority(t *testing.T) {
	d := testDashboard(t)
	now := time.Now()
	single, repeated := strings.Repeat("1", 40), strings.Repeat("2", 40)
	d.records[single] = observation{Hash: single, FirstSeen: now.Add(-time.Hour), SampleHits: 1}
	d.records[repeated] = observation{Hash: repeated, FirstSeen: now.Add(-time.Minute), SampleHits: 3}
	d.pendingSet[single] = struct{}{}
	d.pendingSet[repeated] = struct{}{}
	s := &sampler{d: d, cacheTried: map[string]bool{}}
	job, ok := s.nextJobOf(now, jobCache)
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
