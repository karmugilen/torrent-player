package main

import (
	"bytes"
	"crypto/sha1"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"math/rand"
	"net"
	"os"
	"reflect"
	"slices"
	"sort"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/anacrolix/torrent"
	"github.com/anacrolix/torrent/bencode"
	"github.com/anacrolix/torrent/metainfo"
	"golang.org/x/time/rate"
)

// Generative search over the seek rules. Run with
//
//	SEEK_SEARCH=1 go test -run TestSeekRuleSearch -v -timeout 2h .
//
// It builds a simulated swarm: 55 peers at four speeds with 20–199 ms
// inbound delay, 256 KiB pieces, a 16 MiB Direct Play cache. Candidates are
// scored on the time from a seek into missing video to its first 64 KiB,
// interleaved on the same swarm so they see the same conditions. The best
// survive, mutate and are scored again; a final round compares the winner
// with the shipped rules.

type seekCandidate struct {
	lib        torrent.SeekTuning
	stealAfter time.Duration
	nudgeEvery time.Duration
	nowExtra   int
	samples    []time.Duration
}

func (c *seekCandidate) key() string {
	return fmt.Sprintf("blocks=%d div=%d faster=%.2g queue=%v steal=%v nudge=%v now+%d",
		c.lib.FocusBlocks, c.lib.FocusStealDivisor, c.lib.FasterRatio, c.lib.QueueAware, c.stealAfter, c.nudgeEvery, c.nowExtra)
}

func (c *seekCandidate) apply(s *EngineServer) {
	torrent.SetSeekTuning(c.lib)
	claudeSeekStealAfter, claudeSeekNudgeEvery, urgentNowExtraPieces = c.stealAfter, c.nudgeEvery, c.nowExtra
	s.client.SetUrgentRequestStealing(c.stealAfter)
}

func quantile(samples []time.Duration, q float64) time.Duration {
	if len(samples) == 0 {
		return 0
	}
	sorted := slices.Clone(samples)
	slices.Sort(sorted)
	return sorted[min(len(sorted)-1, int(q*float64(len(sorted))))]
}

// score weighs the typical seek and the slow tail equally.
func (c *seekCandidate) score() time.Duration {
	return (quantile(c.samples, 0.5) + quantile(c.samples, 0.9)) / 2
}

var (
	searchFocusBlocks = []int{1, 2, 4, 8, 16}
	searchStealDiv    = []int{1, 2, 3, 5, 8}
	searchFaster      = []float64{0, 1.25, 1.5, 2, 3}
	searchStealAfter  = []time.Duration{300 * time.Millisecond, 500 * time.Millisecond, 750 * time.Millisecond, time.Second, 1500 * time.Millisecond}
	searchNudge       = []time.Duration{100 * time.Millisecond, 250 * time.Millisecond, 500 * time.Millisecond}
	searchNowExtra    = []int{0, 1, 2}
)

func randomCandidate(r *rand.Rand) *seekCandidate {
	return &seekCandidate{
		lib: torrent.SeekTuning{
			FocusBlocks: searchFocusBlocks[r.Intn(len(searchFocusBlocks))], FocusStealDivisor: searchStealDiv[r.Intn(len(searchStealDiv))],
			FasterRatio: searchFaster[r.Intn(len(searchFaster))], QueueAware: r.Intn(2) == 0,
		},
		stealAfter: searchStealAfter[r.Intn(len(searchStealAfter))],
		nudgeEvery: searchNudge[r.Intn(len(searchNudge))],
		nowExtra:   searchNowExtra[r.Intn(len(searchNowExtra))],
	}
}

// mutate changes one or two rules of parent.
func mutate(r *rand.Rand, parent *seekCandidate) *seekCandidate {
	child := *parent
	child.samples = nil
	donor := randomCandidate(r)
	for n := 1 + r.Intn(2); n > 0; n-- {
		switch r.Intn(7) {
		case 0:
			child.lib.FocusBlocks = donor.lib.FocusBlocks
		case 1:
			child.lib.FocusStealDivisor = donor.lib.FocusStealDivisor
		case 2:
			child.lib.FasterRatio = donor.lib.FasterRatio
		case 3:
			child.lib.QueueAware = !child.lib.QueueAware
		case 4:
			child.stealAfter = donor.stealAfter
		case 5:
			child.nudgeEvery = donor.nudgeEvery
		case 6:
			child.nowExtra = donor.nowExtra
		}
	}
	return &child
}

// latencyConn delays incoming data only. Writes are unchanged, so the
// injected delay adds once to a request's round trip, not once per leg.
type latencyConn struct {
	net.Conn
	chunks  chan latencyChunk
	pending []byte
}

type latencyChunk struct {
	data []byte
	err  error
	at   time.Time
}

func newLatencyConn(c net.Conn, delay time.Duration) net.Conn {
	l := &latencyConn{Conn: c, chunks: make(chan latencyChunk, 4096)}
	go func() {
		defer close(l.chunks)
		for {
			buf := make([]byte, 32<<10)
			n, err := c.Read(buf)
			if n > 0 {
				l.chunks <- latencyChunk{data: buf[:n], at: time.Now().Add(delay)}
			}
			if err != nil {
				l.chunks <- latencyChunk{err: err, at: time.Now().Add(delay)}
				if ne, ok := err.(net.Error); ok && ne.Timeout() {
					continue
				}
				return
			}
		}
	}()
	return l
}

