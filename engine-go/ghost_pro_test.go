package main

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"os"
	"testing"
	"time"

	"github.com/anacrolix/torrent"
	"github.com/anacrolix/torrent/bencode"
	"github.com/anacrolix/torrent/metainfo"
	"golang.org/x/time/rate"
)

// Sparse metadata-only fixture covers large pieces without allocating video data.
func ghostProPlaybackFixture(t *testing.T, pieceLen int64, pieces int, direct bool) (*EngineServer, *TorrentRecord) {
	t.Helper()
	s := testEngine(t)
	info := metainfo.Info{Name: "4k.mkv", Length: pieceLen * int64(pieces), PieceLength: pieceLen, Pieces: make([]byte, 20*pieces)}
	encoded, err := bencode.Marshal(info)
	if err != nil {
		t.Fatal(err)
	}
	var metadata bytes.Buffer
	mi := metainfo.MetaInfo{InfoBytes: encoded}
	if err := mi.Write(&metadata); err != nil {
		t.Fatal(err)
	}
	body, _ := json.Marshal(AddRequest{Prepare: true, TorrentData: base64.StdEncoding.EncodeToString(metadata.Bytes())})
	code, response := s.DispatchControl("POST", "/add", string(body))
	if code != 200 {
		t.Fatalf("add: %d %s", code, response)
	}
	var added AddResponse
	if err := json.Unmarshal(response, &added); err != nil {
		t.Fatal(err)
	}
	f, err := os.CreateTemp(t.TempDir(), "video")
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	fd := int(f.Fd())
	config := ConfigureRequest{ID: added.ID, Selected: []int{0}, Descriptors: []*int{&fd}}
	if direct {
		config.StorageMode, config.StreamCacheBytes, config.Descriptors = StorageModeStream, 64<<20, []*int{nil}
	}
	body, _ = json.Marshal(config)
	if code, response = s.DispatchControl("POST", "/configure", string(body)); code != 200 {
		t.Fatalf("configure: %d %s", code, response)
	}
	if code, response = s.DispatchControl("POST", "/settings", `{"ghostProMode":true}`); code != 200 {
		t.Fatalf("settings: %d %s", code, response)
	}
	return s, s.records[added.ID]
}

