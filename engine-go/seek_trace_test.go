package main

import (
	"fmt"
	"net"
	"os"
	"sort"
	"sync"
	"testing"
	"time"

	"github.com/anacrolix/torrent"
	pp "github.com/anacrolix/torrent/peer_protocol"
)

// TestSeekTrace logs, for a few cold seeks, when each block of the read was
// requested and arrived and from which seed (its delay and rate), with the
// estimates peer choice saw, and any chokes, rejects and closes. Run with
// SEEK_SEARCH=trace.
func TestSeekTrace(t *testing.T) {
	if os.Getenv("SEEK_SEARCH") != "trace" {
		t.Skip("set SEEK_SEARCH=trace to trace seeks block by block")
	}
	type ev struct {
		at    time.Time
		what  string
		index int
		begin int
		peer  string
	}
	var mu sync.Mutex
	var evs []ev
	port := func(p *torrent.Peer) string {
		_, port, _ := net.SplitHostPort(p.RemoteAddr.String())
		return port
	}
	seekLeechConfig = func(cfg *torrent.ClientConfig) {
		cfg.Callbacks.SentRequest = append(cfg.Callbacks.SentRequest, func(e torrent.PeerRequestEvent) {
			mu.Lock()
			rtt, rate, q, c := e.Peer.WebtorEstimates()
			evs = append(evs, ev{time.Now(), fmt.Sprintf("req (rtt %v rate %dK q %d c %d)", rtt.Round(time.Millisecond), int(rate)>>10, q, c), int(e.Index), int(e.Begin), port(e.Peer)})
			mu.Unlock()
		})
		cfg.Callbacks.ReadMessage = func(c *torrent.PeerConn, m *pp.Message) {
			var what string
			switch m.Type {
			case pp.Choke:
				what = "CHOKED"
			case pp.Unchoke:
				what = "unchoked"
			case pp.Reject:
				what = "REJECT"
			default:
				return
			}
			mu.Lock()
			evs = append(evs, ev{time.Now(), what, int(m.Index), int(m.Begin), port(&c.Peer)})
			mu.Unlock()
		}
		cfg.Callbacks.PeerConnClosed = func(c *torrent.PeerConn) {
			mu.Lock()
			evs = append(evs, ev{time.Now(), "CLOSED", -1, 0, port(&c.Peer)})
			mu.Unlock()
		}
		cfg.Callbacks.DeletedRequest = append(cfg.Callbacks.DeletedRequest, func(e torrent.PeerRequestEvent) {
			mu.Lock()
			evs = append(evs, ev{time.Now(), "del", int(e.Index), int(e.Begin), port(e.Peer)})
			mu.Unlock()
		})
		cfg.Callbacks.ReceivedUsefulData = append(cfg.Callbacks.ReceivedUsefulData, func(e torrent.PeerMessageEvent) {
			mu.Lock()
			evs = append(evs, ev{time.Now(), "got", int(e.Message.Index), int(e.Message.Begin), port(e.Peer)})
			mu.Unlock()
		})
	}
	defer func() { seekLeechConfig = nil }()
	const pieceLen = 256 << 10
	sw := newSeekSwarm(t, pieceLen, 192<<20, 16<<20, 16)
	ports := make([]string, 0, len(sw.peers))
	for p := range sw.peers {
		ports = append(ports, p)
	}
	sort.Strings(ports)
	for _, p := range ports {
		t.Logf("peer %s %s", p, sw.peers[p])
	}
	for i := 0; i < 12; i++ {
		mu.Lock()
		evs = nil
		mu.Unlock()
		start := time.Now()
		took := sw.seek()
		off := *sw.lastOff
		piece, blk := int(off/pieceLen), int(off%pieceLen/(16<<10))
		t.Logf("seek %d: piece %d block %d took %v", i, piece, blk, took.Round(time.Millisecond))
		mu.Lock()
		for _, e := range evs {
			if e.what == "CHOKED" || e.what == "unchoked" || e.what == "CLOSED" || (e.what == "REJECT" && took > 400*time.Millisecond) {
				t.Logf("   +%4dms %s %d/%d peer %s", e.at.Sub(start).Milliseconds(), e.what, e.index, e.begin/(16<<10), sw.peers[e.peer])
				continue
			}
			if e.index == piece && e.begin/(16<<10) >= blk && e.begin/(16<<10) < blk+5 {
				t.Logf("   +%4dms %s blk %d peer %s", e.at.Sub(start).Milliseconds(), e.what, e.begin/(16<<10), sw.peers[e.peer])
			}
		}
		mu.Unlock()
	}
}
