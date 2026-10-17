package main

import (
	"bytes"
	"encoding/json"
	"net"
	"testing"
)

func TestBypassDefaultsOff(t *testing.T) {
	s := testEngine(t)
	code, body := s.DispatchControl("GET", "/bypass", "")
	if code != 200 {
		t.Fatalf("status %d %s", code, body)
	}
	var status bypassStatus
	if err := json.Unmarshal(body, &status); err != nil {
		t.Fatal(err)
	}
	if status.Enabled || status.Active || status.Mode != bypassModeOff || status.Error != nil {
		t.Fatalf("unexpected default bypass status: %+v", status)
	}
	if len(status.Coverage) != 0 {
		t.Fatalf("off mode must report empty coverage: %v", status.Coverage)
	}
}

func TestBypassEnableActivatesTCPSplit(t *testing.T) {
	s := testEngine(t)
	s.trackerSockets = &networkDialer{}
	code, body := s.DispatchControl("POST", "/bypass", `{"enabled":true}`)
	if code != 200 {
		t.Fatalf("enable: %d %s", code, body)
	}
	var status bypassStatus
	if err := json.Unmarshal(body, &status); err != nil {
		t.Fatal(err)
	}
	if !status.Enabled || !status.Active || status.Mode != bypassModeTCPSplit {
		t.Fatalf("expected active tcp-write-split, got %+v", status)
	}
	if len(status.Coverage) == 0 {
		t.Fatal("expected coverage list")
	}
	found := false
	for _, c := range status.Coverage {
		if c == "outgoing-tcp-peers" {
			found = true
		}
	}
	if !found {
		t.Fatalf("coverage missing outgoing-tcp-peers: %v", status.Coverage)
	}
	s.trackerSockets.mu.Lock()
	split := s.trackerSockets.split
	s.trackerSockets.mu.Unlock()
	if !split {
		t.Fatal("tracker dialer was not switched to split mode")
	}

	code, body = s.DispatchControl("POST", "/bypass", `{"enabled":false}`)
	if code != 200 {
		t.Fatalf("disable: %d %s", code, body)
	}
	if err := json.Unmarshal(body, &status); err != nil {
		t.Fatal(err)
	}
	if status.Enabled || status.Active || status.Mode != bypassModeOff {
		t.Fatalf("expected off after disable: %+v", status)
	}
}

type recordConn struct {
	net.Conn
	writes [][]byte
}

func (c *recordConn) Write(b []byte) (int, error) {
	c.writes = append(c.writes, append([]byte(nil), b...))
	return len(b), nil
}

func TestSplitConnFragmentsInitialWrite(t *testing.T) {
	rec := &recordConn{}
	wrapped := newSplitConn(rec).(*splitConn)
	wrapped.delay = 0
	payload := []byte("BitTorrent protocolXXXXXXXX")
	if _, err := wrapped.Write(payload); err != nil {
		t.Fatal(err)
	}
	if len(rec.writes) < 2 {
		t.Fatalf("expected fragmented writes, got %d", len(rec.writes))
	}
	if len(rec.writes[0]) != 1 {
		t.Fatalf("first fragment should be 1 byte, got %d", len(rec.writes[0]))
	}
	var joined []byte
	for _, w := range rec.writes {
		joined = append(joined, w...)
	}
	if !bytes.Equal(joined, payload) {
		t.Fatalf("joined writes=%q", joined)
	}
}
