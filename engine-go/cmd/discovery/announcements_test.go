package main

import (
	"net"
	"strings"
	"testing"
	"time"

	"github.com/anacrolix/dht/v2/krpc"
)

func TestAnnouncementFastTracksPublicEndpointWithoutInventingName(t *testing.T) {
	d := testDashboard(t)
	s := &sampler{d: d, hints: &peerHintCache{}, inflight: map[string]bool{}}
	h := strings.Repeat("b", 40)
	now := time.Now()
	s.acceptAnnouncement(announcedPeer{hash: h, peer: krpc.NodeAddr{IP: net.ParseIP("8.8.8.8"), Port: 6881}}, now)
	job, ok := s.nextJobOf(now, jobPeer)
	if !ok || job.hash != h {
		t.Fatal("live endpoint waited for cache miss")
	}
	if len(s.hints.get(h, now)) != 1 || d.records[h].Resolved || d.snapshot().Announcements != 1 {
		t.Fatal("bad announcement bookkeeping")
	}
	s.acceptAnnouncement(announcedPeer{hash: strings.Repeat("c", 40), peer: krpc.NodeAddr{IP: net.ParseIP("127.0.0.1"), Port: 6881}}, now)
	if d.snapshot().Announcements != 1 {
		t.Fatal("local endpoint accepted")
	}
}
