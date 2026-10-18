package main

import (
	"bytes"
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"log/slog"
	"math"
	"mime"
	"net"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/anacrolix/torrent"
	"github.com/anacrolix/torrent/metainfo"
	maxminddb "github.com/oschwald/maxminddb-golang"
	"github.com/pion/webrtc/v4"
	"golang.org/x/time/rate"
)

var defaultAnnounce = []string{
	// High-traffic WebTorrent WebSocket trackers (browser and hybrid swarms)
	"wss://tracker.openwebtorrent.com",
	"wss://tracker.webtorrent.dev",
	"wss://tracker.btorrent.xyz",
	"wss://tracker.files.fm:7073/announce",

	// Top public UDP & HTTP trackers (global BitTorrent swarms)
	"udp://tracker.opentrackr.org:1337/announce",
	"udp://tracker.torrent.eu.org:451/announce",
	"udp://explodie.org:6969/announce",
	"udp://open.stealth.si:80/announce",
	"udp://tracker.moeking.me:6969/announce",
	"udp://opentracker.i2p.rocks:6969/announce",
	"udp://tracker.openbittorrent.com:6969/announce",
	"udp://tracker.bittor.pw:1337/announce",
	"udp://tracker.internetwarriors.net:1337/announce",
	"udp://tracker.dler.org:6969/announce",
	"udp://tracker.altrosky.nl:6969/announce",
	"udp://tracker.tiny-vps.com:6969/announce",
	"udp://tracker.qu.ax:6969/announce",
	"udp://tracker.leechers-paradise.org:6969/announce",
	"http://tracker.openbittorrent.com:80/announce",
	"https://tracker.tamersunion.org:443/announce",
}

type TorrentRecord struct {
	mu                   sync.RWMutex
	ID                   string
	InfoHash             string
	Torrent              *torrent.Torrent
	Prepared             bool
	Configured           bool
	DirectPlay           bool
	Selected             []int
	FocusedFile          *int
	Generation           int64
	Error                *string
	Warning              *string
	DownloadedPre        int64
	Paused               bool
	SupplementalTrackers int
	OriginalTrackers     [][]string
	Checking             bool
	CheckedPieces        int
	CheckTotal           int
	verification         *savedDataCheck
	removed              bool
	rates                transferRates
	reconnectPeers       []torrent.PeerInfo
	// Cached verified-byte totals. Rebuilt only when PiecesComplete changes so
	// GET /torrent does not walk PieceStateRuns on every progress tick.
	verifiedGen   int
	verifiedBytes []int64
	Seeding       bool
	seedBase      seedAccount
}

type EngineServer struct {
	mu              sync.RWMutex
	addMu           sync.Mutex
	client          *torrent.Client
	trackers        *trackerPool
	storage         *DocumentStorageClient
	records         map[string]*TorrentRecord
	recordsByHash   map[string]*TorrentRecord
	ctlListener     net.Listener
	streamListener  net.Listener
	ctlPort         int
	streamPort      int
	downloadDir     string
	maxPeers        int
	downloadLimiter *rate.Limiter
	uploadLimiter   *rate.Limiter
	seedingEnabled  atomic.Bool
	seedingAllowed  atomic.Bool
	seedRatioMilli  atomic.Int64
	seedTimeMinutes atomic.Int64
	seedMu          sync.Mutex
	seedSaveMu      sync.Mutex
	seedAccounts    map[string]seedAccount
	// This is a global payload-byte gate. It deliberately does not pause or
	// remove torrents: peer discovery and metadata exchange must continue.
	payloadTransfersAllowed atomic.Bool
	shuttingDown            atomic.Bool
	done                    chan struct{}
	closeOnce               sync.Once
	eventMu                 sync.Mutex
	eventVersion            uint64
	eventCh                 chan struct{}

	trackerStatusMu sync.RWMutex
	trackerStatus   map[string]map[string]trackerLiveStatus
	playbackMu      sync.Mutex
	playbackOwners  map[*torrent.Torrent]*playbackPriorityCoordinator

	prevDownloadBytes int64
	prevUploadBytes   int64
	lastRateTime      time.Time
	downloadSpeed     int64
	uploadSpeed       int64

	networkMu         sync.Mutex
	networkKnown      bool
	networkGeneration int64
	networkCancel     context.CancelFunc
	networkWorkers    sync.WaitGroup
	trackerSockets    *networkDialer
	geoMu             sync.RWMutex
	geoIP             *maxminddb.Reader
	previewActive     *previewReader
	bypass            bypassController
	peerHintMu        sync.Mutex
	peerHints         map[string][]peerHint
	peerHintGen       uint64
	dhtMu             sync.Mutex
	dhtContacts       []dhtContact
	dhtGen            uint64
	cacheMu           sync.Mutex
	cacheClearGen     uint64
}

