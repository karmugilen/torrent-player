package main

import (
	"bytes"
	"crypto/sha1"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"github.com/anacrolix/torrent"
	"github.com/anacrolix/torrent/bencode"
	"github.com/anacrolix/torrent/metainfo"
	"golang.org/x/time/rate"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestDirectPlayLocalPeerSeeksBeyondRAMBudget(t *testing.T) {
	t.Run("default", func(t *testing.T) { testDirectPlayLocalPeerSeeksBeyondRAMBudget(t, playbackExperimentTestConfig{}) })
	t.Run("adaptive buffer", func(t *testing.T) {
		testDirectPlayLocalPeerSeeksBeyondRAMBudget(t, playbackExperimentTestConfig{adaptive: true})
	})
	t.Run("burst-aware buffer", func(t *testing.T) {
		testDirectPlayLocalPeerSeeksBeyondRAMBudget(t, playbackExperimentTestConfig{burst: true})
	})
	t.Run("combined", func(t *testing.T) {
		testDirectPlayLocalPeerSeeksBeyondRAMBudget(t, playbackExperimentTestConfig{adaptive: true, burst: true})
	})
	t.Run("all four", func(t *testing.T) {
		testDirectPlayLocalPeerSeeksBeyondRAMBudget(t, playbackExperimentTestConfig{adaptive: true, burst: true, boost: true, explore: true})
	})
	t.Run("ghost mode", func(t *testing.T) {
		testDirectPlayLocalPeerSeeksBeyondRAMBudget(t, playbackExperimentTestConfig{ghost: true})
	})
	t.Run("all five", func(t *testing.T) {
		testDirectPlayLocalPeerSeeksBeyondRAMBudget(t, playbackExperimentTestConfig{adaptive: true, burst: true, boost: true, explore: true, ghost: true})
	})
	t.Run("ghost pro", func(t *testing.T) {
		testDirectPlayLocalPeerSeeksBeyondRAMBudget(t, playbackExperimentTestConfig{ghostPro: true})
	})
	t.Run("ghost pro boosted burst buffer", func(t *testing.T) {
		testDirectPlayLocalPeerSeeksBeyondRAMBudget(t, playbackExperimentTestConfig{ghostPro: true, boost: true, burst: true})
	})
	t.Run("claude seek", func(t *testing.T) {
		testDirectPlayLocalPeerSeeksBeyondRAMBudget(t, playbackExperimentTestConfig{claude: true})
	})
	t.Run("claude seek with every experiment", func(t *testing.T) {
		testDirectPlayLocalPeerSeeksBeyondRAMBudget(t, playbackExperimentTestConfig{claude: true, ghostPro: true, boost: true, burst: true, adaptive: true})
	})
}

type playbackExperimentTestConfig struct{ adaptive, burst, boost, explore, ghost, ghostPro, claude bool }

// localPeerSwarm builds a 40 MiB single-file torrent with 64 KiB pieces and
// returns its payload and a factory for loopback seed or Direct Play peers.
func localPeerSwarm(t *testing.T) ([]byte, func(seed bool, configure ...func(*torrent.ClientConfig)) (*EngineServer, *TorrentRecord)) {
	t.Helper()
	return localFilesSwarm(t, "movie.mp4", nil)
}

// localFilesSwarm builds seed/leech engines for one torrent. Nil lengths make a
// single 40 MiB file named name; otherwise name is the folder of numbered files.
// The returned data is the whole torrent payload.
func localFilesSwarm(t *testing.T, name string, lengths []int64) ([]byte, func(seed bool, configure ...func(*torrent.ClientConfig)) (*EngineServer, *TorrentRecord)) {
	t.Helper()
	data, newPeer, _ := localFilesSwarmModes(t, name, lengths)
	return data, newPeer
}

// localFilesSwarmModes also returns a constructor for a leech saving to files.
func localFilesSwarmModes(t *testing.T, name string, lengths []int64) (
	[]byte,
	func(seed bool, configure ...func(*torrent.ClientConfig)) (*EngineServer, *TorrentRecord),
	func(configure ...func(*torrent.ClientConfig)) (*EngineServer, *TorrentRecord),
) {
	t.Helper()
	total := int64(40 << 20)
	info := metainfo.Info{Name: name, PieceLength: 64 << 10}
	if lengths == nil {
		info.Length = total
	} else {
		total = 0
		for i, length := range lengths {
			info.Files = append(info.Files, metainfo.FileInfo{Length: length, Path: []string{fmt.Sprintf("Episode %02d.mkv", i+1)}})
			total += length
		}
	}
	data := make([]byte, total)
	for i := range data {
		data[i] = byte(i*19 + i/(64<<10))
	}
	for start := 0; start < len(data); start += int(info.PieceLength) {
		hash := sha1.Sum(data[start:min(start+int(info.PieceLength), len(data))])
		info.Pieces = append(info.Pieces, hash[:]...)
	}
	files := max(1, len(lengths))
	encoded, err := bencode.Marshal(info)
	if err != nil {
		t.Fatal(err)
	}
	var metadata bytes.Buffer
	mi := metainfo.MetaInfo{InfoBytes: encoded}
	if err := mi.Write(&metadata); err != nil {
		t.Fatal(err)
	}
	newPeerMode := func(seed, download bool, configure ...func(*torrent.ClientConfig)) (*EngineServer, *TorrentRecord) {
		s := testEngine(t, append([]func(*torrent.ClientConfig){func(cfg *torrent.ClientConfig) {
			cfg.DisableTCP = false
			cfg.DisableIPv6 = true
			cfg.ListenHost = func(string) string { return "127.0.0.1" }
			cfg.ListenPort = 0
			cfg.DialForPeerConns = true
			cfg.AcceptPeerConnections = true
			cfg.Seed = seed
			cfg.PieceHashersPerTorrent = backgroundWorkers()
			cfg.DialRateLimiter = rate.NewLimiter(defaultDialRate, defaultDialRate)
		}}, configure...)...)
		request, _ := json.Marshal(AddRequest{Prepare: true, TorrentData: base64.StdEncoding.EncodeToString(metadata.Bytes())})
		code, response := s.DispatchControl("POST", "/add", string(request))
		if code != 200 {
			t.Fatalf("add: %s", response)
		}
		var added AddResponse
		if err := json.Unmarshal(response, &added); err != nil {
			t.Fatal(err)
		}
		rec := s.records[added.ID]
		config := ConfigureRequest{ID: rec.ID, Descriptors: make([]*int, files), StorageMode: StorageModeStream, StreamCacheBytes: 16 << 20}
		for i := 0; i < files; i++ {
			config.Selected = append(config.Selected, i)
		}
		if seed || download {
			for i := range config.Descriptors {
				f, err := os.CreateTemp(t.TempDir(), "seed")
				if err != nil {
					t.Fatal(err)
				}
				defer f.Close()
				fd := int(f.Fd())
				config.Descriptors[i] = &fd
			}
			config.StorageMode = StorageModeDownload
		}
		request, _ = json.Marshal(config)
		code, response = s.DispatchControl("POST", "/configure", string(request))
		if code != 200 {
			t.Fatalf("configure: %s", response)
		}
		if seed {
			for p := 0; p < rec.Torrent.NumPieces(); p++ {
				writeVerifiedPiece(t, s, rec, data, p)
			}
		}
		return s, rec
	}
	newPeer := func(seed bool, configure ...func(*torrent.ClientConfig)) (*EngineServer, *TorrentRecord) {
		return newPeerMode(seed, false, configure...)
	}
	newDownloader := func(configure ...func(*torrent.ClientConfig)) (*EngineServer, *TorrentRecord) {
		return newPeerMode(false, true, configure...)
	}
	return data, newPeer, newDownloader
}

func testDirectPlayLocalPeerSeeksBeyondRAMBudget(t *testing.T, experiment playbackExperimentTestConfig) {
	if testing.Short() {
		t.Skip("local peer transfer")
	}
	data, newPeer := localPeerSwarm(t)
	seed, _ := newPeer(true)
	leech, rec := newPeer(false)
	if priority := rec.Torrent.Files()[0].Priority(); priority != torrent.PiecePriorityNone {
		t.Fatalf("direct play scheduled full file: %v", priority)
	}
	if rec.Torrent.AddClientPeer(seed.client) == 0 {
		t.Fatal("no seed address")
	}
	if experiment.explore {
		if code, response := leech.DispatchControl("POST", "/settings", `{"peerExploration":true}`); code != 200 {
			t.Fatalf("peer exploration: %d %s", code, response)
		}
	}
	if experiment.ghost {
		if code, response := leech.DispatchControl("POST", "/settings", `{"ghostMode":true}`); code != 200 {
			t.Fatalf("ghost mode: %d %s", code, response)
		}
	}
	if experiment.ghostPro {
		if code, response := leech.DispatchControl("POST", "/settings", `{"ghostProMode":true}`); code != 200 {
			t.Fatalf("ghost pro mode: %d %s", code, response)
		}
	}
	if experiment.claude {
		if code, response := leech.DispatchControl("POST", "/settings", `{"claudeSeekMode":true}`); code != 200 {
			t.Fatalf("claude seek mode: %d %s", code, response)
		}
	}
	if experiment.adaptive || experiment.burst || experiment.boost {
		body, _ := json.Marshal(map[string]any{"id": rec.ID, "fileIndex": 0, "adaptiveBuffer": experiment.adaptive, "burstAwareBuffer": experiment.burst, "boost": experiment.boost})
		if code, response := leech.DispatchControl("POST", "/play", string(body)); code != 200 {
			t.Fatalf("adaptive play: %d %s", code, response)
		}
	}
	p, err := leech.OpenPlayback(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer p.Close()
	for iteration, offset := range []int64{0, 25 << 20, 4 << 20, 32 << 20, 4 << 20} {
		if experiment.ghost {
			leech.ghostModeTick(time.Unix(1000, 0).Add(time.Duration(iteration) * 10 * time.Second))
		}
		if experiment.ghostPro {
			leech.ghostProModeTick(time.Unix(1000, 0).Add(time.Duration(iteration) * 10 * time.Second))
		}
		if experiment.explore {
			leech.peerExplorationTick(time.Unix(1000, 0).Add(time.Duration(iteration) * 90 * time.Second))
		}
		got := make([]byte, 8<<20)
		done := make(chan error, 1)
		go func() {
			n, err := p.ReadAt(got, offset)
			if err == nil && n != len(got) {
				err = os.ErrInvalid
			}
			done <- err
		}()
		select {
		case err := <-done:
			if err != nil {
				t.Fatalf("read at %d: %v", offset, err)
			}
		case <-time.After(20 * time.Second):
			p.mu.Lock()
			position := p.lastOffset
			p.mu.Unlock()
			st := leech.storage.GetStorage(rec.InfoHash)
			st.mu.Lock()
			t.Logf("read at %d stalled at %d: cache=%d/%d complete=%d lost=%d whole=%d demand=%d", offset, position, st.cacheBytes, st.maxCacheBytes, len(st.completed), len(st.streamLost), len(st.streamWhole), len(st.streamDemand))
			st.mu.Unlock()
			var status bytes.Buffer
			leech.client.WriteStatus(&status)
			t.Log(status.String())
			p.Close()
			t.Fatal("direct play local-peer read stalled")
		}
		if !bytes.Equal(got, data[offset:offset+int64(len(got))]) {
			t.Fatalf("incorrect bytes after seek to %d", offset)
		}
		st := leech.storage.GetStorage(rec.InfoHash)
		st.mu.Lock()
		used, limit := st.cacheBytes, st.maxCacheBytes
		st.mu.Unlock()
		if used > limit {
			t.Fatalf("RAM limit exceeded: %d > %d", used, limit)
		}
	}
	if stats := leech.seekStats.snapshot(); stats.Count < 3 {
		t.Fatalf("seek stats missed seeks: %+v", stats)
	}
	_, raw := leech.DispatchControl("GET", "/torrent/"+rec.ID, "")
	var view TorrentView
	if err := json.Unmarshal(raw, &view); err != nil {
		t.Fatal(err)
	}
	if view.Done || view.TimeRemaining != nil {
		t.Fatal("direct cache reported offline completion/ETA")
	}
	base := leech.storage.baseDir
	if err := filepath.WalkDir(base, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if !d.IsDir() {
			rel, _ := filepath.Rel(base, path)
			if !bytes.HasPrefix([]byte(rel), []byte(".startup/")) {
				t.Errorf("unexpected saved video backing: %s", rel)
			}
		}
		return nil
	}); err != nil {
		t.Fatal(err)
	}
}

func TestIdleReaderParksAndResumesTheSameDescriptor(t *testing.T) {
	s, rec, data := playbackFixture(t)
	for index := 0; index < rec.Torrent.NumPieces(); index++ {
		writeVerifiedPiece(t, s, rec, data, index)
	}
	p, err := s.OpenPlayback(rec.ID, 1)
	if err != nil {
		t.Fatal(err)
	}
	defer p.Close()
	p.mu.Lock()
	p.inFlight = 1
	p.mu.Unlock()
	p.parkIdle()
	p.mu.Lock()
	if p.reader == nil {
		t.Fatal("active I/O parked")
	}
	p.inFlight = 0
	p.mu.Unlock()
	p.parkIdle()
	p.mu.Lock()
	parked := p.reader == nil
	p.mu.Unlock()
	if !parked {
		t.Fatal("idle reader still running")
	}
	p.priorityOwner.mu.Lock()
	_, owns := p.priorityOwner.owners[p]
	p.priorityOwner.mu.Unlock()
	if owns {
		t.Fatal("idle probe retained explicit demand")
	}
	got := make([]byte, 10)
	if _, err := p.ReadAt(got, 2); err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(got, data[7:17]) {
		t.Fatal("resumed reader lost file offset")
	}
}

func TestStalledReadRetriesWithoutReturningZerosOrDroppingPeers(t *testing.T) {
	for _, direct := range []bool{false, true} {
		name := "Download"
		if direct {
			name = "DirectPlay"
		}
		t.Run(name, func(t *testing.T) { testStalledRead(t, direct) })
	}
}

func testStalledRead(t *testing.T, direct bool) {
	s, rec, data := playbackFixtureMode(t, direct)
	p, err := s.OpenPlayback(rec.ID, 1)
	if err != nil {
		t.Fatal(err)
	}
	defer p.Close()
	p.readAttempts = []time.Duration{15 * time.Millisecond, 15 * time.Millisecond, time.Second}
	got := make([]byte, 10)
	done := make(chan error, 1)
	go func() { _, err := p.ReadAt(got, 0); done <- err }()
	time.Sleep(60 * time.Millisecond)
	for index := 0; index < rec.Torrent.NumPieces(); index++ {
		writeVerifiedPiece(t, s, rec, data, index)
	}
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("retry did not recover")
	}
	if !bytes.Equal(got, data[5:15]) {
		t.Fatal("recovery returned wrong bytes")
	}
	p.mu.Lock()
	recoveries := p.stallRecoveries
	p.mu.Unlock()
	if recoveries < 1 || recoveries > 2 {
		t.Fatalf("retry count: %d", recoveries)
	}
}

