package main

import (
	"bytes"
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"log"
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
	Selected             []int
	Generation           int64
	Error                *string
	Warning              *string
	DownloadedPre        int64
	Paused               bool
	SupplementalTrackers int
	OriginalTrackers     [][]string
}

type EngineServer struct {
	mu             sync.RWMutex
	addMu          sync.Mutex
	client         *torrent.Client
	trackers       *trackerPool
	storage        *DocumentStorageClient
	records        map[string]*TorrentRecord
	recordsByHash  map[string]*TorrentRecord
	ctlListener    net.Listener
	streamListener net.Listener
	ctlPort        int
	streamPort     int
	downloadDir    string
	maxPeers       int
	shuttingDown   atomic.Bool
	done           chan struct{}
	closeOnce      sync.Once
	eventMu        sync.Mutex
	eventVersion   uint64
	eventCh        chan struct{}

	prevDownloadBytes int64
	prevUploadBytes   int64
	lastRateTime      time.Time
	downloadSpeed     int64
	uploadSpeed       int64
}

func NewEngineServer(ctlPort int, downloadDir string, maxPeers int) (*EngineServer, error) {
	_ = os.MkdirAll(downloadDir, 0755)

	storageClient := NewDocumentStorageClient(downloadDir)

	cfg := torrent.NewDefaultClientConfig()
	cfg.DataDir = downloadDir
	cfg.DefaultStorage = storageClient
	cfg.ListenPort = 0
	cfg.NoDHT = false
	cfg.DisableUTP = false
	cfg.DisableTCP = false
	cfg.DisableWebtorrent = false
	cfg.DisablePEX = false
	cfg.DialForPeerConns = true
	cfg.AcceptPeerConnections = true
	cfg.AlwaysWantConns = true
	cfg.PeriodicallyAnnounceTorrentsToDht = true

	// Balanced peer dialing rate limiter (avoids network/router bufferbloat)
	cfg.DialRateLimiter = rate.NewLimiter(30, 30)

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
	cfg.HTTPDialContext = (&net.Dialer{
		Timeout:   6 * time.Second,
		KeepAlive: 30 * time.Second,
	}).DialContext
	cfg.TrackerDialContext = cfg.HTTPDialContext

	cfg.HTTPUserAgent = "TorrentPlayer/1.4.4"
	cfg.ExtendedHandshakeClientVersion = "Torrent Player 1.4.4 (Go)"

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

	client, err := torrent.NewClient(cfg)
	if err != nil {
		return nil, fmt.Errorf("torrent client init error: %w", err)
	}

	s := &EngineServer{
		client:        client,
		storage:       storageClient,
		records:       make(map[string]*TorrentRecord),
		recordsByHash: make(map[string]*TorrentRecord),
		downloadDir:   downloadDir,
		maxPeers:      maxPeers,
		ctlPort:       ctlPort,
		lastRateTime:  time.Now(),
		done:          make(chan struct{}),
		eventCh:       make(chan struct{}),
	}

	s.trackers = newTrackerPool(downloadDir)
	s.trackers.start(s.refreshTorrentTrackers)
	go s.rateTrackerLoop()

	return s, nil
}

func (s *EngineServer) rateTrackerLoop() {
	ticker := time.NewTicker(1 * time.Second)
	defer ticker.Stop()
	var previous [sha256.Size]byte

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
		current := s.statusFingerprint()
		if current != previous {
			previous = current
			s.notifyChange()
		}
	}
}

