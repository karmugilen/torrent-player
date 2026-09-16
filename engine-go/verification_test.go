package main

import (
	"bytes"
	"context"
	"crypto/sha1"
	"encoding/base64"
	"encoding/json"
	"net/http/httptest"
	"os"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/anacrolix/torrent"
	"github.com/anacrolix/torrent/bencode"
	"github.com/anacrolix/torrent/metainfo"
	"github.com/anacrolix/torrent/storage"
)

// Delay actual disk reads to reproduce a check that outlives its control request.
type gatedStorage struct {
	storage.ClientImpl
	enabled atomic.Bool
	entered chan struct{}
	release chan struct{}
	once    sync.Once
}

func (g *gatedStorage) OpenTorrent(ctx context.Context, info *metainfo.Info, hash metainfo.Hash) (storage.TorrentImpl, error) {
	impl, err := g.ClientImpl.OpenTorrent(ctx, info, hash)
	original := impl.Piece
	impl.Piece = func(p metainfo.Piece) storage.PieceImpl { return &gatedPiece{original(p), g} }
	return impl, err
}

type gatedPiece struct {
	storage.PieceImpl
	gate *gatedStorage
}

func (p *gatedPiece) ReadAt(b []byte, off int64) (int, error) {
	if p.gate.enabled.Load() {
		p.gate.once.Do(func() { close(p.gate.entered) })
		<-p.gate.release
	}
	return p.PieceImpl.ReadAt(b, off)
}

func recoveryMetadata(t *testing.T) ([]byte, string) {
	t.Helper()
	data := bytes.Repeat([]byte("saved torrent data."), 8192)
	info := metainfo.Info{Name: "resume.bin", Length: int64(len(data)), PieceLength: 16 * 1024}
	for start := 0; start < len(data); start += int(info.PieceLength) {
		hash := sha1.Sum(data[start:min(start+int(info.PieceLength), len(data))])
		info.Pieces = append(info.Pieces, hash[:]...)
	}
	encoded, err := bencode.Marshal(info)
	if err != nil {
		t.Fatal(err)
	}
	mi := metainfo.MetaInfo{InfoBytes: encoded}
	var buf bytes.Buffer
	if err := mi.Write(&buf); err != nil {
		t.Fatal(err)
	}
	return data, base64.StdEncoding.EncodeToString(buf.Bytes())
}

func addRecoveryTorrent(t *testing.T, s *EngineServer, metadata string) *TorrentRecord {
	t.Helper()
	body, _ := json.Marshal(AddRequest{Prepare: true, TorrentData: metadata})
	code, response := s.DispatchControl("POST", "/add", string(body))
	if code != 200 {
		t.Fatalf("add: %d %s", code, response)
	}
	var added AddResponse
	if err := json.Unmarshal(response, &added); err != nil {
		t.Fatal(err)
	}
	return s.records[added.ID]
}

func recoveryStatus(t *testing.T, s *EngineServer, rec *TorrentRecord) TorrentView {
	t.Helper()
	code, body := s.DispatchControl("GET", "/torrent/"+rec.ID, "")
	if code != 200 {
		t.Fatalf("status: %d %s", code, body)
	}
	var view TorrentView
	if err := json.Unmarshal(body, &view); err != nil {
		t.Fatal(err)
	}
	return view
}

func waitRecovery(t *testing.T, check func() bool) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for !check() {
		if time.Now().After(deadline) {
			t.Fatal("recovery did not finish")
		}
		time.Sleep(time.Millisecond)
	}
}

func gatedRecovery(t *testing.T) (*EngineServer, *TorrentRecord, func()) {
	t.Helper()
	gate := &gatedStorage{entered: make(chan struct{}), release: make(chan struct{})}
	s := testEngine(t, func(cfg *torrent.ClientConfig) {
		gate.ClientImpl = cfg.DefaultStorage
		cfg.DefaultStorage = gate
	})
	var releaseOnce sync.Once
	release := func() { releaseOnce.Do(func() { close(gate.release) }) }
	t.Cleanup(release)
	data, metadata := recoveryMetadata(t)
	rec := addRecoveryTorrent(t, s, metadata)
	f, err := os.CreateTemp(t.TempDir(), "saved")
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	if _, err := f.Write(data); err != nil {
		t.Fatal(err)
	}
	gate.enabled.Store(true)
	fd := int(f.Fd())
	body, _ := json.Marshal(ConfigureRequest{ID: rec.ID, Descriptors: []*int{&fd}, Selected: []int{0}})
	// Hashing must continue independently after this request deadline expires.
	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()
	request := httptest.NewRequest("POST", "/configure", strings.NewReader(string(body))).WithContext(ctx)
	w := httptest.NewRecorder()
	finished := make(chan struct{})
	go func() { s.handleConfigure(w, request); close(finished) }()
	select {
	case <-finished:
	case <-time.After(time.Second):
		t.Fatal("configure blocked on saved data verification")
	}
	if w.Code != 200 {
		t.Fatalf("configure after request deadline: %d %s", w.Code, w.Body.String())
	}
	select {
	case <-gate.entered:
	case <-time.After(time.Second):
		t.Fatal("saved data was not checked")
	}
	<-ctx.Done()
	status := recoveryStatus(t, s, rec)
	if !status.Configured || !status.Checking || status.Done || status.Error != nil || status.CheckTotal == 0 {
		t.Fatalf("invalid checking status: %+v", status)
	}
	return s, rec, release
}