func NewEngineServer(ctlPort int, downloadDir string, maxPeers int) (*EngineServer, error) {
	_ = os.MkdirAll(downloadDir, 0755)

	storageClient := NewDocumentStorageClient(downloadDir)

	cfg := torrent.NewDefaultClientConfig()
	cfg.PieceHashersPerTorrent = backgroundWorkers()
	cfg.DataDir = downloadDir
	cfg.DefaultStorage = storageClient
	cfg.ListenPort = 0
	cfg.NoDHT = false
	cfg.DisableUTP = false
	cfg.DisableTCP = false
	cfg.DisableWebtorrent = false
	cfg.DisablePEX = false
	cfg.Seed = true // Per-torrent applySeedPolicy gates completed uploads.
	cfg.DialForPeerConns = true
	cfg.AcceptPeerConnections = true
	cfg.AlwaysWantConns = true
	cfg.PeriodicallyAnnounceTorrentsToDht = true

	// Balanced peer dialing rate limiter (avoids network/router bufferbloat)
	cfg.DialRateLimiter = rate.NewLimiter(30, 30)
	// These limiters are retained so settings can be changed without rebuilding
	// the torrent client. A zero value means unlimited (the anacrolix default).
	cfg.DownloadRateLimiter = rate.NewLimiter(rate.Inf, 1024*1024)
	cfg.UploadRateLimiter = rate.NewLimiter(rate.Inf, 1024*1024)

	// Resilient timeouts for mobile and residential networks
	cfg.NominalDialTimeout = 8 * time.Second
	cfg.MinDialTimeout = 2 * time.Second
	cfg.HandshakesTimeout = 5 * time.Second

	// Balanced connection pools for Android networking stability
	if maxPeers <= 0 {
		maxPeers = 100
	}
	cfg.EstablishedConnsPerTorrent = maxPeers
	cfg.HalfOpenConnsPerTorrent = 24
	cfg.TotalHalfOpenConns = 48
	cfg.TorrentPeersHighWater = max(120, maxPeers*4)
	cfg.TorrentPeersLowWater = max(40, maxPeers*2)

	// HTTP & WebSocket tracker dialer with sensible 6s timeout (avoid 45s hang)
	trackerSockets := &networkDialer{}
	cfg.HTTPDialContext = trackerSockets.DialContext
	cfg.TrackerDialContext = cfg.HTTPDialContext

	cfg.HTTPUserAgent = "TorrentPlayer/1.4.5"
	cfg.ExtendedHandshakeClientVersion = "Torrent Player 1.4.5 (Go)"

	// Robust STUN servers for WebRTC ICE traversal & hole punching
	cfg.ICEServerList = []webrtc.ICEServer{
		{URLs: []string{
			"stun:stun.l.google.com:19302",
			"stun:stun1.l.google.com:19302",
			"stun:stun2.l.google.com:19302",
			"stun:stun3.l.google.com:19302",
			"stun:stun4.l.google.com:19302",
			"stun:global.stun.twilio.com:3478",
			"stun:stun.cloudflare.com:3478",
		}},
	}

	s := &EngineServer{
		storage:         storageClient,
		records:         make(map[string]*TorrentRecord),
		recordsByHash:   make(map[string]*TorrentRecord),
		trackerStatus:   make(map[string]map[string]trackerLiveStatus),
		playbackOwners:  make(map[*torrent.Torrent]*playbackPriorityCoordinator),
		downloadDir:     downloadDir,
		maxPeers:        maxPeers,
		trackerSockets:  trackerSockets,
		downloadLimiter: cfg.DownloadRateLimiter,
		uploadLimiter:   cfg.UploadRateLimiter,
		ctlPort:         ctlPort,
		lastRateTime:    time.Now(),
		done:            make(chan struct{}),
		eventCh:         make(chan struct{}),
		bypass:          bypassController{mode: bypassModeOff},
	}
	cfg.Slogger = slog.New(&trackerStatusHandler{
		s:    s,
		next: slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: slog.LevelWarn}),
	})
	cfg.Callbacks.StatusUpdated = append(cfg.Callbacks.StatusUpdated, s.onStatusUpdated)
	s.seedRatioMilli.Store(1000)
	s.seedTimeMinutes.Store(60)
	s.seedingAllowed.Store(true)
	s.loadSeedAccounts()
	cfg.Callbacks.CompletedHandshake = func(pc *torrent.PeerConn, hash torrent.InfoHash) {
		s.mu.RLock()
		rec := s.recordsByHash[hash.HexString()]
		s.mu.RUnlock()
		if rec == nil || pc == nil || pc.RemoteAddr == nil {
			return
		}
		// Reject unknown privacy: missing metadata must not be treated as public.
		info := rec.Torrent.Info()
		if info == nil {
			return
		}
		if info.Private != nil && *info.Private {
			return
		}
		// Prefer a dialable listen endpoint over a transient inbound source port.
		addr := peerDialEndpoint(pc)
		if addr == nil {
			return
		}
		s.rememberPeerHint(rec.InfoHash, torrent.PeerInfo{Addr: addr, Source: pc.Discovery}, false)
	}

	client, err := torrent.NewClient(cfg)
	if err != nil {
		return nil, fmt.Errorf("torrent client init error: %w", err)
	}
	s.client = client
	s.payloadTransfersAllowed.Store(true)
	// Optional offline country database. Missing geo data never blocks transfers.
	s.geoIP, _ = maxminddb.Open(filepath.Join(downloadDir, "geoip-country-2026-10.mmdb"))

	s.trackers = newTrackerPool(downloadDir)
	s.trackers.start(s.refreshTorrentTrackers)
	s.loadPeerHints()
	s.loadDHTContacts()
	s.warmDHTContacts()
	go s.rateTrackerLoop()

	return s, nil
}

func (s *EngineServer) rateTrackerLoop() {
	ticker := time.NewTicker(1 * time.Second)
	defer ticker.Stop()
	var previous [sha256.Size]byte
	seedTicks := 0

	for {
		select {
		case <-s.done:
			return
		case <-ticker.C:
		}
		s.mu.Lock()
		stats := s.client.Stats()
		now := time.Now()
		elapsed := now.Sub(s.lastRateTime).Seconds()
		if elapsed >= 0.5 {
			// BytesReadData captures real-time chunks as they arrive over the wire
			curDown := stats.ConnStats.BytesReadData.Int64()
			curUp := stats.ConnStats.BytesWrittenData.Int64()
			instDown := float64(curDown-s.prevDownloadBytes) / elapsed
			instUp := float64(curUp-s.prevUploadBytes) / elapsed
			if instDown < 0 {
				instDown = 0
			}
			if instUp < 0 {
				instUp = 0
			}

			// Smooth out speeds with Exponential Moving Average (60% history, 40% current)
			if s.downloadSpeed == 0 {
				s.downloadSpeed = int64(instDown)
			} else {
				s.downloadSpeed = int64(0.6*float64(s.downloadSpeed) + 0.4*instDown)
			}
			if s.uploadSpeed == 0 {
				s.uploadSpeed = int64(instUp)
			} else {
				s.uploadSpeed = int64(0.6*float64(s.uploadSpeed) + 0.4*instUp)
			}

			s.prevDownloadBytes = curDown
			s.prevUploadBytes = curUp
			s.lastRateTime = now
		}
		s.mu.Unlock()
		s.sampleTorrentRates(now)
		s.seedTick()
		seedTicks++
		if seedTicks%30 == 0 {
			s.saveSeedAccounts()
		}
		current := s.statusFingerprint()
		if current != previous {
			previous = current
			s.notifyChange()
		}
	}
}

// Sample telemetry using the existing rate clock. Only a changed snapshot wakes
// Android; final verification, idle peer changes and completion are not lost.
// Cheap counters only: do not walk or JSON-encode PieceStateRuns here. Playback
// priority churn must not wake the UI every second.
func (s *EngineServer) statusFingerprint() [sha256.Size]byte {
	s.mu.RLock()
	records := make([]*TorrentRecord, 0, len(s.records))
	for _, rec := range s.records {
		records = append(records, rec)
	}
	downSpeed, upSpeed := s.downloadSpeed, s.uploadSpeed
	s.mu.RUnlock()
	sort.Slice(records, func(i, j int) bool { return records[i].ID < records[j].ID })
	h := sha256.New()
	fmt.Fprintf(h, "%d/%d;", downSpeed, upSpeed)
	for _, rec := range records {
		rec.mu.RLock()
		t := rec.Torrent
		stats := t.Stats()
		err := ""
		if rec.Error != nil {
			err = *rec.Error
		}
		fmt.Fprintf(h, "%s:%d:%t:%t:%t:%d:%d:%d:%d:%d:%d:%d:%d:%d:%d:%s;",
			rec.ID, rec.Generation, rec.Paused, rec.Checking, rec.Configured,
			rec.CheckedPieces, rec.CheckTotal, len(rec.Selected),
			stats.ActivePeers, stats.PiecesComplete, stats.BytesHashed.Int64(),
			stats.BytesReadData.Int64(), stats.BytesWrittenData.Int64(),
			rec.rates.down, rec.rates.up, err)
		rec.mu.RUnlock()
	}
	var result [sha256.Size]byte
	copy(result[:], h.Sum(nil))
	return result
}

// verifiedFileBytesCached returns per-file verified totals. rec.mu must be held.
func (rec *TorrentRecord) verifiedFileBytesCached(t *torrent.Torrent, piecesComplete int) []int64 {
	files := t.Files()
	if rec.verifiedBytes != nil && rec.verifiedGen == piecesComplete && len(rec.verifiedBytes) == len(files) {
		return rec.verifiedBytes
	}
	bytes := verifiedFileBytes(t)
	rec.verifiedGen = piecesComplete
	rec.verifiedBytes = bytes
	return bytes
}

func (s *EngineServer) notifyChange() {
	s.eventMu.Lock()
	s.eventVersion++
	if s.eventCh != nil {
		close(s.eventCh)
	}
	s.eventCh = make(chan struct{})
	s.eventMu.Unlock()
}

