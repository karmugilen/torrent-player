package main

import (
	"bytes"
	"encoding/json"
	"sync"
	"testing"
	"time"
)

func TestDeliveryPreviewCanReuseVerifiedTail(t *testing.T) {
	s, rec, pieceLen := largePlaybackFixtureMode(t, 24, true)
	data := bytes.Repeat([]byte("abcdefgh"), int(24*pieceLen/8))
	writeVerifiedPiece(t, s, rec, data, 0)
	writeVerifiedPiece(t, s, rec, data, 23)
	p, err := s.OpenPreview(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer p.Close()
	buf := make([]byte, 8)
	if n, err := p.ReadAt(buf, 23*pieceLen); err != nil || n != len(buf) {
		t.Fatalf("already verified tail must remain readable: n=%d err=%v", n, err)
	}
}

func TestDeliveryWarmCachePreservesExplicitPlayFromStart(t *testing.T) {
	s, rec, pieceLen := largePlaybackFixtureMode(t, 24, true)
	data := bytes.Repeat([]byte("abcdefgh"), int(24*pieceLen/8))
	for _, index := range []int{10, 11, 12, 13} {
		writeVerifiedPiece(t, s, rec, data, index)
	}
	s.persistVerifiedResumeWindow(rec, 0, 10*pieceLen)
	if _, _, ok := s.loadResumeWindow(rec.InfoHash); !ok {
		t.Fatal("fixture did not persist resume")
	}
	zero := int64(0)
	body, _ := json.Marshal(PlayRequest{ID: rec.ID, FileIndex: intPtr(0), Offset: &zero})
	if code, response := s.DispatchControl("POST", "/play", string(body)); code != 200 {
		t.Fatalf("play from start: %d %s", code, response)
	}
	p, err := s.OpenPlayback(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer p.Close()
	if got := p.priorityOwner.resumePosition(p.file); got != 0 {
		t.Fatalf("cache changed explicit start position: got %d, want 0", got)
	}
}

func TestDeliveryConcurrentCacheClears(t *testing.T) {
	s := &EngineServer{downloadDir: t.TempDir()}
	var wg sync.WaitGroup
	for i := 0; i < 4; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for n := 0; n < 20; n++ {
				if err := s.clearCacheCategories([]string{"peers", "dht"}); err != nil {
					t.Error(err)
				}
			}
		}()
	}
	wg.Wait()
}

func TestDeliveryExpiredPreviewReleasesSlot(t *testing.T) {
	_, metadata := recoveryMetadata(t)
	s := testEngineAt(t, t.TempDir())
	rec := addRecoveryTorrent(t, s, metadata)
	p, err := s.OpenPreview(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer p.Close()
	<-p.(*previewReader).ctx.Done()
	deadline := time.Now().Add(200 * time.Millisecond)
	for time.Now().Before(deadline) {
		s.mu.RLock()
		active := s.previewActive
		s.mu.RUnlock()
		if active == nil {
			return
		}
		time.Sleep(time.Millisecond)
	}
	t.Fatal("expired preview still owns slot and temporary storage/demand")
}

func TestPreviewReleaseInvalidatesTemporaryCompletion(t *testing.T) {
	data, metadata := recoveryMetadata(t)
	s := testEngineAt(t, t.TempDir())
	rec := addRecoveryTorrent(t, s, metadata)
	p, err := s.OpenPreview(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer p.Close()
	writeVerifiedPiece(t, s, rec, data, 0)
	if !rec.Torrent.Piece(0).State().Complete {
		t.Fatal("fixture piece not verified")
	}
	p.Close()
	if rec.Torrent.Piece(0).State().Complete {
		t.Fatal("discarded preview bytes still reported complete")
	}
}

func TestPreviewHonorsPayloadGate(t *testing.T) {
	_, metadata := recoveryMetadata(t)
	s := testEngineAt(t, t.TempDir())
	rec := addRecoveryTorrent(t, s, metadata)
	s.payloadTransfersAllowed.Store(false)
	p, err := s.OpenPreview(rec.ID, 0)
	if err != nil {
		return
	}
	defer p.Close()
	if n, err := p.ReadAt(make([]byte, 8), 0); n >= 0 && err == nil {
		t.Fatal("preview bypassed payload policy")
	}
}
