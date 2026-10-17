package main

import (
	"encoding/binary"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"sync/atomic"
	"testing"
	"time"
)

func TestDrainSyncFailurePreservesDirtyAndPending(t *testing.T) {
	oldDelay := checkpointFlushDelay
	checkpointFlushDelay = 0
	t.Cleanup(func() { checkpointFlushDelay = oldDelay })

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
	if code, response := s.DispatchControl("POST", "/configure", string(body)); code != 200 {
		t.Fatalf("configure: %d %s", code, response)
	}
	writeVerifiedPiece(t, s, rec, data, 0)
	st := s.storage.GetStorage(rec.InfoHash)
	if st == nil {
		t.Fatal("missing storage")
	}

	var syncCalls atomic.Int32
	st.mu.Lock()
	for st.checkpointFlushing {
		st.mu.Unlock()
		time.Sleep(time.Millisecond)
		st.mu.Lock()
	}
	st.fileSyncFn = func(f *os.File) error {
		syncCalls.Add(1)
		return errors.New("injected sync failure")
	}
	st.writeCheckpointFn = func(string, *pieceCheckpoint) error {
		t.Error("checkpoint write must not run after failed data sync")
		return errors.New("should not write")
	}
	st.pendingSync = map[*os.File]struct{}{destination: {}}
	st.checkpointDirty = true
	st.checkpointRetry = 0
	st.checkpointFlushing = true
	st.mu.Unlock()
	t.Cleanup(func() {
		st.mu.Lock()
		st.fileSyncFn = nil
		st.writeCheckpointFn = nil
		st.checkpointRetry = 0
		st.checkpointDirty = false
		st.pendingSync = make(map[*os.File]struct{})
		st.mu.Unlock()
		for st.CheckpointDirty() {
			st.mu.Lock()
			st.checkpointDirty = false
			st.pendingSync = make(map[*os.File]struct{})
			st.mu.Unlock()
			time.Sleep(time.Millisecond)
		}
	})

	before, _ := os.Stat(checkpointPath(dir, rec.InfoHash))
	st.drainSyncAndCheckpoint()

	if !st.CheckpointDirty() {
		t.Fatal("failed sync cleared dirty/pending state")
	}
	if st.CheckpointPersistError() == nil {
		t.Fatal("expected persist error feedback")
	}
	if syncCalls.Load() == 0 {
		t.Fatal("sync seam was not exercised")
	}
	after, err := os.Stat(checkpointPath(dir, rec.InfoHash))
	if err == nil && (before == nil || after.ModTime().After(before.ModTime())) {
		t.Fatal("checkpoint must not be published after failed data sync")
	}
}

func TestDrainCheckpointWriteFailurePreservesDirty(t *testing.T) {
	oldDelay := checkpointFlushDelay
	checkpointFlushDelay = 0
	t.Cleanup(func() { checkpointFlushDelay = oldDelay })

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
	if code, response := s.DispatchControl("POST", "/configure", string(body)); code != 200 {
		t.Fatalf("configure: %d %s", code, response)
	}
	writeVerifiedPiece(t, s, rec, data, 0)
	st := s.storage.GetStorage(rec.InfoHash)
	st.mu.Lock()
	for st.checkpointFlushing {
		st.mu.Unlock()
		time.Sleep(time.Millisecond)
		st.mu.Lock()
	}
	st.writeCheckpointFn = func(string, *pieceCheckpoint) error {
		return errors.New("injected checkpoint write failure")
	}
	st.checkpointDirty = true
	st.checkpointRetry = 0
	st.checkpointFlushing = true
	st.mu.Unlock()
	t.Cleanup(func() {
		st.mu.Lock()
		st.writeCheckpointFn = nil
		st.checkpointRetry = 0
		st.checkpointDirty = false
		st.pendingSync = make(map[*os.File]struct{})
		st.mu.Unlock()
	})

	st.drainSyncAndCheckpoint()
	if !st.CheckpointDirty() {
		t.Fatal("failed checkpoint write cleared dirty state")
	}
	if st.CheckpointPersistError() == nil {
		t.Fatal("expected checkpoint write error")
	}
}

func TestDrainCheckpointSucceedsAfterTransientFailure(t *testing.T) {
	oldDelay := checkpointFlushDelay
	checkpointFlushDelay = 0
	t.Cleanup(func() { checkpointFlushDelay = oldDelay })

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
	if code, response := s.DispatchControl("POST", "/configure", string(body)); code != 200 {
		t.Fatalf("configure: %d %s", code, response)
	}
	writeVerifiedPiece(t, s, rec, data, 0)
	st := s.storage.GetStorage(rec.InfoHash)
	var writes atomic.Int32
	st.mu.Lock()
	for st.checkpointFlushing {
		st.mu.Unlock()
		time.Sleep(time.Millisecond)
		st.mu.Lock()
	}
	st.writeCheckpointFn = func(path string, cp *pieceCheckpoint) error {
		if writes.Add(1) == 1 {
			return errors.New("transient write failure")
		}
		return writePieceCheckpoint(path, cp)
	}
	st.checkpointDirty = true
	st.checkpointRetry = 0
	st.checkpointFlushing = true
	st.mu.Unlock()
	t.Cleanup(func() {
		st.mu.Lock()
		st.writeCheckpointFn = nil
		st.checkpointRetry = 0
		st.mu.Unlock()
	})

	st.drainSyncAndCheckpoint()
	if !st.CheckpointDirty() {
		t.Fatal("first failure should keep dirty")
	}
	// Second drain should succeed; clear injected retry delay for determinism.
	st.mu.Lock()
	for st.checkpointFlushing {
		st.mu.Unlock()
		time.Sleep(time.Millisecond)
		st.mu.Lock()
	}
	st.checkpointRetry = 0
	st.checkpointFlushing = true
	st.mu.Unlock()
	st.drainSyncAndCheckpoint()
	if st.CheckpointDirty() {
		t.Fatal("retry success should clear dirty")
	}
	if _, err := os.Stat(checkpointPath(dir, rec.InfoHash)); err != nil {
		t.Fatalf("checkpoint missing after retry: %v", err)
	}
}

