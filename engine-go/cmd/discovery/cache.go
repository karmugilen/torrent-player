package main

import (
	"bytes"
	"context"
	"errors"
	"io"
	"math"
	"net"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/anacrolix/torrent/bencode"
	"github.com/anacrolix/torrent/metainfo"
)

const maxMetadataBytes = 8 << 20
const cacheSourceSlots = 48
const cacheFetchTimeout = 2500 * time.Millisecond

// Inspired by the sibling seen-in-the-swarm resolver: race bounded sources,
// verify the exact info bytes, and distinguish cache misses from outages.
var defaultMetadataCaches = []string{
	"https://itorrents.net/torrent/{hash}.torrent",
	"https://itorrents.org/torrent/{hash}.torrent",
	"https://btcache.me/torrent/{hash}.torrent",
}

type cacheSource struct {
	template string
	slots    chan struct{}
	mu       sync.Mutex
	next     time.Time
	failures int
	misses   metadataMissCache
	meter    sourceMeter
}
type metadataCaches struct {
	client     *http.Client
	sources    []*cacheSource
	jobs       chan struct{}
	requests   atomic.Int64
	missReuses atomic.Int64
}

func newMetadataCaches(templates []string) *metadataCaches {
	// Prefer IPv4: some mirrors reset IPv6 or bounce through unrelated hosts.
	dialer := &net.Dialer{Timeout: 2 * time.Second, KeepAlive: 30 * time.Second}
	transport := &http.Transport{
		Proxy: http.ProxyFromEnvironment,
		DialContext: func(ctx context.Context, network, address string) (net.Conn, error) {
			return dialer.DialContext(ctx, "tcp4", address)
		},
		ForceAttemptHTTP2: true, MaxIdleConns: 256, MaxIdleConnsPerHost: 64,
		MaxConnsPerHost: 64, IdleConnTimeout: 60 * time.Second,
		TLSHandshakeTimeout: 2 * time.Second, ResponseHeaderTimeout: 2500 * time.Millisecond,
		MaxResponseHeaderBytes: 32 << 10,
	}
	c := &metadataCaches{client: &http.Client{Transport: transport, Timeout: 3 * time.Second}, jobs: make(chan struct{}, cacheSourceSlots)}
	for _, template := range templates {
		c.sources = append(c.sources, &cacheSource{template: template, slots: make(chan struct{}, cacheSourceSlots)})
	}
	return c
}
func (c *metadataCaches) close() { c.client.CloseIdleConnections() }

func (c *metadataCaches) sourceStats() []sourceState {
	stats := make([]sourceState, 0, len(c.sources))
	for _, source := range c.sources {
		u, _ := url.Parse(source.template)
		name := source.template
		if u != nil && u.Hostname() != "" {
			name = u.Hostname()
		}
		stats = append(stats, source.meter.snapshot(name))
	}
	return stats
}

func (c *metadataCaches) available() bool {
	if len(c.jobs) >= cap(c.jobs) {
		return false
	}
	for _, source := range c.sources {
		if source.available() {
			return true
		}
	}
	return false
}

func (c *metadataCaches) knownMissing(hash metainfo.Hash) bool {
	if len(c.sources) == 0 {
		return false
	}
	for _, source := range c.sources {
		if !source.misses.has(hash, time.Now()) {
			return false
		}
	}
	return true
}

func parseCacheMetadata(blob []byte, expected metainfo.Hash) (*metainfo.Info, error) {
	if len(blob) > maxMetadataBytes {
		return nil, errors.New("oversized metadata")
	}
	var mi metainfo.MetaInfo
	decoder := bencode.NewDecoder(bytes.NewReader(blob))
	decoder.MaxStrLen = maxMetadataBytes
	if err := decoder.Decode(&mi); err != nil {
		return nil, err
	}
	if err := decoder.ReadEOF(); err != nil {
		return nil, err
	}
	if len(mi.InfoBytes) == 0 || mi.HashInfoBytes() != expected {
		return nil, errors.New("metadata hash mismatch")
	}
	var info metainfo.Info
	decoder = bencode.NewDecoder(bytes.NewReader(mi.InfoBytes))
	decoder.MaxStrLen = maxMetadataBytes
	if err := decoder.Decode(&info); err != nil {
		return nil, err
	}
	if (info.Private != nil && *info.Private) || strings.TrimSpace(info.BestName()) == "" {
		return nil, errors.New("private or unnamed torrent")
	}
	// Check lengths before TotalLength so malformed metadata cannot overflow it.
	var total int64
	for file := range info.UpvertedFilesIter() {
		if file.Length < 0 || file.Length > math.MaxInt64-total {
			return nil, errors.New("invalid file size")
		}
		total += file.Length
	}
	return &info, nil
}

func (s *cacheSource) available() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return !time.Now().Before(s.next)
}
func (s *cacheSource) outcome(healthy bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if healthy {
		s.failures = 0
		s.next = time.Time{}
		return
	}
	// Keep outage windows short. Long exponential backs-offs previously
	// disabled every mirror under parallel load and stalled name throughput.
	s.failures = min(s.failures+1, 3)
	s.next = time.Now().Add(time.Duration(1<<s.failures) * time.Second)
}

