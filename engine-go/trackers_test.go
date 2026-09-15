package main

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"reflect"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/anacrolix/torrent"
	"github.com/anacrolix/torrent/bencode"
	"github.com/anacrolix/torrent/metainfo"
)

func TestPublicTrackerParsing(t *testing.T) {
	got := parseTrackerList("\n# comment\nHTTPS://Tracker.Example.org:443/announce\nhttps://tracker.example.org/announce\nudp://tracker.example.org:01337/announce\nwss://tracker.example.org\n")
	want := []string{"https://tracker.example.org/announce", "udp://tracker.example.org:1337/announce", "wss://tracker.example.org"}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("got %v", got)
	}
	for _, raw := range []string{
		"file:///tmp/tracker", "http://localhost/announce", "http://127.0.0.1/announce",
		"http://[::1]/announce", "http://[::ffff:192.168.1.2]/announce", "http://10.0.0.1/announce",
		"udp://tracker.local:1/announce", "udp://tracker.example.org/announce",
		"http://tracker.example.org:0", "http://tracker.example.org:65536", "http://tracker.example.org:bad",
		"https://user:pass@tracker.example.org/announce", "https://tracker.example.org/#fragment",
		"https://tracker.invalid", "<html>error</html>", "http://bad_host.example.org",
	} {
		if publicTrackerURL(raw) != "" {
			t.Errorf("accepted %q", raw)
		}
	}
	var many strings.Builder
	for i := 0; i < 100; i++ {
		fmt.Fprintf(&many, "https://tracker%d.example.org/announce\n", i)
	}
	if len(parseTrackerList(many.String())) != trackerListLimit {
		t.Fatal("list was not bounded")
	}
}

func TestTrackerCacheConditionalRefreshAndFailure(t *testing.T) {
	var calls atomic.Int32
	var mode atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		if r.URL.RawQuery != "" {
			t.Error("list request contains torrent parameters")
		}
		switch mode.Load() {
		case 0:
			w.Header().Set("ETag", `"list-v1"`)
			w.Header().Set("Last-Modified", "Mon, 14 Sep 2026 00:00:00 GMT")
			fmt.Fprint(w, "udp://tracker.example.org:1337/announce\n")
		case 1:
			if r.Header.Get("If-None-Match") != `"list-v1"` || r.Header.Get("If-Modified-Since") == "" {
				t.Error("missing validators")
			}
			w.WriteHeader(http.StatusNotModified)
		case 2:
			w.WriteHeader(http.StatusServiceUnavailable)
		case 3:
			fmt.Fprint(w, "<html>temporary error</html>")
		case 4:
			fmt.Fprint(w, strings.Repeat("x", trackerResponseLimit+1))
		}
	}))
	defer srv.Close()
	dir := t.TempDir()
	p := newTrackerPool(dir)
	p.source, p.client = srv.URL, srv.Client()
	now := time.Now()
	if changed, err := p.refresh(context.Background(), now); !changed || err != nil {
		t.Fatalf("first refresh: %t %v", changed, err)
	}
	want := p.snapshot()
	if len(want) != 5 || want[4] != "udp://tracker.example.org:1337/announce" {
		t.Fatalf("pool %v", want)
	}
	loaded := newTrackerPool(dir)
	if !reflect.DeepEqual(loaded.snapshot(), want) {
		t.Fatal("cache did not survive restart")
	}
	if stat, err := os.Stat(p.path); err != nil || stat.Mode().Perm() != 0600 {
		t.Fatal("cache must be app-private")
	}
	if _, err := p.refresh(context.Background(), now.Add(time.Hour)); err != nil || calls.Load() != 1 {
		t.Fatal("fresh cache fetched again")
	}
	mode.Store(1)
	now = now.Add(trackerRefreshInterval)
	if changed, err := p.refresh(context.Background(), now); changed || err != nil {
		t.Fatalf("304: %t %v", changed, err)
	}
	for _, m := range []int32{2, 3, 4} {
		mode.Store(m)
		now = now.Add(trackerRefreshInterval)
		if _, err := p.refresh(context.Background(), now); err == nil {
			t.Fatalf("accepted bad response %d", m)
		}
		if !reflect.DeepEqual(p.snapshot(), want) {
			t.Fatal("failure replaced good cache")
		}
		previous := calls.Load()
		if _, err := p.refresh(context.Background(), now.Add(time.Minute)); err != nil || calls.Load() != previous {
			t.Fatal("failure retry was not backed off")
		}
	}
}