func TestDirectPlayCancelsWaitOnSeekAndClose(t *testing.T) {
	s, rec, data := playbackFixtureMode(t, true)
	lastPiece := rec.Torrent.NumPieces() - 1
	writeVerifiedPiece(t, s, rec, data, lastPiece)
	p, err := s.OpenPlayback(rec.ID, 1)
	if err != nil {
		t.Fatal(err)
	}
	defer p.Close()
	p.readAttempts = []time.Duration{5 * time.Second}
	waiting := func() {
		t.Helper()
		deadline := time.Now().Add(time.Second)
		for time.Now().Before(deadline) {
			p.mu.Lock()
			blocked := p.waitCancel != nil
			p.mu.Unlock()
			if blocked {
				return
			}
			time.Sleep(time.Millisecond)
		}
		t.Fatal("read did not enter cancellable wait")
	}
	startRead := func() chan error {
		done := make(chan error, 1)
		go func() { _, err := p.ReadAt(make([]byte, 1), 0); done <- err }()
		waiting()
		return done
	}
	first := startRead()
	got := make([]byte, 1)
	if _, err := p.ReadAt(got, p.length-1); err != nil || got[0] != data[len(data)-1] {
		t.Fatalf("seek failed to supersede blocked read: %x, %v", got, err)
	}
	select {
	case err := <-first:
		if err == nil {
			t.Fatal("superseded read succeeded")
		}
	case <-time.After(time.Second):
		t.Fatal("seek did not cancel old wait")
	}
	second := startRead()
	closed := make(chan struct{})
	go func() { p.Close(); close(closed) }()
	select {
	case <-closed:
	case <-time.After(time.Second):
		t.Fatal("close did not unwind verified-piece wait")
	}
	if err := <-second; err == nil {
		t.Fatal("closed read succeeded")
	}
}

