package main

import (
	"context"
	"encoding/xml"
	"errors"
	"io"
	"net/http"
	"net/url"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/anacrolix/torrent/metainfo"
)

const feedBodyLimit = 4 << 20

type feedItem struct {
	Title string `xml:"title"`
	Links []struct {
		Href string `xml:"href,attr"`
		Text string `xml:",chardata"`
	} `xml:"link"`
	GUID       string `xml:"guid"`
	Magnet     string `xml:"magnetURI"`
	InfoHash   string `xml:"infoHash"`
	Enclosures []struct {
		URL string `xml:"url,attr"`
	} `xml:"enclosure"`
}
type feedEntry struct {
	hash, title string
	size        int64
	trackers    []string
	metadataURL string
}
type feedCursor struct{ url, etag, modified string }

func parseNamedFeed(r io.Reader) ([]feedEntry, error) {
	decoder := xml.NewDecoder(io.LimitReader(r, feedBodyLimit+1))
	items := make([]feedEntry, 0, 100)
	seen := map[string]bool{}
	for {
		token, err := decoder.Token()
		if err == io.EOF {
			return items, nil
		}
		if err != nil {
			return nil, err
		}
		start, ok := token.(xml.StartElement)
		if !ok || (start.Name.Local != "item" && start.Name.Local != "entry") {
			continue
		}
		var item feedItem
		if err := decoder.DecodeElement(&item, &start); err != nil {
			return nil, err
		}
		candidates := []string{item.Magnet, item.GUID}
		for _, link := range item.Links {
			candidates = append(candidates, link.Href, link.Text)
		}
		for _, enc := range item.Enclosures {
			candidates = append(candidates, enc.URL)
		}
		metadataURL := ""
		for _, value := range candidates {
			u, err := url.Parse(strings.TrimSpace(value))
			if err == nil && u.Scheme == "https" && strings.HasSuffix(u.Path, ".torrent") && trackerURL(value) != "" {
				metadataURL = u.String()
				break
			}
		}
		if hash := normalizeInfoHash(item.InfoHash); hash != "" && !seen[hash] {
			if title := cleanMetadataText(item.Title, 512); title != "" {
				seen[hash] = true
				items = append(items, feedEntry{hash: hash, title: title, metadataURL: metadataURL})
			}
		}
		for _, value := range candidates {
			m, err := metainfo.ParseMagnetUri(strings.TrimSpace(value))
			if err != nil {
				continue
			}
			hash := m.InfoHash.HexString()
			title := cleanMetadataText(item.Title, 512)
			if title == "" {
				title = cleanMetadataText(m.DisplayName, 512)
			}
			if seen[hash] && len(items) > 0 && items[len(items)-1].hash == hash && len(items[len(items)-1].trackers) == 0 {
				items[len(items)-1].trackers = m.Trackers
			}
			if title == "" || seen[hash] {
				continue
			}
			seen[hash] = true
			items = append(items, feedEntry{hash: hash, title: title, trackers: m.Trackers, metadataURL: metadataURL})
			break
		}
		if len(items) >= 1000 {
			return items, nil
		}
	}
}

func configuredFeeds() []feedCursor {
	raw, configured := os.LookupEnv("TORRENT_ATLAS_FEEDS")
	if !configured {
		raw = "https://subsplease.org/rss/?r=1080 https://nyaa.si/?page=rss https://eztvx.to/ezrss.xml"
	}
	out := []feedCursor{}
	for _, value := range strings.Fields(raw) {
		u, err := url.Parse(value)
		if err == nil && u.Scheme == "https" && u.Hostname() != "" && u.User == nil && len(out) < 8 {
			out = append(out, feedCursor{url: u.String()})
		}
	}
	return out
}

