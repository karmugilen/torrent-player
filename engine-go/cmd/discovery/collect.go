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
	hash   string
	source krpc.NodeInfo
}
type sampler struct {
	d          *dashboard
	server     *dht.Server
	client     *torrent.Client
	mu         sync.Mutex
	nodes      map[string]krpc.NodeInfo
	nextSample map[string]time.Time
	attempted  map[string]time.Time
	jobs       chan discoveryJob
}

func (d *dashboard) collect(ctx context.Context) error {
	conn, e := net.ListenPacket("udp4", ":0")
	if e != nil {
		return e
	}
	cfg := dht.NewDefaultServerConfig()
	cfg.Conn = conn
	cfg.Passive = true
	cfg.SendLimiter = rate.NewLimiter(12, 4)
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
	tc.EstablishedConnsPerTorrent = 8
	tc.HalfOpenConnsPerTorrent = 4
	tc.TotalHalfOpenConns = 12
	tc.DialRateLimiter = rate.NewLimiter(8, 4)
	tc.NominalDialTimeout = 4 * time.Second
	tc.MinDialTimeout = 2 * time.Second
	tc.HandshakesTimeout = 4 * time.Second
	client, e := torrent.NewClient(tc)
	if e != nil {
		return e
	}
	defer client.Close()
	s := &sampler{d: d, server: server, client: client, nodes: map[string]krpc.NodeInfo{}, nextSample: map[string]time.Time{}, attempted: map[string]time.Time{}, jobs: make(chan discoveryJob, 48)}
	var work sync.WaitGroup
	for i := 0; i < 4; i++ {
		work.Add(1)
		go func() {
			defer work.Done()
			for {
				select {
				case <-ctx.Done():
					return
				case j := <-s.jobs:
					s.resolve(ctx, j)
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
	// Resolve only a few normal bootstrap endpoints; public discoveries expand
	// the routing table. No peer inventory or device files are requested.
	var bootstrap sync.WaitGroup
	for _, host := range []string{"router.bittorrent.com:6881", "router.utorrent.com:6881", "dht.transmissionbt.com:6881", "dht.libtorrent.org:25401"} {
		bootstrap.Add(1)
		go func(h string) {
			defer bootstrap.Done()
			a, e := net.ResolveUDPAddr("udp4", h)
			if e == nil {
				s.query(ctx, a, "find_node", krpc.MsgArgs{Target: randomID()})
			}
		}(host)
	}
	bootstrap.Wait()
	d.setStatus("Sampling public torrent hashes")
	tick := time.NewTicker(2 * time.Second)
	defer tick.Stop()
	for ctx.Err() == nil {
		s.sampleBatch(ctx)
		d.mu.Lock()
		d.state.Queries = int(server.Stats().OutboundQueriesAttempted)
		d.mu.Unlock()
		select {
		case <-ctx.Done():
		case <-tick.C:
		}
	}
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
			if len(s.nodes) < 1500 {
				s.nodes[n.Addr.String()] = n
			}
			_ = s.server.AddNode(n)
		}
	}
}
func (s *sampler) sampleBatch(ctx context.Context) {
	now := time.Now()
	batch := []krpc.NodeInfo{}
	s.mu.Lock()
	for addr, n := range s.nodes {
		if len(batch) < 4 && !now.Before(s.nextSample[addr]) {
			batch = append(batch, n)
			s.nextSample[addr] = now.Add(10 * time.Minute)
		}
	}
	s.mu.Unlock()
	if len(batch) == 0 {
		s.d.setStatus("Waiting for DHT sampling intervals")
		return
	}
	var work sync.WaitGroup
	for _, n := range batch {
		work.Add(1)
		go func(n krpc.NodeInfo) {
			defer work.Done()
			r := s.query(ctx, &net.UDPAddr{IP: n.Addr.IP, Port: n.Addr.Port}, "sample_infohashes", krpc.MsgArgs{Target: randomID()})
			if r == nil || r.Samples == nil {
				return
			}
			interval := 10 * time.Minute
			if r.Interval != nil {
				interval = time.Duration(max(60, min(21600, *r.Interval))) * time.Second
			}
			s.mu.Lock()
			s.nextSample[n.Addr.String()] = time.Now().Add(interval)
			s.mu.Unlock()
			s.d.mu.Lock()
			s.d.state.SamplingNodes++
			s.d.state.Status = "Discovering torrents and resolving names"
			for _, h := range *r.Samples {
				hash := hex.EncodeToString(h[:])
				rec, ok := s.d.records[hash]
				if !ok {
					rec = observation{Hash: hash, FirstSeen: now.UTC(), Countries: map[string]int{}}
				}
				rec.LastSeen = now.UTC()
				s.d.records[hash] = rec
			}
			s.d.pruneLocked()
			s.d.mu.Unlock()
			for _, h := range *r.Samples {
				s.enqueue(discoveryJob{hash: hex.EncodeToString(h[:]), source: n})
			}
		}(n)
	}
	work.Wait()
}
func (s *sampler) enqueue(j discoveryJob) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if time.Since(s.attempted[j.hash]) < 3*time.Minute {
		return
	}
	select {
	case s.jobs <- j:
		s.attempted[j.hash] = time.Now()
	default:
	}
	if len(s.attempted) > 2400 {
		for k, t := range s.attempted {
			if time.Since(t) > 10*time.Minute {
				delete(s.attempted, k)
			}
		}
	}
}
func (s *sampler) resolve(parent context.Context, j discoveryJob) {
	ctx, cancel := context.WithTimeout(parent, 14*time.Second)
	defer cancel()
	var hash metainfo.Hash
	raw, e := hex.DecodeString(j.hash)
	if e != nil {
		return
	}
	copy(hash[:], raw)
	t, _, e := s.client.AddTorrentSpec(&torrent.TorrentSpec{AddTorrentOpts: torrent.AddTorrentOpts{InfoHash: hash, DisallowDataDownload: true, DisallowDataUpload: true, DisableInitialPieceCheck: true}})
	if e != nil {
		return
	}
	defer t.Drop()
	s.d.mu.Lock()
	rec, ok := s.d.records[j.hash]
	if !ok {
		s.d.mu.Unlock()
		return
	}
	s.d.state.Attempts++
	s.d.mu.Unlock()
	uniqueIPs := map[string]bool{}
	countries := map[string]int{}
	var seeds krpc.ScrapeBloomFilter
	hasSeedEstimate := false
	addPeers := func(peers []krpc.NodeAddr) {
		for _, p := range peers {
			if publicIP(p.IP) && p.Port > 0 {
				if !uniqueIPs[p.IP.String()] {
					uniqueIPs[p.IP.String()] = true
					countries[s.d.country(p.IP)]++
				}
				t.AddPeers([]torrent.PeerInfo{{Addr: &net.TCPAddr{IP: p.IP, Port: p.Port}}})
			}
		}
	}
	// Sampled hashes are known to this source node. Ask it first rather than
	// waiting for an entire lookup to finish before opening metadata connections.
	reply := s.query(ctx, &net.UDPAddr{IP: j.source.Addr.IP, Port: j.source.Addr.Port}, "get_peers", krpc.MsgArgs{InfoHash: krpc.ID(hash), Scrape: 1})
	if reply != nil {
		addPeers(reply.Values)
		if reply.BFsd != nil {
			hasSeedEstimate = true
			mergeBloom(&seeds, reply.BFsd)
		}
	}
	ann, e := s.server.AnnounceTraversal(hash, dht.Scrape())
	if e != nil {
		return
	}
	defer ann.Close()
	peerEvents := ann.Peers
	got := t.GotInfo()
	var settle <-chan time.Time
	var settleTimer *time.Timer
	defer func() {
		if settleTimer != nil {
			settleTimer.Stop()
		}
	}()
loop:
	for {
		select {
		case <-ctx.Done():
			break loop
		case <-settle:
			break loop
		case peers, ok := <-peerEvents:
			if !ok {
				peerEvents = nil
				if rec.Resolved || len(uniqueIPs) == 0 {
					break loop
				}
				continue
			}
			addPeers(peers.Peers)
			if peers.BFsd != nil {
				hasSeedEstimate = true
				mergeBloom(&seeds, peers.BFsd)
			}
		case <-got:
			got = nil
			info := t.Info()
			if info.Private != nil && *info.Private {
				return
			}
			rec.Name = info.BestName()
			rec.Resolved = true
			rec.Size = t.Length()
			rec.Kind = "Other"
			rec.Files = nil
			for _, f := range t.Files() {
				name := f.Path()
				lower := strings.ToLower(name)
				video := false
				for _, ext := range []string{".mkv", ".mp4", ".avi", ".webm", ".mov", ".m4v", ".ts"} {
					if strings.HasSuffix(lower, ext) {
						video = true
						break
					}
				}
				if video {
					rec.Kind = "Video"
				}
				if len(rec.Files) < 20 {
					rec.Files = append(rec.Files, name)
				}
			}
			settleTimer = time.NewTimer(1500 * time.Millisecond)
			settle = settleTimer.C
		}
	}
	if parent.Err() != nil {
		return
	}
	rec.Peers = len(uniqueIPs)
	rec.Countries = countries
	rec.CheckedAt = time.Now().UTC()
	if hasSeedEstimate {
		rec.Seeds = countBloom(&seeds)
	} else {
		rec.Seeds = nil
	}
	stats := t.Stats()
	payload := stats.BytesReadData.Int64()
	s.d.mu.Lock()
	if latest, exists := s.d.records[j.hash]; exists {
		rec.LastSeen = latest.LastSeen
		s.d.records[j.hash] = rec
	}
	s.d.state.PayloadBytes += payload
	s.d.mu.Unlock()
}