func (s *EngineServer) WaitForChange(after uint64, timeout time.Duration) uint64 {
	s.eventMu.Lock()
	if s.eventCh == nil {
		s.eventCh = make(chan struct{})
	}
	if s.eventVersion != after {
		version := s.eventVersion
		s.eventMu.Unlock()
		return version
	}
	changed := s.eventCh
	s.eventMu.Unlock()

	timer := time.NewTimer(timeout)
	defer timer.Stop()
	select {
	case <-changed:
	case <-timer.C:
	case <-s.done:
	}
	s.eventMu.Lock()
	version := s.eventVersion
	s.eventMu.Unlock()
	return version
}

func (s *EngineServer) Start() error {
	ctlAddr := fmt.Sprintf("127.0.0.1:%d", s.ctlPort)
	ctlLn, err := net.Listen("tcp", ctlAddr)
	if err != nil {
		// Fallback to random port if specified port fails
		ctlLn, err = net.Listen("tcp", "127.0.0.1:0")
		if err != nil {
			return fmt.Errorf("ctl server listen error: %w", err)
		}
	}
	s.ctlListener = ctlLn
	s.ctlPort = ctlLn.Addr().(*net.TCPAddr).Port

	ctlMux := http.NewServeMux()
	ctlMux.HandleFunc("/", s.handleControl)
	go func() {
		_ = http.Serve(s.ctlListener, ctlMux)
	}()

	if err := s.startStreamListener(); err != nil {
		_ = s.ctlListener.Close()
		return err
	}

	log.Printf("webtor-go engine started: ctlPort=%d streamPort=%d path=%s", s.ctlPort, s.streamPort, s.downloadDir)
	return nil
}

// StartStreaming starts only the media endpoint required by Android players.
// Android control commands use DispatchControl directly through JNI.
func (s *EngineServer) StartStreaming() error {
	s.ctlPort = 0
	if err := s.startStreamListener(); err != nil {
		return err
	}
	log.Printf("webtor-go engine started: JNI control streamPort=%d path=%s", s.streamPort, s.downloadDir)
	return nil
}

func (s *EngineServer) startStreamListener() error {
	streamLn, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return fmt.Errorf("stream server listen error: %w", err)
	}
	s.streamListener = streamLn
	s.streamPort = streamLn.Addr().(*net.TCPAddr).Port
	streamMux := http.NewServeMux()
	streamMux.HandleFunc("/torrent/", s.handleStream)
	go func() {
		_ = http.Serve(s.streamListener, streamMux)
	}()
	return nil
}

func (s *EngineServer) Done() <-chan struct{} { return s.done }

func (s *EngineServer) Close() {
	s.closeOnce.Do(func() {
		s.shuttingDown.Store(true)
		s.stopNetworkRecovery()
		s.closeActivePreview()
		s.bypass.mu.Lock()
		s.deactivateBypassLocked()
		s.bypass.mu.Unlock()
		s.addMu.Lock()
		defer s.addMu.Unlock()
		s.trackers.close()
		if s.ctlListener != nil {
			_ = s.ctlListener.Close()
		}
		if s.streamListener != nil {
			_ = s.streamListener.Close()
		}
		s.mu.RLock()
		records := make([]*TorrentRecord, 0, len(s.records))
		for _, rec := range s.records {
			records = append(records, rec)
		}
		s.mu.RUnlock()
		for _, rec := range records {
			stopSavedDataCheck(rec)
			s.closePlaybackReaders(rec.Torrent)
		}
		// Capture DHT contacts before the client tears down its servers.
		s.snapshotDHTContacts()
		s.seedTick()
		s.saveSeedAccounts()
		s.client.Close()
		s.geoMu.Lock()
		if s.geoIP != nil {
			_ = s.geoIP.Close()
			s.geoIP = nil
		}
		s.geoMu.Unlock()
		s.notifyChange()
		close(s.done)
	})
}

type controlResponse struct {
	header http.Header
	status int
	body   bytes.Buffer
}

func (w *controlResponse) Header() http.Header { return w.header }

func (w *controlResponse) WriteHeader(status int) {
	if w.status == 0 {
		w.status = status
	}
}

func (w *controlResponse) Write(data []byte) (int, error) {
	if w.status == 0 {
		w.status = http.StatusOK
	}
	return w.body.Write(data)
}

// DispatchControl invokes the same handlers as the development HTTP adapter
// without opening a socket or copying the request through OkHttp.
func (s *EngineServer) DispatchControl(method, path, body string) (status int, payload []byte) {
	defer func() {
		if recovered := recover(); recovered != nil {
			log.Printf("control request panic: %v", recovered)
			status = http.StatusInternalServerError
			payload = []byte(`{"error":"engine request failed"}`)
		}
	}()
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, method, "http://127.0.0.1"+path, strings.NewReader(body))
	if err != nil {
		payload, _ = json.Marshal(map[string]string{"error": "invalid engine request"})
		return http.StatusBadRequest, payload
	}
	req.RemoteAddr = "127.0.0.1:jni"
	w := &controlResponse{header: make(http.Header)}
	s.handleControl(w, req)
	if w.status == 0 {
		w.status = http.StatusOK
	}
	return w.status, w.body.Bytes()
}

func waitForInfo(r *http.Request, t *torrent.Torrent) error {
	select {
	case <-t.GotInfo():
		return nil
	case <-t.Closed():
		return fmt.Errorf("torrent removed")
	case <-r.Context().Done():
		return r.Context().Err()
	}
}

func validateSelection(t *torrent.Torrent, selected []int, allowEmpty bool) error {
	if !allowEmpty && len(selected) == 0 {
		return fmt.Errorf("select at least one file")
	}
	seen := make(map[int]bool, len(selected))
	for _, index := range selected {
		if index < 0 || index >= len(t.Files()) || seen[index] {
			return fmt.Errorf("invalid selected file index %d", index)
		}
		seen[index] = true
	}
	return nil
}

// storageForTorrent makes the anacrolix storage handle observable before a
// configure/select request uses it. Metadata arrival and storage OpenTorrent
// happen on separate goroutines; returning a configured record while the
// storage map is still empty loses the descriptors and makes resume appear to
// succeed while reads fail with "storage is not configured".
func (s *EngineServer) storageForTorrent(t *torrent.Torrent) *DocumentTorrentStorage {
	if t == nil {
		return nil
	}
	if st := s.storage.GetStorage(t.InfoHash().HexString()); st != nil {
		return st
	}
	if t.Info() != nil && t.NumPieces() > 0 {
		_ = t.Piece(0).Storage()
	}
	return s.storage.GetStorage(t.InfoHash().HexString())
}

func newRecordID() string {
	var raw [16]byte
	if _, err := rand.Read(raw[:]); err != nil {
		return fmt.Sprintf("torrent-%d", time.Now().UnixNano())
	}
	raw[6] = (raw[6] & 0x0f) | 0x40
	raw[8] = (raw[8] & 0x3f) | 0x80
	encoded := make([]byte, hex.EncodedLen(len(raw)))
	hex.Encode(encoded, raw[:])
	return fmt.Sprintf("%s-%s-%s-%s-%s", encoded[0:8], encoded[8:12], encoded[12:16], encoded[16:20], encoded[20:32])
}

func (s *EngineServer) watchMetadata(rec *TorrentRecord) {
	select {
	case <-rec.Torrent.GotInfo():
		s.supplementTrackers(rec)
		s.notifyChange()
	case <-rec.Torrent.Closed():
	case <-s.done:
	}
}

