package main

import (
	"context"
	"crypto/rand"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/anacrolix/dht/v2/krpc"
	"github.com/anacrolix/log"
	"github.com/anacrolix/torrent/bencode"
	"github.com/anacrolix/torrent/metainfo"
	"github.com/anacrolix/torrent/tracker"
)

type trackerHint struct {
	urls        []string
	at          time.Time
	metadataURL string
}
type trackerEndpoint struct {
	client tracker.Client
	next   map[string]time.Time
}

// Tracker associations come from public feed magnets, never a blind broadcast
// of sampled hashes. URLs and peer addresses remain in bounded, transient RAM.
type trackerResolver struct {
	mu                       sync.Mutex
	hints                    map[string]trackerHint
	endpoints                map[string]*trackerEndpoint
	slots                    chan struct{}
	peerID                   [20]byte
	http                     *http.Client
	requests, hits, failures atomic.Int64
	metadataMeter            sourceMeter
	announce                 func(context.Context, string, metainfo.Hash, int) ([]krpc.NodeAddr, time.Duration, error)
}

func newTrackerResolver() *trackerResolver {
	t := &trackerResolver{hints: map[string]trackerHint{}, endpoints: map[string]*trackerEndpoint{}, slots: make(chan struct{}, 16)}
	_, _ = rand.Read(t.peerID[:])
	copy(t.peerID[:], "-TA0101-")
	t.http = &http.Client{Timeout: 5 * time.Second, Transport: &http.Transport{DialContext: publicTrackerDial, MaxIdleConns: 32, MaxIdleConnsPerHost: 4, IdleConnTimeout: time.Minute, MaxResponseHeaderBytes: 32 << 10}, CheckRedirect: func(_ *http.Request, _ []*http.Request) error { return http.ErrUseLastResponse }}
	t.announce = t.announceRemote
	return t
}

func trackerURL(value string) string {
	u, err := url.Parse(strings.TrimSpace(value))
	if err != nil || u.User != nil || u.Hostname() == "" || len(value) > 2048 {
		return ""
	}
	if u.Scheme != "udp" && u.Scheme != "http" && u.Scheme != "https" {
		return ""
	}
	if ip := net.ParseIP(u.Hostname()); ip != nil && !publicIP(ip) {
		return ""
	}
	if u.Hostname() == "localhost" || strings.HasSuffix(u.Hostname(), ".local") {
		return ""
	}
	if u.Scheme == "udp" && u.Port() == "" {
		return ""
	}
	u.Fragment = ""
	return u.String()
}

func publicTrackerAddress(ctx context.Context, address string) (string, error) {
	host, port, err := net.SplitHostPort(address)
	if err != nil {
		return "", err
	}
	ips, err := net.DefaultResolver.LookupIPAddr(ctx, host)
	if err != nil {
		return "", err
	}
	for _, ip := range ips {
		if publicIP(ip.IP) {
			return net.JoinHostPort(ip.IP.String(), port), nil
		}
	}
	return "", errors.New("tracker has no public IPv4 address")
}

func publicTrackerDial(ctx context.Context, network, address string) (net.Conn, error) {
	address, err := publicTrackerAddress(ctx, address)
	if err != nil {
		return nil, err
	}
	return (&net.Dialer{Timeout: 3 * time.Second, KeepAlive: time.Minute}).DialContext(ctx, "tcp4", address)
}

func (t *trackerResolver) remember(hash string, urls []string, now time.Time) {
	clean := make([]string, 0, 8)
	seen := map[string]bool{}
	for _, raw := range urls {
		if value := trackerURL(raw); value != "" && !seen[value] {
			clean = append(clean, value)
			seen[value] = true
		}
		if len(clean) == 8 {
			break
		}
	}
	if len(clean) == 0 {
		return
	}
	t.mu.Lock()
	defer t.mu.Unlock()
	if len(t.hints) >= 4096 {
		for key, hint := range t.hints {
			if now.Sub(hint.at) >= 24*time.Hour {
				delete(t.hints, key)
			}
		}
		if len(t.hints) >= 4096 {
			for key := range t.hints {
				delete(t.hints, key)
				break
			}
		}
	}
	hint := t.hints[hash]
	hint.urls, hint.at = clean, now
	t.hints[hash] = hint
}

