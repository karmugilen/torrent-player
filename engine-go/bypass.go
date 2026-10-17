package main

import (
	"context"
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/anacrolix/torrent"
)

const (
	bypassModeOff         = "off"
	bypassModeTCPSplit    = "tcp-write-split"
	bypassSplitFirstBytes = 8
)

// bypassCoverage lists transports actually adapted by the active technique.
var bypassTCPCoverage = []string{
	"outgoing-tcp-peers",
	"http-trackers",
	"websocket-trackers",
}

type bypassStatus struct {
	Enabled  bool     `json:"enabled"`
	Active   bool     `json:"active"`
	Mode     string   `json:"mode"`
	Coverage []string `json:"coverage"`
	Error    *string  `json:"error"`
}

type bypassController struct {
	mu       sync.Mutex
	enabled  bool
	active   bool
	mode     string
	coverage []string
	err      *string
	epoch    atomic.Uint64
}

// splitConn fragments the first writes so simple DPI signatures that match a
// single TCP segment see only a prefix. This is a userspace zapret/tpws-style
// split, not packet injection, and requires no root.
type splitConn struct {
	net.Conn
	mu        sync.Mutex
	remaining int
	delay     time.Duration
}

func newSplitConn(c net.Conn) net.Conn {
	if tc, ok := c.(*net.TCPConn); ok {
		_ = tc.SetNoDelay(true)
	}
	return &splitConn{Conn: c, remaining: bypassSplitFirstBytes, delay: 2 * time.Millisecond}
}

func (c *splitConn) Write(b []byte) (int, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.remaining <= 0 || len(b) <= 1 {
		return c.Conn.Write(b)
	}
	total := 0
	for len(b) > 0 {
		n := 1
		if c.remaining <= 0 {
			n = len(b)
		} else if len(b) < n {
			n = len(b)
		}
		if c.remaining > 0 && n > c.remaining {
			n = c.remaining
		}
		wrote, err := c.Conn.Write(b[:n])
		total += wrote
		if wrote > 0 && c.remaining > 0 {
			c.remaining -= wrote
			if c.remaining < 0 {
				c.remaining = 0
			}
		}
		if err != nil {
			return total, err
		}
		b = b[wrote:]
		if c.remaining > 0 && len(b) > 0 && c.delay > 0 {
			time.Sleep(c.delay)
		}
	}
	return total, nil
}

func (s *EngineServer) bypassSnapshot() bypassStatus {
	s.bypass.mu.Lock()
	defer s.bypass.mu.Unlock()
	return s.bypassStatusLocked()
}

func (s *EngineServer) handleGetBypass(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, http.StatusOK, s.bypassSnapshot())
}

func (s *EngineServer) handlePostBypass(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Enabled *bool `json:"enabled"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil || req.Enabled == nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid json"})
		return
	}
	status := s.setBypassEnabled(*req.Enabled)
	writeJSON(w, http.StatusOK, status)
}

func (s *EngineServer) setBypassEnabled(enabled bool) bypassStatus {
	s.bypass.mu.Lock()
	defer s.bypass.mu.Unlock()
	s.bypass.enabled = enabled
	if !enabled {
		s.deactivateBypassLocked()
		return s.bypassStatusLocked()
	}
	if err := s.activateBypassLocked(); err != nil {
		msg := err.Error()
		s.bypass.err = &msg
		s.bypass.active = false
		s.bypass.mode = bypassModeOff
		s.bypass.coverage = nil
		torrent.SetOutgoingTCPConnHook(nil)
		return s.bypassStatusLocked()
	}
	return s.bypassStatusLocked()
}

func (s *EngineServer) activateBypassLocked() error {
	if s.client == nil {
		return fmt.Errorf("torrent client unavailable")
	}
	s.bypass.epoch.Add(1)
	epoch := s.bypass.epoch.Load()
	torrent.SetOutgoingTCPConnHook(func(c net.Conn) net.Conn {
		if s.bypass.epoch.Load() != epoch {
			return c
		}
		return newSplitConn(c)
	})
	if s.trackerSockets != nil {
		s.trackerSockets.setSplit(true)
	}
	s.bypass.active = true
	s.bypass.mode = bypassModeTCPSplit
	s.bypass.coverage = append([]string(nil), bypassTCPCoverage...)
	s.bypass.err = nil
	return nil
}

func (s *EngineServer) deactivateBypassLocked() {
	s.bypass.epoch.Add(1)
	torrent.SetOutgoingTCPConnHook(nil)
	if s.trackerSockets != nil {
		s.trackerSockets.setSplit(false)
		s.trackerSockets.reset()
	}
	s.bypass.active = false
	s.bypass.mode = bypassModeOff
	s.bypass.coverage = nil
	s.bypass.err = nil
}

func (s *EngineServer) bypassStatusLocked() bypassStatus {
	coverage := append([]string(nil), s.bypass.coverage...)
	var errCopy *string
	if s.bypass.err != nil {
		v := *s.bypass.err
		errCopy = &v
	}
	mode := s.bypass.mode
	if mode == "" {
		mode = bypassModeOff
	}
	return bypassStatus{
		Enabled:  s.bypass.enabled,
		Active:   s.bypass.active,
		Mode:     mode,
		Coverage: coverage,
		Error:    errCopy,
	}
}

func (d *networkDialer) setSplit(enabled bool) {
	d.mu.Lock()
	d.split = enabled
	d.mu.Unlock()
}

func (d *networkDialer) DialContext(ctx context.Context, network, address string) (net.Conn, error) {
	d.mu.Lock()
	epoch := d.epoch
	split := d.split
	d.mu.Unlock()
	// Preserve loopback media/control traffic untreated.
	if host, _, err := net.SplitHostPort(address); err == nil {
		if ip := net.ParseIP(host); ip != nil && ip.IsLoopback() {
			split = false
		} else if host == "localhost" {
			split = false
		}
	}
	c, err := (&net.Dialer{Timeout: 6 * time.Second, KeepAlive: 30 * time.Second}).DialContext(ctx, network, address)
	if err != nil {
		return nil, err
	}
	if split && strings.HasPrefix(network, "tcp") {
		c = newSplitConn(c)
	}
	d.mu.Lock()
	defer d.mu.Unlock()
	if epoch != d.epoch {
		_ = c.Close()
		return nil, fmt.Errorf("network changed during tracker dial")
	}
	if d.connections == nil {
		d.connections = make(map[*networkConn]struct{})
	}
	wrapped := &networkConn{Conn: c, owner: d}
	d.connections[wrapped] = struct{}{}
	return wrapped, nil
}