// One throttled and one fast seed: the throttled seed can hold urgent blocks
// after a seek. Both modes must return correct bytes; timings are logged so
// the effect of Claude optimized seek is visible with -v.
func TestClaudeSeekWithSlowAndFastSeed(t *testing.T) {
	if testing.Short() {
		t.Skip("local peer transfer")
	}
	for _, claude := range []bool{false, true} {
		t.Run(map[bool]string{false: "default", true: "claude seek"}[claude], func(t *testing.T) {
			data, newPeer := localPeerSwarm(t)
			slow, _ := newPeer(true, func(cfg *torrent.ClientConfig) {
				cfg.UploadRateLimiter = rate.NewLimiter(48<<10, 16<<10)
			})
			fast, _ := newPeer(true)
			leech, rec := newPeer(false)
			if claude {
				if code, response := leech.DispatchControl("POST", "/settings", `{"claudeSeekMode":true}`); code != 200 {
					t.Fatalf("claude seek mode: %d %s", code, response)
				}
			}
			// The slow seed connects first so it is offered the opening requests.
			if rec.Torrent.AddClientPeer(slow.client) == 0 {
				t.Fatal("no slow seed address")
			}
			time.Sleep(300 * time.Millisecond)
			if rec.Torrent.AddClientPeer(fast.client) == 0 {
				t.Fatal("no fast seed address")
			}
			p, err := leech.OpenPlayback(rec.ID, 0)
			if err != nil {
				t.Fatal(err)
			}
			defer p.Close()
			start := time.Now()
			for _, offset := range []int64{0, 20 << 20, 6 << 20, 33 << 20, 12 << 20, 27 << 20} {
				got := make([]byte, 1<<20)
				began := time.Now()
				done := make(chan error, 1)
				go func() {
					n, err := p.ReadAt(got, offset)
					if err == nil && n != len(got) {
						err = os.ErrInvalid
					}
					done <- err
				}()
				select {
				case err := <-done:
					if err != nil {
						t.Fatalf("read at %d: %v", offset, err)
					}
				case <-time.After(60 * time.Second):
					p.Close()
					t.Fatalf("read at %d stalled", offset)
				}
				if !bytes.Equal(got, data[offset:offset+int64(len(got))]) {
					t.Fatalf("incorrect bytes after seek to %d", offset)
				}
				t.Logf("read at %2d MiB: %v", offset>>20, time.Since(began).Round(time.Millisecond))
			}
			t.Logf("total %v, stats %+v", time.Since(start).Round(time.Millisecond), leech.seekStats.snapshot())
		})
	}
}

