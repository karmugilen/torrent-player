package main

import (
	"context"
	"encoding/json"
	"net"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"time"
)

// Public collaborative torrent catalog (torrents-csv) used as the fast name
// source. DHT sampling still discovers live hashes and geography; this fills
// names at a rate mirrors and peers alone cannot.
const catalogEndpoint = "https://torrents-csv.com/service/search"
// Public API caps pages around 25 rows.
const catalogPageSize = 25
// Catalog HTTP fills names the local dump has not seeded yet.
const catalogWorkers = 16
const catalogTargetPerMin = 1500

var catalogQueries = []string{
	"1080p", "2160p", "720p", "480p", "bluray", "web-dl", "webrip", "hdtv",
	"x264", "x265", "hevc", "remux", "mkv", "mp4", "2020", "2021", "2022",
	"2023", "2024", "2025", "2026", "season", "complete", "documentary",
	"concert", "anime", "french", "german", "spanish", "hindi", "tamil",
	"korean", "japanese", "russian", "italian", "portuguese", "dutch",
	"0", "1", "2", "3", "4", "5", "6", "7", "8", "9",
	"a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m",
	"n", "o", "p", "q", "r", "s", "t", "u", "v", "w", "x", "y", "z",
}

type catalogTorrent struct {
	InfoHash  string `json:"infohash"`
	Name      string `json:"name"`
	SizeBytes int64  `json:"size_bytes"`
	Seeders   int    `json:"seeders"`
	Leechers  int    `json:"leechers"`
}

type catalogPage struct {
	Torrents []catalogTorrent `json:"torrents"`
	Next     *int             `json:"next"`
}

func newCatalogClient() *http.Client {
	dialer := &net.Dialer{Timeout: 3 * time.Second, KeepAlive: 30 * time.Second}
	return &http.Client{
		Timeout: 8 * time.Second,
		Transport: &http.Transport{
			Proxy: http.ProxyFromEnvironment,
			DialContext: func(ctx context.Context, network, address string) (net.Conn, error) {
				return dialer.DialContext(ctx, "tcp4", address)
			},
			// Prefer HTTP/1.1: some torrents-csv edges stall on forced HTTP/2.
			ForceAttemptHTTP2: false, MaxIdleConns: 64, MaxIdleConnsPerHost: 16,
			MaxConnsPerHost: 16, IdleConnTimeout: 60 * time.Second,
			TLSHandshakeTimeout: 4 * time.Second, ResponseHeaderTimeout: 6 * time.Second,
		},
	}
}

func (d *dashboard) ingestCatalog(ctx context.Context) {
	client := newCatalogClient()
	defer client.CloseIdleConnections()
	// target ≈ workers * (minute/interval) * pageSize when pages are all new.
	interval := time.Duration(float64(time.Minute) * float64(catalogWorkers) * float64(catalogPageSize) / float64(catalogTargetPerMin))
	if interval < 200*time.Millisecond {
		interval = 200 * time.Millisecond
	}
	var work sync.WaitGroup
	for i := 0; i < catalogWorkers; i++ {
		work.Add(1)
		go func(worker int) {
			defer work.Done()
			throttle := time.NewTicker(interval)
			defer throttle.Stop()
			query := worker % len(catalogQueries)
			after := 0
			for {
				select {
				case <-ctx.Done():
					return
				case <-throttle.C:
					if !d.catalogShouldIngest() {
						continue
					}
					next, _ := d.fetchCatalogPage(ctx, client, catalogQueries[query], after)
					if next != nil {
						after = *next
						continue
					}
					query = (query + catalogWorkers) % len(catalogQueries)
					after = 0
				}
			}
		}(i)
	}
	<-ctx.Done()
	work.Wait()
}

// catalogShouldIngest always continues: the store has no count cap and the
// paced workers already bound request rate.
func (d *dashboard) catalogShouldIngest() bool { return true }

func (d *dashboard) fetchCatalogPage(ctx context.Context, client *http.Client, query string, after int) (*int, int) {
	started := time.Now()
	if rustDumpEnabled() {
		if page, err := rustFetchCatalogPage(query, catalogPageSize, after); err == nil {
			added := d.applyCatalogTorrents(page.Torrents, time.Since(started))
			return page.Next, added
		}
	}
	u, err := url.Parse(catalogEndpoint)
	if err != nil {
		return nil, 0
	}
	q := u.Query()
	q.Set("q", query)
	q.Set("size", strconv.Itoa(catalogPageSize))
	if after > 0 {
		q.Set("after", strconv.Itoa(after))
	}
	u.RawQuery = q.Encode()
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, u.String(), nil)
	if err != nil {
		return nil, 0
	}
	req.Header.Set("User-Agent", "TorrentAtlas/1.0 (metadata-only; local discovery)")
	req.Header.Set("Accept", "application/json")
	resp, err := client.Do(req)
	if err != nil {
		return nil, 0
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil, 0
	}
	var page catalogPage
	if json.NewDecoder(resp.Body).Decode(&page) != nil {
		return nil, 0
	}
	added := d.applyCatalogTorrents(page.Torrents, time.Since(started))
	return page.Next, added
}

func (d *dashboard) applyCatalogTorrents(items []catalogTorrent, latency time.Duration) int {
	if len(items) == 0 {
		return 0
	}
	now := time.Now().UTC()
	added := 0
	d.mu.Lock()
	defer d.mu.Unlock()
	for _, item := range items {
		hash := strings.ToLower(strings.TrimSpace(item.InfoHash))
		name := cleanMetadataText(item.Name, 512)
		if name == "" {
			continue
		}
		var seeds *int
		peerBump := 0
		if item.Seeders > 0 || item.Leechers > 0 {
			peerBump = item.Seeders + item.Leechers
			s := item.Seeders
			seeds = &s
		}
		if d.applyNameLocked(namePatch{
			Hash: hash, Name: name, Source: "catalog", Size: item.SizeBytes,
			Seeds: seeds, PeerBump: peerBump, Kind: catalogKind(name),
			ResolutionMS: latency.Milliseconds(),
		}, now) {
			d.totalLookupTime += latency
			added++
		}
	}
	d.state.Attempts += len(items)
	d.pruneLocked()
	d.state.Status = "Sampling public torrents and resolving names in parallel"
	return added
}

func catalogKind(name string) string {
	lower := strings.ToLower(name)
	for _, ext := range []string{".mkv", ".mp4", ".avi", ".webm", ".mov", ".m4v", ".ts"} {
		if strings.Contains(lower, ext) {
			return "Video"
		}
	}
	for _, token := range []string{"1080p", "2160p", "720p", "web-dl", "webrip", "bluray", "hdtv", "remux", "x264", "x265", "hevc"} {
		if strings.Contains(lower, token) {
			return "Video"
		}
	}
	return "Other"
}