func TestTrackerCacheCorruptionAndFallback(t *testing.T) {
	dir := t.TempDir()
	p := newTrackerPool(dir)
	for _, data := range []string{`{"version":`, `{"version":99,"trackers":["https://tracker.example.org"]}`, strings.Repeat("x", trackerResponseLimit+1)} {
		if err := os.WriteFile(p.path, []byte(data), 0600); err != nil {
			t.Fatal(err)
		}
		if got := newTrackerPool(dir).snapshot(); !reflect.DeepEqual(got, defaultAnnounce) {
			t.Fatal("lost offline fallback")
		}
	}
	c := trackerCache{Version: 1, Trackers: []string{"http://127.0.0.1"}, ETag: `"invalid"`, NextAttempt: time.Now().Add(365 * 24 * time.Hour)}
	if err := p.save(c); err != nil {
		t.Fatal(err)
	}
	loaded := newTrackerPool(dir)
	if !loaded.cache.NextAttempt.IsZero() || loaded.cache.ETag != "" {
		t.Fatal("corrupt cache suppresses fetching")
	}
}

func TestTrackerFetchIsAsyncAndCancelledOnClose(t *testing.T) {
	started, cancelled := make(chan struct{}), make(chan struct{})
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		close(started)
		<-r.Context().Done()
		close(cancelled)
	}))
	defer srv.Close()
	p := newTrackerPool(t.TempDir())
	p.source, p.client = srv.URL, srv.Client()
	p.start(func() { t.Error("cancelled fetch must not notify") })
	defer p.close()
	select {
	case <-started:
	case <-time.After(3 * time.Second):
		t.Fatal("fetch not started")
	}
	done := make(chan struct{})
	go func() {
		var wg sync.WaitGroup
		for i := 0; i < 20; i++ {
			wg.Add(1)
			go func() { defer wg.Done(); _ = p.snapshot() }()
		}
		wg.Wait()
		p.close()
		close(done)
	}()
	select {
	case <-done:
	case <-time.After(3 * time.Second):
		t.Fatal("cache read or close blocked on network")
	}
	select {
	case <-cancelled:
	case <-time.After(3 * time.Second):
		t.Fatal("request was not cancelled")
	}
}

func trackerMetadata(t *testing.T, private bool) *metainfo.MetaInfo {
	t.Helper()
	mi, err := metainfo.LoadFromFile("testdata/tiny.torrent")
	if err != nil {
		t.Fatal(err)
	}
	info, err := mi.UnmarshalInfo()
	if err != nil {
		t.Fatal(err)
	}
	info.Private = &private
	mi.InfoBytes, err = bencode.Marshal(info)
	if err != nil {
		t.Fatal(err)
	}
	mi.AnnounceList = [][]string{{"https://own.example.org/announce?passkey=keep", "udp://backup.example.org:1337/announce"}}
	mi.Announce = ""
	return mi
}

