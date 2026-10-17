package main

import (
	"bytes"
	"encoding/json"
	"testing"
)

func TestLivePreviewStateFollowsVerifiedPlaybackPosition(t *testing.T) {
	s, rec, pieceLen := largePlaybackFixtureMode(t, 24, true)
	data := bytes.Repeat([]byte("abcdefgh"), int(24*pieceLen/8))
	writeVerifiedPiece(t, s, rec, data, 10)
	p, err := s.OpenPlayback(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer p.Close()
	if _, err := p.ReadAt(make([]byte, 64), 10*pieceLen); err != nil {
		t.Fatal(err)
	}
	code, body := s.DispatchControl("GET", "/preview-state/"+rec.ID+"?fileIndex=0", "")
	if code != 200 {
		t.Fatalf("live state: %d %s", code, body)
	}
	var state struct {
		Active     bool  `json:"active"`
		ByteOffset int64 `json:"byteOffset"`
	}
	if err := json.Unmarshal(body, &state); err != nil {
		t.Fatal(err)
	}
	if !state.Active || state.ByteOffset < 10*pieceLen {
		t.Fatalf("state didn't follow body read: %+v", state)
	}
}

func TestPlaybackHandleSurvivesDownloadCompletion(t *testing.T) {
	s, rec, data := playbackFixture(t)
	p, err := s.OpenPlayback(rec.ID, 1)
	if err != nil {
		t.Fatal(err)
	}
	defer p.Close()
	writeVerifiedPiece(t, s, rec, data, 0)
	writeVerifiedPiece(t, s, rec, data, 1)
	if _, err := p.ReadAt(make([]byte, 4), 0); err != nil {
		t.Fatal(err)
	}
	for i := 2; i < rec.Torrent.NumPieces(); i++ {
		writeVerifiedPiece(t, s, rec, data, i)
	}
	code, body := s.DispatchControl("GET", "/torrent/"+rec.ID, "")
	if code != 200 {
		t.Fatalf("completion status: %d %s", code, body)
	}
	want := data[5:]
	got := make([]byte, len(want))
	n, err := p.ReadAt(got, 0)
	if err != nil || n != len(want) || !bytes.Equal(got, want) {
		t.Fatalf("same handle stopped after completion: n=%d err=%v", n, err)
	}
}

func TestPlayingPreviewDoesNotDemandMissingPieces(t *testing.T) {
	s, rec, pieceLen := largePlaybackFixtureMode(t, 24, true)
	player, err := s.OpenPlayback(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer player.Close()
	preview, err := s.OpenPreview(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer preview.Close()
	if n, err := preview.ReadAt(make([]byte, 64), 10*pieceLen); n >= 0 || err == nil {
		t.Fatalf("missing cached range must be unavailable: n=%d err=%v", n, err)
	}
	owner := s.ensurePlaybackCoordinator(rec.Torrent)
	owner.mu.Lock()
	defer owner.mu.Unlock()
	if len(owner.previewDemand) != 0 {
		t.Fatal("thumbnail added demand alongside player")
	}
}
