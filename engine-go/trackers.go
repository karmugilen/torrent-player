package main

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"
)

const (
	trackerListSource      = "https://raw.githubusercontent.com/ngosang/trackerslist/master/trackers_best.txt"
	trackerListLimit       = 20
	trackerSupplementLimit = 24
	trackerResponseLimit   = 64 * 1024
	trackerRefreshInterval = 24 * time.Hour
)

// This cache contains only the public list and its HTTP validators, never
// torrent hashes or trackers supplied by the user (which may contain passkeys).
type trackerCache struct {
	Version      int       `json:"version"`
	Trackers     []string  `json:"trackers"`
	CheckedAt    time.Time `json:"checkedAt"`
	NextAttempt  time.Time `json:"nextAttempt"`
	Failures     int       `json:"failures"`
	ETag         string    `json:"etag,omitempty"`
	LastModified string    `json:"lastModified,omitempty"`
}

type trackerPool struct {
	mu     sync.Mutex
	cache  trackerCache
	path   string
	client *http.Client
	source string
	cancel context.CancelFunc
	done   chan struct{}
}

func newTrackerPool(dir string) *trackerPool {
	p := &trackerPool{
		path:   filepath.Join(dir, "public-trackers-v1.json"),
		source: trackerListSource,
		client: &http.Client{
			Timeout: 10 * time.Second,
			CheckRedirect: func(req *http.Request, via []*http.Request) error {
				if len(via) >= 3 || req.URL.Scheme != "https" || req.URL.Host != "raw.githubusercontent.com" {
					return fmt.Errorf("unexpected tracker-list redirect")
				}
				return nil
			},
		},
	}
	if f, err := os.Open(p.path); err == nil {
		defer f.Close()
		var c trackerCache
		data, err := io.ReadAll(io.LimitReader(f, trackerResponseLimit+1))
		if err == nil && len(data) <= trackerResponseLimit && json.Unmarshal(data, &c) == nil && c.Version == 1 {
			c.Trackers = parseTrackerList(strings.Join(c.Trackers, "\n"))
			// Do not let a corrupt timestamp suppress refresh indefinitely.
			if c.NextAttempt.After(time.Now().Add(trackerRefreshInterval)) || c.CheckedAt.After(time.Now()) {
				c.NextAttempt = time.Time{}
			}
			c.Failures = max(0, min(c.Failures, 7))
			if len(c.Trackers) == 0 {
				c.ETag, c.LastModified = "", ""
			}
			p.cache = c
		}
	}
	return p
}

func (p *trackerPool) snapshot() []string {
	if p == nil {
		return append([]string(nil), defaultAnnounce...)
	}
	p.mu.Lock()
	defer p.mu.Unlock()
	if len(p.cache.Trackers) == 0 {
		return append([]string(nil), defaultAnnounce...)
	}
	// The maintained best list may have no WebSocket endpoints. Retain the
	// bundled WebRTC signaling trackers alongside the ranked public list.
	result := append([]string(nil), defaultAnnounce[:4]...)
	seen := make(map[string]bool)
	for _, tr := range result {
		seen[tr] = true
	}
	for _, tr := range p.cache.Trackers {
		if !seen[tr] && len(result) < trackerSupplementLimit {
			result = append(result, tr)
			seen[tr] = true
		}
	}
	return result
}

func (p *trackerPool) start(onRefresh func()) {
	ctx, cancel := context.WithCancel(context.Background())
	p.cancel, p.done = cancel, make(chan struct{})
	go func() {
		defer close(p.done)
		for {
			changed, err := p.refresh(ctx, time.Now())
			if ctx.Err() != nil {
				return
			}
			if err != nil {
				log.Printf("public tracker list: %v; retaining cached/fallback trackers", err)
			}
			if changed {
				onRefresh()
			}
			p.mu.Lock()
			delay := time.Until(p.cache.NextAttempt)
			p.mu.Unlock()
			timer := time.NewTimer(max(delay, time.Minute))
			select {
			case <-ctx.Done():
				timer.Stop()
				return
			case <-timer.C:
			}
		}
	}()
}

func (p *trackerPool) close() {
	if p == nil || p.cancel == nil {
		return
	}
	p.cancel()
	<-p.done
	p.client.CloseIdleConnections()
}

// Only the worker calls refresh. Snapshot readers never wait for network I/O.
func (p *trackerPool) refresh(ctx context.Context, now time.Time) (bool, error) {
	p.mu.Lock()
	c := p.cache
	p.mu.Unlock()
	if now.Before(c.NextAttempt) {
		return false, nil
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, p.source, nil)
	if err != nil {
		return false, err
	}
	req.Header.Set("User-Agent", "TorrentPlayer/1.4.4")
	if c.ETag != "" {
		req.Header.Set("If-None-Match", c.ETag)
	}
	if c.LastModified != "" {
		req.Header.Set("If-Modified-Since", c.LastModified)
	}
	res, err := p.client.Do(req)
	changed := false
	if err == nil {
		defer res.Body.Close()
		switch res.StatusCode {
		case http.StatusNotModified:
			if len(c.Trackers) == 0 {
				err = fmt.Errorf("304 without cached trackers")
			}
		case http.StatusOK:
			var body []byte
			body, err = io.ReadAll(io.LimitReader(res.Body, trackerResponseLimit+1))
			if err == nil && len(body) > trackerResponseLimit {
				err = fmt.Errorf("tracker list exceeds size limit")
			}
			if err == nil {
				trackers := parseTrackerList(string(body))
				if len(trackers) == 0 {
					err = fmt.Errorf("tracker list contains no valid public trackers")
				} else {
					changed = strings.Join(c.Trackers, "\n") != strings.Join(trackers, "\n")
					c.Trackers = trackers
					c.ETag, c.LastModified = res.Header.Get("ETag"), res.Header.Get("Last-Modified")
				}
			}
		default:
			err = fmt.Errorf("tracker list HTTP status %d", res.StatusCode)
		}
	}
	if ctx.Err() != nil {
		return false, ctx.Err()
	}
	c.Version = 1
	if err != nil {
		c.Failures = min(c.Failures+1, 7)
		c.NextAttempt = now.Add(min(15*time.Minute*time.Duration(1<<uint(c.Failures-1)), trackerRefreshInterval))
	} else {
		c.Failures = 0
		c.CheckedAt, c.NextAttempt = now, now.Add(trackerRefreshInterval)
	}
	p.mu.Lock()
	p.cache = c
	p.mu.Unlock()
	if saveErr := p.save(c); saveErr != nil {
		log.Printf("public tracker cache could not be saved: %v", saveErr)
	}
	return changed, err
}

