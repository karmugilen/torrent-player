package main

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"errors"
	"math"
	"net"
	"strings"
	"sync"
	"time"
	"unicode"

	"github.com/anacrolix/dht/v2"
	"github.com/anacrolix/dht/v2/krpc"
	"github.com/anacrolix/torrent"
	"github.com/anacrolix/torrent/metainfo"
	"github.com/anacrolix/torrent/storage"
	"golang.org/x/time/rate"
)

// No payload storage exists: metadata discovery cannot save movie data even
// if a future caller accidentally enables torrent payload requests.
type metadataStorage struct{}
type metadataPiece struct{}

func (metadataStorage) OpenTorrent(context.Context, *metainfo.Info, metainfo.Hash) (storage.TorrentImpl, error) {
	return storage.TorrentImpl{Piece: func(metainfo.Piece) storage.PieceImpl { return metadataPiece{} }, Close: func() error { return nil }}, nil
}
func (metadataPiece) ReadAt([]byte, int64) (int, error) {
	return 0, errors.New("metadata-only discovery")
}
func (metadataPiece) WriteAt([]byte, int64) (int, error) {
	return 0, errors.New("metadata-only discovery")
}
func (metadataPiece) MarkComplete() error            { return errors.New("metadata-only discovery") }
func (metadataPiece) MarkNotComplete() error         { return nil }
func (metadataPiece) Completion() storage.Completion { return storage.Completion{Ok: true} }

func publicIP(ip net.IP) bool {
	return ip != nil && ip.To4() != nil && ip.IsGlobalUnicast() && !ip.IsPrivate() && !ip.IsLoopback() && !ip.IsLinkLocalUnicast()
}
func mergeBloom(dst *krpc.ScrapeBloomFilter, src *krpc.ScrapeBloomFilter) {
	if src != nil {
		for i := range dst {
			dst[i] |= src[i]
		}
	}
}
func countBloom(b *krpc.ScrapeBloomFilter) *int {
	empty := true
	for _, v := range b {
		if v != 0 {
			empty = false
			break
		}
	}
	n := 0
	if !empty {
		v := b.EstimateCount()
		if math.IsNaN(v) || math.IsInf(v, 0) || v > 6000 {
			return nil
		}
		n = int(math.Round(v))
	}
	return &n
}

type discoveryJob struct {
	hash    string
	sources []krpc.NodeInfo
}
type sampler struct {
	d             *dashboard
	server        *dht.Server
	client        *torrent.Client
	mu            sync.Mutex
	nodes         map[string]krpc.NodeInfo
	nextSample    map[string]time.Time
	sources       map[string][]krpc.NodeInfo
	inflight      map[string]bool
	turn          int
	caches        *metadataCaches
	samplingSlots chan struct{}
	samplingWork  sync.WaitGroup
	lookup        func(context.Context, *net.UDPAddr, string, krpc.MsgArgs) *krpc.Return
	nextRefresh   time.Time
	hints         *peerHintCache
}

