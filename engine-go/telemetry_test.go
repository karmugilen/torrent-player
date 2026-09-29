package main

import (
	"encoding/json"
	"testing"
	"time"

	"github.com/anacrolix/torrent"
)

func waitUntilNotChecking(t *testing.T, rec *TorrentRecord) {
	t.Helper()
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		rec.mu.RLock()
		checking := rec.Checking
		rec.mu.RUnlock()
		if !checking {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("torrent stayed in restore-checking")
}

func TestStatusFingerprintIgnoresPlaybackPriorityChurn(t *testing.T) {
	s, rec, _ := playbackFixture(t)
	waitUntilNotChecking(t, rec)
	before := s.statusFingerprint()
	n := rec.Torrent.NumPieces()
	if n < 2 {
		t.Fatal("fixture too small")
	}
	rec.Torrent.Piece(0).SetPriority(torrent.PiecePriorityNow)
	rec.Torrent.Piece(1).SetPriority(torrent.PiecePriorityReadahead)
	if s.statusFingerprint() != before {
		t.Fatal("playback priority churn must not wake Android")
	}
	if len(rec.Torrent.PieceStateRuns()) < 1 {
		t.Fatal("expected piece-state runs")
	}
}

func TestStatusFingerprintWakesOnVerificationAndRates(t *testing.T) {
	s, rec, data := playbackFixture(t)
	waitUntilNotChecking(t, rec)
	before := s.statusFingerprint()
	writeVerifiedPiece(t, s, rec, data, 0)
	if s.statusFingerprint() == before {
		t.Fatal("piece verification must stay observable without PieceStateRuns JSON")
	}
	before = s.statusFingerprint()
	rec.mu.Lock()
	rec.rates.sample(rec.rates.lastSample.Add(1e9), rec.rates.downloaded+1000, rec.rates.uploaded, false)
	rec.mu.Unlock()
	if s.statusFingerprint() == before {
		t.Fatal("per-torrent rate change must still wake Android")
	}
}

func TestVerifiedFileBytesCacheMatchesWalk(t *testing.T) {
	s, rec, data := playbackFixture(t)
	waitUntilNotChecking(t, rec)
	code, body := s.DispatchControl("GET", "/torrent/"+rec.ID, "")
	if code != 200 {
		t.Fatalf("status: %d %s", code, body)
	}
	var first TorrentView
	if err := json.Unmarshal(body, &first); err != nil {
		t.Fatal(err)
	}
	code, body = s.DispatchControl("GET", "/torrent/"+rec.ID, "")
	var second TorrentView
	if code != 200 || json.Unmarshal(body, &second) != nil {
		t.Fatalf("repeat status: %d %s", code, body)
	}
	if first.VerifiedBytes != second.VerifiedBytes || first.Files[1].VerifiedBytes != second.Files[1].VerifiedBytes {
		t.Fatalf("cached verified bytes drifted: %+v vs %+v", first, second)
	}
	writeVerifiedPiece(t, s, rec, data, 0)
	got := verifiedFileBytes(rec.Torrent)
	code, body = s.DispatchControl("GET", "/torrent/"+rec.ID, "")
	var after TorrentView
	if code != 200 || json.Unmarshal(body, &after) != nil {
		t.Fatalf("after verify: %d %s", code, body)
	}
	if after.Files[0].VerifiedBytes != got[0] || after.Files[1].VerifiedBytes != got[1] {
		t.Fatalf("cache missed completion: view=%+v walk=%v", after.Files, got)
	}
}
