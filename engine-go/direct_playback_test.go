package main

import (
	"bytes"
	"crypto/sha1"
	"encoding/base64"
	"encoding/json"
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
}

type playbackExperimentTestConfig struct{ adaptive, burst, boost, explore, ghost bool }

func testDirectPlayLocalPeerSeeksBeyondRAMBudget(t *testing.T, experiment playbackExperimentTestConfig) {
	if testing.Short() {
		t.Skip("local peer transfer")
	}
	data := make([]byte, 40<<20)
	for i := range data {
		data[i] = byte(i*19 + i/(64<<10))
	}
	info := metainfo.Info{Name: "movie.mp4", Length: int64(len(data)), PieceLength: 64 << 10}
	for start := 0; start < len(data); start += int(info.PieceLength) {
		hash := sha1.Sum(data[start : start+int(info.PieceLength)])
		info.Pieces = append(info.Pieces, hash[:]...)
	}
	encoded, err := bencode.Marshal(info)
	if err != nil {
		t.Fatal(err)
	}
	var metadata bytes.Buffer
	mi := metainfo.MetaInfo{InfoBytes: encoded}
	if err := mi.Write(&metadata); err != nil {
		t.Fatal(err)
	}
	newPeer := func(seed bool) (*EngineServer, *TorrentRecord) {
		s := testEngine(t, func(cfg *torrent.ClientConfig) {
			cfg.DisableTCP = false
			cfg.DisableIPv6 = true
			cfg.ListenHost = func(string) string { return "127.0.0.1" }
			cfg.ListenPort = 0
			cfg.DialForPeerConns = true
			cfg.AcceptPeerConnections = true
			cfg.Seed = seed
			cfg.PieceHashersPerTorrent = backgroundWorkers()
			cfg.DialRateLimiter = rate.NewLimiter(defaultDialRate, defaultDialRate)
		})
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
		config := ConfigureRequest{ID: rec.ID, Selected: []int{0}, Descriptors: []*int{nil}, StorageMode: StorageModeStream, StreamCacheBytes: 16 << 20}
		if seed {
			f, err := os.CreateTemp(t.TempDir(), "seed")
			if err != nil {
				t.Fatal(err)
			}
			defer f.Close()
			fd := int(f.Fd())
			config.Descriptors = []*int{&fd}
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