// Sample telemetry using the existing rate clock. Only a changed snapshot wakes
// Android; final verification, idle peer changes and completion are not lost.
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
		fmt.Fprintf(h, "%s:%d:%t:%t:%d:%d:%d;", rec.ID, rec.Generation, rec.Paused,
			t.Info() != nil, stats.ActivePeers, stats.BytesReadData.Int64(), stats.BytesWrittenData.Int64())
		// Includes hash completion, even when the last received byte was already
		// accounted for by the previous sample.
		_ = json.NewEncoder(h).Encode(t.PieceStateRuns())
		rec.mu.RUnlock()
	}
	var result [sha256.Size]byte
	copy(result[:], h.Sum(nil))
	return result
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
		s.trackers.close()
		if s.ctlListener != nil {
			_ = s.ctlListener.Close()
		}
		if s.streamListener != nil {
			_ = s.streamListener.Close()
		}
		s.client.Close()
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
	// Guard against non-local requests
	if !strings.HasPrefix(r.RemoteAddr, "127.0.0.1:") && !strings.HasPrefix(r.RemoteAddr, "[::1]:") {
		http.Error(w, "Forbidden", http.StatusForbidden)
		return
	}

	path := strings.Trim(r.URL.Path, "/")
	parts := strings.Split(path, "/")

	switch r.Method {
	case http.MethodGet:
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
		if len(parts) >= 2 && parts[0] == "metadata" {
			s.handleMetadata(w, r, parts[1])
			return
		}
		if len(parts) == 1 && parts[0] == "settings" {
			s.handleGetSettings(w, r)
			return
		}
	case http.MethodPost:
		if len(parts) == 1 && parts[0] == "add" {
			s.handleAdd(w, r)
			return
		}
		if len(parts) == 1 && parts[0] == "play" {
			s.handlePlay(w, r)
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
		DownloadSpeed: s.downloadSpeed,
		UploadSpeed:   s.uploadSpeed,
		Progress:      totalProgress,
		Ratio:         0.0,
		Torrents:      totalTorrents,
		CtlPort:       s.ctlPort,
		StreamPort:    s.streamPort,
		Path:          s.downloadDir,
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
		ID:         id,
		InfoHash:   hashHex,
		Torrent:    t,
		Prepared:   req.Prepare,
		Configured: !req.Prepare,
		Selected:   []int{},
		Generation: 1,
		Paused:     false,
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
	downSpeed, upSpeed := s.downloadSpeed, s.uploadSpeed
	s.mu.RUnlock()

	if !ok {
		writeJSON(w, http.StatusNotFound, map[string]string{"error": "torrent not found"})
		return
	}
	rec.mu.RLock()
	defer rec.mu.RUnlock()

	t := rec.Torrent
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
	var completed int64
	var files []FileView
	var progress float64

	if ready {
		n := t.Name()
		name = &n
		totalLen = t.Length()
		tfFiles := t.Files()
		verified := verifiedFileBytes(t)
		files = make([]FileView, len(tfFiles))
		for i, f := range tfFiles {
			fLen := f.Length()
			fProg := 1.0
			if fLen > 0 {
				fProg = float64(verified[i]) / float64(fLen)
			}
			completed += verified[i]
			ext := filepath.Ext(f.DisplayPath())
			mimeType := mime.TypeByExtension(ext)
			if mimeType == "" {
				mimeType = "application/octet-stream"
			}

			files[i] = FileView{
				Index:    i,
				Name:     filepath.Base(f.DisplayPath()),
				Path:     f.DisplayPath(),
				Length:   fLen,
				Progress: math.Min(fProg, 1.0),
				Type:     mimeType,
			}
		}
		if totalLen > 0 {
			progress = float64(completed) / float64(totalLen)
		}
	}

	done := ready && totalLen > 0 && completed >= totalLen
	tStats := t.Stats()
	numPeers := tStats.ActivePeers
	if numPeers == 0 {
		numPeers = len(t.PeerConns())
	}

	var timeRemaining *int64
	if downSpeed > 0 && totalLen > completed {
		rem := int64(float64(totalLen-completed) / float64(downSpeed) * 1000)
		timeRemaining = &rem
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
		Downloaded:    completed,
		Uploaded:      0,
		Files:         files,
		Error:         rec.Error,
		TimeRemaining: timeRemaining,
		Configured:    rec.Configured,
		Selected:      rec.Selected,
	}

	writeJSON(w, http.StatusOK, view)
}

// Count only verified pieces. BytesCompleted includes unverified received
// chunks and cannot be used to decide when a file is safe for offline playback.
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
	rec.mu.RLock()
	defer rec.mu.RUnlock()

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
	targetFile.Download()

	// Prioritize the first 5MB of the video for instant playback start & codec header detection
	if t.Info() != nil && t.Info().PieceLength > 0 && targetFile.Length() > 0 {
		pieceLen := t.Info().PieceLength
		beginPiece := targetFile.Offset() / pieceLen
		endPiece := (targetFile.Offset() + min(targetFile.Length(), 5*1024*1024) - 1) / pieceLen
		for p := beginPiece; p <= endPiece; p++ {
			t.Piece(int(p)).SetPriority(torrent.PiecePriorityNow)
		}

		// Also prioritize the last 3MB (crucial for MP4 moov atom index located at end of file)
		if targetFile.Length() > 3*1024*1024 {
			lastBeginPiece := (targetFile.Offset() + targetFile.Length() - 3*1024*1024) / pieceLen
			lastEndPiece := (targetFile.Offset() + targetFile.Length() - 1) / pieceLen
			for p := lastBeginPiece; p <= lastEndPiece; p++ {
				t.Piece(int(p)).SetPriority(torrent.PiecePriorityNow)
			}
		}
	}

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

	t := rec.Torrent
	if err := waitForInfo(r, t); err != nil {
		writeJSON(w, http.StatusConflict, map[string]string{"error": err.Error()})
		return
	}
	if err := validateSelection(t, req.Selected, false); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": err.Error()})
		return
	}

	st := s.storage.GetStorage(rec.InfoHash)
	if st != nil {
		if err := st.AttachDescriptors(req.Descriptors, req.Selected); err != nil {
			writeJSON(w, http.StatusBadRequest, map[string]string{"error": err.Error()})
			return
		}
	}
	if st != nil && st.NeedsVerify() {
		pieces := make(map[int]bool)
		for _, index := range req.Selected {
			file := t.Files()[index]
			for p := file.BeginPieceIndex(); p < file.EndPieceIndex(); p++ {
				pieces[p] = true
			}
		}
		for p := range pieces {
			if err := t.Piece(p).VerifyDataContext(r.Context()); err != nil {
				writeJSON(w, http.StatusInternalServerError, map[string]string{"error": fmt.Sprintf("verify saved data: %v", err)})
				return
			}
		}
	}

	rec.Configured = true
	rec.Selected = req.Selected
	rec.Paused = false
	rec.Generation++

	// Apply selection priority
	selectedSet := make(map[int]bool)
	for _, idx := range req.Selected {
		selectedSet[idx] = true
	}

	for i, f := range t.Files() {
		if selectedSet[i] {
			f.SetPriority(torrent.PiecePriorityNormal)
		} else {
			f.SetPriority(torrent.PiecePriorityNone)
		}
	}
	t.AllowDataDownload()
	t.AllowDataUpload()

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

	rec.Selected = req.Selected
	rec.Generation++

	selectedSet := make(map[int]bool)
	for _, idx := range req.Selected {
		selectedSet[idx] = true
	}

	for i, f := range t.Files() {
		if !rec.Configured || rec.Paused {
			f.SetPriority(torrent.PiecePriorityNone)
			continue
		}
		if selectedSet[i] {
			f.SetPriority(torrent.PiecePriorityNormal)
		} else {
			f.SetPriority(torrent.PiecePriorityNone)
		}
	}

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

	t := rec.Torrent
	if pause {
		t.DisallowDataDownload()
		t.DisallowDataUpload()
		for _, f := range t.Files() {
			f.SetPriority(torrent.PiecePriorityNone)
		}
	} else {
		if !rec.Configured {
			writeJSON(w, http.StatusConflict, map[string]string{"error": "download storage is not configured"})
			return
		}
		selectedSet := make(map[int]bool)
		for _, idx := range rec.Selected {
			selectedSet[idx] = true
		}
		for i, f := range t.Files() {
			if selectedSet[i] {
				f.SetPriority(torrent.PiecePriorityNormal)
			} else {
				f.SetPriority(torrent.PiecePriorityNone)
			}
		}
		t.AllowDataDownload()
		t.AllowDataUpload()
	}

	rec.Generation++
	rec.Paused = pause

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

	rec.Torrent.Drop()
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

	// Gather piece states
	runs := t.PieceStateRuns()
	pieceStates := make([]torrent.PieceState, totalPieces)
	idx := 0
	for _, run := range runs {
		for k := 0; k < run.Length && idx < totalPieces; k++ {
			pieceStates[idx] = run.PieceState
			idx++
		}
	}

	// Compute ranges
	var bucketList []PieceBucket
	if totalPieces <= maxBuckets {
		for i := 0; i < totalPieces; i++ {
			bucketList = append(bucketList, PieceBucket{Start: i, End: i})
		}
	} else {
		bucketSize := int(math.Ceil(float64(totalPieces) / float64(maxBuckets)))
		for start := 0; start < totalPieces; start += bucketSize {
			end := start + bucketSize - 1
			if end >= totalPieces {
				end = totalPieces - 1
			}
			bucketList = append(bucketList, PieceBucket{Start: start, End: end})
		}
	}

	for i := range bucketList {
		b := &bucketList[i]
		b.Total = b.End - b.Start + 1
		b.Selected = b.Total // Default all pieces selected
		for p := b.Start; p <= b.End; p++ {
			st := pieceStates[p]
			if st.Complete {
				b.Verified++
			} else if st.Partial || st.Checking || st.Hashing {
				b.Receiving++
			}
		}
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
	s.mu.RUnlock()
	writeJSON(w, http.StatusOK, SettingsResponse{MaxPeers: peers})
}

func (s *EngineServer) handlePostSettings(w http.ResponseWriter, r *http.Request) {
	var req SettingsRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid json"})
		return
	}
	s.mu.Lock()
	if req.MaxPeers >= 8 && req.MaxPeers <= 80 {
		s.maxPeers = req.MaxPeers
		for _, rec := range s.records {
			rec.Torrent.SetMaxEstablishedConns(req.MaxPeers)
		}
	}
	peers := s.maxPeers
	s.mu.Unlock()
	writeJSON(w, http.StatusOK, SettingsResponse{MaxPeers: peers})
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
	if !selected {
		http.Error(w, "file is not selected", http.StatusForbidden)
		return
	}
	reader := file.NewReader()
	defer reader.Close()

	// Tie reader context to client HTTP connection to cleanly abort on client seek/disconnect
	reader.SetContext(r.Context())
	// SetReadaheadFunc(nil) enables fixed readahead rather than dynamic piece-count throttling
	reader.SetReadaheadFunc(nil)
	reader.SetReadahead(20 * 1024 * 1024)

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
