package main

import (
	"encoding/json"
	"net"
	"testing"
	"time"

	"github.com/anacrolix/torrent"
	"github.com/anacrolix/torrent/metainfo"
)

func TestPeerExplorationProtectsUsefulUploadingAndNewPeers(t *testing.T) {
	var policy peerExplorationState
	start := time.Unix(1000, 0)
	idle, downloading, uploading := &torrent.PeerConn{}, &torrent.PeerConn{}, &torrent.PeerConn{}
	for seconds := 0; seconds <= 90; seconds += 10 {
		peers := []peerExplorationObservation{
			{peer: idle},
			{peer: downloading, useful: int64(seconds) * 1024},
			{peer: uploading, uploaded: int64(seconds) * 1024},
		}
		chosen := policy.choose(start.Add(time.Duration(seconds)*time.Second), peers, 3, 10)
		if seconds < 90 && chosen != nil {
			t.Fatal("replaced a peer before the 90-second grace period")
		}
		if seconds == 90 && chosen != idle {
			t.Fatal("exploration must release only the long-idle peer")
		}
	}
}

func TestPeerExplorationLimitsChurnAndRequiresAlternatives(t *testing.T) {
	start := time.Unix(1000, 0)
	a, b, c := &torrent.PeerConn{}, &torrent.PeerConn{}, &torrent.PeerConn{}
	peers := []peerExplorationObservation{{peer: a}, {peer: b}, {peer: c}}
	var policy peerExplorationState
	policy.choose(start, peers, 3, 4)
	if policy.choose(start.Add(90*time.Second), peers, 3, 0) != nil {
		t.Fatal("cannot explore without a waiting alternative")
	}
	victim := policy.choose(start.Add(100*time.Second), peers, 3, 4)
	if victim == nil {
		t.Fatal("expected one idle slot to be released")
	}
	for i := range peers {
		if peers[i].peer == victim {
			peers[i].peer = &torrent.PeerConn{}
		}
	}
	if policy.choose(start.Add(110*time.Second), peers, 3, 4) != nil {
		t.Fatal("rotated more than one slot within 30 seconds")
	}
	if policy.choose(start.Add(130*time.Second), peers, 3, 4) == nil {
		t.Fatal("expected exploration after cooldown")
	}
	var spare, sole peerExplorationState
	spare.choose(start, peers, 5, 4)
	if spare.choose(start.Add(100*time.Second), peers, 5, 4) != nil {
		t.Fatal("must use free connection slots before replacing peers")
	}
	sole.choose(start, peers[:1], 1, 4)
	if sole.choose(start.Add(100*time.Second), peers[:1], 1, 4) != nil {
		t.Fatal("must preserve the only connected peer")
	}
}

func TestPeerExplorationSettingsAreIndependent(t *testing.T) {
	s := testEngine(t)
	request := func(method, body string) SettingsResponse {
		t.Helper()
		code, response := s.DispatchControl(method, "/settings", body)
		if code != 200 {
			t.Fatalf("settings: %d %s", code, response)
		}
		var settings SettingsResponse
		if err := json.Unmarshal(response, &settings); err != nil {
			t.Fatal(err)
		}
		return settings
	}
	if request("GET", "").PeerExploration {
		t.Fatal("peer exploration must default off")
	}
	if !request("POST", `{"peerExploration":true}`).PeerExploration || !request("GET", "").PeerExploration {
		t.Fatal("peer exploration was not enabled")
	}
	if !request("POST", `{"maxPeers":100}`).PeerExploration {
		t.Fatal("an unrelated setting disabled peer exploration")
	}
	if request("POST", `{"peerExploration":false}`).PeerExploration {
		t.Fatal("peer exploration was not disabled")
	}
}

func TestPeerExplorationProtectsTheOnlyKnownSeed(t *testing.T) {
	var policy peerExplorationState
	start := time.Unix(1000, 0)
	seed, idle := &torrent.PeerConn{}, &torrent.PeerConn{}
	peers := []peerExplorationObservation{{peer: seed, protected: true}, {peer: idle}}
	policy.choose(start, peers, 2, 10)
	if victim := policy.choose(start.Add(90*time.Second), peers, 2, 10); victim != idle {
		t.Fatal("must retain the sole known seed")
	}
}

func TestPeerExplorationReleasesOneRealIdleConnection(t *testing.T) {
	newPeer := func() (*EngineServer, *TorrentRecord) {
		t.Helper()
		s := testEngine(t, func(cfg *torrent.ClientConfig) {
			cfg.DisableTCP = false
			cfg.DisableIPv6 = true
			cfg.ListenHost = func(string) string { return "127.0.0.1" }
			cfg.ListenPort = 0
			cfg.DialForPeerConns, cfg.AcceptPeerConnections, cfg.AlwaysWantConns = true, true, true
			cfg.EstablishedConnsPerTorrent = 2
		})
		mi, err := metainfo.LoadFromFile("testdata/tiny.torrent")
		if err != nil {
			t.Fatal(err)
		}
		tr, err := s.client.AddTorrent(mi)
		if err != nil {
			t.Fatal(err)
		}
		hash := tr.InfoHash().HexString()
		rec := &TorrentRecord{ID: hash, InfoHash: hash, Torrent: tr, Configured: true, DirectPlay: true, Selected: []int{0}}
		s.records[hash], s.recordsByHash[hash], s.maxPeers = rec, rec, 2
		st := s.storage.GetStorage(hash)
		if err := st.AttachDescriptorsMode(make([]*int, len(tr.Files())), []int{0}, true); err != nil {
			t.Fatal(err)
		}
		// Demand allows connections, but none of these peers has payload to
		// offer, so they remain genuinely idle throughout the test.
		tr.Files()[0].Download()
		return s, rec
	}
	s, rec := newPeer()
	first, _ := newPeer()
	second, _ := newPeer()
	rec.Torrent.AddClientPeer(first.client)
	rec.Torrent.AddClientPeer(second.client)
	deadline := time.Now().Add(3 * time.Second)
	for len(rec.Torrent.PeerConns()) < 2 && time.Now().Before(deadline) {
		time.Sleep(time.Millisecond)
	}
	original := rec.Torrent.PeerConns()
	if len(original) != 2 {
		t.Fatal("two local peers did not connect")
	}
	rec.Torrent.AddPeers([]torrent.PeerInfo{{Addr: &net.TCPAddr{IP: net.ParseIP("127.0.0.1"), Port: 1}, Source: torrent.PeerSourceDirect}})
	if rec.Torrent.Stats().PendingPeers == 0 {
		t.Fatal("test requires a pending alternative")
	}
	start := time.Unix(1000, 0)
	s.peerExplorationTick(start)
	s.peerExplorationTick(start.Add(90 * time.Second))
	if len(rec.Torrent.PeerConns()) != 2 {
		t.Fatal("disabled exploration replaced a connection")
	}
	if code, body := s.DispatchControl("POST", "/settings", `{"peerExploration":true}`); code != 200 {
		t.Fatalf("enable: %d %s", code, body)
	}
	s.peerExplorationTick(start)
	s.peerExplorationTick(start.Add(90 * time.Second))
	retained := 2
	deadline = time.Now().Add(time.Second)
	for time.Now().Before(deadline) {
		retained = 0
		for _, connected := range rec.Torrent.PeerConns() {
			for _, previous := range original {
				if connected == previous {
					retained++
				}
			}
		}
		if retained == 1 {
			break
		}
		time.Sleep(time.Millisecond)
	}
	if retained != 1 {
		t.Fatalf("must release exactly one original idle connection, retained %d", retained)
	}
}