func TestTrackerSupplementPublicPrivateAndMagnet(t *testing.T) {
	for _, private := range []bool{false, true} {
		for _, magnet := range []bool{false, true} {
			t.Run(fmt.Sprintf("private=%t/magnet=%t", private, magnet), func(t *testing.T) {
				s := testEngine(t)
				mi := trackerMetadata(t, private)
				var buf bytes.Buffer
				if err := mi.Write(&buf); err != nil {
					t.Fatal(err)
				}
				req := AddRequest{Prepare: true, TorrentData: base64.StdEncoding.EncodeToString(buf.Bytes())}
				if magnet {
					req.TorrentData = ""
					req.TorrentID = mi.Magnet(nil, nil).String()
				}
				body, _ := json.Marshal(req)
				code, res := s.DispatchControl("POST", "/add", string(body))
				if code != 200 {
					t.Fatalf("add: %s", res)
				}
				var added AddResponse
				if err := json.Unmarshal(res, &added); err != nil {
					t.Fatal(err)
				}
				rec := s.records[added.ID]
				if magnet {
					s.supplementTrackers(rec)
					if len(rec.Torrent.Metainfo().AnnounceList) != 1 {
						t.Fatal("extra trackers before privacy known")
					}
					if err := rec.Torrent.SetInfoBytes(mi.InfoBytes); err != nil {
						t.Fatal(err)
					}
				}
				s.supplementTrackers(rec)
				got := rec.Torrent.Metainfo().AnnounceList
				if !reflect.DeepEqual(got[0], mi.AnnounceList[0]) {
					t.Fatalf("original tier changed: %v", got[0])
				}
				if private && len(got) != 1 || !private && len(got) <= 1 {
					t.Fatalf("wrong supplements: %v", got)
				}
				before := len(got)
				s.supplementTrackers(rec)
				if len(rec.Torrent.Metainfo().AnnounceList) != before {
					t.Fatal("duplicate supplements")
				}
			})
		}
	}
}

func TestTrackerRefreshIsBoundedForExistingTorrents(t *testing.T) {
	s := testEngine(t)
	s.trackers = newTrackerPool(t.TempDir())
	mi := trackerMetadata(t, false)
	tr, err := s.client.AddTorrent(mi)
	if err != nil {
		t.Fatal(err)
	}
	rec := &TorrentRecord{ID: "bounded", Torrent: tr, Prepared: true}
	s.records[rec.ID] = rec
	for round := 0; round < 50; round++ {
		s.trackers.mu.Lock()
		s.trackers.cache.Trackers = []string{fmt.Sprintf("https://tracker%d.example.org/announce", round)}
		s.trackers.mu.Unlock()
		s.refreshTorrentTrackers()
	}
	if rec.SupplementalTrackers != trackerSupplementLimit {
		t.Fatalf("added %d", rec.SupplementalTrackers)
	}
	if len(tr.Metainfo().AnnounceList) != 1+trackerSupplementLimit {
		t.Fatal("tracker set grew beyond cap")
	}
	if !rec.Prepared || rec.Configured || rec.Paused {
		t.Fatal("refresh changed download state")
	}
}

