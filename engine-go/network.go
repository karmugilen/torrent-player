package main

import (
	"context"
	"encoding/json"
	"log"
	"net"
	"net/http"
	"strings"
	"sync"
	"time"

	analog "github.com/anacrolix/log"
	"github.com/anacrolix/torrent"
	"github.com/anacrolix/torrent/tracker"
	httpTracker "github.com/anacrolix/torrent/tracker/http"
)

// Tracks HTTP/WebSocket transports so an old route cannot leave a tracker
// apparently connected for minutes. The library reconnects closed WebSockets.
type networkDialer struct {
	mu          sync.Mutex
	epoch       uint64
	split       bool
	connections map[*networkConn]struct{}
}

type networkConn struct {
	net.Conn
	owner *networkDialer
}

func (c *networkConn) Close() error {
	c.owner.mu.Lock()
	delete(c.owner.connections, c)
	c.owner.mu.Unlock()
	return c.Conn.Close()
}

func (d *networkDialer) reset() {
	if d == nil {
		return
	}
	d.mu.Lock()
	d.epoch++
	connections := d.connections
	d.connections = nil
	d.mu.Unlock()
	for c := range connections {
		_ = c.Conn.Close()
	}
}

func (s *EngineServer) handleNetworkChange(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Generation int64 `json:"generation"`
		Connected  *bool `json:"connected"`
	}
	if json.NewDecoder(r.Body).Decode(&req) != nil || req.Generation < 0 || req.Connected == nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid network state"})
		return
	}
	s.networkMu.Lock()
	defer s.networkMu.Unlock()
	if s.shuttingDown.Load() {
		writeJSON(w, http.StatusServiceUnavailable, map[string]string{"error": "engine is stopping"})
		return
	}
	// Ignore duplicate and delayed callbacks. A policy update is independent of
	// route recovery and must not tear down healthy peers on the same network.
	if s.networkKnown && req.Generation <= s.networkGeneration {
		writeJSON(w, http.StatusOK, OkResponse{Ok: true})
		return
	}
	s.networkKnown, s.networkGeneration = true, req.Generation
	s.mu.Lock()
	s.resetGhostLocked()
	s.mu.Unlock()
	if s.networkCancel != nil {
		s.networkCancel()
	}
	s.trackerSockets.reset()
	if *req.Connected {
		ctx, cancel := context.WithCancel(context.Background())
		s.networkCancel = cancel
		s.networkWorkers.Add(1)
		go func() {
			defer s.networkWorkers.Done()
			s.recoverNetwork(ctx)
		}()
	}
	writeJSON(w, http.StatusOK, OkResponse{Ok: true})
}

func (s *EngineServer) stopNetworkRecovery() {
	s.networkMu.Lock()
	if s.networkCancel != nil {
		s.networkCancel()
	}
	s.networkMu.Unlock()
	s.networkWorkers.Wait()
	s.trackerSockets.reset()
}