func (t *trackerResolver) rememberMetadata(hash, raw string, now time.Time) {
	u, err := url.Parse(raw)
	if err != nil || u.Scheme != "https" || trackerURL(raw) == "" {
		return
	}
	t.mu.Lock()
	defer t.mu.Unlock()
	if len(t.hints) >= 4096 {
		if _, exists := t.hints[hash]; !exists {
			return
		}
	}
	hint := t.hints[hash]
	hint.metadataURL, hint.at = u.String(), now
	t.hints[hash] = hint
}

func (t *trackerResolver) fetchMetadata(ctx context.Context, hash metainfo.Hash) *metainfo.Info {
	t.mu.Lock()
	hint := t.hints[hash.HexString()]
	t.mu.Unlock()
	if hint.metadataURL == "" || time.Since(hint.at) >= 24*time.Hour {
		return nil
	}
	start, outcome := time.Now(), "failure"
	defer func() { t.metadataMeter.record(start, outcome) }()
	req, err := http.NewRequestWithContext(ctx, "GET", hint.metadataURL, nil)
	if err != nil {
		return nil
	}
	req.Header.Set("User-Agent", "TorrentAtlas/1.1 (metadata-only)")
	response, err := t.http.Do(req)
	if err != nil {
		if errors.Is(ctx.Err(), context.Canceled) {
			outcome = "cancelled"
		}
		return nil
	}
	defer response.Body.Close()
	if response.StatusCode == 404 || response.StatusCode == 410 {
		outcome = "miss"
		return nil
	}
	if response.StatusCode != 200 || response.ContentLength > maxMetadataBytes {
		return nil
	}
	blob, err := io.ReadAll(io.LimitReader(response.Body, maxMetadataBytes+1))
	if err != nil {
		return nil
	}
	info, err := parseCacheMetadata(blob, hash)
	if err != nil {
		return nil
	}
	outcome = "hit"
	return info
}

func (t *trackerResolver) lookup(ctx context.Context, hash metainfo.Hash, port int) []krpc.NodeAddr {
	if t == nil {
		return nil
	}
	now := time.Now()
	key := hash.HexString()
	t.mu.Lock()
	hint, ok := t.hints[key]
	var urls []string
	if ok && now.Sub(hint.at) < 24*time.Hour {
		for _, u := range hint.urls {
			ep := t.endpoints[u]
			if ep == nil {
				if len(t.endpoints) >= 64 {
					continue
				}
				ep = &trackerEndpoint{next: map[string]time.Time{}}
				t.endpoints[u] = ep
			}
			if now.Before(ep.next[key]) {
				continue
			}
			if len(ep.next) >= 8192 {
				for h, deadline := range ep.next {
					if !now.Before(deadline) {
						delete(ep.next, h)
					}
				}
				if len(ep.next) >= 8192 {
					continue
				}
			}
			// Claim before unlocking so overlapping requests cannot reannounce.
			ep.next[key] = now.Add(time.Minute)
			urls = append(urls, u)
			if len(urls) == 2 {
				break
			}
		}
	}
	t.mu.Unlock()
	var work sync.WaitGroup
	results := make(chan []krpc.NodeAddr, len(urls))
	for _, u := range urls {
		work.Add(1)
		go func(u string) {
			defer work.Done()
			select {
			case t.slots <- struct{}{}:
			case <-ctx.Done():
				return
			}
			defer func() { <-t.slots }()
			call, cancel := context.WithTimeout(ctx, 5*time.Second)
			defer cancel()
			t.requests.Add(1)
			peers, interval, err := t.announce(call, u, hash, port)
			if err != nil {
				if ctx.Err() == nil {
					t.failures.Add(1)
				}
				return
			}
			if interval < time.Minute {
				interval = time.Minute
			}
			if interval > 24*time.Hour {
				interval = 24 * time.Hour
			}
			t.mu.Lock()
			t.endpoints[u].next[key] = time.Now().Add(interval)
			t.mu.Unlock()
			valid := make([]krpc.NodeAddr, 0, 32)
			for _, peer := range peers {
				if publicIP(peer.IP) && peer.Port > 0 && peer.Port <= 65535 {
					valid = append(valid, peer)
				}
				if len(valid) == 32 {
					break
				}
			}
			if len(valid) > 0 {
				t.hits.Add(1)
			}
			results <- valid
		}(u)
	}
	work.Wait()
	close(results)
	var peers []krpc.NodeAddr
	seen := map[string]bool{}
	for batch := range results {
		for _, p := range batch {
			if !seen[p.String()] {
				seen[p.String()] = true
				peers = append(peers, p)
			}
		}
	}
	return peers
}