func (p *trackerPool) save(c trackerCache) error {
	data, err := json.Marshal(c)
	if err != nil {
		return err
	}
	f, err := os.CreateTemp(filepath.Dir(p.path), ".public-trackers-*")
	if err != nil {
		return err
	}
	defer os.Remove(f.Name())
	if _, err = f.Write(data); err != nil {
		f.Close()
		return err
	}
	if err = f.Sync(); err != nil {
		f.Close()
		return err
	}
	if err = f.Close(); err != nil {
		return err
	}
	return os.Rename(f.Name(), p.path)
}

func publicTrackerURL(raw string) string {
	if len(raw) > 2048 {
		return ""
	}
	u, err := url.Parse(strings.TrimSpace(raw))
	if err != nil || u.Opaque != "" || u.User != nil || u.Fragment != "" {
		return ""
	}
	u.Scheme = strings.ToLower(u.Scheme)
	switch u.Scheme {
	case "udp", "http", "https", "ws", "wss":
	default:
		return ""
	}
	host := strings.ToLower(strings.TrimSuffix(u.Hostname(), "."))
	if host == "" || strings.ContainsAny(host, "% \\\t\r\n") {
		return ""
	}
	if ip := net.ParseIP(host); ip != nil {
		if !ip.IsGlobalUnicast() || ip.IsPrivate() || ip.IsLoopback() || ip.IsLinkLocalUnicast() {
			return ""
		}
	} else {
		if !strings.Contains(host, ".") {
			return ""
		}
		for _, suffix := range []string{".localhost", ".local", ".internal", ".invalid", ".test", ".onion", ".i2p"} {
			if strings.HasSuffix(host, suffix) {
				return ""
			}
		}
		for _, label := range strings.Split(host, ".") {
			if len(label) == 0 || len(label) > 63 || label[0] == '-' || label[len(label)-1] == '-' {
				return ""
			}
			for _, ch := range label {
				if !(ch >= 'a' && ch <= 'z' || ch >= '0' && ch <= '9' || ch == '-') {
					return ""
				}
			}
		}
	}
	port := u.Port()
	if port != "" {
		n, err := strconv.Atoi(port)
		if err != nil || n < 1 || n > 65535 {
			return ""
		}
		port = strconv.Itoa(n)
	} else if u.Scheme == "udp" {
		return ""
	}
	if ((u.Scheme == "https" || u.Scheme == "wss") && port == "443") || ((u.Scheme == "http" || u.Scheme == "ws") && port == "80") {
		port = ""
	}
	u.Host = host
	if strings.Contains(host, ":") {
		u.Host = "[" + host + "]"
	}
	if port != "" {
		u.Host = net.JoinHostPort(host, port)
	}
	return u.String()
}

func parseTrackerList(body string) []string {
	var result []string
	seen := make(map[string]bool)
	scanner := bufio.NewScanner(strings.NewReader(body))
	for scanner.Scan() {
		tr := publicTrackerURL(scanner.Text())
		if tr != "" && !seen[tr] {
			seen[tr] = true
			result = append(result, tr)
			if len(result) == trackerListLimit {
				break
			}
		}
	}
	return result
}

func (s *EngineServer) supplementTrackers(rec *TorrentRecord) {
	rec.mu.Lock()
	defer rec.mu.Unlock()
	if s.shuttingDown.Load() {
		return
	}
	info := rec.Torrent.Info()
	if info == nil || info.Private != nil && *info.Private {
		return
	}
	select {
	case <-rec.Torrent.Closed():
		return
	default:
	}
	seen := make(map[string]bool)
	mi := rec.Torrent.Metainfo()
	for _, tier := range mi.UpvertedAnnounceList() {
		for _, tr := range tier {
			seen[tr] = true
			if normalized := publicTrackerURL(tr); normalized != "" {
				seen[normalized] = true
			}
		}
	}
	var added [][]string
	for _, tr := range s.trackers.snapshot() {
		if rec.SupplementalTrackers >= trackerSupplementLimit {
			break
		}
		if !seen[tr] {
			added = append(added, []string{tr})
			seen[tr] = true
			rec.SupplementalTrackers++
		}
	}
	if len(added) > 0 {
		// AddTrackers merges by tier index. Prefix empty tiers so original
		// tracker ordering and tiers are retained exactly.
		rec.Torrent.AddTrackers(append(make([][]string, len(mi.UpvertedAnnounceList())), added...))
	}
}

func (s *EngineServer) refreshTorrentTrackers() {
	s.mu.RLock()
	records := make([]*TorrentRecord, 0, len(s.records))
	for _, rec := range s.records {
		records = append(records, rec)
	}
	s.mu.RUnlock()
	for _, rec := range records {
		s.supplementTrackers(rec)
	}
}
