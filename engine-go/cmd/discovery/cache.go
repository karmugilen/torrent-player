package main

import (
	"bytes"
	"context"
	"errors"
	"io"
	"math"
	"net"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/anacrolix/torrent/bencode"
	"github.com/anacrolix/torrent/metainfo"
)

const maxMetadataBytes = 8 << 20
const cacheSourceSlots = 12
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
}
type metadataCaches struct {
	client  *http.Client
	sources []*cacheSource
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
	c := &metadataCaches{client: &http.Client{Transport: transport, Timeout: 3 * time.Second}}
	for _, template := range templates {
		c.sources = append(c.sources, &cacheSource{template: template, slots: make(chan struct{}, cacheSourceSlots)})
	}
	return c
}
func (c *metadataCaches) close() { c.client.CloseIdleConnections() }

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
	ctx, cancel := context.WithTimeout(parent, timeout)
	results := make(chan *metainfo.Info, len(c.sources))
	var work sync.WaitGroup
	defer func() { cancel(); work.Wait() }()
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
			info := c.fetchSource(ctx, s, hash)
			if info != nil {
				select {
				case results <- info:
				case <-ctx.Done():
				}
			}
		}(source)
	}
	if started == 0 {
		return nil
	}
	done := make(chan struct{})
	go func() { work.Wait(); close(done) }()
	select {
	case info := <-results:
		return info
	case <-done:
		select {
		case info := <-results:
			return info
		default:
			return nil
		}
	case <-ctx.Done():
		return nil
	}
}

func (c *metadataCaches) fetchSource(ctx context.Context, source *cacheSource, hash metainfo.Hash) *metainfo.Info {
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
	resp, err := c.client.Do(req)
	if err != nil {
		// Deadlines, cancels and ordinary dial noise stay healthy so a burst of
		// parallel workers cannot disable the only working mirrors.
		source.outcome(true)
		return nil
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		// Rate limits and gateway failures back off briefly; other statuses are misses.
		if resp.StatusCode == http.StatusTooManyRequests || resp.StatusCode == http.StatusRequestTimeout || resp.StatusCode >= 500 {
			source.outcome(false)
		} else {
			source.outcome(true)
		}
		_, _ = io.Copy(io.Discard, io.LimitReader(resp.Body, 4096))
		return nil
	}
	if resp.ContentLength > maxMetadataBytes {
		source.outcome(true)
		return nil
	}
	blob, err := io.ReadAll(io.LimitReader(resp.Body, maxMetadataBytes+1))
	if err != nil {
		source.outcome(true)
		return nil
	}
	info, err := parseCacheMetadata(blob, hash)
	// Challenge pages and unknown hashes are healthy misses.
	source.outcome(true)
	if err != nil {
		return nil
	}
	return info
}
