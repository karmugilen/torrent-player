package main

import (
	"bytes"
	"encoding/json"
	"testing"
	"time"
)

func idleView(t *testing.T, s *EngineServer, id string) TorrentView {
	t.Helper()
	code, raw := s.DispatchControl("GET", "/torrent/"+id, "")
	if code != 200 {
		t.Fatalf("torrent view: %d %s", code, raw)
	}
	var view TorrentView
	if err := json.Unmarshal(raw, &view); err != nil {
		t.Fatal(err)
	}
	return view
}

func waitConns(t *testing.T, rec *TorrentRecord, want func(int) bool) {
	t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		if want(len(rec.Torrent.PeerConns())) {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatalf("connection count never matched; last %d", len(rec.Torrent.PeerConns()))
}

func TestPlaybackRestWaitsForReadaheadNotLookahead(t *testing.T) {
	window := playbackWindowState{
		urgent: pieceSpan{10, 12}, readahead: pieceSpan{12, 20}, lookahead: pieceSpan{20, 30},
	}
	complete := map[int]bool{}
	for i := 10; i < 20; i++ {
		complete[i] = true
	}
	filled := func(i int) bool { return complete[i] }
	if playbackWindowFilling(window, filled) {
		t.Fatal("an evicted lookahead piece kept the session from resting")
	}
	delete(complete, 14)
	if !playbackWindowFilling(window, filled) {
		t.Fatal("a missing readahead piece let the session rest")
	}
}

func TestRestDoesNotZeroTheConnectionCapAfterWake(t *testing.T) {
	s, rec := ghostProPlaybackFixture(t, 1<<20, 4, true)
	const peers = 40
	rec.Torrent.SetMaxEstablishedConns(peers)
	s.mu.Lock()
	s.maxPeers = peers
	s.mu.Unlock()
	started := make(chan struct{})
	release := make(chan struct{})
	restBeforeLimitTestHook = func() {
		close(started)
		<-release
	}
	t.Cleanup(func() { restBeforeLimitTestHook = nil })
	rec.playbackIdle.resting.Store(true)
	done := make(chan struct{})
	go func() {
		restDirectPlay(rec)
		close(done)
	}()
	select {
	case <-started:
	case <-time.After(5 * time.Second):
		t.Fatal("rest did not reach the connection-cap update")
	}
	if !s.wakeDirectPlay(rec) {
		t.Fatal("wake did not see the session resting")
	}
	close(release)
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("rest did not finish")
	}
	if rec.playbackIdle.resting.Load() {
		t.Fatal("session still resting after wake")
	}
	if got := rec.Torrent.SetMaxEstablishedConns(peers); got == 0 {
		t.Fatal("wake restored peers and rest then closed the connection cap")
	}
}

func TestIdleDirectPlayRestsWakesAndBecomesDropReady(t *testing.T) {
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
	read := func(p *playbackReader, offset int64) {
		t.Helper()
		got := make([]byte, 1<<20)
		done := make(chan error, 1)
		go func() {
			_, err := p.ReadAt(got, offset)
			done <- err
		}()
		select {
		case err := <-done:
			if err != nil {
				t.Fatal(err)
			}
		case <-time.After(20 * time.Second):
			t.Fatalf("read at %d stalled", offset)
		}
		if !bytes.Equal(got, data[offset:offset+int64(len(got))]) {
			t.Fatalf("incorrect bytes at %d", offset)
		}
	}
	// Playing connects the seed; closing the player leaves the session idle.
	first, err := leech.OpenPlayback(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	read(first, 0)
	waitConns(t, rec, func(n int) bool { return n > 0 })
	first.Close()

	start := time.Now()
	leech.directPlayIdleTick(start)
	leech.directPlayIdleTick(start.Add(directPlayRestAfter - time.Second))
	if v := idleView(t, leech, rec.ID); v.Resting || v.IdleDropReady {
		t.Fatalf("rested early: %+v", v)
	}

	// Rest: peers close, the record stays, and it is not yet drop-ready.
	leech.directPlayIdleTick(start.Add(directPlayRestAfter + time.Second))
	waitConns(t, rec, func(n int) bool { return n == 0 })
	if v := idleView(t, leech, rec.ID); !v.Resting || v.IdleDropReady {
		t.Fatalf("expected resting without drop: %+v", v)
	}
	leech.directPlayIdleTick(start.Add(5*time.Minute + time.Second))
	if v := idleView(t, leech, rec.ID); !v.IdleDropReady || v.PlaybackReaders != 0 {
		t.Fatalf("expected drop-ready with no player: %+v", v)
	}

	// An open player blocks the drop even past the limit.
	p, err := leech.OpenPlayback(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	leech.directPlayIdleTick(start.Add(7 * time.Minute))
	if v := idleView(t, leech, rec.ID); !v.Resting || v.IdleDropReady || v.PlaybackReaders != 1 {
		t.Fatalf("open player must keep the session: %+v", v)
	}

	// A read wakes the session, reconnects and returns correct bytes.
	read(p, 10<<20)
	if v := idleView(t, leech, rec.ID); v.Resting || len(rec.Torrent.PeerConns()) == 0 {
		t.Fatalf("read did not wake the session: %+v", v)
	}

	// Player closed and idle past the limit: rest, then drop-ready.
	p.Close()
	later := time.Now()
	leech.directPlayIdleTick(later.Add(directPlayRestAfter + time.Second))
	if v := idleView(t, leech, rec.ID); !v.Resting || v.IdleDropReady || v.PlaybackReaders != 0 {
		t.Fatalf("expected rest before the limit: %+v", v)
	}
	leech.directPlayIdleTick(later.Add(5*time.Minute + time.Second))
	if v := idleView(t, leech, rec.ID); !v.IdleDropReady {
		t.Fatalf("expected drop-ready: %+v", v)
	}

	// Pausing or turning the feature off wakes and clears the state.
	if code, response := leech.DispatchControl("POST", "/pause", `{"id":"`+rec.ID+`"}`); code != 200 {
		t.Fatalf("pause: %d %s", code, response)
	}
	leech.directPlayIdleTick(later.Add(6 * time.Minute))
	if v := idleView(t, leech, rec.ID); v.Resting || v.IdleDropReady {
		t.Fatalf("paused session kept idle state: %+v", v)
	}
	if code, response := leech.DispatchControl("POST", "/resume", `{"id":"`+rec.ID+`"}`); code != 200 {
		t.Fatalf("resume: %d %s", code, response)
	}
	leech.directPlayIdleTick(later.Add(7 * time.Minute))
	leech.directPlayIdleTick(later.Add(10 * time.Minute))
	if v := idleView(t, leech, rec.ID); !v.Resting {
		t.Fatalf("resumed session did not rest again: %+v", v)
	}
	if code, response := leech.DispatchControl("POST", "/settings", `{"idleDropMinutes":0}`); code != 200 {
		t.Fatalf("idle off: %d %s", code, response)
	}
	leech.directPlayIdleTick(later.Add(11 * time.Minute))
	if v := idleView(t, leech, rec.ID); v.Resting || v.IdleDropReady {
		t.Fatalf("feature off kept idle state: %+v", v)
	}
	var settings SettingsResponse
	_, raw := leech.DispatchControl("GET", "/settings", "")
	if err := json.Unmarshal(raw, &settings); err != nil || settings.IdleDropMinutes != 0 {
		t.Fatalf("settings: %+v %v", settings, err)
	}
}