func TestGhostProPlaybackStartupSeekAndScaledWindows(t *testing.T) {
	for _, tc := range []struct {
		name                       string
		pieceLen                   int64
		boost                      bool
		urgent, forward, lookahead int64
	}{
		{"standard", 1 << 20, false, 16 << 20, 64 << 20, 32 << 20},
		{"boost", 1 << 20, true, 16 << 20, 96 << 20, 32 << 20},
		{"large pieces", 16 << 20, false, 32 << 20, 96 << 20, 64 << 20},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s, rec := ghostProPlaybackFixture(t, tc.pieceLen, 256, false)
			s.ensurePlaybackCoordinator(rec.Torrent).setPlayBoost(0, tc.boost)
			p, err := s.OpenPlayback(rec.ID, 0)
			if err != nil {
				t.Fatal(err)
			}
			defer p.Close()
			if p.readerReadahead > playbackUrgentBytes || !p.readahead.empty() || !p.lookahead.empty() {
				t.Fatalf("startup must stay narrow: %+v reader=%d", p.appliedWindow, p.readerReadahead)
			}
			offset := 32 * tc.pieceLen
			p.testApplyOffset(offset)
			// Seeking prioritizes the first request before broadening urgency.
			if p.urgent != spanFromFile(p.file, offset, offset+playbackUrgentBytes) {
				t.Fatalf("seek urgency=%v", p.urgent)
			}
			p.mu.Lock()
			p.finishFocusLocked()
			p.mu.Unlock()
			if p.urgent != spanFromFile(p.file, offset, offset+tc.urgent) ||
				p.readahead != spanFromFile(p.file, offset+tc.urgent, offset+tc.forward) ||
				p.lookahead != spanFromFile(p.file, offset+tc.forward, offset+tc.forward+tc.lookahead) {
				t.Fatalf("scaled windows: %+v", p.appliedWindow)
			}
			// Only the pieces holding the playhead's 2 MiB are Now; every
			// block after them is Next, fetched in order.
			nowEnd := min(p.urgent.end, p.urgent.begin+int((playbackUrgentBytes+tc.pieceLen-1)/tc.pieceLen)+urgentNowExtraPieces)
			for _, tier := range []struct {
				span     pieceSpan
				priority torrent.PiecePriority
			}{
				{pieceSpan{p.urgent.begin, nowEnd}, torrent.PiecePriorityNow}, {pieceSpan{nowEnd, p.urgent.end}, torrent.PiecePriorityNext},
				{p.readahead, torrent.PiecePriorityNext}, {p.lookahead, torrent.PiecePriorityNext},
			} {
				for i := tier.span.begin; i < tier.span.end; i++ {
					if got := rawPiecePriority(rec.Torrent.Piece(i)); got != tier.priority {
						t.Fatalf("piece %d priority %v want %v", i, got, tier.priority)
					}
				}
			}
			oldUrgent := p.urgent.begin
			p.testApplyOffset(180 * tc.pieceLen)
			if rawPiecePriority(rec.Torrent.Piece(oldUrgent)) >= torrent.PiecePriorityReadahead {
				t.Fatal("seek left stale high-priority demand")
			}
			p.testApplyOffset(p.length - 1)
			if p.urgent.end != rec.Torrent.NumPieces() || !p.readahead.empty() || !p.lookahead.empty() {
				t.Fatalf("EOF windows %+v", p.appliedWindow)
			}
		})
	}
}

func TestGhostProInstantPlayKeepsAllTiersWithinRAMAllowance(t *testing.T) {
	s, rec := ghostProPlaybackFixture(t, 1<<20, 256, true)
	const cachePieces = 64 // 64 MiB cache of 1 MiB pieces
	span := func(p *playbackReader) int {
		return p.lookahead.end - p.urgent.begin
	}
	readers := make([]*playbackReader, 0, 4)
	for i := 0; i < 4; i++ {
		p, err := s.OpenPlayback(rec.ID, 0)
		if err != nil {
			t.Fatal(err)
		}
		defer p.Close()
		readers = append(readers, p)
		// Every open reader recomputes its window on its next read.
		for j, r := range readers {
			r.testApplyOffset(int64(32+j*60) << 20)
			r.mu.Lock()
			r.finishFocusLocked()
			r.mu.Unlock()
		}
		total := 0
		for _, r := range readers {
			if r.urgent.empty() || r.readahead.empty() || r.lookahead.empty() {
				t.Fatalf("cache should hold all tiers: %+v", r.appliedWindow)
			}
			total += span(r)
		}
		if total > cachePieces {
			t.Fatalf("%d readers need %d pieces of a %d-piece cache", len(readers), total, cachePieces)
		}
		// A lone reader fills the cache ahead of its playhead.
		if len(readers) == 1 && total < cachePieces*90/100 {
			t.Fatalf("single reader uses only %d of %d pieces: %+v", total, cachePieces, p.appliedWindow)
		}
	}
	if len(s.playbackWindows(rec.Torrent)) != len(readers) {
		t.Fatal("missing reader windows")
	}
}

func windowPieces(p *playbackReader) int {
	spans := []pieceSpan{p.urgent, p.readahead, p.lookahead}
	begin, end := 0, 0
	for _, span := range spans {
		if span.empty() {
			continue
		}
		if begin == end || span.begin < begin {
			begin = span.begin
		}
		if span.end > end {
			end = span.end
		}
	}
	return end - begin
}