func (l *latencyConn) Read(b []byte) (int, error) {
	if len(l.pending) == 0 {
		chunk, ok := <-l.chunks
		if !ok {
			return 0, io.EOF
		}
		time.Sleep(time.Until(chunk.at))
		if chunk.err != nil {
			return 0, chunk.err
		}
		l.pending = chunk.data
	}
	n := copy(b, l.pending)
	l.pending = l.pending[n:]
	return n, nil
}

type seekSwarm struct {
	leech   *EngineServer
	rec     *TorrentRecord
	seek    func() time.Duration
	rng     *rand.Rand
	reader  *playbackReader
	lastOff *int64
	// partial makes seeks read like HTTP: return once some bytes are ready.
	partial *bool
	// peers describes each seed by port: its one-way delay and upload rate.
	peers map[string]string
}

// seekLeechConfig, when set, adjusts the leech's client config (callbacks).
var seekLeechConfig func(*torrent.ClientConfig)

// newSeekSwarm builds the leech and its delayed, rate-limited seeds and
// returns a cold-seek probe.
func newSeekSwarm(t *testing.T, pieceLen, fileLen, cacheBytes int64, peers int) *seekSwarm {
	rng := rand.New(rand.NewSource(1))
	info := metainfo.Info{Name: "movie.mkv", PieceLength: pieceLen, Length: fileLen}
	data := make([]byte, fileLen)
	rng.Read(data)
	for start := int64(0); start < fileLen; start += pieceLen {
		hash := sha1.Sum(data[start:min(start+pieceLen, fileLen)])
		info.Pieces = append(info.Pieces, hash[:]...)
	}
	encoded, err := bencode.Marshal(info)
	if err != nil {
		t.Fatal(err)
	}
	var metadata bytes.Buffer
	if err := (&metainfo.MetaInfo{InfoBytes: encoded}).Write(&metadata); err != nil {
		t.Fatal(err)
	}
	newEngine := func(seed bool, configure func(*torrent.ClientConfig)) (*EngineServer, *TorrentRecord) {
		s := testEngine(t, func(cfg *torrent.ClientConfig) {
			cfg.DisableTCP, cfg.DisableIPv6 = false, true
			cfg.ListenHost = func(string) string { return "127.0.0.1" }
			cfg.ListenPort = 0
			cfg.DialForPeerConns, cfg.AcceptPeerConnections, cfg.Seed = true, true, seed
			cfg.DialRateLimiter = rate.NewLimiter(1000, 1000)
			configure(cfg)
		})
		request, _ := json.Marshal(AddRequest{Prepare: true, TorrentData: base64.StdEncoding.EncodeToString(metadata.Bytes())})
		code, response := s.DispatchControl("POST", "/add", string(request))
		if code != 200 {
			t.Fatalf("add: %s", response)
		}
		var added AddResponse
		_ = json.Unmarshal(response, &added)
		rec := s.records[added.ID]
		config := ConfigureRequest{ID: rec.ID, Selected: []int{0}, Descriptors: []*int{nil}, StorageMode: StorageModeStream, StreamCacheBytes: cacheBytes}
		if !seed && os.Getenv("SEEK_MODE") == "download" {
			// Download playback, as the phone plays without Direct Play.
			f, err := os.CreateTemp(t.TempDir(), "leech")
			if err != nil {
				t.Fatal(err)
			}
			t.Cleanup(func() { f.Close() })
			fd := int(f.Fd())
			config.Descriptors, config.StorageMode = []*int{&fd}, StorageModeDownload
		}
		if seed {
			// Each seed needs its own file: a shared one changes under the
			// others, and they stop trusting their pieces.
			s.storage.SetCacheLimit(16 << 20)
			f, err := os.CreateTemp(t.TempDir(), "seed")
			if err != nil {
				t.Fatal(err)
			}
			t.Cleanup(func() { f.Close() })
			fd := int(f.Fd())
			config.Descriptors, config.StorageMode = []*int{&fd}, StorageModeDownload
		}
		request, _ = json.Marshal(config)
		if code, response = s.DispatchControl("POST", "/configure", string(request)); code != 200 {
			t.Fatalf("configure: %s", response)
		}
		if seed {
			for p := 0; p < rec.Torrent.NumPieces(); p++ {
				writeVerifiedPiece(t, s, rec, data, p)
			}
		}
		return s, rec
	}

	leech, rec := newEngine(false, func(cfg *torrent.ClientConfig) {
		cfg.EstablishedConnsPerTorrent, cfg.HalfOpenConnsPerTorrent, cfg.TotalHalfOpenConns = peers, peers, 2*peers
		if seekLeechConfig != nil {
			seekLeechConfig(cfg)
		}
	})
	if os.Getenv("SEEK_MODE") == "download" {
		// A real film is far from complete when it is seeked; a background
		// download of this small file would finish it within minutes and
		// leave nothing cold. Only playback asks for pieces.
		rec.Torrent.Files()[0].SetPriority(torrent.PiecePriorityNone)
	}
	leech.DispatchControl("POST", "/settings", `{"ghostProMode":true,"claudeSeekMode":true}`)
	speeds := []rate.Limit{40 << 10, 150 << 10, 400 << 10, 1 << 20}
	delays := map[string]time.Duration{}
	peerInfo := map[string]string{}
	p, err := leech.OpenPlayback(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(p.Close)
	leech.client.SetOutgoingTCPConnHook(func(c net.Conn) net.Conn {
		_, port, _ := net.SplitHostPort(c.RemoteAddr().String())
		return newLatencyConn(c, delays[port])
	})
	for i := 0; i < peers; i++ {
		speed := speeds[i%len(speeds)]
		seed, _ := newEngine(true, func(cfg *torrent.ClientConfig) {
			cfg.UploadRateLimiter = rate.NewLimiter(speed, 64<<10)
		})
		_, port, _ := net.SplitHostPort(seed.client.ListenAddrs()[0].String())
		delays[port] = time.Duration(20+rng.Intn(180)) * time.Millisecond // incoming leg only
		peerInfo[port] = fmt.Sprintf("%v@%dK", delays[port], int(speed)>>10)
		rec.Torrent.AddClientPeer(seed.client)
	}
	st := leech.storage.GetStorage(rec.InfoHash)
	readKiB := 64
	if n, err := strconv.Atoi(os.Getenv("SEEK_READ_KB")); err == nil && n > 0 {
		readKiB = n
	}
	buf := make([]byte, readKiB<<10)
	var lastOff int64
	var partial bool
	seek := func() time.Duration {
		// A random point in a piece that is neither in RAM nor verified.
		var off int64
		for tries := 0; ; tries++ {
			piece := rng.Intn(rec.Torrent.NumPieces() - 1)
			st.mu.Lock()
			cold := !st.completed[piece] && st.cache[piece] == nil
			st.mu.Unlock()
			if cold || tries > 200 {
				off = int64(piece)*pieceLen + rng.Int63n(pieceLen-int64(len(buf)))
				break
			}
		}
		lastOff = off
		start := time.Now()
		type readResult struct {
			n   int
			err error
		}
		done := make(chan readResult, 1)
		go func() {
			var n int
			var err error
			if partial {
				n, err = p.ReadSome(buf, off)
			} else {
				n, err = p.ReadAt(buf, off)
			}
			done <- readResult{n, err}
		}()
		select {
		case result := <-done:
			if result.err != nil || result.n <= 0 || (!partial && result.n != len(buf)) {
				t.Fatalf("seek offset=%d partial=%v: read %d/%d bytes: %v", off, partial, result.n, len(buf), result.err)
			}
		case <-time.After(10 * time.Second):
			// Do not score a failed read as a slow success or leave it using
			// buf while the next probe starts.
			p.Close()
			<-done
			t.Fatalf("seek offset=%d: read timed out after 10s", off)
		}
		took := time.Since(start)
		time.Sleep(300 * time.Millisecond) // watch a moment before the next seek
		return took
	}
	deadline := time.Now().Add(120 * time.Second)
	for len(rec.Torrent.PeerConns()) < peers-5 && time.Now().Before(deadline) {
		seek()
	}
	t.Logf("peers connected: %d", len(rec.Torrent.PeerConns()))

	return &seekSwarm{leech: leech, rec: rec, seek: seek, rng: rng, reader: p, lastOff: &lastOff, partial: &partial, peers: peerInfo}
}

func TestSeekRuleSearch(t *testing.T) {
	if os.Getenv("SEEK_SEARCH") != "1" {
		t.Skip("set SEEK_SEARCH=1 to run the seek rule search")
	}
	const (
		generations = 4
		population  = 12
		survivors   = 4
		rounds      = 8
		seeksEach   = 3
	)
	sw := newSeekSwarm(t, 256<<10, 48<<20, 16<<20, 55)
	leech, seek, rng := sw.leech, sw.seek, sw.rng
	shipped := &seekCandidate{lib: torrent.DefaultSeekTuning, stealAfter: claudeSeekStealAfter, nudgeEvery: claudeSeekNudgeEvery, nowExtra: urgentNowExtraPieces}
	evaluate := func(candidates []*seekCandidate, rounds int) {
		for round := 0; round < rounds; round++ {
			for _, i := range rng.Perm(len(candidates)) {
				candidates[i].apply(leech)
				for k := 0; k < seeksEach; k++ {
					candidates[i].samples = append(candidates[i].samples, seek())
				}
			}
		}
		sort.SliceStable(candidates, func(i, j int) bool { return candidates[i].score() < candidates[j].score() })
	}
	report := func(label string, candidates []*seekCandidate) {
		for _, c := range candidates {
			t.Logf("%s score=%v median=%v p90=%v n=%d  %s", label, c.score().Round(time.Millisecond),
				quantile(c.samples, 0.5).Round(time.Millisecond), quantile(c.samples, 0.9).Round(time.Millisecond), len(c.samples), c.key())
		}
	}
	candidates := []*seekCandidate{shipped}
	for len(candidates) < population {
		candidates = append(candidates, randomCandidate(rng))
	}
	for gen := 0; gen < generations; gen++ {
		evaluate(candidates, rounds)
		report("gen"+strconv.Itoa(gen), candidates)
		next := slices.Clone(candidates[:survivors])
		for len(next) < population {
			next = append(next, mutate(rng, next[rng.Intn(survivors)]))
		}
		candidates = next
	}
	// Final: the best three against the shipped rules, scored afresh.
	finalists := []*seekCandidate{}
	for _, c := range candidates[:3] {
		fresh := *c
		fresh.samples = nil
		finalists = append(finalists, &fresh)
	}
	baseline := *shipped
	baseline.samples = nil
	finalists = append(finalists, &baseline)
	evaluate(finalists, 20)
	report("final", finalists)
	shipped.apply(leech)
}

// Seek time against RAM size, with and without the request candidate bound.
// Run with SEEK_SEARCH=cache.
func TestSeekCacheSizeComparison(t *testing.T) {
	if os.Getenv("SEEK_SEARCH") != "cache" {
		t.Skip("set SEEK_SEARCH=cache to compare seek time across RAM sizes")
	}
	sw := newSeekSwarm(t, 256<<10, 192<<20, 16<<20, 16)
	type variant struct {
		cache    int64
		pipeline float64
		samples  []time.Duration
		bytes    int64
		elapsed  time.Duration
	}
	var variants []*variant
	for _, cache := range []int64{16 << 20, 128 << 20} {
		for _, pipeline := range []float64{0, 0.5, 0.25} {
			variants = append(variants, &variant{cache: cache, pipeline: pipeline})
		}
	}
	apply := func(v *variant) {
		tuning := torrent.DefaultSeekTuning
		tuning.PipelineSeconds, tuning.MinPipeline = v.pipeline, 2
		torrent.SetSeekTuning(tuning)
		if err := sw.leech.storage.SetCacheLimit(v.cache); err != nil {
			t.Fatal(err)
		}
		sw.leech.refreshPlaybackWindows(sw.rec.Torrent)
		// Let the window refill at this size before measuring.
		for i := 0; i < 2; i++ {
			sw.seek()
		}
	}
	for round := 0; round < 10; round++ {
		for _, i := range sw.rng.Perm(len(variants)) {
			v := variants[i]
			apply(v)
			stats := sw.rec.Torrent.Stats()
			before, start := stats.BytesReadUsefulData.Int64(), time.Now()
			for k := 0; k < 5; k++ {
				v.samples = append(v.samples, sw.seek())
			}
			after := sw.rec.Torrent.Stats()
			v.bytes += after.BytesReadUsefulData.Int64() - before
			v.elapsed += time.Since(start)
		}
	}
	for _, v := range variants {
		t.Logf("cache=%dMiB pipeline=%.1fs median=%v p90=%v download=%.1f MiB/s n=%d", v.cache>>20, v.pipeline,
			quantile(v.samples, 0.5).Round(time.Millisecond), quantile(v.samples, 0.9).Round(time.Millisecond),
			float64(v.bytes)/v.elapsed.Seconds()/(1<<20), len(v.samples))
	}
	torrent.SetSeekTuning(torrent.DefaultSeekTuning)
}

// TestSeekRefocusComparison measures seeks with and without clearing the old
// request queues, interleaved, at a small and a large cache. Run with
// SEEK_SEARCH=refocus.
func TestSeekRefocusComparison(t *testing.T) {
	if os.Getenv("SEEK_SEARCH") != "refocus" {
		t.Skip("set SEEK_SEARCH=refocus to compare seeks with and without refocus")
	}
	sw := newSeekSwarm(t, 256<<10, 192<<20, 16<<20, 16)
	type variant struct {
		cache   int64
		refocus bool
		samples []time.Duration
	}
	var variants []*variant
	for _, cache := range []int64{16 << 20, 128 << 20} {
		for _, refocus := range []bool{false, true} {
			variants = append(variants, &variant{cache: cache, refocus: refocus})
		}
	}
	for round := 0; round < 10; round++ {
		for _, i := range sw.rng.Perm(len(variants)) {
			v := variants[i]
			refocusOnSeek = v.refocus
			if err := sw.leech.storage.SetCacheLimit(v.cache); err != nil {
				t.Fatal(err)
			}
			sw.leech.refreshPlaybackWindows(sw.rec.Torrent)
			for k := 0; k < 2; k++ {
				sw.seek()
			}
			for k := 0; k < 5; k++ {
				v.samples = append(v.samples, sw.seek())
			}
		}
	}
	refocusOnSeek = true
	for _, v := range variants {
		t.Logf("cache=%dMiB refocus=%v median=%v p90=%v n=%d", v.cache>>20, v.refocus,
			quantile(v.samples, 0.5).Round(time.Millisecond), quantile(v.samples, 0.9).Round(time.Millisecond), len(v.samples))
	}
}

// TestSeekFullPipelineComparison measures seeks and the download rate around
// them with and without FullPipeline, interleaved, at a small and a large
// cache. Run with SEEK_SEARCH=pipeline.
func TestSeekFullPipelineComparison(t *testing.T) {
	if os.Getenv("SEEK_SEARCH") != "pipeline" {
		t.Skip("set SEEK_SEARCH=pipeline to compare seeks with and without FullPipeline")
	}
	sw := newSeekSwarm(t, 256<<10, 192<<20, 16<<20, 16)
	type variant struct {
		cache   int64
		full    bool
		samples []time.Duration
		bytes   int64
		elapsed time.Duration
	}
	var variants []*variant
	for _, cache := range []int64{16 << 20, 128 << 20} {
		for _, full := range []bool{false, true} {
			variants = append(variants, &variant{cache: cache, full: full})
		}
	}
	for round := 0; round < 10; round++ {
		for _, i := range sw.rng.Perm(len(variants)) {
			v := variants[i]
			tuning := torrent.DefaultSeekTuning
			tuning.FullPipeline = v.full
			torrent.SetSeekTuning(tuning)
			if err := sw.leech.storage.SetCacheLimit(v.cache); err != nil {
				t.Fatal(err)
			}
			sw.leech.refreshPlaybackWindows(sw.rec.Torrent)
			for k := 0; k < 2; k++ {
				sw.seek()
			}
			stats := sw.rec.Torrent.Stats()
			before, start := stats.BytesReadUsefulData.Int64(), time.Now()
			for k := 0; k < 5; k++ {
				v.samples = append(v.samples, sw.seek())
			}
			after := sw.rec.Torrent.Stats()
			v.bytes += after.BytesReadUsefulData.Int64() - before
			v.elapsed += time.Since(start)
		}
	}
	torrent.SetSeekTuning(torrent.DefaultSeekTuning)
	for _, v := range variants {
		t.Logf("cache=%dMiB full=%v median=%v p90=%v download=%.2f MiB/s n=%d", v.cache>>20, v.full,
			quantile(v.samples, 0.5).Round(time.Millisecond), quantile(v.samples, 0.9).Round(time.Millisecond),
			float64(v.bytes)/v.elapsed.Seconds()/(1<<20), len(v.samples))
	}
}

// TestSeekPipelineRTTComparison measures seeks and the download rate with the
// round-trip pipeline sizing off and at two multipliers, interleaved, at a
// small and a large cache. Run with SEEK_SEARCH=rtt.
func TestSeekPipelineRTTComparison(t *testing.T) {
	if os.Getenv("SEEK_SEARCH") != "rtt" {
		t.Skip("set SEEK_SEARCH=rtt to compare round-trip pipeline sizing")
	}
	sw := newSeekSwarm(t, 256<<10, 192<<20, 16<<20, 16)
	type variant struct {
		cache   int64
		rtt     float64
		samples []time.Duration
		bytes   int64
		elapsed time.Duration
	}
	var variants []*variant
	for _, cache := range []int64{16 << 20, 128 << 20} {
		for _, rtt := range []float64{0, 2, 4} {
			variants = append(variants, &variant{cache: cache, rtt: rtt})
		}
	}
	for round := 0; round < 10; round++ {
		for _, i := range sw.rng.Perm(len(variants)) {
			v := variants[i]
			tuning := torrent.DefaultSeekTuning
			tuning.PipelineRTT = v.rtt
			torrent.SetSeekTuning(tuning)
			if err := sw.leech.storage.SetCacheLimit(v.cache); err != nil {
				t.Fatal(err)
			}
			sw.leech.refreshPlaybackWindows(sw.rec.Torrent)
			for k := 0; k < 2; k++ {
				sw.seek()
			}
			stats := sw.rec.Torrent.Stats()
			before, start := stats.BytesReadUsefulData.Int64(), time.Now()
			for k := 0; k < 5; k++ {
				v.samples = append(v.samples, sw.seek())
			}
			after := sw.rec.Torrent.Stats()
			v.bytes += after.BytesReadUsefulData.Int64() - before
			v.elapsed += time.Since(start)
		}
	}
	torrent.SetSeekTuning(torrent.DefaultSeekTuning)
	for _, v := range variants {
		t.Logf("cache=%dMiB rtt=%.0f median=%v p90=%v download=%.2f MiB/s n=%d", v.cache>>20, v.rtt,
			quantile(v.samples, 0.5).Round(time.Millisecond), quantile(v.samples, 0.9).Round(time.Millisecond),
			float64(v.bytes)/v.elapsed.Seconds()/(1<<20), len(v.samples))
	}
}

// TestSeekModeComparison turns each playback mode off, one at a time, against
// the built-in combination (Ghost Pro, optimized seek, burst-aware buffer),
// interleaved, at a small and a large cache. SEEK_SEARCH=modes runs it;
// SEEK_MODES picks the variants (comma separated: ghostpro, claude,
// nobuffer, adaptive).
func TestSeekModeComparison(t *testing.T) {
	if os.Getenv("SEEK_SEARCH") != "modes" {
		t.Skip("set SEEK_SEARCH=modes to compare playback modes")
	}
	sw := newSeekSwarm(t, 256<<10, 192<<20, 16<<20, 16)
	p := sw.reader
	type variant struct {
		cache   int64
		name    string
		samples []time.Duration
		bytes   int64
		elapsed time.Duration
	}
	names := append([]string{"best"}, strings.Split(os.Getenv("SEEK_MODES"), ",")...)
	var variants []*variant
	for _, cache := range []int64{16 << 20, 128 << 20} {
		for _, name := range names {
			variants = append(variants, &variant{cache: cache, name: name})
		}
	}
	apply := func(v *variant) {
		ghostPro, claude := v.name != "ghostpro", v.name != "claude"
		body, _ := json.Marshal(map[string]bool{"ghostProMode": ghostPro, "claudeSeekMode": claude})
		if code, response := sw.leech.DispatchControl("POST", "/settings", string(body)); code != 200 {
			t.Fatalf("settings: %s", response)
		}
		p.mu.Lock()
		switch v.name {
		case "nobuffer":
			p.adaptive, p.bufferControl = false, nil
		case "adaptive":
			p.adaptive, p.bufferControl = true, &demandBufferController{}
		default:
			p.adaptive, p.bufferControl = true, &demandBufferController{burstAware: true}
		}
		p.mu.Unlock()
		if err := sw.leech.storage.SetCacheLimit(v.cache); err != nil {
			t.Fatal(err)
		}
		sw.leech.refreshPlaybackWindows(sw.rec.Torrent)
	}
	for round := 0; round < 10; round++ {
		for _, i := range sw.rng.Perm(len(variants)) {
			v := variants[i]
			apply(v)
			for k := 0; k < 2; k++ {
				sw.seek()
			}
			stats := sw.rec.Torrent.Stats()
			before, start := stats.BytesReadUsefulData.Int64(), time.Now()
			for k := 0; k < 5; k++ {
				v.samples = append(v.samples, sw.seek())
			}
			after := sw.rec.Torrent.Stats()
			v.bytes += after.BytesReadUsefulData.Int64() - before
			v.elapsed += time.Since(start)
		}
	}
	for _, v := range variants {
		t.Logf("cache=%dMiB mode=%s median=%v p90=%v download=%.2f MiB/s n=%d", v.cache>>20, v.name,
			quantile(v.samples, 0.5).Round(time.Millisecond), quantile(v.samples, 0.9).Round(time.Millisecond),
			float64(v.bytes)/v.elapsed.Seconds()/(1<<20), len(v.samples))
	}
}

// TestSeekLatencyBreakdown splits each seek into scheduling (until the block
// at the playhead is requested), network (until it arrives) and delivery
// (until the read returns), to find where seek time goes. Run with
// SEEK_SEARCH=breakdown.
func TestSeekLatencyBreakdown(t *testing.T) {
	if os.Getenv("SEEK_SEARCH") != "breakdown" {
		t.Skip("set SEEK_SEARCH=breakdown to break seek time into stages")
	}
	type key struct{ index, begin int }
	var mu sync.Mutex
	sent := map[key]time.Time{}
	sentQueue := map[key]int{}
	sentRate := map[key]float64{}
	recv := map[key]time.Time{}
	seekLeechConfig = func(cfg *torrent.ClientConfig) {
		cfg.Callbacks.SentRequest = append(cfg.Callbacks.SentRequest, func(e torrent.PeerRequestEvent) {
			k := key{int(e.Index), int(e.Begin)}
			mu.Lock()
			if _, ok := sent[k]; !ok {
				sent[k] = time.Now()
				sentQueue[k], sentRate[k] = e.Peer.WebtorRequestInfo()
			}
			mu.Unlock()
		})
		cfg.Callbacks.ReceivedUsefulData = append(cfg.Callbacks.ReceivedUsefulData, func(e torrent.PeerMessageEvent) {
			k := key{int(e.Message.Index), int(e.Message.Begin)}
			mu.Lock()
			if _, ok := recv[k]; !ok {
				recv[k] = time.Now()
			}
			mu.Unlock()
		})
	}
	defer func() { seekLeechConfig = nil }()
	const pieceLen = 256 << 10
	sw := newSeekSwarm(t, pieceLen, 192<<20, 128<<20, 16)
	var total, sched, network, deliver []time.Duration
	var queues []int
	var rates []float64
	for i := 0; i < 60; i++ {
		mu.Lock()
		clear(sent)
		clear(recv)
		mu.Unlock()
		start := time.Now()
		took := sw.seek()
		off := *sw.lastOff
		k := key{int(off / pieceLen), int(off % pieceLen / (16 << 10) * (16 << 10))}
		mu.Lock()
		s, sok := sent[k]
		r, rok := recv[k]
		q, rate := sentQueue[k], sentRate[k]
		mu.Unlock()
		if !sok || !rok {
			continue
		}
		total = append(total, took)
		sched = append(sched, s.Sub(start))
		network = append(network, r.Sub(s))
		deliver = append(deliver, start.Add(took).Sub(r))
		queues = append(queues, q)
		rates = append(rates, rate)
	}
	q := func(d []time.Duration, f float64) time.Duration { return quantile(d, f).Round(time.Millisecond) }
	t.Logf("n=%d total median=%v p90=%v", len(total), q(total, 0.5), q(total, 0.9))
	t.Logf("schedule (seek -> playhead block requested) median=%v p90=%v", q(sched, 0.5), q(sched, 0.9))
	t.Logf("network  (requested -> block arrived)     median=%v p90=%v", q(network, 0.5), q(network, 0.9))
	t.Logf("deliver  (arrived -> read returned)       median=%v p90=%v", q(deliver, 0.5), q(deliver, 0.9))
	slices.Sort(queues)
	slices.Sort(rates)
	if n := len(queues); n > 0 {
		t.Logf("peer chosen for the playhead block: queue median=%d p90=%d, rate median=%.0f KiB/s p10=%.0f KiB/s",
			queues[n/2], queues[n*9/10], rates[n/2]/1024, rates[n/10]/1024)
	}
}

// TestSeekArrivalComparison compares the shipped rules with arrival-aware
// peer choice and with HTTP-style partial reads, interleaved, at a small and
// a large cache. Run with SEEK_SEARCH=arrival.
func TestSeekArrivalComparison(t *testing.T) {
	if os.Getenv("SEEK_SEARCH") != "arrival" {
		t.Skip("set SEEK_SEARCH=arrival to compare arrival-aware peer choice and partial reads")
	}
	sw := newSeekSwarm(t, 256<<10, 192<<20, 16<<20, 16)
	type variant struct {
		cache            int64
		arrival, partial bool
		samples          []time.Duration
	}
	var variants []*variant
	for _, cache := range []int64{16 << 20, 128 << 20} {
		for _, arrival := range []bool{false, true} {
			for _, partial := range []bool{false, true} {
				variants = append(variants, &variant{cache: cache, arrival: arrival, partial: partial})
			}
		}
	}
	for round := 0; round < 10; round++ {
		for _, i := range sw.rng.Perm(len(variants)) {
			v := variants[i]
			tuning := torrent.DefaultSeekTuning
			tuning.ArrivalAware, tuning.ArrivalRatio = v.arrival, 1.5
			torrent.SetSeekTuning(tuning)
			*sw.partial = v.partial
			if err := sw.leech.storage.SetCacheLimit(v.cache); err != nil {
				t.Fatal(err)
			}
			sw.leech.refreshPlaybackWindows(sw.rec.Torrent)
			for k := 0; k < 2; k++ {
				sw.seek()
			}
			for k := 0; k < 5; k++ {
				v.samples = append(v.samples, sw.seek())
			}
		}
	}
	torrent.SetSeekTuning(torrent.DefaultSeekTuning)
	*sw.partial = false
	for _, v := range variants {
		t.Logf("cache=%dMiB arrival=%v partial=%v median=%v p90=%v n=%d", v.cache>>20, v.arrival, v.partial,
			quantile(v.samples, 0.5).Round(time.Millisecond), quantile(v.samples, 0.9).Round(time.Millisecond), len(v.samples))
	}
}

// TestSeekTuningComparison runs named seek tuning variants interleaved on one
// swarm. SEEK_TUNE lists them, separated by ";", each "name:Field=value,..."
// over DefaultSeekTuning ("partial=1" reads like HTTP). SEEK_ROUNDS sets the
// rounds (5 seeks per variant each) and SEEK_CACHE the RAM sizes in MiB.
// Run with SEEK_SEARCH=tune.
func TestSeekTuningComparison(t *testing.T) {
	if os.Getenv("SEEK_SEARCH") != "tune" {
		t.Skip("set SEEK_SEARCH=tune to compare seek tuning variants")
	}
	type variant struct {
		name          string
		cache         int64
		tuning        torrent.SeekTuning
		partial       bool
		nudge         time.Duration
		samples       []time.Duration
		bytes, wasted int64
		elapsed       time.Duration
	}
	caches := []int64{16, 128}
	if env := os.Getenv("SEEK_CACHE"); env != "" {
		caches = nil
		for _, f := range strings.Split(env, ",") {
			n, _ := strconv.ParseInt(f, 10, 64)
			caches = append(caches, n)
		}
	}
	rounds := 20
	if n, err := strconv.Atoi(os.Getenv("SEEK_ROUNDS")); err == nil {
		rounds = n
	}
	var variants []*variant
	for _, spec := range strings.Split(os.Getenv("SEEK_TUNE"), ";") {
		name, tuning, partial, nudge := parseSeekTune(t, spec)
		for _, c := range caches {
			variants = append(variants, &variant{name: name, cache: c << 20, tuning: tuning, partial: partial, nudge: nudge})
		}
	}
	sw := newSeekSwarm(t, 256<<10, 192<<20, 16<<20, 16)
	defer torrent.SetSeekTuning(torrent.DefaultSeekTuning)
	defer func(nudge time.Duration) { claudeSeekNudgeEvery = nudge }(claudeSeekNudgeEvery)
	if os.Getenv("SEEK_THROUGHPUT") == "1" {
		// Sustained download: read 12 MiB from a cold spot as fast as the
		// swarm delivers it.
		const span = 12 << 20
		buf := make([]byte, 256<<10)
		rates := make([][]float64, len(variants))
		for round := 0; round < rounds; round++ {
			for _, i := range sw.rng.Perm(len(variants)) {
				v := variants[i]
				torrent.SetSeekTuning(v.tuning)
				claudeSeekNudgeEvery = v.nudge
				if err := sw.leech.storage.SetCacheLimit(v.cache); err != nil {
					t.Fatal(err)
				}
				sw.leech.refreshPlaybackWindows(sw.rec.Torrent)
				// A span with no piece in RAM or verified.
				st := sw.leech.storage.GetStorage(sw.rec.InfoHash)
				var off int64
				for tries := 0; ; tries++ {
					off = sw.rng.Int63n(192<<20-span) / (256 << 10) * (256 << 10)
					cold := true
					st.mu.Lock()
					for p := int(off / (256 << 10)); p < int((off+span)/(256<<10)); p++ {
						// Download playback keeps verified pieces in its file.
						if st.completed[p] || st.cache[p] != nil || sw.rec.Torrent.PieceState(p).Complete {
							cold = false
							break
						}
					}
					st.mu.Unlock()
					if cold || tries > 500 {
						break
					}
				}
				start := time.Now()
				for pos := off; pos < off+span; pos += int64(len(buf)) {
					if _, err := sw.reader.ReadAt(buf, pos); err != nil {
						t.Fatal(err)
					}
				}
				rates[i] = append(rates[i], span/time.Since(start).Seconds()/(1<<20))
			}
		}
		for i, v := range variants {
			slices.Sort(rates[i])
			var sum float64
			for _, r := range rates[i] {
				sum += r
			}
			t.Logf("cache=%dMiB %-10s throughput mean=%.2f median=%.2f MiB/s n=%d", v.cache>>20, v.name,
				sum/float64(len(rates[i])), rates[i][len(rates[i])/2], len(rates[i]))
		}
		return
	}
	for round := 0; round < rounds; round++ {
		for _, i := range sw.rng.Perm(len(variants)) {
			v := variants[i]
			torrent.SetSeekTuning(v.tuning)
			claudeSeekNudgeEvery = v.nudge
			*sw.partial = v.partial
			if err := sw.leech.storage.SetCacheLimit(v.cache); err != nil {
				t.Fatal(err)
			}
			sw.leech.refreshPlaybackWindows(sw.rec.Torrent)
			for k := 0; k < 2; k++ {
				sw.seek()
			}
			before, start := sw.rec.Torrent.Stats(), time.Now()
			for k := 0; k < 5; k++ {
				v.samples = append(v.samples, sw.seek())
			}
			after := sw.rec.Torrent.Stats()
			v.bytes += after.BytesReadUsefulData.Int64() - before.BytesReadUsefulData.Int64()
			v.wasted += (after.ChunksReadWasted.Int64() - before.ChunksReadWasted.Int64()) * 16 << 10
			v.elapsed += time.Since(start)
		}
	}
	for _, v := range variants {
		t.Logf("cache=%dMiB %-10s median=%v p75=%v p90=%v download=%.2f MiB/s wasted=%.1f%% n=%d", v.cache>>20, v.name,
			quantile(v.samples, 0.5).Round(time.Millisecond), quantile(v.samples, 0.75).Round(time.Millisecond),
			quantile(v.samples, 0.9).Round(time.Millisecond), float64(v.bytes)/v.elapsed.Seconds()/(1<<20),
			100*float64(v.wasted)/float64(max(v.bytes, 1)), len(v.samples))
	}
}

// parseSeekTune reads one SEEK_TUNE variant, "name:Field=val,...", over
// DefaultSeekTuning. The pseudo-fields partial=1 and nudge=ms set the read mode
// and claudeSeekNudgeEvery.
func parseSeekTune(t *testing.T, spec string) (name string, tuning torrent.SeekTuning, partial bool, nudge time.Duration) {
	name, fields, _ := strings.Cut(spec, ":")
	tuning, nudge = torrent.DefaultSeekTuning, claudeSeekNudgeEvery
	for _, kv := range strings.Split(fields, ",") {
		k, v, ok := strings.Cut(kv, "=")
		if !ok {
			continue
		}
		if k == "partial" {
			partial = v == "1"
			continue
		}
		if k == "nudge" {
			ms, _ := strconv.Atoi(v)
			nudge = time.Duration(ms) * time.Millisecond
			continue
		}
		f := reflect.ValueOf(&tuning).Elem().FieldByName(k)
		if !f.IsValid() {
			t.Fatalf("no tuning field %q", k)
		}
		switch f.Kind() {
		case reflect.Bool:
			f.SetBool(v == "1" || v == "true")
		case reflect.Int:
			n, _ := strconv.Atoi(v)
			f.SetInt(int64(n))
		case reflect.Float64:
			x, _ := strconv.ParseFloat(v, 64)
			f.SetFloat(x)
		}
	}
	return
}
