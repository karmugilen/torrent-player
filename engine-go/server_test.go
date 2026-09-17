package main

import (
	"bytes"
	"crypto/sha1"
	"encoding/base64"
	"encoding/json"
	"net"
	"net/http/httptest"
	"os"
	"strings"
	"testing"

	"github.com/anacrolix/torrent"
	"github.com/anacrolix/torrent/bencode"
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
	// NewEngineServer enables payload traffic by default. Unit tests build the
	// server directly so they mirror that production default explicitly.
	s.payloadTransfersAllowed.Store(true)
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

func TestPayloadTransferSettingGatesBytesButPreservesManualPause(t *testing.T) {
	s := testEngine(t)
	mi, err := metainfo.LoadFromFile("testdata/tiny.torrent")
	if err != nil {
		t.Fatal(err)
	}
	tr, err := s.client.AddTorrent(mi)
	if err != nil {
		t.Fatal(err)
	}
	rec := &TorrentRecord{
		ID: "payload-gate", InfoHash: tr.InfoHash().HexString(), Torrent: tr,
		Configured: true, Selected: []int{0},
	}
	s.records[rec.ID] = rec
	rec.mu.Lock()
	s.applyTransferState(rec)
	rec.mu.Unlock()
	if got := tr.Files()[0].Priority(); got != torrent.PiecePriorityNormal {
		t.Fatalf("initial priority = %v, want normal", got)
	}

	post := func(body string) SettingsResponse {
		t.Helper()
		w := httptest.NewRecorder()
		s.handlePostSettings(w, httptest.NewRequest("POST", "/settings", strings.NewReader(body)))
		if w.Code != 200 {
			t.Fatalf("settings: %d %s", w.Code, w.Body.String())
		}
		var response SettingsResponse
		if err := json.Unmarshal(w.Body.Bytes(), &response); err != nil {
			t.Fatal(err)
		}
		return response
	}

	response := post(`{"payloadTransfersAllowed":false}`)
	if response.PayloadTransfersAllowed || rec.Paused {
		t.Fatalf("gate changed visible state incorrectly: response=%+v paused=%t", response, rec.Paused)
	}
	if got := tr.Files()[0].Priority(); got != torrent.PiecePriorityNone {
		t.Fatalf("disabled payload priority = %v, want none", got)
	}

	// A manual pause is independent of the global policy. Turning global
	// transfer back on must not silently resume this torrent.
	w := httptest.NewRecorder()
	s.handlePauseResume(w, httptest.NewRequest("POST", "/pause", strings.NewReader(`{"id":"payload-gate"}`)), true)
	if w.Code != 200 || !rec.Paused {
		t.Fatalf("pause: %d %s", w.Code, w.Body.String())
	}
	response = post(`{"payloadTransfersAllowed":true}`)
	if !response.PayloadTransfersAllowed || !rec.Paused {
		t.Fatalf("global enable changed manual pause: response=%+v paused=%t", response, rec.Paused)
	}
	if got := tr.Files()[0].Priority(); got != torrent.PiecePriorityNone {
		t.Fatalf("manual pause priority = %v, want none", got)
	}

	w = httptest.NewRecorder()
	s.handlePauseResume(w, httptest.NewRequest("POST", "/resume", strings.NewReader(`{"id":"payload-gate"}`)), false)
	if w.Code != 200 || rec.Paused || tr.Files()[0].Priority() != torrent.PiecePriorityNormal {
		t.Fatalf("resume did not restore selected payload: %d %s", w.Code, w.Body.String())
	}

	// The response is also available through the read endpoint for Android
	// process-recreation and settings-screen initialization.
	w = httptest.NewRecorder()
	s.handleGetSettings(w, httptest.NewRequest("GET", "/settings", nil))
	var status SettingsResponse
	if err := json.Unmarshal(w.Body.Bytes(), &status); err != nil {
		t.Fatal(err)
	}
	if !status.PayloadTransfersAllowed {
		t.Fatalf("GET settings did not report payload state: %+v", status)
	}
}

func TestPayloadGateKeepsPendingMagnetDiscoverable(t *testing.T) {
	s := testEngine(t)
	add := httptest.NewRecorder()
	request := httptest.NewRequest("POST", "/add", strings.NewReader(`{"torrentId":"magnet:?xt=urn:btih:0123456789012345678901234567890123456789","prepare":true}`))
	request.RemoteAddr = "127.0.0.1:1234"
	s.handleControl(add, request)
	if add.Code != 200 {
		t.Fatalf("add pending magnet: %d %s", add.Code, add.Body.String())
	}
	var added AddResponse
	if err := json.Unmarshal(add.Body.Bytes(), &added); err != nil {
		t.Fatal(err)
	}
	rec := s.records[added.ID]
	if rec == nil || rec.Torrent.Info() != nil {
		t.Fatalf("expected metadata-pending torrent, got %#v", rec)
	}

	w := httptest.NewRecorder()
	s.handlePostSettings(w, httptest.NewRequest("POST", "/settings", strings.NewReader(`{"payloadTransfersAllowed":false}`)))
	if w.Code != 200 {
		t.Fatalf("disable payload: %d %s", w.Code, w.Body.String())
	}
	if rec.Paused || rec.Torrent.Info() != nil || len(s.client.Torrents()) != 1 {
		t.Fatalf("payload setting paused or removed metadata discovery: paused=%t info=%v torrents=%d", rec.Paused, rec.Torrent.Info() != nil, len(s.client.Torrents()))
	}
}

func TestPayloadGateRejectsIncompletePlaybackClearly(t *testing.T) {
	s := testEngine(t)
	mi, err := metainfo.LoadFromFile("testdata/tiny.torrent")
	if err != nil {
		t.Fatal(err)
	}
	tr, err := s.client.AddTorrent(mi)
	if err != nil {
		t.Fatal(err)
	}
	rec := &TorrentRecord{
		ID: "blocked-play", InfoHash: tr.InfoHash().HexString(), Torrent: tr,
		Configured: true, Selected: []int{0},
	}
	s.records[rec.ID] = rec
	w := httptest.NewRecorder()
	s.handlePostSettings(w, httptest.NewRequest("POST", "/settings", strings.NewReader(`{"payloadTransfersAllowed":false}`)))
	if w.Code != 200 {
		t.Fatalf("disable payload: %d %s", w.Code, w.Body.String())
	}

	playBody, _ := json.Marshal(PlayRequest{ID: rec.ID})
	w = httptest.NewRecorder()
	s.handlePlay(w, httptest.NewRequest("POST", "/play", strings.NewReader(string(playBody))))
	if w.Code != 409 || !strings.Contains(w.Body.String(), "payload transfers are disabled") {
		t.Fatalf("incomplete play should explain global gate: %d %s", w.Code, w.Body.String())
	}
}

func TestPieceTelemetryReportsSelectedPieces(t *testing.T) {
	s := testEngine(t)
	info := metainfo.Info{
		Name:        "bundle",
		PieceLength: 4,
		Files: []metainfo.FileInfo{
			{Length: 4, Path: []string{"wanted.bin"}},
			{Length: 4, Path: []string{"unwanted.bin"}},
		},
		// The hashes are irrelevant to telemetry, but the info dictionary must
		// contain one 20-byte v1 hash per piece.
		Pieces: bytes.Repeat([]byte{0}, 40),
	}
	encoded, err := bencode.Marshal(info)
	if err != nil {
		t.Fatal(err)
	}
	tr, err := s.client.AddTorrent(&metainfo.MetaInfo{InfoBytes: encoded})
	if err != nil {
		t.Fatal(err)
	}
	rec := &TorrentRecord{
		ID:         "telemetry",
		InfoHash:   tr.InfoHash().HexString(),
		Torrent:    tr,
		Configured: true,
		Selected:   []int{0},
	}
	s.records[rec.ID] = rec
	w := httptest.NewRecorder()
	s.handlePieces(w, httptest.NewRequest("GET", "/pieces/telemetry", nil), rec.ID)
	if w.Code != 200 {
		t.Fatalf("pieces: %d %s", w.Code, w.Body.String())
	}
	var response PieceTelemetryResponse
	if err := json.Unmarshal(w.Body.Bytes(), &response); err != nil {
		t.Fatal(err)
	}
	if len(response.Buckets) != 2 {
		t.Fatalf("buckets=%+v", response.Buckets)
	}
	if response.Buckets[0].Selected != 1 || response.Buckets[1].Selected != 0 {
		t.Fatalf("selected telemetry=%+v, want [1 0]", response.Buckets)
	}
	// Hashing may briefly mark pieces receiving; selected* must still respect selection.
	if response.Buckets[0].SelectedVerified > response.Buckets[0].Verified {
		t.Fatalf("selectedVerified exceeds verified: %+v", response.Buckets[0])
	}
	if response.Buckets[0].SelectedReceiving > response.Buckets[0].Receiving {
		t.Fatalf("selectedReceiving exceeds receiving: %+v", response.Buckets[0])
	}
	if response.Buckets[1].SelectedVerified != 0 || response.Buckets[1].SelectedReceiving != 0 {
		t.Fatalf("excluded selected* counts=%+v", response.Buckets[1])
	}
}

func TestAccumulateBucketPieceSelectedMixedCounts(t *testing.T) {
	var b PieceBucket
	// Selected verified.
	accumulateBucketPiece(&b, true, true, false)
	// Excluded verified must inflate Verified only.
	accumulateBucketPiece(&b, false, true, false)
	// Selected receiving (Partial/Checking/Hashing path).
	accumulateBucketPiece(&b, true, false, true)
	// Excluded receiving must inflate Receiving only.
	accumulateBucketPiece(&b, false, false, true)
	// Complete wins over receiving when both flags are set.
	accumulateBucketPiece(&b, true, true, true)
	// Missing selected piece.
	accumulateBucketPiece(&b, true, false, false)

	if b.Selected != 4 {
		t.Fatalf("Selected=%d want 4", b.Selected)
	}
	if b.Verified != 3 {
		t.Fatalf("Verified=%d want 3", b.Verified)
	}
	if b.Receiving != 2 {
		t.Fatalf("Receiving=%d want 2", b.Receiving)
	}
	if b.SelectedVerified != 2 {
		t.Fatalf("SelectedVerified=%d want 2 (excluded verified omitted)", b.SelectedVerified)
	}
	if b.SelectedReceiving != 1 {
		t.Fatalf("SelectedReceiving=%d want 1 (excluded receiving omitted)", b.SelectedReceiving)
	}
	if b.SelectedVerified+b.SelectedReceiving > b.Selected {
		t.Fatalf("selected invariant broken: %+v", b)
	}
	if b.Verified < b.SelectedVerified || b.Receiving < b.SelectedReceiving {
		t.Fatalf("all-piece counts must dominate selected*: %+v", b)
	}
}

func TestPieceTelemetrySelectedVerifiedIgnoresExcluded(t *testing.T) {
	s := testEngine(t)
	data := []byte("AAAABBBB")
	info := metainfo.Info{
		Name:        "bundle",
		PieceLength: 4,
		Files: []metainfo.FileInfo{
			{Length: 4, Path: []string{"wanted.bin"}},
			{Length: 4, Path: []string{"unwanted.bin"}},
		},
	}
	for start := 0; start < len(data); start += 4 {
		hash := sha1.Sum(data[start : start+4])
		info.Pieces = append(info.Pieces, hash[:]...)
	}
	encoded, err := bencode.Marshal(info)
	if err != nil {
		t.Fatal(err)
	}
	mi := metainfo.MetaInfo{InfoBytes: encoded}
	var metadata bytes.Buffer
	if err := mi.Write(&metadata); err != nil {
		t.Fatal(err)
	}
	body, _ := json.Marshal(AddRequest{Prepare: true, TorrentData: base64.StdEncoding.EncodeToString(metadata.Bytes())})
	code, response := s.DispatchControl("POST", "/add", string(body))
	if code != 200 {
		t.Fatalf("add: %d %s", code, response)
	}
	var added AddResponse
	if err := json.Unmarshal(response, &added); err != nil {
		t.Fatal(err)
	}
	rec := s.records[added.ID]
	wanted, err := os.CreateTemp(t.TempDir(), "wanted")
	if err != nil {
		t.Fatal(err)
	}
	unwanted, err := os.CreateTemp(t.TempDir(), "unwanted")
	if err != nil {
		t.Fatal(err)
	}
	wfd, ufd := int(wanted.Fd()), int(unwanted.Fd())
	body, _ = json.Marshal(ConfigureRequest{
		ID:          added.ID,
		Descriptors: []*int{&wfd, &ufd},
		Selected:    []int{0},
	})
	code, response = s.DispatchControl("POST", "/configure", string(body))
	_ = wanted.Close()
	_ = unwanted.Close()
	if code != 200 {
		t.Fatalf("configure: %d %s", code, response)
	}

	writeVerifiedPiece(t, s, rec, data, 0)
	writeVerifiedPiece(t, s, rec, data, 1)

	w := httptest.NewRecorder()
	s.handlePieces(w, httptest.NewRequest("GET", "/pieces/"+rec.ID, nil), rec.ID)
	if w.Code != 200 {
		t.Fatalf("pieces: %d %s", w.Code, w.Body.String())
	}
	var telemetry PieceTelemetryResponse
	if err := json.Unmarshal(w.Body.Bytes(), &telemetry); err != nil {
		t.Fatal(err)
	}
	if len(telemetry.Buckets) != 2 {
		t.Fatalf("buckets=%+v", telemetry.Buckets)
	}
	selected := telemetry.Buckets[0]
	excluded := telemetry.Buckets[1]
	if selected.Start != 0 || selected.End != 0 || selected.Total != 1 {
		t.Fatalf("inclusive selected range=%+v", selected)
	}
	if selected.Selected != 1 || selected.Verified != 1 || selected.SelectedVerified != 1 {
		t.Fatalf("selected bucket=%+v", selected)
	}
	if excluded.Selected != 0 || excluded.Verified != 1 || excluded.SelectedVerified != 0 {
		t.Fatalf("excluded verified must not count as selectedVerified: %+v", excluded)
	}
	if selected.SelectedReceiving != 0 || excluded.SelectedReceiving != 0 {
		t.Fatalf("unexpected receiving: selected=%+v excluded=%+v", selected, excluded)
	}
}

func TestPieceTelemetrySharedBoundaryCountsAsSelected(t *testing.T) {
	s := testEngine(t)
	data := []byte("AAAABBBB")
	info := metainfo.Info{
		Name:        "shared",
		PieceLength: 8,
		Files: []metainfo.FileInfo{
			{Length: 4, Path: []string{"a.bin"}},
			{Length: 4, Path: []string{"b.bin"}},
		},
	}
	hash := sha1.Sum(data)
	info.Pieces = hash[:]
	encoded, err := bencode.Marshal(info)
	if err != nil {
		t.Fatal(err)
	}
	mi := metainfo.MetaInfo{InfoBytes: encoded}
	var metadata bytes.Buffer
	if err := mi.Write(&metadata); err != nil {
		t.Fatal(err)
	}
	body, _ := json.Marshal(AddRequest{Prepare: true, TorrentData: base64.StdEncoding.EncodeToString(metadata.Bytes())})
	code, response := s.DispatchControl("POST", "/add", string(body))
	if code != 200 {
		t.Fatalf("add: %d %s", code, response)
	}
	var added AddResponse
	if err := json.Unmarshal(response, &added); err != nil {
		t.Fatal(err)
	}
	rec := s.records[added.ID]
	aFile, err := os.CreateTemp(t.TempDir(), "a")
	if err != nil {
		t.Fatal(err)
	}
	bFile, err := os.CreateTemp(t.TempDir(), "b")
	if err != nil {
		t.Fatal(err)
	}
	afd, bfd := int(aFile.Fd()), int(bFile.Fd())
	body, _ = json.Marshal(ConfigureRequest{
		ID:          added.ID,
		Descriptors: []*int{&afd, &bfd},
		Selected:    []int{0},
	})
	code, response = s.DispatchControl("POST", "/configure", string(body))
	_ = aFile.Close()
	_ = bFile.Close()
	if code != 200 {
		t.Fatalf("configure: %d %s", code, response)
	}
	writeVerifiedPiece(t, s, rec, data, 0)

	w := httptest.NewRecorder()
	s.handlePieces(w, httptest.NewRequest("GET", "/pieces/"+rec.ID, nil), rec.ID)
	if w.Code != 200 {
		t.Fatalf("pieces: %d %s", w.Code, w.Body.String())
	}
	var telemetry PieceTelemetryResponse
	if err := json.Unmarshal(w.Body.Bytes(), &telemetry); err != nil {
		t.Fatal(err)
	}
	if len(telemetry.Buckets) != 1 {
		t.Fatalf("buckets=%+v", telemetry.Buckets)
	}
	b := telemetry.Buckets[0]
	if b.Start != 0 || b.End != 0 || b.Total != 1 {
		t.Fatalf("shared piece range=%+v", b)
	}
	if b.Selected != 1 || b.Verified != 1 || b.SelectedVerified != 1 {
		t.Fatalf("shared boundary should count as selected when either file is selected: %+v", b)
	}
}

func TestPieceTelemetryInclusiveRangesAndMaxBuckets(t *testing.T) {
	s := testEngine(t)
	info := metainfo.Info{
		Name:        "many",
		PieceLength: 4,
		Length:      40,
		Pieces:      bytes.Repeat([]byte{1}, 200),
	}
	encoded, err := bencode.Marshal(info)
	if err != nil {
		t.Fatal(err)
	}
	tr, err := s.client.AddTorrent(&metainfo.MetaInfo{InfoBytes: encoded})
	if err != nil {
		t.Fatal(err)
	}
	rec := &TorrentRecord{
		ID:         "ranges",
		InfoHash:   tr.InfoHash().HexString(),
		Torrent:    tr,
		Configured: true,
		Selected:   []int{0},
	}
	s.records[rec.ID] = rec
	w := httptest.NewRecorder()
	s.handlePieces(w, httptest.NewRequest("GET", "/pieces/ranges?maxBuckets=4", nil), rec.ID)
	if w.Code != 200 {
		t.Fatalf("pieces: %d %s", w.Code, w.Body.String())
	}
	var telemetry PieceTelemetryResponse
	if err := json.Unmarshal(w.Body.Bytes(), &telemetry); err != nil {
		t.Fatal(err)
	}
	if telemetry.TotalPieces != 10 {
		t.Fatalf("totalPieces=%d", telemetry.TotalPieces)
	}
	if telemetry.MaxBuckets != 4 || len(telemetry.Buckets) != 4 {
		t.Fatalf("maxBuckets contract: max=%d buckets=%d", telemetry.MaxBuckets, len(telemetry.Buckets))
	}
	covered := 0
	for i, b := range telemetry.Buckets {
		if b.End < b.Start {
			t.Fatalf("bucket %d end < start: %+v", i, b)
		}
		if b.Total != b.End-b.Start+1 {
			t.Fatalf("bucket %d total not inclusive: %+v", i, b)
		}
		covered += b.Total
		if b.SelectedVerified < 0 || b.SelectedReceiving < 0 {
			t.Fatalf("negative selected*: %+v", b)
		}
		if b.SelectedVerified+b.SelectedReceiving > b.Selected {
			t.Fatalf("selected invariant: %+v", b)
		}
	}
	if covered != telemetry.TotalPieces {
		t.Fatalf("covered=%d total=%d", covered, telemetry.TotalPieces)
	}
	if telemetry.Buckets[0].Start != 0 || telemetry.Buckets[len(telemetry.Buckets)-1].End != telemetry.TotalPieces-1 {
		t.Fatalf("range span=%+v", telemetry.Buckets)
	}
}
