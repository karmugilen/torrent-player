package main

import (
	"context"
	"encoding/binary"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/anacrolix/torrent/metainfo"
)

func TestQueuedCacheRequestKeepsItsNetworkBudget(t *testing.T) {
	blob, hash := torrentFixture(t, false)
	started := make(chan struct{}, cacheSourceSlots+1)
	release := make(chan struct{})
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		started <- struct{}{}
		<-release
		_, _ = w.Write(blob)
	}))
	defer server.Close()
	caches := newMetadataCaches([]string{server.URL + "/{hash}"})
	defer caches.close()
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	var work sync.WaitGroup
	for i := 0; i < cacheSourceSlots; i++ {
		work.Add(1)
		go func() { defer work.Done(); caches.fetchWithin(ctx, hash, 2*time.Second) }()
	}
	for i := 0; i < cacheSourceSlots; i++ {
		select {
		case <-started:
		case <-ctx.Done():
			close(release)
			work.Wait()
			t.Fatal("initial requests did not fill source slots")
		}
	}
	result := make(chan *metainfo.Info, 1)
	go func() { result <- caches.fetchWithin(ctx, hash, 75*time.Millisecond) }()
	time.Sleep(100 * time.Millisecond)
	close(release)
	work.Wait()
	if <-result == nil {
		t.Fatal("request spent its network deadline waiting for an occupied slot")
	}
}

func TestConfirmedMissIsReusedWithoutCountingAnotherNetworkRequest(t *testing.T) {
	var requests atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		requests.Add(1)
		w.WriteHeader(http.StatusNotFound)
	}))
	defer server.Close()
	caches := newMetadataCaches([]string{server.URL + "/{hash}"})
	defer caches.close()
	_, hash := torrentFixture(t, false)
	caches.fetch(context.Background(), hash)
	_, attempted := caches.fetchAttempt(context.Background(), hash, time.Second)
	if requests.Load() != 1 || attempted {
		t.Fatal("repeat confirmed miss made or counted another network request")
	}
}

func TestCachedMissHandsOffToPeersWithoutInflatingLookupRate(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusNotFound)
	}))
	defer server.Close()
	caches := newMetadataCaches([]string{server.URL + "/{hash}"})
	defer caches.close()
	d := testDashboard(t)
	hash := strings.Repeat("ab", 20)
	now := time.Now()
	d.upsertSampleLocked(hash, now)
	s := &sampler{d: d, caches: caches}
	job, ok := s.nextJobOf(now, jobCache)
	if !ok {
		t.Fatal("cache job missing")
	}
	s.resolve(context.Background(), job)
	s.releaseJob(hash)
	s.clearCacheTried(hash) // Model a completed first peer pass.
	rec := d.records[hash]
	rec.NextAttempt = now
	d.records[hash] = rec
	job, ok = s.nextJobOf(time.Now().Add(dispatchRefresh), jobCache)
	if !ok {
		t.Fatal("cache retry missing")
	}
	s.resolve(context.Background(), job)
	s.releaseJob(hash)
	if d.state.Completed != 1 || caches.requests.Load() != 1 || caches.missReuses.Load() != 1 {
		t.Fatal("cached miss counted as a second network lookup")
	}
	if job, ok = s.nextJobOf(time.Now(), jobPeer); !ok || job.hash != hash {
		t.Fatal("cached miss did not reach the peer resolver")
	}
}

// Fixed-latency local mirrors isolate HTTP capacity from public-network noise.
func BenchmarkCacheLookupBatch(b *testing.B) {
	var templates []string
	for i := 0; i < 3; i++ {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			time.Sleep(40 * time.Millisecond)
			w.WriteHeader(http.StatusNotFound)
		}))
		b.Cleanup(server.Close)
		templates = append(templates, server.URL+"/{hash}")
	}
	caches := newMetadataCaches(templates)
	b.Cleanup(caches.close)
	b.ResetTimer()
	for round := 0; round < b.N; round++ {
		var work sync.WaitGroup
		for i := 0; i < 96; i++ {
			var hash metainfo.Hash
			binary.BigEndian.PutUint64(hash[12:], uint64(round*96+i))
			work.Add(1)
			go func() { defer work.Done(); caches.fetch(context.Background(), hash) }()
		}
		work.Wait()
	}
}
