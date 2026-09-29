package main

import (
	"encoding/json"
	"fmt"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestNetworkChangeIgnoresStaleAndDuplicateGenerations(t *testing.T) {
	s := testEngine(t)
	s.trackerSockets = &networkDialer{}

	post := func(generation int64, connected bool) int {
		t.Helper()
		body := strings.NewReader(fmt.Sprintf(`{"generation":%d,"connected":%t}`, generation, connected))
		w := httptest.NewRecorder()
		s.handleNetworkChange(w, httptest.NewRequest("POST", "/network", body))
		return w.Code
	}

	if code := post(1, true); code != 200 {
		t.Fatalf("first network event: %d", code)
	}
	if code := post(1, false); code != 200 {
		t.Fatalf("duplicate generation should be ignored: %d", code)
	}
	s.networkMu.Lock()
	if !s.networkKnown || s.networkGeneration != 1 {
		s.networkMu.Unlock()
		t.Fatalf("generation mutated by duplicate: known=%v gen=%d", s.networkKnown, s.networkGeneration)
	}
	s.networkMu.Unlock()

	if code := post(0, false); code != 200 {
		t.Fatalf("stale generation should be ignored: %d", code)
	}
	if code := post(2, true); code != 200 {
		t.Fatalf("newer generation rejected: %d", code)
	}
	s.networkMu.Lock()
	defer s.networkMu.Unlock()
	if s.networkGeneration != 2 {
		t.Fatalf("expected generation 2, got %d", s.networkGeneration)
	}
}

func TestNetworkChangeRejectsInvalidBody(t *testing.T) {
	s := testEngine(t)
	w := httptest.NewRecorder()
	s.handleNetworkChange(w, httptest.NewRequest("POST", "/network", strings.NewReader(`{"generation":1}`)))
	if w.Code != 400 {
		t.Fatalf("missing connected: %d %s", w.Code, w.Body.String())
	}
	var payload map[string]string
	if err := json.Unmarshal(w.Body.Bytes(), &payload); err != nil {
		t.Fatal(err)
	}
	if payload["error"] == "" {
		t.Fatalf("expected error payload, got %#v", payload)
	}
}
