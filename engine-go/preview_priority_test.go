package main

import (
	"bytes"
	"testing"
	"time"

	"github.com/anacrolix/torrent"
	"golang.org/x/time/rate"
)

// A thumbnail decoder reads a few hundred KiB inside a later episode. With
// no player open, those pieces must not wait behind every selected episode's
// startup head and tail.
func TestPreviewOfLaterEpisodeIsNotStarvedByStartupHints(t *testing.T) {
	if testing.Short() {
		t.Skip("local peer transfer")
	}
	const episode = int64(24 << 20)
	data, newPeer, newDownloader := localFilesSwarmModes(t, "Season", []int64{episode, episode, episode, episode})
	seed, _ := newPeer(true, func(cfg *torrent.ClientConfig) {
		// 2 MiB/s: the four episodes' 32 MiB of startup hints take ~16 s.
		cfg.UploadRateLimiter = rate.NewLimiter(2<<20, 256<<10)
	})
	leech, rec := newDownloader()
	if rec.Torrent.AddClientPeer(seed.client) == 0 {
		t.Fatal("no seed address")
	}
	reader, err := leech.OpenPreview(rec.ID, 3)
	if err != nil {
		t.Fatal(err)
	}
	defer reader.Close()
	offset := episode / 2
	got := make([]byte, 256<<10)
	start := time.Now()
	n, err := reader.ReadAt(got, offset)
	if err != nil || n != len(got) {
		t.Fatalf("preview read after %v: n=%d err=%v", time.Since(start), n, err)
	}
	base := 3 * episode
	if !bytes.Equal(got, data[base+offset:base+offset+int64(len(got))]) {
		t.Fatal("incorrect preview bytes")
	}
	if elapsed := time.Since(start); elapsed > 5*time.Second {
		t.Fatalf("preview read took %v behind startup hints", elapsed)
	}
}

// A thumbnail of an idle-rested Direct Play session reconnects its peers.
func TestPreviewWakesRestingDirectPlay(t *testing.T) {
	if testing.Short() {
		t.Skip("local peer transfer")
	}
	data, newPeer := localPeerSwarm(t)
	seed, _ := newPeer(true)
	leech, rec := newPeer(false)
	if code, response := leech.DispatchControl("POST", "/settings", `{"idleDropMinutes":5}`); code != 200 {
		t.Fatalf("idle setting: %d %s", code, response)
	}
	if rec.Torrent.AddClientPeer(seed.client) == 0 {
		t.Fatal("no seed address")
	}
	player, err := leech.OpenPlayback(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := player.ReadAt(make([]byte, 64<<10), 0); err != nil {
		t.Fatal(err)
	}
	waitConns(t, rec, func(n int) bool { return n > 0 })
	player.Close()
	now := time.Now()
	leech.directPlayIdleTick(now)
	leech.directPlayIdleTick(now.Add(directPlayRestAfter + time.Second))
	if !idleView(t, leech, rec.ID).Resting {
		t.Fatal("session did not rest")
	}
	waitConns(t, rec, func(n int) bool { return n == 0 })

	reader, err := leech.OpenPreview(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer reader.Close()
	offset := int64(20 << 20)
	got := make([]byte, 256<<10)
	if n, err := reader.ReadAt(got, offset); err != nil || n != len(got) {
		t.Fatalf("preview of a resting session: n=%d err=%v", n, err)
	}
	if !bytes.Equal(got, data[offset:offset+int64(len(got))]) {
		t.Fatal("incorrect preview bytes")
	}
	if idleView(t, leech, rec.ID).Resting {
		t.Fatal("preview left the session resting")
	}
}

// Pause, remove and preview close hold rec.mu while joining a reader's I/O.
// A read that wakes a resting session must therefore never wait for rec.mu.
func TestReadsWakingRestingDirectPlayDoNotDeadlockWithRecordLock(t *testing.T) {
	_, newPeer := localPeerSwarm(t)
	leech, rec := newPeer(false) // no seed: reads wait for data
	if code, response := leech.DispatchControl("POST", "/settings", `{"idleDropMinutes":5}`); code != 200 {
		t.Fatalf("idle setting: %d %s", code, response)
	}
	rest := func() {
		t.Helper()
		now := time.Now()
		leech.directPlayIdleTick(now)
		leech.directPlayIdleTick(now.Add(directPlayRestAfter + time.Second))
		if !rec.playbackIdle.resting.Load() {
			t.Fatal("session did not rest")
		}
	}
	within := func(what string, f func()) {
		t.Helper()
		done := make(chan struct{})
		go func() { f(); close(done) }()
		select {
		case <-done:
		case <-time.After(5 * time.Second):
			t.Fatalf("%s deadlocked", what)
		}
	}

	rest()
	preview, err := leech.OpenPreview(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	rec.mu.Lock()
	read := make(chan struct{})
	go func() { _, _ = preview.ReadAt(make([]byte, 64<<10), 20<<20); close(read) }()
	time.Sleep(200 * time.Millisecond)
	within("preview close under rec.mu", func() { _ = preview.(*previewReader).closeWithRecordLocked() })
	rec.mu.Unlock()
	within("preview read", func() { <-read })

	rest()
	player, err := leech.OpenPlayback(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	rec.mu.Lock()
	read = make(chan struct{})
	go func() { _, _ = player.ReadAt(make([]byte, 64<<10), 20<<20); close(read) }()
	time.Sleep(200 * time.Millisecond)
	within("player close under rec.mu", player.Close)
	rec.mu.Unlock()
	within("player read", func() { <-read })
}
