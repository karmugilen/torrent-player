package main

import (
	"testing"
	"time"
)

// An external player stops reading when paused or when its own buffer is
// full. The playing reader keeps its window and its peers until the buffer
// ahead is filled; probes still park, and /play wakes a resting session.
func TestPlayingReaderKeepsWindowAndPeersWhileIdle(t *testing.T) {
	s, rec, _, data := configuredLargeVideo(t, 48)
	if code, response := s.DispatchControl("POST", "/settings", `{"ghostProMode":true,"idleDropMinutes":5}`); code != 200 {
		t.Fatalf("settings: %d %s", code, response)
	}
	rec.mu.Lock()
	rec.DirectPlay = true
	rec.mu.Unlock()
	writeVerifiedPiece(t, s, rec, data, 10)
	windows := func() int { return len(s.playbackWindows(rec.Torrent)) }
	expire := func(p *playbackReader) {
		p.mu.Lock()
		generation := p.idleGeneration
		p.mu.Unlock()
		p.idleExpired(generation)
	}

	playing, err := s.OpenPlayback(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer playing.Close()
	if n, err := playing.ReadAt(make([]byte, 4), 10<<20); err != nil || n != 4 {
		t.Fatalf("body read: %d %v", n, err)
	}
	probe, err := s.OpenPlayback(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer probe.Close()
	// The probe opened on the resume position without reading there: only
	// the playing reader's spot is fetched.
	if windows() != 1 {
		t.Fatalf("windows before idle: %d", windows())
	}
	expire(probe)
	expire(playing)
	if windows() != 1 {
		t.Fatalf("idle probe must park and the playing reader keep its window: %d windows", windows())
	}

	start := time.Now()
	s.directPlayIdleTick(start)
	s.directPlayIdleTick(start.Add(directPlayRestAfter + time.Second))
	if v := idleView(t, s, rec.ID); v.Resting {
		t.Fatalf("rested while the buffer ahead was missing pieces: %+v", v)
	}

	// Without a window to fill the session rests; /play reconnects it at once.
	playing.Close()
	s.directPlayIdleTick(start.Add(directPlayRestAfter + 2*time.Second))
	if v := idleView(t, s, rec.ID); !v.Resting {
		t.Fatalf("expected rest with no window to fill: %+v", v)
	}
	playWithNext(t, s, rec.ID, 0, nil)
	deadline := time.Now().Add(5 * time.Second)
	for idleView(t, s, rec.ID).Resting {
		if time.Now().After(deadline) {
			t.Fatal("/play did not wake the resting session")
		}
		time.Sleep(10 * time.Millisecond)
	}
}

// The idle timer fires once. A reader that is still the playhead has to arm
// another timer, or it keeps its window after a newer viewer starts playing.
func TestFormerPlayheadParksOnceItIsNoLongerPlaying(t *testing.T) {
	s, rec := ghostProPlaybackFixture(t, 1<<20, 64, false)
	playing, err := s.OpenPlayback(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer playing.Close()
	playing.testApplyOffset(20 << 20)
	playing.mu.Lock()
	playing.finishFocusLocked()
	generation := playing.idleGeneration
	playing.mu.Unlock()
	playing.priorityOwner.setPlaying(playing)
	playing.idleExpired(generation)
	playing.mu.Lock()
	rearmed := playing.idleGeneration
	stillOpen := playing.windowApplied
	playing.mu.Unlock()
	if rearmed == generation {
		t.Fatal("playing reader dropped its idle timer")
	}
	if !stillOpen {
		t.Fatal("playing reader parked itself")
	}
	next, err := s.OpenPlayback(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer next.Close()
	next.testApplyOffset(40 << 20)
	next.mu.Lock()
	next.finishFocusLocked()
	next.mu.Unlock()
	next.priorityOwner.setPlaying(next)
	playing.idleExpired(rearmed)
	for _, window := range s.playbackWindows(rec.Torrent) {
		if window.urgent.contains(20) {
			t.Fatal("former playhead kept the old window")
		}
	}
}