func (d *dashboard) collect(ctx context.Context) error {
	conn, e := net.ListenPacket("udp4", ":0")
	if e != nil {
		return e
	}
	cfg := dht.NewDefaultServerConfig()
	cfg.Conn = conn
	cfg.Passive = true
	cfg.SendLimiter = rate.NewLimiter(128, 32)
	server, e := dht.NewServer(cfg)
	if e != nil {
		conn.Close()
		return e
	}
	defer server.Close()
	tc := torrent.NewDefaultClientConfig()
	tc.DefaultStorage = metadataStorage{}
	tc.NoDHT = true
	tc.DisableTrackers = true
	tc.DisablePEX = true
	tc.DisableWebtorrent = true
	tc.NoDefaultPortForwarding = true
	tc.DisableIPv6 = true
	tc.ListenPort = 0
	tc.NoUpload = true
	tc.AcceptPeerConnections = false
	tc.EstablishedConnsPerTorrent = 12
	tc.HalfOpenConnsPerTorrent = 6
	tc.TotalHalfOpenConns = 72
	tc.DialRateLimiter = rate.NewLimiter(48, 12)
	tc.NominalDialTimeout = 4 * time.Second
	tc.MinDialTimeout = 2 * time.Second
	tc.HandshakesTimeout = 4 * time.Second
	client, e := torrent.NewClient(tc)
	if e != nil {
		return e
	}
	defer client.Close()
	s := &sampler{d: d, server: server, client: client, nodes: map[string]krpc.NodeInfo{}, nextSample: map[string]time.Time{}, inflight: map[string]bool{}, caches: newMetadataCaches(defaultMetadataCaches), samplingSlots: make(chan struct{}, samplingConcurrency), hints: &peerHintCache{}}
	s.lookup = s.query
	defer s.caches.close()
	d.mu.Lock()
	d.state.Workers = resolverWorkers
	d.mu.Unlock()
	s.bootstrapNodes(ctx)
	var work sync.WaitGroup
	for i := 0; i < resolverWorkers; i++ {
		work.Add(1)
		go func() {
			defer work.Done()
			idle := time.NewTicker(200 * time.Millisecond)
			defer idle.Stop()
			for ctx.Err() == nil {
				if j, ok := s.nextJob(time.Now()); ok {
					s.resolve(ctx, j)
					s.releaseJob(j.hash)
					continue
				}
				select {
				case <-ctx.Done():
					return
				case <-idle.C:
				}
			}
		}()
	}
	work.Add(1)
	go func() {
		defer work.Done()
		t := time.NewTicker(10 * time.Second)
		defer t.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case <-t.C:
				d.persist()
			}
		}
	}()
	d.setStatus("Sampling public torrent hashes")
	tick := time.NewTicker(250 * time.Millisecond)
	defer tick.Stop()
	for ctx.Err() == nil {
		s.refreshRoutes(ctx)
		s.sampleBatch(ctx)
		d.mu.Lock()
		d.state.Queries = int(server.Stats().OutboundQueriesAttempted)
		d.state.ActiveSamplers = len(s.samplingSlots)
		d.mu.Unlock()
		select {
		case <-ctx.Done():
		case <-tick.C:
		}
	}
	s.samplingWork.Wait()
	d.mu.Lock()
	d.state.ActiveSamplers = 0
	d.mu.Unlock()
	work.Wait()
	return ctx.Err()
}
func randomID() (id krpc.ID) { _, _ = rand.Read(id[:]); return }
func (s *sampler) query(ctx context.Context, a *net.UDPAddr, q string, args krpc.MsgArgs) *krpc.Return {
	if !publicIP(a.IP) || a.Port == 0 {
		return nil
	}
	c, cancel := context.WithTimeout(ctx, 2500*time.Millisecond)
	defer cancel()
	args.Want = []krpc.Want{krpc.WantNodes}
	r := s.server.Query(c, dht.NewAddr(a), q, dht.QueryInput{MsgArgs: args, NumTries: 1})
	if r.Reply.R == nil {
		return nil
	}
	s.d.mu.Lock()
	s.d.state.Responses++
	s.d.mu.Unlock()
	s.addNodes(r.Reply.R)
	return r.Reply.R
}
func (s *sampler) addNodes(r *krpc.Return) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, n := range r.Nodes {
		if publicIP(n.Addr.IP) && n.Addr.Port > 0 {
			if len(s.nodes) < 8192 {
				s.nodes[n.Addr.String()] = n
			}
			_ = s.server.AddNode(n)
		}
	}
}

func cleanMetadataText(value string, limit int) string {
	value = strings.ToValidUTF8(value, "�")
	value = strings.Map(func(r rune) rune {
		if unicode.IsControl(r) {
			return -1
		}
		return r
	}, value)
	runes := []rune(value)
	if len(runes) > limit {
		value = string(runes[:limit])
	}
	return strings.TrimSpace(value)
}

func applyMetadata(rec *observation, info *metainfo.Info, source string, latency time.Duration) bool {
	if (info.Private != nil && *info.Private) || strings.TrimSpace(info.BestName()) == "" {
		return false
	}
	rec.Name = cleanMetadataText(info.BestName(), 512)
	if rec.Name == "" {
		return false
	}
	rec.Resolved = true
	rec.Source = source
	rec.ResolutionMS = latency.Milliseconds()
	rec.Size = info.TotalLength()
	rec.Kind = "Other"
	rec.Files = nil
	for file := range info.UpvertedFilesIter() {
		name := cleanMetadataText(file.DisplayPath(info), 1024)
		lower := strings.ToLower(name)
		for _, ext := range []string{".mkv", ".mp4", ".avi", ".webm", ".mov", ".m4v", ".ts"} {
			if strings.HasSuffix(lower, ext) {
				rec.Kind = "Video"
				break
			}
		}
		if len(rec.Files) < 20 {
			rec.Files = append(rec.Files, name)
		}
	}
	return true
}

