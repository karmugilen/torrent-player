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
	transport := &http.Transport{
		Proxy:             http.ProxyFromEnvironment,
		DialContext:       (&net.Dialer{Timeout: 2 * time.Second, KeepAlive: 30 * time.Second}).DialContext,
		ForceAttemptHTTP2: true, MaxIdleConns: 16, MaxIdleConnsPerHost: 3,
		MaxConnsPerHost: 3, IdleConnTimeout: 60 * time.Second,
		TLSHandshakeTimeout: 2 * time.Second, ResponseHeaderTimeout: 3 * time.Second,
		MaxResponseHeaderBytes: 32 << 10,
	}
	c := &metadataCaches{client: &http.Client{Transport: transport, Timeout: 4 * time.Second}}
	for _, template := range templates {
		c.sources = append(c.sources, &cacheSource{template: template, slots: make(chan struct{}, 3)})
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
	s.failures = min(s.failures+1, 5)
	s.next = time.Now().Add(time.Duration(1<<s.failures) * 15 * time.Second)
}

func (c *metadataCaches) fetch(parent context.Context, hash metainfo.Hash) *metainfo.Info {
	ctx, cancel := context.WithTimeout(parent, 4*time.Second)
	results := make(chan *metainfo.Info, len(c.sources))
	var work sync.WaitGroup
	defer func() { cancel(); work.Wait() }()
	for _, source := range c.sources {
		if !source.available() {
			continue
		}
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
	req.Header.Set("User-Agent", "TorrentAtlas/1.0 (metadata-only)")
	req.Header.Set("Accept", "application/x-bittorrent, application/octet-stream")
	resp, err := c.client.Do(req)
	if err != nil {
		// Losing a race is not a source outage.
		if ctx.Err() == nil || errors.Is(ctx.Err(), context.DeadlineExceeded) {
			source.outcome(false)
		}
		return nil
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		source.outcome(resp.StatusCode >= 400 && resp.StatusCode < 500 && resp.StatusCode != 408 && resp.StatusCode != 429)
		_, _ = io.Copy(io.Discard, io.LimitReader(resp.Body, 4096))
		return nil
	}
	if resp.ContentLength > maxMetadataBytes {
		source.outcome(false)
		return nil
	}
	blob, err := io.ReadAll(io.LimitReader(resp.Body, maxMetadataBytes+1))
	if err != nil {
		if ctx.Err() == nil || errors.Is(ctx.Err(), context.DeadlineExceeded) {
			source.outcome(false)
		}
		return nil
	}
	info, err := parseCacheMetadata(blob, hash)
	// A challenge page or bogus metadata is not a usable cache source.
	source.outcome(err == nil)
	return info
}