// Exercise the real tracker announce -> compact peer response -> TCP handshake
// path. HTTP is redirected to a local tracker so this test needs no public swarm.
func TestTrackerDiscoveredPeerSurvivesRefresh(t *testing.T) {
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
	seedTorrent, err := seed.client.AddTorrent(mi)
	if err != nil {
		t.Fatal(err)
	}
	seedTorrent.DownloadAll()
	var seedAddr *net.TCPAddr
	for _, addr := range seed.client.ListenAddrs() {
		if tcp, ok := addr.(*net.TCPAddr); ok {
			seedAddr = tcp
			break
		}
	}
	if seedAddr == nil {
		t.Fatal("no seed listener")
	}
	compact := make([]byte, 6)
	copy(compact, net.IPv4(127, 0, 0, 1).To4())
	binary.BigEndian.PutUint16(compact[4:], uint16(seedAddr.Port))
	var announces atomic.Int32
	tracker := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		hash := mi.HashInfoBytes()
		if r.URL.Query().Get("info_hash") != string(hash[:]) {
			t.Error("wrong info hash announced")
		}
		announces.Add(1)
		response, err := bencode.Marshal(map[string]interface{}{"interval": 1800, "peers": string(compact)})
		if err != nil {
			t.Error(err)
			return
		}
		w.Write(response)
	}))
	defer tracker.Close()
	leech := testEngine(t, peerConfig, func(cfg *torrent.ClientConfig) {
		cfg.DisableTrackers = false
		cfg.TrackerDialContext = func(ctx context.Context, network, address string) (net.Conn, error) {
			return (&net.Dialer{}).DialContext(ctx, "tcp", tracker.Listener.Addr().String())
		}
	})
	leech.trackers = newTrackerPool(t.TempDir())
	leech.trackers.cache.Trackers = []string{"http://tracker.example.org/announce"}
	tr, err := leech.client.AddTorrent(mi)
	if err != nil {
		t.Fatal(err)
	}
	tr.DownloadAll()
	rec := &TorrentRecord{ID: "discovery", Torrent: tr}
	leech.records[rec.ID] = rec
	leech.supplementTrackers(rec)
	deadline := time.Now().Add(5 * time.Second)
	for len(tr.PeerConns()) == 0 && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	peers := tr.PeerConns()
	if len(peers) == 0 || announces.Load() == 0 {
		t.Fatalf("supplemental tracker did not discover local seed: announces=%d stats=%+v", announces.Load(), tr.Stats())
	}
	leech.trackers.mu.Lock()
	leech.trackers.cache.Trackers = []string{"http://new.example.org/announce"}
	leech.trackers.mu.Unlock()
	leech.refreshTorrentTrackers()
	for _, connected := range tr.PeerConns() {
		if connected == peers[0] {
			return
		}
	}
	t.Fatal("refresh dropped the existing peer connection")
}

func TestManagedTrackersDoNotAccumulateAcrossRestore(t *testing.T) {
	s := testEngine(t)
	mi := trackerMetadata(t, false)
	var buf bytes.Buffer
	if err := mi.Write(&buf); err != nil {
		t.Fatal(err)
	}
	data := base64.StdEncoding.EncodeToString(buf.Bytes())
	for round := 0; round < 3; round++ {
		body, _ := json.Marshal(AddRequest{Prepare: true, TorrentData: data})
		code, result := s.DispatchControl("POST", "/add", string(body))
		if code != 200 {
			t.Fatalf("add: %s", result)
		}
		var added AddResponse
		if err := json.Unmarshal(result, &added); err != nil {
			t.Fatal(err)
		}
		rec := s.records[added.ID]
		s.supplementTrackers(rec)
		code, result = s.DispatchControl("GET", "/metadata/"+rec.ID, "")
		if code != 200 {
			t.Fatalf("metadata: %s", result)
		}
		var exported MetadataResponse
		if err := json.Unmarshal(result, &exported); err != nil {
			t.Fatal(err)
		}
		encoded, err := base64.StdEncoding.DecodeString(exported.TorrentData)
		if err != nil {
			t.Fatal(err)
		}
		decoded, err := metainfo.Load(bytes.NewReader(encoded))
		if err != nil {
			t.Fatal(err)
		}
		if !reflect.DeepEqual(decoded.UpvertedAnnounceList(), mi.UpvertedAnnounceList()) {
			t.Fatal("persisted managed trackers as originals")
		}
		if decoded.HashInfoBytes() != mi.HashInfoBytes() {
			t.Fatal("export changed infohash")
		}
		code, result = s.DispatchControl("GET", "/torrent/"+rec.ID, "")
		var view TorrentView
		if code != 200 || json.Unmarshal(result, &view) != nil || view.MagnetURI == nil {
			t.Fatal("missing magnet")
		}
		magnet, err := metainfo.ParseMagnetUri(*view.MagnetURI)
		if err != nil || !reflect.DeepEqual(magnet.Trackers, mi.AnnounceList[0]) {
			t.Fatalf("persisted managed magnet trackers: %+v", magnet)
		}
		data = exported.TorrentData
		body, _ = json.Marshal(map[string]string{"id": rec.ID})
		code, result = s.DispatchControl("POST", "/remove", string(body))
		if code != 200 {
			t.Fatalf("remove: %s", result)
		}
	}
}
