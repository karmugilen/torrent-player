package main

import (
	"encoding/binary"
	"encoding/json"
	"net"
	"net/http"
	"net/http/httptest"
	"sync/atomic"
	"testing"
	"time"

	"github.com/anacrolix/torrent"
	"github.com/anacrolix/torrent/bencode"
	"github.com/anacrolix/torrent/metainfo"
)

func TestPendingMagnetCanResumeBeforeStorageIsConfigured(t *testing.T) {
	s := testEngine(t)
	code, body := s.DispatchControl("POST", "/add", `{"torrentId":"magnet:?xt=urn:btih:0123456789012345678901234567890123456789","prepare":true}`)
	var added AddResponse
	if code != 200 || json.Unmarshal([]byte(body), &added) != nil {
		t.Fatalf("add: %d %s", code, body)
	}
	for _, action := range []string{"/pause", "/resume"} {
		code, body = s.DispatchControl("POST", action, `{"id":"`+added.ID+`"}`)
		if code != 200 {
			t.Fatalf("%s before metadata: %d %s", action, code, body)
		}
	}
	rec := s.records[added.ID]
	rec.mu.RLock()
	defer rec.mu.RUnlock()
	stats := rec.Torrent.Stats()
	if rec.Paused || rec.Configured || rec.Torrent.Info() != nil || stats.BytesReadData.Int64() != 0 {
		t.Fatal("resume must retain metadata discovery without enabling payload")
	}
}

func TestColdRestoredPendingMagnetDiscoversTrackerPeerAndMetadata(t *testing.T) {
	peerConfig := func(cfg *torrent.ClientConfig) {
		cfg.DisableTCP, cfg.DisableIPv6 = false, true
		cfg.ListenHost = func(string) string { return "127.0.0.1" }
		cfg.ListenPort = 0
		cfg.Seed = true
		cfg.DialForPeerConns, cfg.AcceptPeerConnections, cfg.AlwaysWantConns = true, true, true
	}
	seed := testEngine(t, peerConfig)
	mi := trackerMetadata(t, false)
	mi.AnnounceList = nil
	if _, err := seed.client.AddTorrent(mi); err != nil {
		t.Fatal(err)
	}
	var seedAddr *net.TCPAddr
	for _, addr := range seed.client.ListenAddrs() {
		if tcp, ok := addr.(*net.TCPAddr); ok {
			seedAddr = tcp
			break
		}
	}
	if seedAddr == nil {
		t.Fatal("missing local seed listener")
	}
	compact := make([]byte, 6)
	copy(compact, net.IPv4(127, 0, 0, 1).To4())
	binary.BigEndian.PutUint16(compact[4:], uint16(seedAddr.Port))
	var available atomic.Bool
	var announces atomic.Int32
	tracker := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		announces.Add(1)
		peers := ""
		if available.Load() {
			peers = string(compact)
		}
		body, err := bencode.Marshal(map[string]interface{}{"interval": 1, "peers": peers})
		if err != nil {
			t.Error(err)
			return
		}
		_, _ = w.Write(body)
	}))
	defer tracker.Close()
	source := (metainfo.Magnet{InfoHash: mi.HashInfoBytes(), Trackers: []string{tracker.URL + "/announce"}}).String()
	add := func(s *EngineServer) *TorrentRecord {
		t.Helper()
		request, _ := json.Marshal(AddRequest{TorrentID: source, Prepare: true})
		code, body := s.DispatchControl("POST", "/add", string(request))
		var result AddResponse
		if code != 200 || json.Unmarshal([]byte(body), &result) != nil {
			t.Fatalf("add pending: %d %s", code, body)
		}
		return s.records[result.ID]
	}
	newClient := func() *EngineServer {
		return testEngine(t, peerConfig, func(cfg *torrent.ClientConfig) { cfg.DisableTrackers = false })
	}
	before := newClient()
	pending := add(before)
	if pending.Torrent.Info() != nil {
		t.Fatal("expected a metadata-pending magnet before process restart")
	}
	before.Close()
	available.Store(true)
	restored := newClient()
	rec := add(restored)
	if rec.ID == pending.ID {
		t.Fatal("cold restart reused the old engine identity")
	}
	select {
	case <-rec.Torrent.GotInfo():
	case <-time.After(5 * time.Second):
		t.Fatalf("cold restore failed tracker discovery/metadata exchange: announces=%d stats=%+v", announces.Load(), rec.Torrent.Stats())
	}
	code, body := restored.DispatchControl("GET", "/torrent/"+rec.ID, "")
	var status TorrentView
	if code != 200 || json.Unmarshal([]byte(body), &status) != nil {
		t.Fatalf("poll restored status: %d %s", code, body)
	}
	if !status.Ready || status.Paused || status.Configured || len(status.Files) == 0 || announces.Load() == 0 {
		t.Fatalf("restored discovery did not reach metadata-ready state: %+v", status)
	}
	stats := rec.Torrent.Stats()
	if stats.BytesReadData.Int64() != 0 {
		t.Fatal("discovery downloaded payload before storage configuration")
	}
	for _, file := range rec.Torrent.Files() {
		if file.Priority() != torrent.PiecePriorityNone {
			t.Fatal("discovery enabled file priority before storage configuration")
		}
	}
}