func fetchNamedFeed(ctx context.Context, client *http.Client, cursor *feedCursor) ([]feedEntry, error) {
	req, err := http.NewRequestWithContext(ctx, "GET", cursor.url, nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("User-Agent", "TorrentAtlas/1.1 (public metadata feeds)")
	req.Header.Set("Accept", "application/rss+xml, application/atom+xml, application/xml")
	if cursor.etag != "" {
		req.Header.Set("If-None-Match", cursor.etag)
	}
	if cursor.modified != "" {
		req.Header.Set("If-Modified-Since", cursor.modified)
	}
	response, err := client.Do(req)
	if err != nil {
		return nil, err
	}
	defer response.Body.Close()
	if response.StatusCode == http.StatusNotModified {
		return nil, nil
	}
	if response.StatusCode != http.StatusOK {
		return nil, errors.New("feed unavailable")
	}
	b, err := io.ReadAll(io.LimitReader(response.Body, feedBodyLimit+1))
	if err != nil {
		return nil, err
	}
	if len(b) > feedBodyLimit {
		return nil, errors.New("feed too large")
	}
	items, err := parseNamedFeed(strings.NewReader(string(b)))
	if err != nil {
		return nil, err
	}
	cursor.etag = response.Header.Get("ETag")
	cursor.modified = response.Header.Get("Last-Modified")
	return items, nil
}

func (s *sampler) applyFeedEntries(items []feedEntry, now time.Time) int {
	added := 0
	for _, item := range items {
		item.title = cleanMetadataText(item.title, 512)
		item.hash = normalizeInfoHash(item.hash)
		if item.hash == "" || item.title == "" {
			continue
		}
		s.d.mu.Lock()
		rec, exists := s.d.records[item.hash]
		if rec.Resolved {
			s.d.mu.Unlock()
			continue
		}
		if !exists {
			rec = observation{Hash: item.hash, FirstSeen: now, LastSeen: now, Countries: map[string]int{}}
		}
		if rec.NameHint == "" {
			s.d.state.FeedHints++
			added++
		}
		rec.NameHint = cleanMetadataText(item.title, 512)
		rec.Source = "feed"
		rec.Kind = catalogKind(rec.NameHint)
		// Fresh publisher/tracker associations should not wait behind a long
		// retry deadline inherited from an earlier blind DHT failure.
		if (item.metadataURL != "" || len(item.trackers) > 0) && now.Sub(rec.CheckedAt) >= time.Minute && rec.NextAttempt.After(now) {
			rec.NextAttempt = now
		}
		s.d.records[item.hash] = rec
		s.d.markPendingLocked(item.hash)
		s.d.indexNameLocked(item.hash, rec.NameHint)
		s.d.mu.Unlock()
		if s.trackers != nil {
			s.trackers.remember(item.hash, item.trackers, now)
			s.trackers.rememberMetadata(item.hash, item.metadataURL, now)
		}
		// Public feed tracker hints can bypass unavailable HTTP mirrors.
		s.mu.Lock()
		if s.cacheTried == nil {
			s.cacheTried = map[string]bool{}
		}
		s.cacheTried[item.hash] = true
		s.mu.Unlock()
		if s.d.tryResolveHashFromDump(item.hash) {
			s.clearCacheTried(item.hash)
		}
	}
	return added
}

func (s *sampler) ingestFeeds(ctx context.Context) {
	feeds := configuredFeeds()
	if len(feeds) == 0 {
		return
	}
	client := &http.Client{Timeout: 10 * time.Second}
	defer client.CloseIdleConnections()
	var work sync.WaitGroup
	for _, cursor := range feeds {
		work.Add(1)
		go func(cursor feedCursor) {
			defer work.Done()
			timer := time.NewTimer(0)
			defer timer.Stop()
			for {
				select {
				case <-ctx.Done():
					return
				case <-timer.C:
				}
				s.d.mu.Lock()
				s.d.state.FeedRequests++
				s.d.mu.Unlock()
				items, err := fetchNamedFeed(ctx, client, &cursor)
				if err == nil {
					s.applyFeedEntries(items, time.Now().UTC())
				} else if ctx.Err() == nil {
					s.d.mu.Lock()
					s.d.state.FeedErrors++
					s.d.mu.Unlock()
				}
				timer.Reset(5 * time.Minute)
			}
		}(cursor)
	}
	work.Wait()
}