func TestDrainDoesNotRewriteDeletedCheckpoint(t *testing.T) {
	oldDelay := checkpointFlushDelay
	checkpointFlushDelay = 0
	t.Cleanup(func() { checkpointFlushDelay = oldDelay })

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
	if code, response := s.DispatchControl("POST", "/configure", string(body)); code != 200 {
		t.Fatalf("configure: %d %s", code, response)
	}
	writeVerifiedPiece(t, s, rec, data, 0)
	st := s.storage.GetStorage(rec.InfoHash)
	st.mu.Lock()
	for st.checkpointFlushing {
		st.mu.Unlock()
		time.Sleep(time.Millisecond)
		st.mu.Lock()
	}
	st.checkpointDirty = true
	st.checkpointFlushing = true
	st.mu.Unlock()

	// Delete invalidates in-flight generations before the stale drain writes.
	if err := st.DeleteCheckpoint(); err != nil {
		t.Fatal(err)
	}
	st.drainSyncAndCheckpoint()
	if _, err := os.Stat(checkpointPath(dir, rec.InfoHash)); !os.IsNotExist(err) {
		t.Fatalf("deleted checkpoint was recreated: %v", err)
	}
}

func TestMalformedCheckpointPieceCountRejected(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "bad.chk")
	hdr := make([]byte, 40)
	binary.BigEndian.PutUint32(hdr[0:], checkpointMagic)
	binary.BigEndian.PutUint32(hdr[4:], checkpointVersion)
	binary.BigEndian.PutUint64(hdr[28:], 16*1024)
	binary.BigEndian.PutUint32(hdr[36:], uint32(maxCheckpointPieces+1))
	if err := os.WriteFile(path, hdr, 0600); err != nil {
		t.Fatal(err)
	}
	if _, err := readPieceCheckpoint(path); err == nil {
		t.Fatal("expected oversized piece count to be rejected")
	}
}

func TestCheckpointDeleteDuringPublishDoesNotResurrect(t *testing.T) {
	oldDelay := checkpointFlushDelay
	checkpointFlushDelay = 0
	t.Cleanup(func() { checkpointFlushDelay = oldDelay })

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
	if code, response := s.DispatchControl("POST", "/configure", string(body)); code != 200 {
		t.Fatalf("configure: %d %s", code, response)
	}
	writeVerifiedPiece(t, s, rec, data, 0)
	st := s.storage.GetStorage(rec.InfoHash)
	if st == nil {
		t.Fatal("missing storage")
	}

	st.mu.Lock()
	for st.checkpointFlushing {
		st.mu.Unlock()
		time.Sleep(time.Millisecond)
		st.mu.Lock()
	}
	st.writeCheckpointFn = func(path string, cp *pieceCheckpoint) error {
		// Concurrent delete bumps generation while the stale writer is publishing.
		st.mu.Lock()
		st.checkpointGen++
		st.checkpointDirty = false
		st.identities = nil
		base, hash := st.baseDir, st.infoHash.HexString()
		st.mu.Unlock()
		_ = deletePieceCheckpoint(base, hash)
		return writePieceCheckpoint(path, cp)
	}
	st.checkpointDirty = true
	st.checkpointFlushing = true
	gen := st.checkpointGen
	st.mu.Unlock()

	st.drainSyncAndCheckpoint()
	if _, err := os.Stat(checkpointPath(dir, rec.InfoHash)); !os.IsNotExist(err) {
		t.Fatalf("checkpoint resurrected after delete during publish: %v", err)
	}
	st.mu.Lock()
	if st.checkpointGen == gen {
		t.Fatal("delete must bump generation")
	}
	// Stale writer must not clear dirty for a superseded generation when needCP
	// still applied; generation mismatch leaves dirty only if gen matched.
	st.mu.Unlock()
}

func TestCheckpointPersistErrorSurfacedInTorrentStatus(t *testing.T) {
	oldDelay := checkpointFlushDelay
	checkpointFlushDelay = 0
	t.Cleanup(func() { checkpointFlushDelay = oldDelay })

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
	if code, response := s.DispatchControl("POST", "/configure", string(body)); code != 200 {
		t.Fatalf("configure: %d %s", code, response)
	}
	writeVerifiedPiece(t, s, rec, data, 0)
	st := s.storage.GetStorage(rec.InfoHash)
	st.mu.Lock()
	for st.checkpointFlushing {
		st.mu.Unlock()
		time.Sleep(time.Millisecond)
		st.mu.Lock()
	}
	st.writeCheckpointFn = func(string, *pieceCheckpoint) error {
		return errors.New("injected checkpoint write failure")
	}
	st.checkpointDirty = true
	st.checkpointFlushing = true
	st.mu.Unlock()
	st.drainSyncAndCheckpoint()
	view := recoveryStatus(t, s, rec)
	if view.Warning == nil || *view.Warning == "" {
		t.Fatalf("expected checkpoint persist warning in status, got %+v", view)
	}
}