// Short jumps stay inside the RAM window. A jump to another part of the file
// moves that window, and a second viewer shares the cache with the one already playing.
func TestDirectPlaySeekIsSmoothForwardBackwardAndMiddle(t *testing.T) {
	s, rec := ghostProPlaybackFixture(t, 1<<20, 256, true)
	st := s.storage.GetStorage(rec.InfoHash)
	p, err := s.OpenPlayback(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer p.Close()
	const piece = int64(1 << 20)
	middle := 128 * piece
	p.testApplyOffset(middle)
	p.mu.Lock()
	p.finishFocusLocked()
	p.mu.Unlock()
	st.mu.Lock()
	behind := st.streamDemand[int((middle-1)/piece)]
	st.mu.Unlock()
	if behind || p.urgent.begin != int(middle/piece) {
		t.Fatalf("window must start at the playhead with nothing behind it: %+v", p.appliedWindow)
	}
	p.testApplyOffset(middle - piece)
	p.mu.Lock()
	p.finishFocusLocked()
	p.mu.Unlock()
	if !p.urgent.contains(int((middle - piece) / piece)) {
		t.Fatalf("one-piece rewind did not move the playhead: %+v", p.urgent)
	}
	far := middle + 80*piece
	p.testApplyOffset(far)
	p.mu.Lock()
	p.finishFocusLocked()
	p.mu.Unlock()
	if p.urgent.contains(int(middle/piece)) || !p.urgent.contains(int(far/piece)) {
		t.Fatalf("jump did not move the playhead: %+v", p.urgent)
	}

	alone := windowPieces(p)
	other, err := s.OpenPlayback(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer other.Close()
	other.testApplyOffset(40 * piece)
	other.mu.Lock()
	other.finishFocusLocked()
	other.mu.Unlock()
	deadline := time.Now().Add(2 * time.Second)
	for {
		total := windowPieces(p) + windowPieces(other)
		if total <= 64 && windowPieces(p) < alone && windowPieces(p) > 0 && windowPieces(other) > 0 {
			return
		}
		if time.Now().After(deadline) {
			t.Fatalf("two viewers pinned %d pieces (playing %d, second %d) of 64", total, windowPieces(p), windowPieces(other))
		}
		time.Sleep(10 * time.Millisecond)
	}
}

func TestDirectPlayLargePiecesStayInsideTheCache(t *testing.T) {
	s, rec := ghostProPlaybackFixture(t, 16<<20, 16, true)
	open := func(offset int64) *playbackReader {
		t.Helper()
		p, err := s.OpenPlayback(rec.ID, 0)
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { p.Close() })
		p.testApplyOffset(offset)
		p.mu.Lock()
		p.finishFocusLocked()
		p.mu.Unlock()
		return p
	}
	first := open(16<<20 + 1)
	second := open(16<<20 + 1)
	deadline := time.Now().Add(2 * time.Second)
	for {
		total := windowPieces(first) + windowPieces(second)
		if total <= 4 && total > 0 {
			return
		}
		if time.Now().After(deadline) {
			t.Fatalf("16 MiB pieces pinned %d of 4 cache pieces", total)
		}
		time.Sleep(10 * time.Millisecond)
	}
}

func TestGhostProFirstVerifiedReadExpandsUrgency(t *testing.T) {
	s, rec, _, data := configuredLargeVideo(t, 48)
	if code, response := s.DispatchControl("POST", "/settings", `{"ghostProMode":true}`); code != 200 {
		t.Fatalf("settings: %d %s", code, response)
	}
	writeVerifiedPiece(t, s, rec, data, 10)
	p, err := s.OpenPlayback(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer p.Close()
	if n, err := p.ReadAt(make([]byte, 4), 10<<20); err != nil || n != 4 {
		t.Fatalf("verified read: %d %v", n, err)
	}
	if p.focusPending || p.urgent != (pieceSpan{10, 26}) {
		t.Fatalf("first bytes did not expand steady urgency: %+v", p.appliedWindow)
	}
}

func TestGhostProPlaybackTelemetryMovesClearsAndToggles(t *testing.T) {
	s, rec := ghostProPlaybackFixture(t, 1<<20, 256, false)
	p, err := s.OpenPlayback(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer p.Close()
	p.testApplyOffset(32 << 20)
	p.mu.Lock()
	p.finishFocusLocked()
	p.mu.Unlock()
	read := func() PieceTelemetryResponse {
		t.Helper()
		code, body := s.DispatchControl("GET", "/pieces/"+rec.ID+"?maxBuckets=256", "")
		if code != 200 {
			t.Fatalf("pieces: %d %s", code, body)
		}
		var response PieceTelemetryResponse
		if err := json.Unmarshal(body, &response); err != nil {
			t.Fatal(err)
		}
		return response
	}
	response := read()
	if !response.GhostProMode || len(response.Playback) != 2 {
		t.Fatalf("missing tier telemetry: %+v", response.Playback)
	}
	// Only the playhead's 2 MiB is this block (U); every block after it is N.
	for i, want := range []PlaybackPieceRange{{32, 33, "U"}, {34, 127, "N"}} {
		if response.Playback[i] != want {
			t.Fatalf("range %d: %+v want %+v", i, response.Playback[i], want)
		}
	}
	if code, body := s.DispatchControl("POST", "/settings", `{"ghostProMode":false}`); code != 200 {
		t.Fatalf("disable: %d %s", code, body)
	}
	// Without Ghost Pro the normal, shorter window is still shown.
	response = read()
	if response.GhostProMode || !p.lookahead.empty() || p.urgent != (pieceSpan{32, 34}) {
		t.Fatal("disable did not restore normal window")
	}
	if len(response.Playback) != 2 || response.Playback[0] != (PlaybackPieceRange{32, 33, "U"}) || response.Playback[1].End >= 127 {
		t.Fatalf("normal window telemetry: %+v", response.Playback)
	}
	if code, body := s.DispatchControl("POST", "/settings", `{"ghostProMode":true}`); code != 200 {
		t.Fatalf("enable: %d %s", code, body)
	}
	p.parkIdle()
	if len(read().Playback) != 0 {
		t.Fatal("parked reader still published demand")
	}
	p.testApplyOffset(100 << 20)
	p.Close()
	if len(read().Playback) != 0 {
		t.Fatal("closed reader still published demand")
	}
}

func TestContiguousWantedPiecesExcludesReaderGapsAndCompletedPieces(t *testing.T) {
	windows := []playbackWindowState{
		{urgent: pieceSpan{10, 12}, readahead: pieceSpan{12, 14}, lookahead: pieceSpan{14, 16}},
		{urgent: pieceSpan{100, 102}},
	}
	wanted := map[int]bool{8: true, 10: true, 11: true, 13: true, 15: true, 50: true, 100: true, 101: true}
	got := contiguousWantedPieces(windows, wanted)
	if len(got) != 6 || got[8] || got[12] || got[50] || !got[100] || !got[15] {
		t.Fatalf("contiguous wanted=%v", got)
	}
}

func TestGhostProPrioritizesIdlePeersWithoutPlaybackPathPieces(t *testing.T) {
	now := time.Unix(1000, 0)
	outside, onPath, active, seeder := &torrent.PeerConn{}, &torrent.PeerConn{}, &torrent.PeerConn{}, &torrent.PeerConn{}
	peers := []ghostProPeerObservation{
		{peer: onPath, overlapPieces: 1, contiguousOverlap: 1},
		{peer: active, overlapPieces: 50},
		{peer: seeder, overlapPieces: 100, isSeeder: true},
		{peer: outside, overlapPieces: 80},
	}
	for _, tc := range []struct {
		name              string
		playback          bool
		capacity, pending int
		want              *torrent.PeerConn
	}{
		{"playback path", true, 4, 1, outside}, {"background", false, 4, 1, nil},
		{"spare slots", true, 5, 1, nil}, {"no candidates", true, 4, 0, nil},
	} {
		t.Run(tc.name, func(t *testing.T) {
			state := &ghostProSwarmState{}
			obs := append([]ghostProPeerObservation(nil), peers...)
			state.chooseVictim(now, obs, tc.capacity, tc.pending, tc.playback)
			obs[1].useful++
			if got := state.chooseVictim(now.Add(20*time.Second), obs, tc.capacity, tc.pending, tc.playback); got != tc.want {
				t.Fatalf("victim=%p want %p", got, tc.want)
			}
		})
	}
}

func TestGhostProChoosesZeroOverlapIdleVictimAndProtectsSeeders(t *testing.T) {
	state := &ghostProSwarmState{}
	now := time.Unix(1000, 0)

	p1 := &torrent.PeerConn{} // 0-overlap leecher, idle
	p2 := &torrent.PeerConn{} // active leecher with overlap
	p3 := &torrent.PeerConn{} // seeder with 100% pieces, idle

	// First sample establishes baseline idle time.
	obs1 := []ghostProPeerObservation{
		{peer: p1, useful: 1000, uploaded: 500, overlapPieces: 0, isSeeder: false},
		{peer: p2, useful: 2000, uploaded: 1000, overlapPieces: 5, isSeeder: false},
		{peer: p3, useful: 5000, uploaded: 2000, overlapPieces: 10, isSeeder: true},
	}
	if victim := state.chooseVictim(now, obs1, 3, 5, false); victim != nil {
		t.Fatalf("unexpected victim on first sample: %v", victim)
	}

	// 10 seconds later: p1 still idle (10s), p2 transferred data.
	t10 := now.Add(10 * time.Second)
	obs2 := []ghostProPeerObservation{
		{peer: p1, useful: 1000, uploaded: 500, overlapPieces: 0, isSeeder: false},
		{peer: p2, useful: 2500, uploaded: 1200, overlapPieces: 5, isSeeder: false},
		{peer: p3, useful: 5000, uploaded: 2000, overlapPieces: 10, isSeeder: true},
	}
	// Idle time is 10s < 15s threshold, so no eviction yet.
	if victim := state.chooseVictim(t10, obs2, 3, 5, false); victim != nil {
		t.Fatalf("expected nil victim before 15s threshold, got: %v", victim)
	}

	// 20 seconds later: p1 idle for 20s >= 15s with 0 overlap. Should be chosen!
	t20 := now.Add(20 * time.Second)
	obs3 := []ghostProPeerObservation{
		{peer: p1, useful: 1000, uploaded: 500, overlapPieces: 0, isSeeder: false},
		{peer: p2, useful: 3000, uploaded: 1500, overlapPieces: 5, isSeeder: false},
		{peer: p3, useful: 5000, uploaded: 2000, overlapPieces: 10, isSeeder: true},
	}
	victim := state.chooseVictim(t20, obs3, 3, 5, false)
	if victim != p1 {
		t.Fatalf("expected p1 (0-overlap idle) as victim, got: %v", victim)
	}

	// Within 20s rotation cooldown, no new victim even if another candidate exists.
	t25 := now.Add(25 * time.Second)
	p4 := &torrent.PeerConn{}
	obs4 := []ghostProPeerObservation{
		{peer: p4, useful: 500, uploaded: 200, overlapPieces: 0, isSeeder: false},
		{peer: p2, useful: 3500, uploaded: 1800, overlapPieces: 5, isSeeder: false},
		{peer: p3, useful: 5000, uploaded: 2000, overlapPieces: 10, isSeeder: true},
	}
	if v := state.chooseVictim(t25, obs4, 3, 5, false); v != nil {
		t.Fatalf("expected cooldown protection at t25, got: %v", v)
	}
}

func TestGhostProMutualExclusivityWithGhostModeAndPeerExploration(t *testing.T) {
	s := testEngine(t)

	request := func(method, body string) SettingsResponse {
		t.Helper()
		code, response := s.DispatchControl(method, "/settings", body)
		if code != 200 {
			t.Fatalf("%s /settings: code %d, body %s", method, code, response)
		}
		var resp SettingsResponse
		if err := json.Unmarshal(response, &resp); err != nil {
			t.Fatalf("unmarshal response: %v", err)
		}
		return resp
	}

	// Initial default state
	initial := request("GET", "")
	if initial.GhostMode || initial.GhostProMode || initial.PeerExploration {
		t.Fatalf("expected all off initially: %+v", initial)
	}

	// 1. Turn on Ghost Mode and Peer Exploration first
	resp1 := request("POST", `{"ghostMode":true,"peerExploration":true}`)
	if !resp1.GhostMode || !resp1.PeerExploration || resp1.GhostProMode {
		t.Fatalf("expected ghostMode and peerExploration on: %+v", resp1)
	}

	// 2. Turning on Ghost Pro mode MUST automatically turn off Ghost Mode and Peer Exploration
	resp2 := request("POST", `{"ghostProMode":true}`)
	if !resp2.GhostProMode {
		t.Fatalf("expected ghostProMode to be true: %+v", resp2)
	}
	if resp2.GhostMode {
		t.Fatalf("expected ghostMode to be automatically turned OFF when ghostProMode is enabled: %+v", resp2)
	}
	if resp2.PeerExploration {
		t.Fatalf("expected peerExploration to be automatically turned OFF when ghostProMode is enabled: %+v", resp2)
	}

	// 3. GET verify persistence
	respGet := request("GET", "")
	if !respGet.GhostProMode || respGet.GhostMode || respGet.PeerExploration {
		t.Fatalf("GET /settings mismatch: %+v", respGet)
	}

	// 4. Turning on Ghost Mode turns OFF Ghost Pro mode
	resp3 := request("POST", `{"ghostMode":true}`)
	if !resp3.GhostMode || resp3.GhostProMode {
		t.Fatalf("expected ghostMode ON and ghostProMode OFF: %+v", resp3)
	}

	// 5. Turning on Ghost Pro mode again
	resp4 := request("POST", `{"ghostProMode":true}`)
	if !resp4.GhostProMode || resp4.GhostMode {
		t.Fatalf("expected ghostProMode ON and ghostMode OFF: %+v", resp4)
	}

	// 6. Turning on Peer Exploration turns OFF Ghost Pro mode
	resp5 := request("POST", `{"peerExploration":true}`)
	if !resp5.PeerExploration || resp5.GhostProMode {
		t.Fatalf("expected peerExploration ON and ghostProMode OFF: %+v", resp5)
	}

	// 7. Turning off Ghost Pro mode directly
	request("POST", `{"ghostProMode":true}`)
	respOff := request("POST", `{"ghostProMode":false}`)
	if respOff.GhostProMode {
		t.Fatalf("expected ghostProMode OFF: %+v", respOff)
	}
}

func TestGhostProCanToggleWhileSamplerRuns(t *testing.T) {
	s, _, _ := playbackFixtureMode(t, true)
	s.dialLimiter = rate.NewLimiter(defaultDialRate, defaultDialRate)
	s.maxPeers = 55
	done := make(chan struct{})
	go func() {
		defer close(done)
		for i := 0; i < 200; i++ {
			s.ghostProModeTick(time.Unix(1000+int64(i), 0))
		}
	}()
	for i := 0; i < 20; i++ {
		if code, response := s.DispatchControl("POST", "/settings", `{"ghostProMode":true}`); code != 200 {
			t.Fatalf("enable: %d %s", code, response)
		}
		if code, response := s.DispatchControl("POST", "/settings", `{"ghostProMode":false}`); code != 200 {
			t.Fatalf("disable: %d %s", code, response)
		}
	}
	<-done
	if got := limiterRate(s.dialLimiter); got != defaultDialRate {
		t.Fatalf("sampler overwrote disabled base rate: %d", got)
	}
}
