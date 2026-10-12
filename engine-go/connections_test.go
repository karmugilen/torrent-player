package main

import (
	"encoding/json"
	"net"
	"os"
	"testing"
	"time"

	"github.com/anacrolix/torrent"
	"github.com/anacrolix/torrent/metainfo"
	maxminddb "github.com/oschwald/maxminddb-golang"
)

func connectionSnapshot(t *testing.T, s *EngineServer, id string) ConnectionsResponse {
	t.Helper()
	code, body := s.DispatchControl("GET", "/connections/"+id, "")
	if code != 200 {
		t.Fatalf("connections status %d: %s", code, body)
	}
	var view ConnectionsResponse
	if err := json.Unmarshal(body, &view); err != nil {
		t.Fatal(err)
	}
	if view.Peers == nil {
		t.Fatal("peer list must be an array even when empty")
	}
	return view
}

func TestOfflinePeerCountries(t *testing.T) {
	s := testEngine(t)
	db, err := maxminddb.Open("../android/app/src/main/assets/geoip-country-2026-10.mmdb")
	if err != nil {
		t.Fatal(err)
	}
	if err := db.Verify(); err != nil {
		t.Fatal(err)
	}
	s.geoIP = db
	// Expected results are from the pinned October 2026 database, including its
	// approximate IPv6 country assignment, not the physical DNS server location.
	for address, expected := range map[string]string{"8.8.8.8:6881": "US", "[2001:4860:4860::8888]:6881": "CA"} {
		if country := s.peerCountry(address); country != expected {
			t.Fatalf("country for %s = %q", address, country)
		}
	}
	for _, address := range []string{"127.0.0.1:6881", "192.168.1.2:6881", "[::1]:6881", "[fc00::1]:6881", "WebRTC peer", "invalid"} {
		if country := s.peerCountry(address); country != "" {
			t.Fatalf("invented country for %s = %s", address, country)
		}
	}
}

func TestConnectionsExcludeCandidatesAndUnknownTorrent(t *testing.T) {
	s := testEngine(t)
	mi, err := metainfo.LoadFromFile("testdata/tiny.torrent")
	if err != nil {
		t.Fatal(err)
	}
	tr, err := s.client.AddTorrent(mi)
	if err != nil {
		t.Fatal(err)
	}
	s.records["one"] = &TorrentRecord{ID: "one", Torrent: tr}
	tr.AddPeers([]torrent.PeerInfo{{Addr: &net.TCPAddr{IP: net.ParseIP("127.0.0.1"), Port: 1}}})
	if peers := connectionSnapshot(t, s, "one").Peers; len(peers) != 0 {
		t.Fatalf("discovered candidates rendered as connected: %+v", peers)
	}
	if code, _ := s.DispatchControl("GET", "/connections/missing", ""); code != 404 {
		t.Fatalf("missing torrent status %d", code)
	}
}

func TestConnectionsReportsRealTCPPeerAndClearsOnPause(t *testing.T) {
	newPeer := func() (*EngineServer, *TorrentRecord) {
		s := testEngine(t, func(cfg *torrent.ClientConfig) {
			cfg.DisableTCP, cfg.DisableIPv6 = false, true
			cfg.ListenHost = func(string) string { return "127.0.0.1" }
			cfg.ListenPort = 0
			cfg.DialForPeerConns, cfg.AcceptPeerConnections = true, true
			cfg.Seed = true
		})
		mi, err := metainfo.LoadFromFile("testdata/tiny.torrent")
		if err != nil {
			t.Fatal(err)
		}
		tr, err := s.client.AddTorrent(mi)
		if err != nil {
			t.Fatal(err)
		}
		rec := &TorrentRecord{ID: "one", InfoHash: tr.InfoHash().HexString(), Torrent: tr}
		s.records[rec.ID] = rec
		file, err := os.CreateTemp(t.TempDir(), "peer")
		if err != nil {
			t.Fatal(err)
		}
		defer file.Close()
		fd := int(file.Fd())
		request, _ := json.Marshal(ConfigureRequest{ID: rec.ID, Descriptors: []*int{&fd}, Selected: []int{0}})
		if code, body := s.DispatchControl("POST", "/configure", string(request)); code != 200 {
			t.Fatalf("configure %d: %s", code, body)
		}
		return s, rec
	}
	remote, _ := newPeer()
	s, rec := newPeer()
	if rec.Torrent.AddClientPeer(remote.client) == 0 {
		t.Fatal("no local peer address")
	}
	deadline := time.Now().Add(5 * time.Second)
	for {
		view := connectionSnapshot(t, s, rec.ID)
		if len(view.Peers) > 0 {
			peer := view.Peers[0]
			host, _, err := net.SplitHostPort(peer.Address)
			if err != nil || host != "127.0.0.1" || peer.Transport != "TCP" {
				t.Fatalf("incorrect live peer: %+v", peer)
			}
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("local peer did not connect")
		}
		time.Sleep(10 * time.Millisecond)
	}
	if code, body := s.DispatchControl("POST", "/pause", `{"id":"one"}`); code != 200 {
		t.Fatalf("pause %d: %s", code, body)
	}
	view := connectionSnapshot(t, s, rec.ID)
	if !view.Paused || len(view.Peers) != 0 {
		t.Fatalf("paused map retains connections: %+v", view)
	}
}
