package main

import (
	"encoding/json"
	"net"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/anacrolix/torrent"
	"github.com/anacrolix/torrent/metainfo"
)

func testEngine(t *testing.T, configure ...func(*torrent.ClientConfig)) *EngineServer {
	t.Helper()
	cfg := torrent.NewDefaultClientConfig()
	cfg.DataDir = t.TempDir()
	cfg.NoDHT, cfg.DisableTrackers, cfg.DisableTCP, cfg.DisableUTP = true, true, true, true
	cfg.DisableWebtorrent, cfg.DisablePEX = true, true
	cfg.DialForPeerConns, cfg.AcceptPeerConnections = false, false
	store := NewDocumentStorageClient(cfg.DataDir)
	cfg.DefaultStorage = store
	for _, apply := range configure {
		apply(cfg)
	}
	client, err := torrent.NewClient(cfg)
	if err != nil {
		t.Fatal(err)
	}
	s := &EngineServer{client: client, storage: store, records: make(map[string]*TorrentRecord), recordsByHash: make(map[string]*TorrentRecord), done: make(chan struct{})}
	t.Cleanup(s.Close)
	return s
}

func TestStatusReportsConnectionsNotDiscoveredCandidates(t *testing.T) {
	s := testEngine(t)
	mi, err := metainfo.LoadFromFile("testdata/tiny.torrent")
	if err != nil {
		t.Fatal(err)
	}
	tr, err := s.client.AddTorrent(mi)
	if err != nil {
		t.Fatal(err)
	}
	tr.AddPeers([]torrent.PeerInfo{{Addr: &net.TCPAddr{IP: net.ParseIP("127.0.0.1"), Port: 1}}})
	rec := &TorrentRecord{ID: "candidates", InfoHash: tr.InfoHash().HexString(), Torrent: tr}
	s.records[rec.ID] = rec
	w := httptest.NewRecorder()
	s.handleGetTorrent(w, httptest.NewRequest("GET", "/torrent/candidates", nil), rec.ID)
	var view TorrentView
	if err := json.Unmarshal(w.Body.Bytes(), &view); err != nil {
		t.Fatal(err)
	}
	if view.NumPeers != 0 {
		t.Fatalf("reported %d peers without a connection", view.NumPeers)
	}
}

func TestAddMagnetBeforeMetadataReturnsRecord(t *testing.T) {
	s := testEngine(t)
	req := httptest.NewRequest("POST", "/add", strings.NewReader(`{"torrentId":"magnet:?xt=urn:btih:0123456789012345678901234567890123456789","prepare":true}`))
	req.RemoteAddr = "127.0.0.1:1234"
	w := httptest.NewRecorder()
	s.handleControl(w, req)
	if w.Code != 200 {
		t.Fatalf("status %d: %s", w.Code, w.Body.String())
	}
	var result AddResponse
	if err := json.Unmarshal(w.Body.Bytes(), &result); err != nil {
		t.Fatal(err)
	}
	if result.ID == "" || len(s.records) != 1 || len(s.client.Torrents()) != 1 {
		t.Fatalf("invalid add result: %+v", result)
	}
}

func TestPauseIsReportedAndResumeRestoresSelection(t *testing.T) {
	s := testEngine(t)
	mi, err := metainfo.LoadFromFile("testdata/tiny.torrent")
	if err != nil {
		t.Fatal(err)
	}
	tr, err := s.client.AddTorrent(mi)
	if err != nil {
		t.Fatal(err)
	}
	rec := &TorrentRecord{ID: "one", InfoHash: tr.InfoHash().HexString(), Torrent: tr, Configured: true, Selected: []int{0}}
	s.records[rec.ID] = rec

	pause := httptest.NewRecorder()
	s.handlePauseResume(pause, httptest.NewRequest("POST", "/pause", strings.NewReader(`{"id":"one"}`)), true)
	if pause.Code != 200 || !rec.Paused {
		t.Fatalf("pause failed: %d %s", pause.Code, pause.Body.String())
	}
	status := httptest.NewRecorder()
	s.handleGetTorrent(status, httptest.NewRequest("GET", "/torrent/one", nil), "one")
	var view TorrentView
	if err := json.Unmarshal(status.Body.Bytes(), &view); err != nil {
		t.Fatal(err)
	}
	if !view.Paused {
		t.Fatal("paused state was not reported")
	}

	resume := httptest.NewRecorder()
	s.handlePauseResume(resume, httptest.NewRequest("POST", "/resume", strings.NewReader(`{"id":"one"}`)), false)
	if resume.Code != 200 || rec.Paused {
		t.Fatalf("resume failed: %d %s", resume.Code, resume.Body.String())
	}
	// A piece being checked has an effective priority of None until hashing
	// finishes, even though the resumed file has a download priority.
	if tr.Files()[0].Priority() != torrent.PiecePriorityNormal {
		t.Fatal("selected file was not resumed")
	}
}
