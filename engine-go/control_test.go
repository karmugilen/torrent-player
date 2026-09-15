package main

import (
	"testing"
	"time"
)

func TestNativeControlNeedsNoControlListener(t *testing.T) {
	s := testEngine(t)
	if err := s.StartStreaming(); err != nil {
		t.Fatal(err)
	}
	if s.ctlListener != nil || s.ctlPort != 0 || s.streamPort == 0 {
		t.Fatal("unexpected listener configuration")
	}
	code, body := s.DispatchControl("GET", "/stats", "")
	if code != 200 {
		t.Fatalf("stats: %d %s", code, body)
	}
	code, _ = s.DispatchControl("GET", "/torrent/missing", "")
	if code != 404 {
		t.Fatalf("lost error status: %d", code)
	}
	code, _ = s.DispatchControl("POST", "/pause", "invalid JSON")
	if code != 400 {
		t.Fatalf("invalid request: %d", code)
	}
}

func TestChangeEventsAreRetainedAndBroadcast(t *testing.T) {
	s := testEngine(t)
	initial := s.WaitForChange(^uint64(0), time.Second)
	s.notifyChange() // Event before the wait must still be delivered.
	version := s.WaitForChange(initial, time.Second)
	if version == initial {
		t.Fatal("lost event before wait")
	}
	results := make(chan uint64, 2)
	for i := 0; i < 2; i++ {
		go func() { results <- s.WaitForChange(version, 2*time.Second) }()
	}
	s.notifyChange()
	for i := 0; i < 2; i++ {
		select {
		case next := <-results:
			if next <= version {
				t.Fatal("observer missed broadcast")
			}
		case <-time.After(3 * time.Second):
			t.Fatal("observer blocked")
		}
	}
	latest := s.WaitForChange(^uint64(0), time.Second)
	if next := s.WaitForChange(latest, 10*time.Millisecond); next != latest {
		t.Fatal("idle wait manufactured an update")
	}
}