func (t *trackerResolver) announceRemote(ctx context.Context, raw string, hash metainfo.Hash, port int) ([]krpc.NodeAddr, time.Duration, error) {
	u, _ := url.Parse(raw)
	if u.Scheme != "udp" {
		return t.announceHTTP(ctx, u, hash, port)
	}
	t.mu.Lock()
	client := t.endpoints[raw].client
	t.mu.Unlock()
	if client == nil {
		address, err := publicTrackerAddress(ctx, u.Host)
		if err != nil {
			return nil, 0, err
		}
		u.Host = address
		created, err := tracker.NewClient(u.String(), tracker.NewClientOpts{UdpNetwork: "udp4", Logger: log.Default})
		if err != nil {
			return nil, 0, err
		}
		t.mu.Lock()
		if t.endpoints[raw].client == nil {
			t.endpoints[raw].client = created
			client = created
		} else {
			client = t.endpoints[raw].client
			_ = created.Close()
		}
		t.mu.Unlock()
	}
	response, err := client.Announce(ctx, tracker.AnnounceRequest{InfoHash: hash, PeerId: t.peerID, Port: uint16(port), Left: 1, NumWant: 32}, tracker.AnnounceOpt{UserAgent: "TorrentAtlas/1.1"})
	if err != nil {
		return nil, 0, err
	}
	peers := make([]krpc.NodeAddr, 0, len(response.Peers))
	for _, p := range response.Peers {
		peers = append(peers, krpc.NodeAddr{IP: p.IP, Port: p.Port})
	}
	return peers, time.Duration(response.Interval) * time.Second, nil
}

func (t *trackerResolver) announceHTTP(ctx context.Context, u *url.URL, hash metainfo.Hash, port int) ([]krpc.NodeAddr, time.Duration, error) {
	q := u.Query()
	q.Set("info_hash", string(hash[:]))
	q.Set("peer_id", string(t.peerID[:]))
	q.Set("port", strconv.Itoa(port))
	q.Set("uploaded", "0")
	q.Set("downloaded", "0")
	q.Set("left", "1")
	q.Set("compact", "1")
	q.Set("numwant", "32")
	u.RawQuery = q.Encode()
	req, err := http.NewRequestWithContext(ctx, "GET", u.String(), nil)
	if err != nil {
		return nil, 0, err
	}
	req.Header.Set("User-Agent", "TorrentAtlas/1.1")
	response, err := t.http.Do(req)
	if err != nil {
		return nil, 0, err
	}
	defer response.Body.Close()
	if response.StatusCode != 200 {
		return nil, 0, fmt.Errorf("tracker status %d", response.StatusCode)
	}
	b, err := io.ReadAll(io.LimitReader(response.Body, (1<<20)+1))
	if err != nil {
		return nil, 0, err
	}
	if len(b) > 1<<20 {
		return nil, 0, errors.New("tracker response too large")
	}
	var decoded struct {
		Interval int64  `bencode:"interval"`
		Failure  string `bencode:"failure reason"`
		Peers    string `bencode:"peers"`
	}
	if err := bencode.Unmarshal(b, &decoded); err != nil {
		return nil, 0, err
	}
	if decoded.Failure != "" {
		return nil, 0, errors.New("tracker rejected announce")
	}
	var compact krpc.CompactIPv4NodeAddrs
	if err := compact.UnmarshalBinary([]byte(decoded.Peers)); err != nil {
		return nil, 0, err
	}
	return []krpc.NodeAddr(compact), time.Duration(min(max(decoded.Interval, 60), 86400)) * time.Second, nil
}

func (t *trackerResolver) close() {
	if t == nil {
		return
	}
	t.http.CloseIdleConnections()
	t.mu.Lock()
	defer t.mu.Unlock()
	for _, ep := range t.endpoints {
		if ep.client != nil {
			_ = ep.client.Close()
		}
	}
}