func (c *metadataCaches) fetch(parent context.Context, hash metainfo.Hash) *metainfo.Info {
	return c.fetchWithin(parent, hash, cacheFetchTimeout)
}

func (c *metadataCaches) fetchWithin(parent context.Context, hash metainfo.Hash, timeout time.Duration) *metainfo.Info {
	info, _ := c.fetchAttempt(parent, hash, timeout)
	return info
}

// attempted distinguishes a real network lookup from all mirrors cooling down
// or the entire deadline spent waiting for a source's concurrency slot.
func (c *metadataCaches) fetchAttempt(parent context.Context, hash metainfo.Hash, timeout time.Duration) (info *metainfo.Info, attempted bool) {
	// Admit an entire hash before starting its network deadline. The shared
	// hash limit matches each source's capacity, so races cannot queue behind
	// unrelated hashes and spend the entire deadline waiting for a slot.
	select {
	case c.jobs <- struct{}{}:
		defer func() { <-c.jobs }()
	case <-parent.Done():
		return nil, false
	}
	ctx, cancel := context.WithTimeout(parent, timeout)
	results := make(chan *metainfo.Info, len(c.sources))
	var work sync.WaitGroup
	var calls atomic.Int32
	defer func() { cancel(); work.Wait(); attempted = calls.Load() > 0 }()
	started := 0
	for _, source := range c.sources {
		if !source.available() {
			continue
		}
		started++
		work.Add(1)
		go func(s *cacheSource) {
			defer work.Done()
			select {
			case s.slots <- struct{}{}:
			case <-ctx.Done():
				return
			}
			defer func() { <-s.slots }()
			if !s.available() {
				return
			}
			info := c.fetchSource(ctx, s, hash, &calls)
			if info != nil {
				select {
				case results <- info:
				case <-ctx.Done():
				}
			}
		}(source)
	}
	if started == 0 {
		return nil, false
	}
	done := make(chan struct{})
	go func() { work.Wait(); close(done) }()
	select {
	case info := <-results:
		return info, false
	case <-done:
		select {
		case info := <-results:
			return info, false
		default:
			return nil, false
		}
	case <-ctx.Done():
		return nil, false
	}
}

func (c *metadataCaches) fetchSource(ctx context.Context, source *cacheSource, hash metainfo.Hash, calls *atomic.Int32) *metainfo.Info {
	if source.misses.has(hash, time.Now()) {
		c.missReuses.Add(1)
		return nil
	}
	u := strings.ReplaceAll(source.template, "{hash}", strings.ToUpper(hash.HexString()))
	u = strings.ReplaceAll(u, "{hash_lower}", hash.HexString())
	req, err := http.NewRequestWithContext(ctx, "GET", u, nil)
	if err != nil {
		source.outcome(false)
		return nil
	}
	// Browser-like headers match the sibling swarm resolver; some mirrors
	// return landing pages for uncommon client identifiers.
	req.Header.Set("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
	req.Header.Set("Accept", "application/x-bittorrent, application/octet-stream, */*")
	req.Header.Set("Accept-Encoding", "identity")
	calls.Add(1)
	c.requests.Add(1)
	started, outcome := time.Now(), "failure"
	defer func() { source.meter.record(started, outcome) }()
	resp, err := c.client.Do(req)
	if err != nil {
		if errors.Is(ctx.Err(), context.Canceled) {
			outcome = "cancelled"
			return nil
		}
		source.outcome(false)
		return nil
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		if resp.StatusCode == http.StatusNotFound || resp.StatusCode == http.StatusGone {
			source.misses.put(hash, time.Now())
			outcome = "miss"
		}
		// Rate limits and gateway failures back off briefly; other statuses are misses.
		if resp.StatusCode == http.StatusForbidden || resp.StatusCode == http.StatusTooManyRequests || resp.StatusCode == http.StatusRequestTimeout || resp.StatusCode >= 500 {
			source.outcome(false)
		} else {
			source.outcome(true)
		}
		_, _ = io.Copy(io.Discard, io.LimitReader(resp.Body, 4096))
		return nil
	}
	if resp.ContentLength > maxMetadataBytes {
		source.outcome(false)
		return nil
	}
	blob, err := io.ReadAll(io.LimitReader(resp.Body, maxMetadataBytes+1))
	if err != nil {
		if errors.Is(ctx.Err(), context.Canceled) {
			outcome = "cancelled"
			return nil
		}
		source.outcome(false)
		return nil
	}
	info, err := parseCacheMetadata(blob, hash)
	if err != nil {
		// HTML challenges and malformed/hash-mismatched metadata are unusable
		// responses, not confirmed per-hash absence. Probe again after cooldown.
		source.outcome(false)
		return nil
	}
	outcome = "hit"
	source.outcome(true)
	return info
}
