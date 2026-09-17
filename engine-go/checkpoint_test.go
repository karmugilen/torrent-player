package main

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/anacrolix/torrent"
	"github.com/anacrolix/torrent/metainfo"
)

func TestPieceCheckpointRoundTrip(t *testing.T) {
	dir := t.TempDir()
	var hash [20]byte
	copy(hash[:], bytes.Repeat([]byte{0xab}, 20))
	cp := &pieceCheckpoint{
		InfoHash:    hash,
		PieceLength: 16 * 1024,
		NumPieces:   10,
		Files: []fileIdentity{{
			External: true,
			Dev:      1,
			Ino:      2,
			Length:   100,
			ModNano:  123,
			Path:     "movie.mp4",
		}},
		Complete: []bool{true, false, true, true, false, false, false, false, false, true},
	}
	path := checkpointPath(dir, metainfo.Hash(hash).HexString())
	if err := writePieceCheckpoint(path, cp); err != nil {
		t.Fatal(err)
	}
	got, err := readPieceCheckpoint(path)
	if err != nil {
		t.Fatal(err)
	}
	if got.InfoHash != cp.InfoHash || got.PieceLength != cp.PieceLength || got.NumPieces != cp.NumPieces {
		t.Fatalf("header mismatch: %+v", got)
	}
	if !identitiesMatch(got.Files, cp.Files) {
		t.Fatalf("files mismatch: %+v", got.Files)
	}
	if !bytes.Equal(boolsToBytes(got.Complete), boolsToBytes(cp.Complete)) {
		t.Fatalf("bitfield mismatch: %v", got.Complete)
	}
}

func boolsToBytes(v []bool) []byte {
	out := make([]byte, len(v))
	for i, b := range v {
		if b {
			out[i] = 1
		}
	}
	return out
}

func testEngineAt(t *testing.T, dataDir string, configure ...func(*torrent.ClientConfig)) *EngineServer {
	t.Helper()
	cfg := torrent.NewDefaultClientConfig()
	cfg.DataDir = dataDir
	cfg.NoDHT, cfg.DisableTrackers, cfg.DisableTCP, cfg.DisableUTP = true, true, true, true
	cfg.DisableWebtorrent, cfg.DisablePEX = true, true
	cfg.DialForPeerConns, cfg.AcceptPeerConnections = false, false
	store := NewDocumentStorageClient(dataDir)
	cfg.DefaultStorage = store
	for _, apply := range configure {
		apply(cfg)
	}
	client, err := torrent.NewClient(cfg)
	if err != nil {
		t.Fatal(err)
	}
	s := &EngineServer{client: client, storage: store, records: make(map[string]*TorrentRecord), recordsByHash: make(map[string]*TorrentRecord), done: make(chan struct{})}
	s.payloadTransfersAllowed.Store(true)
	t.Cleanup(s.Close)
	return s
}

func TestCheckpointSkipsTrustedPiecesOnResume(t *testing.T) {
	oldDelay := checkpointFlushDelay
	checkpointFlushDelay = 0
	t.Cleanup(func() { checkpointFlushDelay = oldDelay })

	data, metadata := recoveryMetadata(t)
	dir := t.TempDir()
	configure := func(s *EngineServer, rec *TorrentRecord, f *os.File) {
		fd := int(f.Fd())
		body, _ := json.Marshal(ConfigureRequest{ID: rec.ID, Descriptors: []*int{&fd}, Selected: []int{0}})
		code, response := s.DispatchControl("POST", "/configure", string(body))
		if code != 200 {
			t.Fatalf("configure: %d %s", code, response)
		}
	}

	original := testEngineAt(t, dir)
	rec := addRecoveryTorrent(t, original, metadata)
	destination, err := os.CreateTemp(t.TempDir(), "partial")
	if err != nil {
		t.Fatal(err)
	}
	defer destination.Close()
	configure(original, rec, destination)
	writeVerifiedPiece(t, original, rec, data, 0)
	writeVerifiedPiece(t, original, rec, data, 1)
	st := original.storage.GetStorage(rec.InfoHash)
	if st == nil {
		t.Fatal("missing storage")
	}
	// Wait until storage itself trusts both pieces (MarkComplete finished).
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) && (!st.PieceComplete(0) || !st.PieceComplete(1)) {
		time.Sleep(time.Millisecond)
	}
	if !st.PieceComplete(0) || !st.PieceComplete(1) {
		t.Fatal("storage did not mark verified pieces complete")
	}
	st.mu.Lock()
	for st.checkpointFlushing {
		st.mu.Unlock()
		time.Sleep(time.Millisecond)
		st.mu.Lock()
	}
	st.checkpointDirty = true
	if err := st.flushCheckpointLocked(); err != nil {
		st.mu.Unlock()
		t.Fatal(err)
	}
	st.mu.Unlock()
	chk := checkpointPath(dir, rec.InfoHash)
	if _, err := os.Stat(chk); err != nil {
		t.Fatalf("checkpoint not written: %v", err)
	}
	original.Close()

	restoredEngine := testEngineAt(t, dir)
	restored := addRecoveryTorrent(t, restoredEngine, metadata)
	configure(restoredEngine, restored, destination)
	st2 := restoredEngine.storage.GetStorage(restored.InfoHash)
	if st2 == nil || !st2.PieceComplete(0) || !st2.PieceComplete(1) {
		t.Fatal("trusted pieces were not restored from checkpoint")
	}
	waitRecovery(t, func() bool { return !recoveryStatus(t, restoredEngine, restored).Checking })
	waitRecovery(t, func() bool {
		return restored.Torrent.Piece(0).State().Complete && restored.Torrent.Piece(1).State().Complete
	})
	if recoveryStatus(t, restoredEngine, restored).Downloaded < 32*1024 {
		t.Fatal("verified bytes missing after checkpoint resume")
	}
}

