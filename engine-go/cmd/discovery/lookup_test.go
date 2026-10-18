package main

import (
	"context"
	"net"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/anacrolix/dht/v2"
	"github.com/anacrolix/torrent"
)

func TestEmptyCompletedLookupReleasesResolverSlotPromptly(t *testing.T) {
	// A real local DHT node answers get_peers with no peers and no further
	// routes. No public network or torrent payload is involved in this check.
	makeServer := func(starting dht.StartingNodesGetter) *dht.Server {
		conn, err := net.ListenPacket("udp4", "127.0.0.1:0")
		if err != nil {
			t.Fatal(err)
		}
		cfg := dht.NewDefaultServerConfig()
		cfg.Conn, cfg.NoSecurity, cfg.StartingNodes = conn, true, starting
		cfg.QueryResendDelay = func() time.Duration { return 50 * time.Millisecond }
		server, err := dht.NewServer(cfg)
		if err != nil {
			conn.Close()
			t.Fatal(err)
		}
		t.Cleanup(server.Close)
		return server
	}
	remote := makeServer(nil)
	local := makeServer(func() ([]dht.Addr, error) { return []dht.Addr{dht.NewAddr(remote.Addr())}, nil })
	cache := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(http.StatusNotFound) }))
	defer cache.Close()
	tc := torrent.NewDefaultClientConfig()
	tc.NoDHT, tc.DisableTrackers, tc.DisableWebtorrent, tc.NoDefaultPortForwarding = true, true, true, true
	tc.DisableUTP, tc.DisableIPv6, tc.NoUpload = true, true, true
	tc.AcceptPeerConnections, tc.ListenPort, tc.DefaultStorage = false, 0, metadataStorage{}
	client, err := torrent.NewClient(tc)
	if err != nil {
		t.Fatal(err)
	}
	defer client.Close()
	d := testDashboard(t)
	hash := strings.Repeat("c", 40)
	d.records[hash] = observation{Hash: hash, FirstSeen: time.Now(), LastSeen: time.Now()}
	s := &sampler{d: d, server: local, client: client, caches: newMetadataCaches([]string{cache.URL + "/{hash}"})}
	defer s.caches.close()
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	start := time.Now()
	s.resolve(ctx, discoveryJob{hash: hash})
	if time.Since(start) > time.Second {
		t.Fatal("empty completed lookup wasted the resolver deadline")
	}
	if d.snapshot().Completed != 1 || d.snapshot().PayloadBytes != 0 {
		t.Fatal("empty lookup outcome was not recorded safely")
	}
}

func TestUnresponsiveDHTReleasesSlotWhileKeepingPayloadDisabled(t *testing.T) {
	dead, err := net.ListenPacket("udp4", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer dead.Close()
	conn, err := net.ListenPacket("udp4", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	cfg := dht.NewDefaultServerConfig()
	cfg.Conn, cfg.NoSecurity = conn, true
	cfg.StartingNodes = func() ([]dht.Addr, error) { return []dht.Addr{dht.NewAddr(dead.LocalAddr())}, nil }
	cfg.QueryResendDelay = func() time.Duration { return 10 * time.Second }
	server, err := dht.NewServer(cfg)
	if err != nil {
		conn.Close()
		t.Fatal(err)
	}
	defer server.Close()
	cache := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(http.StatusNotFound) }))
	defer cache.Close()
	tc := torrent.NewDefaultClientConfig()
	tc.NoDHT, tc.DisableTrackers, tc.DisableWebtorrent, tc.NoDefaultPortForwarding = true, true, true, true
	tc.DisableUTP, tc.DisableIPv6, tc.NoUpload = true, true, true
	tc.AcceptPeerConnections, tc.ListenPort, tc.DefaultStorage = false, 0, metadataStorage{}
	client, err := torrent.NewClient(tc)
	if err != nil {
		t.Fatal(err)
	}
	defer client.Close()
	d := testDashboard(t)
	hash := strings.Repeat("d", 40)
	d.upsertSampleLocked(hash, time.Now())
	s := &sampler{d: d, server: server, client: client, caches: newMetadataCaches([]string{cache.URL + "/{hash}"})}
	defer s.caches.close()
	ctx, cancel := context.WithTimeout(context.Background(), 6*time.Second)
	defer cancel()
	start := time.Now()
	s.resolve(ctx, discoveryJob{hash: hash})
	if time.Since(start) > 4*time.Second {
		t.Fatal("unresponsive swarm held a peer slot despite having no connections")
	}
	if d.state.Completed != 1 || d.state.PayloadBytes != 0 {
		t.Fatal("lookup did not complete safely")
	}
}