func recoveryCommand(t *testing.T, s *EngineServer, rec *TorrentRecord, path string) {
	t.Helper()
	body, _ := json.Marshal(IdRequest{ID: rec.ID})
	code, response := s.DispatchControl("POST", path, string(body))
	if code != 200 {
		t.Fatalf("%s: %d %s", path, code, response)
	}
}

func TestRestoreOutlivesRequestAndPauseResumeRemainResponsive(t *testing.T) {
	s, rec, release := gatedRecovery(t)
	recoveryCommand(t, s, rec, "/pause")
	if !recoveryStatus(t, s, rec).Paused {
		t.Fatal("pause was not reported")
	}
	recoveryCommand(t, s, rec, "/resume")
	if rec.Torrent.Files()[0].Priority() != torrent.PiecePriorityNone {
		t.Fatal("resume enabled payload before checking saved data")
	}
	recoveryCommand(t, s, rec, "/pause")
	release()
	waitRecovery(t, func() bool { return recoveryStatus(t, s, rec).CheckedPieces == 1 })
	status := recoveryStatus(t, s, rec)
	if !status.Checking || !status.Paused {
		t.Fatalf("pause was lost: %+v", status)
	}
	recoveryCommand(t, s, rec, "/resume")
	waitRecovery(t, func() bool { status := recoveryStatus(t, s, rec); return !status.Checking && status.Done })
	if rec.Torrent.Files()[0].Priority() != torrent.PiecePriorityNormal {
		t.Fatal("resume did not restore selection")
	}
}

func TestRemoveAndShutdownCancelSavedDataCheck(t *testing.T) {
	for _, shutdown := range []bool{false, true} {
		name := "remove"
		if shutdown {
			name = "shutdown"
		}
		t.Run(name, func(t *testing.T) {
			s, rec, release := gatedRecovery(t)
			finished := make(chan struct{})
			go func() {
				if shutdown {
					s.Close()
				} else {
					body, _ := json.Marshal(IdRequest{ID: rec.ID})
					s.DispatchControl("POST", "/remove", string(body))
				}
				close(finished)
			}()
			// Verification cancels even while the library has an outstanding read.
			select {
			case <-rec.verification.done:
			case <-time.After(time.Second):
				t.Fatal("verification did not cancel")
			}
			release()
			select {
			case <-finished:
			case <-time.After(3 * time.Second):
				t.Fatal("close stalled")
			}
			if !shutdown {
				_, metadata := recoveryMetadata(t)
				if addRecoveryTorrent(t, s, metadata).ID == rec.ID {
					t.Fatal("removed record was reused")
				}
			}
		})
	}
}

func TestRestartPreservesValidPiecesAndDownloadsOnlyMissingData(t *testing.T) {
	data, metadata := recoveryMetadata(t)
	peerConfig := func(cfg *torrent.ClientConfig) {
		cfg.DisableTCP, cfg.DisableIPv6 = false, true
		cfg.ListenHost = func(string) string { return "127.0.0.1" }
		cfg.ListenPort = 0
		cfg.DialForPeerConns, cfg.AcceptPeerConnections, cfg.Seed = true, true, true
	}
	configure := func(s *EngineServer, rec *TorrentRecord, f *os.File) {
		fd := int(f.Fd())
		body, _ := json.Marshal(ConfigureRequest{ID: rec.ID, Descriptors: []*int{&fd}, Selected: []int{0}})
		code, response := s.DispatchControl("POST", "/configure", string(body))
		if code != 200 {
			t.Fatalf("configure: %d %s", code, response)
		}
	}
	original := testEngine(t)
	rec := addRecoveryTorrent(t, original, metadata)
	destination, err := os.CreateTemp(t.TempDir(), "partial")
	if err != nil {
		t.Fatal(err)
	}
	defer destination.Close()
	configure(original, rec, destination)
	writeVerifiedPiece(t, original, rec, data, 0)
	writeVerifiedPiece(t, original, rec, data, 1)
	original.Close() // The replacement engine has no in-memory completion state.
	if _, err := destination.WriteAt([]byte("corrupted"), 16*1024); err != nil {
		t.Fatal(err)
	}
	leech := testEngine(t, peerConfig)
	restored := addRecoveryTorrent(t, leech, metadata)
	configure(leech, restored, destination)
	waitRecovery(t, func() bool { return !recoveryStatus(t, leech, restored).Checking })
	waitRecovery(t, func() bool { return restored.Torrent.Piece(0).State().Complete })
	if restored.Torrent.Piece(1).State().Complete || recoveryStatus(t, leech, restored).Downloaded != 16*1024 {
		t.Fatal("corrupt or missing data was accepted as complete")
	}
	seed := testEngine(t, peerConfig)
	seedRec := addRecoveryTorrent(t, seed, metadata)
	seedFile, err := os.CreateTemp(t.TempDir(), "seed")
	if err != nil {
		t.Fatal(err)
	}
	defer seedFile.Close()
	configure(seed, seedRec, seedFile)
	for p := 0; p < seedRec.Torrent.NumPieces(); p++ {
		writeVerifiedPiece(t, seed, seedRec, data, p)
	}
	if restored.Torrent.AddClientPeer(seed.client) == 0 {
		t.Fatal("no seed address")
	}
	waitRecovery(t, func() bool { return recoveryStatus(t, leech, restored).Done })
	stats := restored.Torrent.Stats()
	if got := stats.BytesReadData.Int64(); got != int64(len(data))-16*1024 {
		t.Fatalf("received %d bytes; already verified piece was downloaded again", got)
	}
	got := make([]byte, len(data))
	if _, err := destination.ReadAt(got, 0); err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(got, data) {
		t.Fatal("restored file differs from seed")
	}
}