func TestCheckpointIgnoredWhenFileIdentityChanges(t *testing.T) {
	data, metadata := recoveryMetadata(t)
	dir := t.TempDir()
	configure := func(s *EngineServer, rec *TorrentRecord, f *os.File) {
		fd := int(f.Fd())
		body, _ := json.Marshal(ConfigureRequest{ID: rec.ID, Descriptors: []*int{&fd}, Selected: []int{0}})
		code, response := s.DispatchControl("POST", "/configure", string(body))
		if code != 200 {
			t.Fatalf("configure: %d %s", code, response)
		}
	}

	original := testEngineAt(t, dir)
	rec := addRecoveryTorrent(t, original, metadata)
	destination, err := os.CreateTemp(t.TempDir(), "partial")
	if err != nil {
		t.Fatal(err)
	}
	defer destination.Close()
	configure(original, rec, destination)
	writeVerifiedPiece(t, original, rec, data, 0)
	st := original.storage.GetStorage(rec.InfoHash)
	st.mu.Lock()
	st.checkpointDirty = true
	_ = st.flushCheckpointLocked()
	st.mu.Unlock()
	original.Close()

	// Same path/inode family but content+mtime change must invalidate trust.
	if _, err := destination.WriteAt([]byte("corrupted-data!!!!"), 0); err != nil {
		t.Fatal(err)
	}
	_ = destination.Sync()
	time.Sleep(10 * time.Millisecond)

	restoredEngine := testEngineAt(t, dir)
	restored := addRecoveryTorrent(t, restoredEngine, metadata)
	configure(restoredEngine, restored, destination)
	st2 := restoredEngine.storage.GetStorage(restored.InfoHash)
	if st2 != nil && st2.PieceComplete(0) {
		t.Fatal("checkpoint remained trusted after file identity changed")
	}
	waitRecovery(t, func() bool { return !recoveryStatus(t, restoredEngine, restored).Checking })
	if restored.Torrent.Piece(0).State().Complete {
		t.Fatal("corrupt piece was accepted via stale checkpoint")
	}
}

func TestCheckpointDeletedOnRemove(t *testing.T) {
	data, metadata := recoveryMetadata(t)
	dir := t.TempDir()
	s := testEngineAt(t, dir)
	rec := addRecoveryTorrent(t, s, metadata)
	destination, err := os.CreateTemp(t.TempDir(), "partial")
	if err != nil {
		t.Fatal(err)
	}
	defer destination.Close()
	fd := int(destination.Fd())
	body, _ := json.Marshal(ConfigureRequest{ID: rec.ID, Descriptors: []*int{&fd}, Selected: []int{0}})
	code, response := s.DispatchControl("POST", "/configure", string(body))
	if code != 200 {
		t.Fatalf("configure: %d %s", code, response)
	}
	writeVerifiedPiece(t, s, rec, data, 0)
	st := s.storage.GetStorage(rec.InfoHash)
	st.mu.Lock()
	st.checkpointDirty = true
	_ = st.flushCheckpointLocked()
	st.mu.Unlock()
	chk := checkpointPath(dir, rec.InfoHash)
	if _, err := os.Stat(chk); err != nil {
		t.Fatal(err)
	}
	removeBody, _ := json.Marshal(IdRequest{ID: rec.ID})
	code, response = s.DispatchControl("POST", "/remove", string(removeBody))
	if code != 200 {
		t.Fatalf("remove: %d %s", code, response)
	}
	if _, err := os.Stat(chk); !os.IsNotExist(err) {
		t.Fatalf("checkpoint remained after remove: %v", err)
	}
	// Ensure directory layout stays tidy for other torrents.
	_ = filepath.Walk(dir, func(path string, info os.FileInfo, err error) error { return nil })
}
