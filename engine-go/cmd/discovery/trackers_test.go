package main

import (
	"context"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/anacrolix/dht/v2/krpc"
	"github.com/anacrolix/torrent/bencode"
	"github.com/anacrolix/torrent/metainfo"
)

func TestTrackerLookupDeduplicatesClaimsRespectsIntervalAndFiltersPeers(t *testing.T) {
	r := newTrackerResolver()
	defer r.close()
	var calls atomic.Int32
	r.announce = func(ctx context.Context, _ string, _ metainfo.Hash, _ int) ([]krpc.NodeAddr, time.Duration, error) {
		calls.Add(1)
		return []krpc.NodeAddr{{IP: net.ParseIP("8.8.8.8"), Port: 6881}, {IP: net.ParseIP("127.0.0.1"), Port: 6881}, {IP: net.ParseIP("8.8.8.8"), Port: 6881}}, 10 * time.Minute, nil
	}
	var hash metainfo.Hash
	hash[0] = 1
	r.remember(hash.HexString(), []string{"udp://tracker.example:6969/announce", "https://tracker.example/announce"}, time.Now())
	var wg sync.WaitGroup
	results := make(chan []krpc.NodeAddr, 2)
	for i := 0; i < 2; i++ {
		wg.Add(1)
		go func() { defer wg.Done(); results <- r.lookup(context.Background(), hash, 6881) }()
	}
	wg.Wait()
	close(results)
	var count int
	for peers := range results {
		count += len(peers)
	}
	if calls.Load() != 2 || count != 1 {
		t.Fatalf("duplicate announce/peer claims: calls=%d peers=%d", calls.Load(), count)
	}
	if len(r.lookup(context.Background(), hash, 6881)) != 0 || calls.Load() != 2 {
		t.Fatal("ignored tracker interval")
	}
	for _, raw := range []string{"http://127.0.0.1/announce", "udp://192.168.0.1:80/announce", "http://localhost/a", "file:///etc/passwd"} {
		if trackerURL(raw) != "" {
			t.Fatalf("unsafe tracker accepted: %s", raw)
		}
	}
	if _, err := publicTrackerAddress(context.Background(), "127.0.0.1:80"); err == nil {
		t.Fatal("DNS dial allowed local endpoint")
	}
}

func TestTrackerLookupCancellationJoinsWorkers(t *testing.T) {
	r := newTrackerResolver()
	defer r.close()
	started := make(chan struct{})
	r.announce = func(ctx context.Context, _ string, _ metainfo.Hash, _ int) ([]krpc.NodeAddr, time.Duration, error) {
		close(started)
		<-ctx.Done()
		return nil, 0, ctx.Err()
	}
	var hash metainfo.Hash
	hash[0] = 2
	r.remember(hash.HexString(), []string{"udp://tracker.example:6969/announce"}, time.Now())
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	done := make(chan struct{})
	go func() { r.lookup(ctx, hash, 6881); close(done) }()
	select {
	case <-started:
	case <-time.After(time.Second):
		t.Fatal("not started")
	}
	cancel()
	select {
	case <-done:
	case <-time.After(time.Second):
		t.Fatal("tracker work outlived cancellation")
	}
	if len(r.slots) != 0 {
		t.Fatal("tracker slot leaked")
	}
}

func TestHTTPTrackerCompactResponse(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if len(r.URL.Query().Get("info_hash")) != 20 || r.URL.Query().Get("compact") != "1" {
			t.Error("incorrect announce")
		}
		blob, _ := bencode.Marshal(map[string]any{"interval": 600, "peers": string([]byte{8, 8, 8, 8, 26, 225})})
		_, _ = w.Write(blob)
	}))
	defer server.Close()
	r := newTrackerResolver()
	defer r.close()
	r.http = server.Client()
	u, _ := url.Parse(server.URL)
	peers, interval, err := r.announceHTTP(context.Background(), u, metainfo.Hash{}, 6881)
	if err != nil || interval != 10*time.Minute || len(peers) != 1 || peers[0].Port != 6881 {
		t.Fatalf("response: %+v %v %v", peers, interval, err)
	}
}