func (s *EngineServer) handleControl(w http.ResponseWriter, r *http.Request) {
	if s.shuttingDown.Load() {
		writeJSON(w, http.StatusServiceUnavailable, map[string]string{"error": "engine is shutting down"})
		return
	}
	// Guard against non-local requests
	if !strings.HasPrefix(r.RemoteAddr, "127.0.0.1:") && !strings.HasPrefix(r.RemoteAddr, "[::1]:") {
		http.Error(w, "Forbidden", http.StatusForbidden)
		return
	}

	path := strings.Trim(r.URL.Path, "/")
	parts := strings.Split(path, "/")

	switch r.Method {
	case http.MethodGet:
		if len(parts) == 2 && parts[0] == "preview-state" {
			s.handlePreviewState(w, r, parts[1])
			return
		}
		if len(parts) == 1 && parts[0] == "stats" {
			s.handleStats(w, r)
			return
		}
		if len(parts) >= 2 && parts[0] == "torrent" {
			s.handleGetTorrent(w, r, parts[1])
			return
		}
		if len(parts) >= 2 && parts[0] == "pieces" {
			s.handlePieces(w, r, parts[1])
			return
		}
		if len(parts) == 2 && parts[0] == "connections" {
			s.handleConnections(w, r, parts[1])
			return
		}
		if len(parts) >= 2 && parts[0] == "metadata" {
			s.handleMetadata(w, r, parts[1])
			return
		}
		if len(parts) == 1 && parts[0] == "settings" {
			s.handleGetSettings(w, r)
			return
		}
		if len(parts) == 1 && parts[0] == "bypass" {
			s.handleGetBypass(w, r)
			return
		}
		if len(parts) == 1 && parts[0] == "cache" {
			s.handleGetCache(w, r)
			return
		}
	case http.MethodPost:
		if len(parts) == 1 && parts[0] == "network" {
			s.handleNetworkChange(w, r)
			return
		}
		if len(parts) == 1 && parts[0] == "bypass" {
			s.handlePostBypass(w, r)
			return
		}
		if len(parts) == 2 && parts[0] == "cache" && parts[1] == "clear" {
			s.handleClearCache(w, r)
			return
		}
		if len(parts) == 1 && parts[0] == "add" {
			s.handleAdd(w, r)
			return
		}
		if len(parts) == 1 && parts[0] == "play" {
			s.handlePlay(w, r)
			return
		}
		if len(parts) == 1 && parts[0] == "focus" {
			s.handleFocus(w, r)
			return
		}
		if len(parts) == 1 && parts[0] == "configure" {
			s.handleConfigure(w, r)
			return
		}
		if len(parts) == 1 && parts[0] == "select" {
			s.handleSelect(w, r)
			return
		}
		if len(parts) == 1 && (parts[0] == "pause" || parts[0] == "resume") {
			s.handlePauseResume(w, r, parts[0] == "pause")
			return
		}
		if len(parts) == 1 && parts[0] == "remove" {
			s.handleRemove(w, r)
			return
		}
		if len(parts) == 1 && parts[0] == "settings" {
			s.handlePostSettings(w, r)
			return
		}
		if len(parts) == 1 && parts[0] == "shutdown" {
			s.handleShutdown(w, r)
			return
		}
	}

	writeJSON(w, http.StatusNotFound, map[string]string{"error": "not found"})
}

// handleFocus sets a lower-priority background focus. Playback readers use the
// shared coordinator and apply PiecePriorityNow/Readahead afterward, so active
// reads always win over this hint.
func (s *EngineServer) handleFocus(w http.ResponseWriter, r *http.Request) {
	var req FocusRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid json"})
		return
	}
	s.mu.RLock()
	rec, ok := s.records[req.ID]
	s.mu.RUnlock()
	if !ok {
		writeJSON(w, http.StatusNotFound, map[string]string{"error": "torrent not found"})
		return
	}
	rec.mu.Lock()
	defer rec.mu.Unlock()
	if req.FileIndex != nil {
		if *req.FileIndex < 0 || *req.FileIndex >= len(rec.Torrent.Files()) {
			writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid file index"})
			return
		}
		selected := false
		for _, i := range rec.Selected {
			if i == *req.FileIndex {
				selected = true
				break
			}
		}
		if !selected {
			writeJSON(w, http.StatusBadRequest, map[string]string{"error": "file is not selected"})
			return
		}
		v := *req.FileIndex
		rec.FocusedFile = &v
	} else {
		rec.FocusedFile = nil
	}
	s.applyTransferState(rec)
	s.applyBackgroundFocus(rec)
	writeJSON(w, http.StatusOK, OkResponse{Ok: true, ID: rec.ID})
	s.notifyChange()
}

func (s *EngineServer) applyBackgroundFocus(rec *TorrentRecord) {
	c := s.ensurePlaybackCoordinator(rec.Torrent)
	if rec.DirectPlay {
		c.setFocus(nil)
		return
	}
	if !s.payloadTransfersAllowed.Load() || rec.FocusedFile == nil || rec.Paused || !rec.Configured || rec.Checking {
		c.setFocus(nil)
		return
	}
	files := rec.Torrent.Files()
	idx := *rec.FocusedFile
	if idx < 0 || idx >= len(files) {
		c.setFocus(nil)
		return
	}
	demand := make(map[int]torrent.PiecePriority)
	for p := files[idx].BeginPieceIndex(); p < files[idx].EndPieceIndex(); p++ {
		demand[p] = torrent.PiecePriorityHigh
	}
	c.setFocus(demand)
}

func (s *EngineServer) handleStats(w http.ResponseWriter, r *http.Request) {
	s.mu.RLock()
	defer s.mu.RUnlock()

	totalTorrents := len(s.records)
	var totalProgress float64
	if totalTorrents > 0 {
		var sum float64
		for _, rec := range s.records {
			if rec.Torrent.Info() != nil && rec.Torrent.Length() > 0 {
				sum += float64(rec.Torrent.BytesCompleted()) / float64(rec.Torrent.Length())
			}
		}
		totalProgress = sum / float64(totalTorrents)
	}

	stats := StatsResponse{
		DownloadSpeed:           s.downloadSpeed,
		UploadSpeed:             s.uploadSpeed,
		Progress:                totalProgress,
		Ratio:                   0.0,
		Torrents:                totalTorrents,
		CtlPort:                 s.ctlPort,
		StreamPort:              s.streamPort,
		Path:                    s.downloadDir,
		PayloadTransfersAllowed: s.payloadTransfersAllowed.Load(),
	}

	writeJSON(w, http.StatusOK, stats)
}

