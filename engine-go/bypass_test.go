package main

import (
	"bytes"
	"encoding/json"
	"net"
	"testing"
	"time"
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

func TestSplitConnZeroProgressDoesNotSpin(t *testing.T) {
	rec := &recordConn{}
	wrapped := newSplitConn(rec).(*splitConn)
	wrapped.delay = 0
	// Force underlying write to report 0,nil via a stub.
	zero := &zeroProgressConn{}
	wrapped.Conn = zero
	_, err := wrapped.Write([]byte("hello-world"))
	if err == nil {
		t.Fatal("expected zero-progress error")
	}
}

type zeroProgressConn struct{ net.Conn }

func (c *zeroProgressConn) Write(b []byte) (int, error) { return 0, nil }
func (c *zeroProgressConn) Close() error                { return nil }
func (c *zeroProgressConn) LocalAddr() net.Addr         { return &net.TCPAddr{} }
func (c *zeroProgressConn) RemoteAddr() net.Addr {
	return &net.TCPAddr{IP: net.ParseIP("1.1.1.1"), Port: 80}
}
func (c *zeroProgressConn) SetDeadline(time.Time) error      { return nil }
func (c *zeroProgressConn) SetReadDeadline(time.Time) error  { return nil }
func (c *zeroProgressConn) SetWriteDeadline(time.Time) error { return nil }

func TestBypassUsesPerClientHook(t *testing.T) {
	a := testEngine(t)
	b := testEngine(t)
	a.trackerSockets = &networkDialer{}
	b.trackerSockets = &networkDialer{}
	if code, body := a.DispatchControl("POST", "/bypass", `{"enabled":true}`); code != 200 {
		t.Fatalf("enable a: %d %s", code, body)
	}
	// Second server must stay off: hooks are per-client, not process-global.
	status := b.bypassSnapshot()
	if status.Enabled || status.Active {
		t.Fatalf("second server unexpectedly active: %+v", status)
	}
	if code, body := a.DispatchControl("POST", "/bypass", `{"enabled":false}`); code != 200 {
		t.Fatalf("disable a: %d %s", code, body)
	}
}
