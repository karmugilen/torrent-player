package main

import (
	"bytes"
	"crypto/sha1"
	"encoding/base64"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/anacrolix/torrent/bencode"
	"github.com/anacrolix/torrent/metainfo"
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

func TestPreviewBudgetEnforcedAcrossSeeks(t *testing.T) {
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
	// Only the head piece is verified. A far seek must admit new pieces and
	// fail once the session budget cannot fit them.
	writeVerifiedPiece(t, s, rec, data, 0)
	preview, err := s.OpenPreview(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer preview.Close()
	pr := preview.(*previewReader)
	pr.mu.Lock()
	pr.budget = rec.Torrent.Info().PieceLength
	pr.admitted = map[int]bool{}
	pr.admittedN = 0
	pr.mu.Unlock()

	if _, err := preview.ReadAt(make([]byte, 8), 0); err != nil {
		t.Fatalf("head read within budget: %v", err)
	}
	tail := preview.Length() - 8
	if _, err := preview.ReadAt(make([]byte, 8), tail); err == nil {
		t.Fatal("expected budget failure for far seek outside admitted union")
	}
}

func TestPreviewSessionDeadlineBlocksUnlimitedReads(t *testing.T) {
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
	defer preview.Close()
	pr := preview.(*previewReader)
	pr.mu.Lock()
	pr.cancel()
	pr.mu.Unlock()
	if _, err := preview.ReadAt(make([]byte, 4), 0); err == nil {
		t.Fatal("expected session deadline to reject further reads")
	}
}

func TestPreviewCloseJoinsBlockedRead(t *testing.T) {
	_, metadata := recoveryMetadata(t)
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
	preview, err := s.OpenPreview(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	pr := preview.(*previewReader)
	started := make(chan struct{})
	done := make(chan error, 1)
	go func() {
		pr.readIO.Lock()
		close(started)
		time.Sleep(50 * time.Millisecond)
		pr.readIO.Unlock()
		_, err := preview.ReadAt(make([]byte, 4), 0)
		done <- err
	}()
	<-started
	closed := make(chan struct{})
	go func() {
		_ = preview.Close()
		close(closed)
	}()
	select {
	case <-closed:
	case <-time.After(2 * time.Second):
		t.Fatal("Close did not join active I/O")
	}
	select {
	case <-done:
	case <-time.After(2 * time.Second):
		t.Fatal("blocked read did not finish after Close")
	}
}

func TestPreviewConcurrentOpenAtomic(t *testing.T) {
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

	const n = 8
	type result struct {
		p   PreviewReader
		err error
	}
	results := make(chan result, n)
	for i := 0; i < n; i++ {
		go func() {
			p, err := s.OpenPreview(rec.ID, 0)
			results <- result{p: p, err: err}
		}()
	}
	ok := 0
	var keep PreviewReader
	for i := 0; i < n; i++ {
		r := <-results
		if r.err == nil {
			ok++
			if keep == nil {
				keep = r.p
			} else {
				_ = r.p.Close()
			}
		}
	}
	if keep != nil {
		_ = keep.Close()
	}
	if ok != 1 {
		t.Fatalf("expected exactly one successful open, got %d", ok)
	}
}

func TestPauseOtherTorrentLeavesPreview(t *testing.T) {
	data, metadata := recoveryMetadata(t)
	dir := t.TempDir()
	s := testEngineAt(t, dir)
	rec := addRecoveryTorrent(t, s, metadata)
	otherData := bytes.Repeat([]byte("other torrent payload."), 8192)
	otherMeta := func() string {
		info := metainfo.Info{Name: "other.bin", Length: int64(len(otherData)), PieceLength: 16 * 1024}
		for start := 0; start < len(otherData); start += int(info.PieceLength) {
			hash := sha1.Sum(otherData[start:min(start+int(info.PieceLength), len(otherData))])
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
		return base64.StdEncoding.EncodeToString(buf.Bytes())
	}()
	other := addRecoveryTorrent(t, s, otherMeta)
	if other.ID == rec.ID || other.InfoHash == rec.InfoHash {
		t.Fatal("expected a distinct torrent for target-aware pause")
	}
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
	defer preview.Close()
	if code, response := s.DispatchControl("POST", "/pause", `{"id":"`+other.ID+`"}`); code != 200 {
		t.Fatalf("pause other: %d %s", code, response)
	}
	s.mu.Lock()
	active := s.previewActive
	s.mu.Unlock()
	if active == nil {
		t.Fatal("pausing unrelated torrent must not close this preview")
	}
}