func (s *EngineServer) handleAdd(w http.ResponseWriter, r *http.Request) {
	var req AddRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid json"})
		return
	}

	if req.TorrentID == "" && req.TorrentData == "" {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "torrentId required"})
		return
	}
	s.addMu.Lock()
	defer s.addMu.Unlock()
	if s.shuttingDown.Load() {
		writeJSON(w, http.StatusServiceUnavailable, map[string]string{"error": "engine is shutting down"})
		return
	}

	var t *torrent.Torrent
	var err error

	if req.TorrentData != "" {
		data, errDecode := base64.StdEncoding.DecodeString(req.TorrentData)
		if errDecode != nil {
			writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid torrentData base64"})
			return
		}
		mi, errMeta := metainfo.Load(bytes.NewReader(data))
		if errMeta != nil {
			writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid torrent data"})
			return
		}
		s.mu.RLock()
		existing, ok := s.recordsByHash[mi.HashInfoBytes().HexString()]
		s.mu.RUnlock()
		if ok {
			writeJSON(w, http.StatusOK, AddResponse{ID: existing.ID, InfoHash: &existing.InfoHash})
			return
		}
		spec, specErr := torrent.TorrentSpecFromMetaInfoErr(mi)
		if specErr != nil {
			writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid torrent metadata"})
			return
		}
		t, _, err = s.addTorrentSpec(spec)
	} else if strings.HasPrefix(req.TorrentID, "magnet:") {
		spec, errMag := torrent.TorrentSpecFromMagnetUri(req.TorrentID)
		if errMag != nil {
			writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid magnet link"})
			return
		}
		s.mu.RLock()
		existing, ok := s.recordsByHash[spec.InfoHash.HexString()]
		s.mu.RUnlock()
		if ok {
			writeJSON(w, http.StatusOK, AddResponse{ID: existing.ID, InfoHash: &existing.InfoHash})
			return
		}
		t, _, err = s.addTorrentSpec(spec)
	} else if _, errStat := os.Stat(req.TorrentID); errStat == nil {
		mi, errLoad := metainfo.LoadFromFile(req.TorrentID)
		if errLoad != nil {
			writeJSON(w, http.StatusBadRequest, map[string]string{"error": "cannot load torrent file"})
			return
		}
		s.mu.RLock()
		existing, ok := s.recordsByHash[mi.HashInfoBytes().HexString()]
		s.mu.RUnlock()
		if ok {
			writeJSON(w, http.StatusOK, AddResponse{ID: existing.ID, InfoHash: &existing.InfoHash})
			return
		}
		spec, specErr := torrent.TorrentSpecFromMetaInfoErr(mi)
		if specErr != nil {
			writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid torrent metadata"})
			return
		}
		t, _, err = s.addTorrentSpec(spec)
	} else {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "unsupported torrentId"})
		return
	}

	if err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
		return
	}

	// Metadata exchange does not require piece priorities. Payload transfer is
	// enabled only after Android has attached its destination descriptors.
	t.DisallowDataDownload()
	t.DisallowDataUpload()

	id := newRecordID()
	hashHex := t.InfoHash().HexString()

	rec := &TorrentRecord{
		seedBase:   s.seedAccount(hashHex),
		ID:         id,
		InfoHash:   hashHex,
		Torrent:    t,
		Prepared:   req.Prepare,
		Configured: !req.Prepare,
		Selected:   []int{},
		Generation: 1,
		Paused:     false,
		rates:      transferRates{lastSample: time.Now()},
	}
	// Keep managed supplements out of exported metadata and the persisted
	// magnet, otherwise each restore would promote them to permanent originals.
	mi := t.Metainfo()
	for _, tier := range mi.UpvertedAnnounceList() {
		rec.OriginalTrackers = append(rec.OriginalTrackers, append([]string(nil), tier...))
	}

	s.mu.Lock()
	s.records[id] = rec
	s.recordsByHash[hashHex] = rec
	s.mu.Unlock()
	go s.watchMetadata(rec)

	writeJSON(w, http.StatusOK, AddResponse{
		ID:       id,
		InfoHash: &hashHex,
	})
	s.notifyChange()
}

func (s *EngineServer) handleGetTorrent(w http.ResponseWriter, r *http.Request, id string) {
	s.mu.RLock()
	rec, ok := s.records[id]
	s.mu.RUnlock()

	if !ok {
		writeJSON(w, http.StatusNotFound, map[string]string{"error": "torrent not found"})
		return
	}
	rec.mu.Lock()
	defer rec.mu.Unlock()
	downSpeed, upSpeed := rec.rates.down, rec.rates.up
	if rec.Paused {
		downSpeed, upSpeed = 0, 0
	}

	t := rec.Torrent
	tStats := t.Stats()
	ready := t.Info() != nil
	var name *string
	infoHashStr := rec.InfoHash
	ih := t.InfoHash()
	var magnetURIStr string
	if ready {
		magnetURIStr = recordMetainfo(rec).Magnet(&ih, t.Info()).String()
	} else {
		mag := metainfo.Magnet{InfoHash: ih}
		for _, tier := range rec.OriginalTrackers {
			mag.Trackers = append(mag.Trackers, tier...)
		}
		magnetURIStr = mag.String()
	}

	var totalLen int64
	var received int64
	var verifiedTotal int64
	var files []FileView
	var progress float64

	if ready {
		n := t.Name()
		name = &n
		tfFiles := t.Files()
		verified := rec.verifiedFileBytesCached(t, tStats.PiecesComplete)
		// Cache eviction can move completion between pieces without changing
		// their count. Stream snapshots must use the current distribution.
		if rec.DirectPlay {
			verified = verifiedFileBytes(t)
		}
		selected := make(map[int]bool, len(rec.Selected))
		for _, index := range rec.Selected {
			selected[index] = true
		}
		files = make([]FileView, len(tfFiles))
		for i, f := range tfFiles {
			fLen := f.Length()
			// Display progress uses received bytes (partial pieces included) so
			// the UI bar/MB ticks continuously. VerifiedBytes stay authoritative
			// for Done / offline-safe completion.
			got := f.BytesCompleted()
			if got < verified[i] {
				got = verified[i]
			}
			if got > fLen && fLen > 0 {
				got = fLen
			}
			fProg := 1.0
			if fLen > 0 {
				fProg = float64(got) / float64(fLen)
			}
			if selected[i] {
				totalLen += fLen
				received += got
				verifiedTotal += verified[i]
			}
			ext := filepath.Ext(f.DisplayPath())
			mimeType := mime.TypeByExtension(ext)
			if mimeType == "" {
				mimeType = "application/octet-stream"
			}

			files[i] = FileView{
				Index:         i,
				Name:          filepath.Base(f.DisplayPath()),
				Path:          f.DisplayPath(),
				Length:        fLen,
				Progress:      math.Min(fProg, 1.0),
				VerifiedBytes: verified[i],
				Type:          mimeType,
			}
		}
		if totalLen > 0 {
			progress = float64(received) / float64(totalLen)
		}
	}

	// An explicit empty selection means “download nothing”, not a completed
	// torrent. This distinction matters when users uncheck every file.
	done := !rec.DirectPlay && ready && len(rec.Selected) > 0 && !rec.Checking && totalLen > 0 && verifiedTotal >= totalLen
	numPeers := tStats.ActivePeers
	if numPeers == 0 {
		numPeers = len(t.PeerConns())
	}

	var timeRemaining *int64
	if !rec.DirectPlay && downSpeed > 0 && totalLen > received {
		rem := int64(float64(totalLen-received) / float64(downSpeed) * 1000)
		timeRemaining = &rem
	}

	warning := rec.Warning
	if warning == nil {
		if st := s.storage.GetStorage(rec.InfoHash); st != nil {
			if err := st.CheckpointPersistError(); err != nil {
				msg := "checkpoint persist failed: " + err.Error()
				warning = &msg
			}
		}
	}
	view := TorrentView{
		ID:            rec.ID,
		InfoHash:      &infoHashStr,
		Name:          name,
		MagnetURI:     &magnetURIStr,
		Ready:         ready,
		Done:          done,
		Paused:        rec.Paused,
		Progress:      progress,
		DownloadSpeed: downSpeed,
		UploadSpeed:   upSpeed,
		NumPeers:      numPeers,
		Length:        totalLen,
		Downloaded:    received,
		VerifiedBytes: verifiedTotal,
		Uploaded:      seedTotals(rec).Uploaded,
		Seeding:       rec.Seeding,
		SeedRatio:     float64(seedTotals(rec).Uploaded) / float64(max(totalLen, seedTotals(rec).Downloaded, 1)),
		Files:         files,
		Trackers:      buildTrackerViews(rec, s.trackerStatusSnapshot(infoHashStr)),
		Error:         rec.Error,
		TimeRemaining: timeRemaining,
		Configured:    rec.Configured,
		Selected:      rec.Selected,
		FocusedFile:   rec.FocusedFile,
		Checking:      rec.Checking,
		CheckedPieces: rec.CheckedPieces,
		CheckTotal:    rec.CheckTotal,
		Warning:       warning,
	}

	writeJSON(w, http.StatusOK, view)
}