func (s *sampler) resolve(parent context.Context, j discoveryJob) {
	started := time.Now()
	ctx, cancel := context.WithTimeout(parent, 12*time.Second)
	defer cancel()
	var hash metainfo.Hash
	raw, err := hex.DecodeString(j.hash)
	if err != nil || len(raw) != len(hash) {
		return
	}
	copy(hash[:], raw)
	s.d.mu.Lock()
	rec, exists := s.d.records[j.hash]
	if exists {
		s.d.state.Attempts++
	}
	s.d.mu.Unlock()
	if !exists {
		return
	}
	wasResolved := rec.Resolved
	t, _, err := s.client.AddTorrentSpec(&torrent.TorrentSpec{AddTorrentOpts: torrent.AddTorrentOpts{InfoHash: hash, DisallowDataDownload: true, DisallowDataUpload: true, DisableInitialPieceCheck: true}})
	if err != nil {
		return
	}
	defer t.Drop()

	// All work for this hash is joined before its slot is released or collection
	// is paused. Winning metadata immediately cancels its HTTP competitors.
	var work sync.WaitGroup
	defer func() { cancel(); work.Wait() }()
	cacheCtx, cancelCache := context.WithCancel(ctx)
	defer cancelCache()
	cacheEvents := make(chan *metainfo.Info, 1)
	if !wasResolved {
		work.Add(1)
		go func() {
			defer work.Done()
			defer close(cacheEvents)
			if info := s.caches.fetch(cacheCtx, hash); info != nil {
				cacheEvents <- info
			}
		}()
	}
	if wasResolved {
		close(cacheEvents)
	}
	// Known addresses only accelerate dialing. They are not fresh map sightings.
	if !wasResolved {
		if hints := s.hints.get(j.hash, time.Now()); len(hints) > 0 {
			connections := make([]torrent.PeerInfo, 0, len(hints))
			for _, peer := range hints {
				connections = append(connections, torrent.PeerInfo{Addr: &net.TCPAddr{IP: peer.IP, Port: peer.Port}})
			}
			t.AddPeers(connections)
			s.d.mu.Lock()
			s.d.state.PeerHintUses++
			s.d.mu.Unlock()
		}
	}
	initialReplies := make(chan *krpc.Return, len(j.sources))
	for _, source := range j.sources {
		work.Add(1)
		go func(source krpc.NodeInfo) {
			defer work.Done()
			if reply := s.query(ctx, &net.UDPAddr{IP: source.Addr.IP, Port: source.Addr.Port}, "get_peers", krpc.MsgArgs{InfoHash: krpc.ID(hash), Scrape: 1}); reply != nil {
				initialReplies <- reply
			}
		}(source)
	}
	prefetchDone := make(chan struct{})
	go func() { work.Wait(); close(prefetchDone) }()
	var prefetchEvents <-chan struct{} = prefetchDone
	prefetchComplete := false
	ann, err := s.server.AnnounceTraversal(hash, dht.Scrape())
	if err != nil {
		return
	}
	defer ann.Close()
	peerEvents := ann.Peers
	got := t.GotInfo()
	if wasResolved {
		got = nil
	}
	uniqueIPs := map[string]bool{}
	countries := map[string]int{}
	var seeds krpc.ScrapeBloomFilter
	hasSeedEstimate := false
	addPeers := func(peers []krpc.NodeAddr) {
		s.hints.put(j.hash, peers, time.Now())
		connections := make([]torrent.PeerInfo, 0, len(peers))
		for _, p := range peers {
			if publicIP(p.IP) && p.Port > 0 {
				if !uniqueIPs[p.IP.String()] {
					uniqueIPs[p.IP.String()] = true
					countries[s.d.country(p.IP)]++
				}
				if !rec.Resolved {
					connections = append(connections, torrent.PeerInfo{Addr: &net.TCPAddr{IP: p.IP, Port: p.Port}})
				}
			}
		}
		if len(connections) > 0 {
			t.AddPeers(connections)
		}
	}
	var settle <-chan time.Time
	var settleTimer *time.Timer
	defer func() {
		if settleTimer != nil {
			settleTimer.Stop()
		}
	}()
	startSettling := func() {
		if settleTimer == nil {
			settleTimer = time.NewTimer(1200 * time.Millisecond)
			settle = settleTimer.C
		}
	}
	if wasResolved {
		startSettling()
	}
	accept := func(info *metainfo.Info, source string) bool {
		if rec.Resolved {
			return true
		}
		if !applyMetadata(&rec, info, source, time.Since(started)) {
			return false
		}
		cancelCache()
		got = nil
		startSettling()
		// Show a name as soon as it arrives, while counts finish settling.
		s.d.mu.Lock()
		latest, retained := s.d.records[j.hash]
		if retained {
			rec.LastSeen = latest.LastSeen
			rec.SampleHits = latest.SampleHits
		}
		s.d.records[j.hash] = rec
		s.d.pruneLocked()
		s.d.mu.Unlock()
		return true
	}
	noPeerTimer := time.NewTimer(8 * time.Second)
	defer noPeerTimer.Stop()
	noPeerDeadline, emptyExit := false, false
loop:
	for {
		if !rec.Resolved && prefetchComplete && len(cacheEvents) == 0 && len(initialReplies) == 0 && len(uniqueIPs) == 0 {
			stats := t.Stats()
			if (peerEvents == nil && stats.TotalPeers == 0) || (noPeerDeadline && stats.ActivePeers == 0) {
				emptyExit = true
				break loop
			}
		}
		select {
		case <-noPeerTimer.C:
			noPeerDeadline = true
		case <-prefetchEvents:
			prefetchEvents = nil
			prefetchComplete = true
		case <-ctx.Done():
			break loop
		case <-settle:
			break loop
		case reply := <-initialReplies:
			addPeers(reply.Values)
			if reply.BFsd != nil {
				hasSeedEstimate = true
				mergeBloom(&seeds, reply.BFsd)
			}
		case peers, ok := <-peerEvents:
			if !ok {
				peerEvents = nil
				continue
			}
			addPeers(peers.Peers)
			if peers.BFsd != nil {
				hasSeedEstimate = true
				mergeBloom(&seeds, peers.BFsd)
			}
		case info, ok := <-cacheEvents:
			if !ok {
				cacheEvents = nil
				continue
			}
			if !accept(info, "cache") {
				return
			}
		case <-got:
			got = nil
			if !accept(t.Info(), "peer") {
				return
			}
		}
	}
	if parent.Err() != nil {
		return
	}
	rec.Peers = len(uniqueIPs)
	rec.Countries = countries
	rec.CheckedAt = time.Now().UTC()
	rec.NextAttempt = rec.CheckedAt.Add(retryDelay(rec.ResolveAttempts))
	if rec.Resolved {
		rec.NextAttempt = rec.CheckedAt.Add(30 * time.Minute)
	}
	if hasSeedEstimate {
		rec.Seeds = countBloom(&seeds)
	} else {
		rec.Seeds = nil
	}
	stats := t.Stats()
	s.d.mu.Lock()
	if latest, exists := s.d.records[j.hash]; exists {
		rec.LastSeen = latest.LastSeen
		rec.SampleHits = latest.SampleHits
	}
	// A slow successful job must survive a newer sample evicting its unresolved
	// row. Failed evicted jobs need no further retention.
	if _, exists := s.d.records[j.hash]; exists || rec.Resolved {
		s.d.records[j.hash] = rec
		s.d.pruneLocked()
	}
	s.d.state.PayloadBytes += stats.BytesReadData.Int64()
	s.d.state.Completed++
	if emptyExit {
		s.d.state.EmptyLookups++
	}
	s.d.totalLookupTime += time.Since(started)
	if !wasResolved && rec.Resolved {
		s.d.state.Successes++
		if rec.Source == "cache" {
			s.d.state.CacheHits++
		} else {
			s.d.state.PeerHits++
		}
	}
	s.d.mu.Unlock()
}

func (s *sampler) bootstrapNodes(ctx context.Context) {
	// Resolve only a few normal bootstrap endpoints; public discoveries expand
	// the routing table. No peer inventory or device files are requested.
	var bootstrap sync.WaitGroup
	for _, host := range []string{"router.bittorrent.com:6881", "router.utorrent.com:6881", "dht.transmissionbt.com:6881", "dht.libtorrent.org:25401"} {
		bootstrap.Add(1)
		go func(h string) {
			defer bootstrap.Done()
			a, e := net.ResolveUDPAddr("udp4", h)
			if e == nil {
				s.lookup(ctx, a, "find_node", krpc.MsgArgs{Target: randomID()})
			}
		}(host)
	}
	bootstrap.Wait()
}
