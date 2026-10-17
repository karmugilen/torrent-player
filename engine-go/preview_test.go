package main

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestOpenPreviewReadsVerifiedBytesWithoutFocusChange(t *testing.T) {
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
	writeVerifiedPiece(t, s, rec, data, 1)

	beforeFocus := rec.FocusedFile
	preview, err := s.OpenPreview(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer preview.Close()

	buf := make([]byte, 32)
	n, err := preview.ReadAt(buf, 0)
	if err != nil {
		t.Fatal(err)
	}
	if n != 32 || !bytes.Equal(buf, data[:32]) {
		t.Fatalf("preview read mismatch n=%d", n)
	}
	if rec.FocusedFile != beforeFocus {
		t.Fatal("preview must not mutate focused file")
	}
	owner := s.ensurePlaybackCoordinator(rec.Torrent)
	owner.mu.Lock()
	hasPreview := len(owner.previewDemand) > 0
	owner.mu.Unlock()
	if !hasPreview {
		t.Fatal("preview demand missing")
	}
	_ = preview.Close()
	owner.mu.Lock()
	released := len(owner.previewDemand) == 0
	owner.mu.Unlock()
	if !released {
		t.Fatal("preview demand was not released on close")
	}
}

func TestPreviewRejectsSecondConcurrentJob(t *testing.T) {
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

	first, err := s.OpenPreview(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer first.Close()
	if _, err := s.OpenPreview(rec.ID, 0); err == nil {
		t.Fatal("expected second preview to fail")
	}
}

func TestPreviewTempForReadyDraftWithoutFullFile(t *testing.T) {
	_, metadata := recoveryMetadata(t)
	dir := t.TempDir()
	s := testEngineAt(t, dir)
	rec := addRecoveryTorrent(t, s, metadata)
	if rec.Configured {
		t.Fatal("draft should be unconfigured")
	}
	preview, err := s.OpenPreview(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	st := s.storage.GetStorage(rec.InfoHash)
	if st == nil || !st.previewTemp {
		t.Fatal("expected temporary preview storage")
	}
	// No full-sized destination under boundary/stream.
	boundary := boundaryDir(dir, rec.InfoHash)
	if entries, _ := os.ReadDir(boundary); len(entries) != 0 {
		t.Fatalf("preview created boundary files: %v", entries)
	}
	_ = preview.Close()
	deadline := time.Now().Add(time.Second)
	for time.Now().Before(deadline) && st.previewTemp {
		time.Sleep(time.Millisecond)
	}
	if st.previewTemp {
		t.Fatal("preview temp storage was not released")
	}
	if _, err := os.Stat(filepath.Join(dir, ".preview", rec.InfoHash)); !os.IsNotExist(err) {
		t.Fatalf("preview dir remained: %v", err)
	}
}

func TestPreviewCloseIsIdempotentAndCancelsRead(t *testing.T) {
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
	preview, err := s.OpenPreview(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	if err := preview.Close(); err != nil {
		t.Fatal(err)
	}
	if err := preview.Close(); err != nil {
		t.Fatal(err)
	}
	if n, err := preview.ReadAt(make([]byte, 8), 0); n >= 0 && err == nil {
		t.Fatal("read after close should fail")
	}
}

func TestPauseReleasesPreviewDemand(t *testing.T) {
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
	if _, err := s.OpenPreview(rec.ID, 0); err != nil {
		t.Fatal(err)
	}
	if code, response := s.DispatchControl("POST", "/pause", `{"id":"`+rec.ID+`"}`); code != 200 {
		t.Fatalf("pause: %d %s", code, response)
	}
	s.mu.Lock()
	active := s.previewActive
	s.mu.Unlock()
	if active != nil {
		t.Fatal("pause must close active preview")
	}
}