// Count only verified pieces. Display progress uses File.BytesCompleted
// instead; verified bytes decide Done and offline-safe playback.
func verifiedFileBytes(t *torrent.Torrent) []int64 {
	files := t.Files()
	verified := make([]int64, len(files))
	info := t.Info()
	if info == nil {
		return verified
	}
	var start int64
	fileIndex := 0
	for _, run := range t.PieceStateRuns() {
		end := min(start+int64(run.Length)*info.PieceLength, t.Length())
		if run.Complete && run.Ok {
			for fileIndex < len(files) && files[fileIndex].Offset()+files[fileIndex].Length() <= start {
				fileIndex++
			}
			for i := fileIndex; i < len(files) && files[i].Offset() < end; i++ {
				f := files[i]
				verified[i] += max(int64(0), min(end, f.Offset()+f.Length())-max(start, f.Offset()))
			}
		}
		start = end
	}
	return verified
}

func (s *EngineServer) handlePlay(w http.ResponseWriter, r *http.Request) {
	var req PlayRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid json"})
		return
	}

	s.mu.RLock()
	rec, ok := s.records[req.ID]
	s.mu.RUnlock()

	if !ok {
		writeJSON(w, http.StatusNotFound, map[string]string{"error": "torrent not found"})
		return
	}
	rec.mu.Lock()
	defer rec.mu.Unlock()

	t := rec.Torrent
	if err := waitForInfo(r, t); err != nil {
		writeJSON(w, http.StatusConflict, map[string]string{"error": err.Error()})
		return
	}
	files := t.Files()
	if len(files) == 0 {
		writeJSON(w, http.StatusNotFound, map[string]string{"error": "no files in torrent"})
		return
	}

	if !rec.Configured {
		writeJSON(w, http.StatusConflict, map[string]string{"error": "download storage is not configured"})
		return
	}
	selected := make(map[int]bool, len(rec.Selected))
	for _, i := range rec.Selected {
		selected[i] = true
	}
	fileIdx := -1
	if req.FileIndex != nil {
		if *req.FileIndex < 0 || *req.FileIndex >= len(files) || !selected[*req.FileIndex] {
			writeJSON(w, http.StatusBadRequest, map[string]string{"error": "file is not selected"})
			return
		}
		fileIdx = *req.FileIndex
	} else {
		maxLen := int64(-1)
		for i, f := range files {
			if selected[i] && f.Length() > maxLen {
				maxLen = f.Length()
				fileIdx = i
			}
		}
	}
	if fileIdx < 0 {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "no selected file"})
		return
	}

	targetFile := files[fileIdx]
	// A completed local file remains playable while the global data policy is
	// off. An incomplete stream would otherwise appear to start and then hang
	// waiting for bytes which this policy deliberately blocks.
	if !s.payloadTransfersAllowed.Load() && !fileIsVerified(t, targetFile) {
		writeJSON(w, http.StatusConflict, map[string]string{
			"error": "payload transfers are disabled in settings",
		})
		return
	}
	// Persist one focused file so status and future priority recalculation agree
	// with the active playback demand. The playback coordinator still owns
	// urgent read regions and therefore wins over this background focus.
	focused := fileIdx
	rec.FocusedFile = &focused
	if !rec.DirectPlay {
		targetFile.Download()
	}
	s.applyVideoStartupHints(rec)
	s.applyBackgroundFocus(rec)
	coordinator := s.ensurePlaybackCoordinator(t)
	coordinator.prepareResume(targetFile, req.Offset)

	streamURL := fmt.Sprintf("http://127.0.0.1:%d/torrent/%s/file/%d", s.streamPort, rec.ID, fileIdx)

	writeJSON(w, http.StatusOK, PlayResponse{
		ID:        rec.ID,
		FileIndex: fileIdx,
		Name:      filepath.Base(targetFile.DisplayPath()),
		Length:    targetFile.Length(),
		StreamURL: streamURL,
	})
}

func (s *EngineServer) handleConfigure(w http.ResponseWriter, r *http.Request) {
	var req ConfigureRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid json"})
		return
	}

	s.mu.Lock()
	rec, ok := s.records[req.ID]
	s.mu.Unlock()

	if !ok {
		writeJSON(w, http.StatusNotFound, map[string]string{"error": "torrent not found"})
		return
	}
	rec.mu.Lock()
	defer rec.mu.Unlock()
	if rec.removed || s.shuttingDown.Load() {
		writeJSON(w, http.StatusConflict, map[string]string{"error": "torrent is closing"})
		return
	}
	// Configuration replaces temporary preview storage for this torrent only.
	s.closePreviewFor(rec)

	t := rec.Torrent
	if err := waitForInfo(r, t); err != nil {
		writeJSON(w, http.StatusConflict, map[string]string{"error": err.Error()})
		return
	}
	// An empty selection is a valid configured state (for example after the
	// user unchecks every file). The engine remains metadata-ready and can be
	// reconfigured through /select without treating the torrent as complete.
	if err := validateSelection(t, req.Selected, true); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": err.Error()})
		return
	}

	st := s.storageForTorrent(t)
	if st == nil {
		writeJSON(w, http.StatusConflict, map[string]string{"error": "download storage is not available"})
		return
	}
	if req.StorageMode != "" && req.StorageMode != StorageModeDownload && req.StorageMode != StorageModeStream {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid storageMode"})
		return
	}
	direct := req.StorageMode == StorageModeStream
	if direct {
		st.mu.Lock()
		if req.StreamCacheBytes > 0 {
			st.streamMaxBytes = max(int64(16<<20), min(int64(96<<20), req.StreamCacheBytes))
		}
		st.onStreamEvict = func(index int) {
			rec.mu.RLock()
			defer rec.mu.RUnlock()
			if !rec.removed {
				t.Piece(index).UpdateCompletion()
			}
		}
		st.mu.Unlock()
	}
	if err := st.AttachDescriptorsMode(req.Descriptors, req.Selected, direct); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": err.Error()})
		return
	}
	rec.Configured = true
	rec.DirectPlay = direct
	if direct {
		// Temporary pressure must not trigger the library's default permanent
		// download-disable handler. It requeues the failed chunk itself.
		t.SetOnWriteChunkError(func(err error) {
			if !errors.Is(err, errStreamBusy) {
				t.DisallowDataDownload()
			}
		})
	}
	rec.Selected = req.Selected
	if rec.FocusedFile != nil {
		keep := false
		for _, i := range req.Selected {
			if i == *rec.FocusedFile {
				keep = true
				break
			}
		}
		if !keep {
			rec.FocusedFile = nil
		}
	}
	rec.Paused = false
	rec.Generation++

	if st != nil && st.NeedsVerify() {
		s.startSavedDataCheck(rec)
	} else if st != nil {
		st.UnfreezeTrusted()
		s.publishStorageCompletions(rec)
	}
	s.applyPeerHints(rec)
	s.applyTransferState(rec)

	writeJSON(w, http.StatusOK, OkResponse{Ok: true})
	s.notifyChange()
}

