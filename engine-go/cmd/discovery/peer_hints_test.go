package main

import (
	"fmt"
	"net"
	"testing"
	"time"

	"github.com/anacrolix/dht/v2/krpc"
)

func TestPeerHintsExpireDeduplicateAndDoNotAliasAddresses(t *testing.T) {
	h := &peerHintCache{}
	now := time.Now()
	peer := krpc.NodeAddr{IP: net.ParseIP("8.8.8.8"), Port: 6881}
	h.put("hash", []krpc.NodeAddr{peer, peer, {IP: net.ParseIP("127.0.0.1"), Port: 80}}, now)
	peer.IP[0] ^= 1
	got := h.get("hash", now.Add(time.Second))
	if len(got) != 1 || got[0].IP.String() != "8.8.8.8" {
		t.Fatal("hints were duplicated, private, or aliased")
	}
	got[0].IP[0] ^= 1
	if h.get("hash", now)[0].IP.String() != "8.8.8.8" {
		t.Fatal("caller mutated hint cache")
	}
	if len(h.get("hash", now.Add(peerHintTTL))) != 0 {
		t.Fatal("stale peer address reused")
	}
}

func TestPeerHintsRemainBounded(t *testing.T) {
	h := &peerHintCache{}
	now := time.Now()
	var peers []krpc.NodeAddr
	for port := 1; port <= 50; port++ {
		peers = append(peers, krpc.NodeAddr{IP: net.ParseIP("8.8.8.8"), Port: port})
	}
	for i := 0; i < maxHintSwarms+100; i++ {
		h.put(fmt.Sprint(i), peers, now.Add(time.Duration(i)*time.Millisecond))
	}
	if len(h.entries) > maxHintSwarms {
		t.Fatal("peer hint memory is unbounded")
	}
	if len(h.get(fmt.Sprint(maxHintSwarms+99), now.Add(time.Minute))) != 24 {
		t.Fatal("unexpected peer hint bound")
	}
}