func TestClaudeSeekSettingTogglesStealingAndResetsStats(t *testing.T) {
	s := testEngine(t)
	s.seekStats.recordSeek(1500 * time.Millisecond)
	s.seekStats.recordSeek(500 * time.Millisecond)
	s.seekStats.recordSeek(900 * time.Millisecond)
	s.seekStats.recordStall()
	var before SettingsResponse
	_, raw := s.DispatchControl("GET", "/settings", "")
	if err := json.Unmarshal(raw, &before); err != nil {
		t.Fatal(err)
	}
	if before.ClaudeSeekMode || before.SeekStats != (SeekStats{Count: 3, LastMs: 900, MedianMs: 900, Stalls: 1}) {
		t.Fatalf("unexpected settings before toggle: %+v", before)
	}
	if code, response := s.DispatchControl("POST", "/settings", `{"claudeSeekMode":true}`); code != 200 {
		t.Fatalf("enable: %d %s", code, response)
	}
	var after SettingsResponse
	_, raw = s.DispatchControl("GET", "/settings", "")
	if err := json.Unmarshal(raw, &after); err != nil {
		t.Fatal(err)
	}
	if !after.ClaudeSeekMode || after.SeekStats != (SeekStats{}) {
		t.Fatalf("toggle did not enable and reset: %+v", after)
	}
	// Unrelated settings leave the mode and its comparison window alone.
	s.seekStats.recordSeek(time.Second)
	if code, response := s.DispatchControl("POST", "/settings", `{"maxPeers":40}`); code != 200 {
		t.Fatalf("max peers: %d %s", code, response)
	}
	if !s.isClaudeSeekEnabled() || s.seekStats.snapshot().Count != 1 {
		t.Fatal("unrelated setting changed claude seek state")
	}
	if code, response := s.DispatchControl("POST", "/settings", `{"claudeSeekMode":false}`); code != 200 {
		t.Fatalf("disable: %d %s", code, response)
	}
	if s.isClaudeSeekEnabled() || s.seekStats.snapshot().Count != 0 {
		t.Fatal("disable did not turn off and reset")
	}
}