func (s *EngineServer) handleSelect(w http.ResponseWriter, r *http.Request) {
	var req SelectRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid json"})
		return
	}

	s.mu.Lock()
	rec, ok := s.records[req.ID]
	s.mu.Unlock()

	if !ok {
		writeJSON(w, http.StatusNotFound, map[string]string{"error": "torrent not found"})
		return
	}
	rec.mu.Lock()
	defer rec.mu.Unlock()

	t := rec.Torrent
	if err := waitForInfo(r, t); err != nil {
		writeJSON(w, http.StatusConflict, map[string]string{"error": err.Error()})
		return
	}
	if err := validateSelection(t, req.Selected, true); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": err.Error()})
		return
	}
	if !rec.Configured {
		writeJSON(w, http.StatusConflict, map[string]string{"error": "download storage is not configured"})
		return
	}
	st := s.storageForTorrent(t)
	if st == nil {
		writeJSON(w, http.StatusConflict, map[string]string{"error": "download storage is not configured"})
		return
	}
	if rec.DirectPlay {
		// RAM-backed files need no Android destination descriptors.
	} else if req.Descriptors != nil {
		if err := st.UpdateSelection(req.Descriptors, req.Selected); err != nil {
			writeJSON(w, http.StatusBadRequest, map[string]string{"error": err.Error()})
			return
		}
	} else if !st.SelectionCanUseExistingFiles(req.Selected) {
		writeJSON(w, http.StatusBadRequest, map[string]string{
			"error": "descriptors are required when selecting a file for the first time",
		})
		return
	}

	rec.Selected = req.Selected
	rec.Generation++

	s.applyTransferState(rec)

	writeJSON(w, http.StatusOK, SelectResponse{Ok: true, Selected: req.Selected})
	s.notifyChange()
}

func (s *EngineServer) handlePauseResume(w http.ResponseWriter, r *http.Request, pause bool) {
	var req IdRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid json"})
		return
	}

	s.mu.Lock()
	rec, ok := s.records[req.ID]
	s.mu.Unlock()

	if !ok {
		writeJSON(w, http.StatusNotFound, map[string]string{"error": "torrent not found"})
		return
	}
	rec.mu.Lock()
	defer rec.mu.Unlock()

	// A prepared magnet can resume discovery before metadata/storage exist.
	// applyTransferState still requires Configured before enabling file bytes.
	rec.Generation++
	rec.Paused = pause
	if pause {
		s.closePreviewFor(rec)
	}
	if !pause && rec.Error != nil {
		// A failed saved-data check is recoverable: resume clears the visible
		// error and starts a fresh check before allowing payload transfer. This
		// keeps retry useful while preserving the safety gate for unverified data.
		rec.Error = nil
		if st := s.storage.GetStorage(rec.InfoHash); st != nil && st.NeedsVerify() {
			s.startSavedDataCheck(rec)
		}
	}
	s.applyTransferState(rec)
	if rec.verification != nil {
		select {
		case rec.verification.wake <- struct{}{}:
		default:
		}
	}

	writeJSON(w, http.StatusOK, OkResponse{Ok: true})
	s.notifyChange()
}

func (s *EngineServer) handleRemove(w http.ResponseWriter, r *http.Request) {
	var req IdRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid json"})
		return
	}
	s.addMu.Lock()
	defer s.addMu.Unlock()

	s.mu.Lock()
	rec, ok := s.records[req.ID]
	if ok {
		delete(s.records, req.ID)
		delete(s.recordsByHash, rec.InfoHash)
	}
	s.mu.Unlock()

	if !ok {
		writeJSON(w, http.StatusNotFound, map[string]string{"error": "torrent not found"})
		return
	}
	s.clearTrackerStatus(rec.InfoHash)

	stopSavedDataCheck(rec)
	destroy := req.DestroyStore == nil || *req.DestroyStore
	// Drop flushes storage; clear the durable bitfield after Close so remove
	// cannot leave a resume shortcut for deleted library entries.
	if st := s.storage.GetStorage(rec.InfoHash); st != nil {
		_ = st.DeleteCheckpoint()
	}
	s.closePreviewFor(rec)
	s.closePlaybackReaders(rec.Torrent)
	s.forgetPeerHints(rec.InfoHash)
	s.deleteResumeWindow(rec.InfoHash)
	rec.Torrent.Drop()
	_ = s.storage.DeleteCheckpoint(rec.InfoHash)
	if destroy {
		_ = DeleteEngineOwnedStorage(s.downloadDir, rec.InfoHash)
	}
	writeJSON(w, http.StatusOK, OkResponse{Ok: true, ID: req.ID})
	s.notifyChange()
}

func (s *EngineServer) handlePieces(w http.ResponseWriter, r *http.Request, id string) {
	s.mu.RLock()
	rec, ok := s.records[id]
	s.mu.RUnlock()

	if !ok {
		writeJSON(w, http.StatusNotFound, map[string]string{"error": "torrent not found"})
		return
	}
	rec.mu.RLock()
	defer rec.mu.RUnlock()

	t := rec.Torrent
	if t.Info() == nil {
		writeJSON(w, http.StatusConflict, map[string]string{"error": "Metadata not ready"})
		return
	}

	maxBuckets := 256
	if mb := r.URL.Query().Get("maxBuckets"); mb != "" {
		if n, err := strconv.Atoi(mb); err == nil && n > 0 {
			maxBuckets = n
		}
	}
	if maxBuckets > 1024 {
		maxBuckets = 1024
	}

	totalPieces := t.NumPieces()
	pieceLength := t.Info().PieceLength
	lastPieceLength := t.Length() - int64(totalPieces-1)*pieceLength

	bucketList, err := pieceBuckets(t, rec.Selected, maxBuckets)
	if err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
		return
	}

	resp := PieceTelemetryResponse{
		ID:              rec.ID,
		InfoHash:        &rec.InfoHash,
		Generation:      rec.Generation,
		Timestamp:       time.Now().UnixMilli(),
		TotalPieces:     totalPieces,
		PieceLength:     pieceLength,
		LastPieceLength: lastPieceLength,
		MaxBuckets:      maxBuckets,
		Buckets:         bucketList,
	}

	writeJSON(w, http.StatusOK, resp)
}

// accumulateBucketPiece updates inclusive bucket counters for one piece.
// Verified and Receiving cover all pieces; Selected* only intersect selection.
// Receiving is mutually exclusive with verified (complete never counts as receiving).
func accumulateBucketPiece(b *PieceBucket, selected, complete, receiving bool) {
	if selected {
		b.Selected++
	}
	if complete {
		b.Verified++
		if selected {
			b.SelectedVerified++
		}
		return
	}
	if receiving {
		b.Receiving++
		if selected {
			b.SelectedReceiving++
		}
	}
}

func (s *EngineServer) handleMetadata(w http.ResponseWriter, r *http.Request, id string) {
	s.mu.RLock()
	rec, ok := s.records[id]
	s.mu.RUnlock()

	if !ok {
		writeJSON(w, http.StatusNotFound, map[string]string{"error": "torrent not found"})
		return
	}
	rec.mu.RLock()
	defer rec.mu.RUnlock()

	t := rec.Torrent
	if t.Info() == nil {
		writeJSON(w, http.StatusConflict, map[string]string{"error": "Metadata not ready"})
		return
	}

	mi := recordMetainfo(rec)
	var buf bytes.Buffer
	if err := mi.Write(&buf); err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
		return
	}

	writeJSON(w, http.StatusOK, MetadataResponse{
		TorrentData: base64.StdEncoding.EncodeToString(buf.Bytes()),
	})
}