func (s *EngineServer) recoverNetwork(ctx context.Context) {
	s.mu.RLock()
	records := make([]*TorrentRecord, 0, len(s.records))
	for _, rec := range s.records {
		records = append(records, rec)
	}
	s.mu.RUnlock()
	for _, rec := range records {
		rec.mu.Lock()
		if ctx.Err() == nil && !rec.removed && !rec.Paused {
			peers := rec.Torrent.PeerConns()
			// Keep dialable addresses through rapid handoffs. KnownSwarm in the
			// pinned library reads its pending map without locking; do not use it.
			seen := make(map[string]bool)
			cached := make([]torrent.PeerInfo, 0, 80)
			remember := func(p torrent.PeerInfo) {
				if p.Addr == nil || len(cached) >= 80 {
					return
				}
				host, port, err := net.SplitHostPort(p.Addr.String())
				if err != nil || net.ParseIP(host) == nil || port == "0" || seen[p.Addr.String()] {
					return
				}
				seen[p.Addr.String()] = true
				cached = append(cached, p)
			}
			for _, peer := range peers {
				remember(torrent.PeerInfo{Addr: peer.RemoteAddr, Source: peer.Discovery})
				_ = peer.Close()
			}
			for _, peer := range rec.reconnectPeers {
				remember(peer)
			}
			rec.reconnectPeers = cached
		}
		rec.mu.Unlock()
	}
	s.notifyChange()
	// Fresh tracker sockets bypass the old network's UDP connection IDs and
	// long announce backoff. Only failed announces are retried in this episode.
	announced := &sync.Map{}
	for _, delay := range []time.Duration{0, 3 * time.Second, 10 * time.Second, 30 * time.Second} {
		if !waitForNetworkRetry(ctx, delay) {
			return
		}
		var work sync.WaitGroup
		permits := make(chan struct{}, 4)
		pending := false
		for _, rec := range records {
			rec.mu.RLock()
			if ctx.Err() != nil || rec.Paused || rec.removed || (delay != 0 && rec.Torrent.Stats().ActivePeers > 0) {
				rec.mu.RUnlock()
				continue
			}
			pending = true
			t := rec.Torrent
			t.AddPeers(rec.reconnectPeers)
			mi := t.Metainfo()
			info := t.Info()
			private := info != nil && info.Private != nil && *info.Private
			rec.mu.RUnlock()
			if !private {
				for _, dht := range s.client.DhtServers() {
					work.Add(1)
					go func() {
						defer work.Done()
						if ctx.Err() != nil {
							return
						}
						done, stop, err := t.AnnounceToDht(dht)
						if err != nil {
							return
						}
						defer stop()
						timer := time.NewTimer(8 * time.Second)
						defer timer.Stop()
						select {
						case <-ctx.Done():
						case <-t.Closed():
						case <-done:
						case <-timer.C:
						}
					}()
				}
			}
			urls := make(map[string]bool)
			for _, tier := range mi.UpvertedAnnounceList() {
				for _, url := range tier {
					if urls[url] || len(urls) >= 64 || !(strings.HasPrefix(url, "http://") || strings.HasPrefix(url, "https://") || strings.HasPrefix(url, "udp://")) {
						continue
					}
					urls[url] = true
					key := rec.ID + "\x00" + url
					if _, ok := announced.Load(key); ok {
						continue
					}
					work.Add(1)
					go func() {
						defer work.Done()
						select {
						case permits <- struct{}{}:
						case <-ctx.Done():
							return
						}
						defer func() { <-permits }()
						if s.announceAfterNetworkChange(ctx, rec, url) {
							announced.Store(key, true)
						}
					}()
				}
			}
		}
		work.Wait()
		if !pending {
			return
		}
		s.notifyChange()
	}
	log.Print("network recovery discovery attempts finished")
}

func waitForNetworkRetry(ctx context.Context, delay time.Duration) bool {
	timer := time.NewTimer(delay)
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return false
	case <-timer.C:
		return ctx.Err() == nil
	}
}

func (s *EngineServer) announceAfterNetworkChange(ctx context.Context, rec *TorrentRecord, url string) bool {
	ctx, cancel := context.WithTimeout(ctx, 6*time.Second)
	defer cancel()
	rec.mu.RLock()
	if ctx.Err() != nil || rec.Paused || rec.removed {
		rec.mu.RUnlock()
		return false
	}
	t := rec.Torrent
	stats := t.Stats()
	left := int64(-1)
	if t.Info() != nil {
		left = t.BytesMissing()
	}
	req := tracker.AnnounceRequest{
		InfoHash: t.InfoHash(), PeerId: s.client.PeerID(),
		Downloaded: stats.BytesReadData.Int64(), Uploaded: stats.BytesWrittenData.Int64(),
		Left: left, Port: uint16(s.client.LocalPort()), NumWant: 80,
	}
	rec.mu.RUnlock()
	opts := tracker.NewClientOpts{Logger: analog.Default}
	if s.trackerSockets != nil {
		opts.Http = httpTracker.NewClientOpts{DialContext: s.trackerSockets.DialContext}
	}
	client, err := tracker.NewClient(url, opts)
	if err != nil {
		return false
	}
	defer client.Close()
	response, err := client.Announce(ctx, req, tracker.AnnounceOpt{})
	if ctx.Err() != nil {
		return false
	}
	if err != nil {
		s.setTrackerStatus(rec.InfoHash, url, trackerStatusError, err.Error())
		return false
	}
	rec.mu.RLock()
	defer rec.mu.RUnlock()
	if rec.Paused || rec.removed {
		return false
	}
	peers := make([]torrent.PeerInfo, 0, len(response.Peers))
	for _, p := range response.Peers {
		if p.Port == 0 {
			continue
		}
		peers = append(peers, torrent.PeerInfo{Addr: &net.TCPAddr{IP: p.IP, Port: p.Port}, Source: torrent.PeerSourceTracker})
	}
	t.AddPeers(peers)
	s.setTrackerStatus(rec.InfoHash, url, trackerStatusWorking, formatPeerCount(len(peers)))
	return true
}