func (s *EngineServer) handleGetSettings(w http.ResponseWriter, r *http.Request) {
	s.mu.RLock()
	peers := s.maxPeers
	down := limiterRate(s.downloadLimiter)
	up := limiterRate(s.uploadLimiter)
	s.mu.RUnlock()
	writeJSON(w, http.StatusOK, SettingsResponse{
		SeedingEnabled: s.seedingEnabled.Load(), SeedRatioMilli: s.seedRatioMilli.Load(), SeedTimeMinutes: s.seedTimeMinutes.Load(),
		MaxPeers:                peers,
		DownloadRateBytesSec:    down,
		UploadRateBytesSec:      up,
		PayloadTransfersAllowed: s.payloadTransfersAllowed.Load(),
	})
}

// limiterRate exposes zero for unlimited to keep the API simple for Android.
func limiterRate(l *rate.Limiter) int64 {
	if l == nil || l.Limit() == rate.Inf {
		return 0
	}
	return int64(l.Limit())
}

func setLimiterRate(l *rate.Limiter, bytesPerSecond int64) {
	if bytesPerSecond <= 0 {
		l.SetLimit(rate.Inf)
		return
	}
	l.SetLimit(rate.Limit(bytesPerSecond))
	burst := bytesPerSecond
	if burst > 1024*1024 {
		burst = 1024 * 1024
	}
	if burst < 1 {
		burst = 1
	}
	l.SetBurst(int(burst))
}

func (s *EngineServer) handlePostSettings(w http.ResponseWriter, r *http.Request) {
	var req SettingsRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid json"})
		return
	}
	s.mu.Lock()
	if req.SeedingEnabled != nil {
		s.seedingEnabled.Store(*req.SeedingEnabled)
	}
	if req.SeedingAllowed != nil {
		s.seedingAllowed.Store(*req.SeedingAllowed)
	}
	if req.SeedRatioMilli != nil && *req.SeedRatioMilli >= 100 && *req.SeedRatioMilli <= 10000 {
		s.seedRatioMilli.Store(*req.SeedRatioMilli)
	}
	if req.SeedTimeMinutes != nil && *req.SeedTimeMinutes >= 1 && *req.SeedTimeMinutes <= 1440 {
		s.seedTimeMinutes.Store(*req.SeedTimeMinutes)
	}
	if req.MaxPeers >= 8 && req.MaxPeers <= 80 {
		s.maxPeers = req.MaxPeers
		for _, rec := range s.records {
			rec.Torrent.SetMaxEstablishedConns(req.MaxPeers)
		}
	}
	if req.DownloadRateBytesSec != nil && *req.DownloadRateBytesSec >= 0 {
		setLimiterRate(s.downloadLimiter, *req.DownloadRateBytesSec)
	}
	if req.UploadRateBytesSec != nil && *req.UploadRateBytesSec >= 0 {
		setLimiterRate(s.uploadLimiter, *req.UploadRateBytesSec)
	}
	if req.PayloadTransfersAllowed != nil {
		s.payloadTransfersAllowed.Store(*req.PayloadTransfersAllowed)
	}
	records := make([]*TorrentRecord, 0, len(s.records))
	for _, rec := range s.records {
		records = append(records, rec)
	}
	peers := s.maxPeers
	down := limiterRate(s.downloadLimiter)
	up := limiterRate(s.uploadLimiter)
	payloadAllowed := s.payloadTransfersAllowed.Load()
	s.mu.Unlock()
	// Re-evaluate every torrent after releasing the map lock. This changes only
	// piece-data admission; it never mutates rec.Paused, so a manual pause is
	// still in force when the global setting is enabled again.
	if req.PayloadTransfersAllowed != nil || req.SeedingEnabled != nil || req.SeedingAllowed != nil || req.SeedRatioMilli != nil || req.SeedTimeMinutes != nil {
		for _, rec := range records {
			rec.mu.Lock()
			s.applyTransferState(rec)
			rec.mu.Unlock()
		}
	}
	writeJSON(w, http.StatusOK, SettingsResponse{
		SeedingEnabled: s.seedingEnabled.Load(), SeedRatioMilli: s.seedRatioMilli.Load(), SeedTimeMinutes: s.seedTimeMinutes.Load(),
		MaxPeers:                peers,
		DownloadRateBytesSec:    down,
		UploadRateBytesSec:      up,
		PayloadTransfersAllowed: payloadAllowed,
	})
	s.notifyChange()
}

func (s *EngineServer) handleShutdown(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, http.StatusOK, OkResponse{Ok: true})
	time.AfterFunc(50*time.Millisecond, s.Close)
}

// Media Streaming handler
func (s *EngineServer) handleStream(w http.ResponseWriter, r *http.Request) {
	// Parse /torrent/{id}/file/{index}
	path := strings.Trim(r.URL.Path, "/")
	parts := strings.Split(path, "/")
	if len(parts) < 4 || parts[0] != "torrent" || parts[2] != "file" {
		http.NotFound(w, r)
		return
	}

	id := parts[1]
	fileIdx, err := strconv.Atoi(parts[3])
	if err != nil {
		http.NotFound(w, r)
		return
	}

	s.mu.RLock()
	rec, ok := s.records[id]
	s.mu.RUnlock()

	if !ok {
		http.NotFound(w, r)
		return
	}
	rec.mu.RLock()
	configured := rec.Configured
	selectedFiles := append([]int(nil), rec.Selected...)
	rec.mu.RUnlock()

	t := rec.Torrent
	if err := waitForInfo(r, t); err != nil {
		http.Error(w, err.Error(), http.StatusConflict)
		return
	}

	files := t.Files()
	if fileIdx < 0 || fileIdx >= len(files) {
		http.NotFound(w, r)
		return
	}

	file := files[fileIdx]
	if !configured {
		http.Error(w, "download storage is not configured", http.StatusConflict)
		return
	}
	selected := false
	for _, i := range selectedFiles {
		if i == fileIdx {
			selected = true
			break
		}
	}
	// A deselected file may still be opened for verified offline playback. It
	// must remain rejected while any byte could require network transfer.
	verified := fileIsVerified(t, file)
	if !selected && !verified {
		http.Error(w, "file is not selected", http.StatusForbidden)
		return
	}
	if !s.payloadTransfersAllowed.Load() && !verified {
		http.Error(w, "payload transfers are disabled in settings", http.StatusConflict)
		return
	}
	playback, err := s.OpenPlayback(id, fileIdx)
	if err != nil {
		http.Error(w, err.Error(), http.StatusConflict)
		return
	}
	defer playback.Close()
	stopCancel := context.AfterFunc(r.Context(), playback.Close)
	defer stopCancel()
	reader := &playbackHTTPReader{playback: playback}

	ext := filepath.Ext(file.DisplayPath())
	mimeType := mime.TypeByExtension(ext)
	if mimeType != "" {
		w.Header().Set("Content-Type", mimeType)
	} else {
		w.Header().Set("Content-Type", "video/mp4")
	}
	w.Header().Set("Accept-Ranges", "bytes")
	w.Header().Set("Access-Control-Allow-Origin", "*")
	w.Header().Set("Access-Control-Allow-Headers", "*")
	w.Header().Set("Cache-Control", "no-cache, no-store, must-revalidate")

	http.ServeContent(w, r, filepath.Base(file.DisplayPath()), time.Time{}, reader)
}

func fileIsVerified(t *torrent.Torrent, file *torrent.File) bool {
	if t == nil || file == nil || file.Length() == 0 {
		return file != nil && file.Length() == 0
	}
	for _, state := range file.State() {
		if !state.Complete || !state.Ok {
			return false
		}
	}
	return true
}

func writeJSON(w http.ResponseWriter, status int, data any) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(data)
}

func urlUnescape(s string) string {
	u, err := url.QueryUnescape(s)
	if err != nil {
		return s
	}
	return u
}
