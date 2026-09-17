package main

import (
	"bytes"
	"context"
	"crypto/sha1"
	"encoding/base64"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"

	"github.com/anacrolix/torrent"
	"github.com/anacrolix/torrent/bencode"
	"github.com/anacrolix/torrent/metainfo"
)

func playbackFixture(t *testing.T) (*EngineServer, *TorrentRecord, []byte) {
	t.Helper()
	s := testEngine(t)
	// Two files share a piece; the movie also spans several complete pieces.
	data := []byte("note!\x00\x00\x00\x18ftypmp42ABCDEFGHIJKLMN0123456789")
	info := metainfo.Info{Name: "media", PieceLength: 8, Files: []metainfo.FileInfo{
		{Length: 5, Path: []string{"readme.txt"}},
		{Length: int64(len(data) - 5), Path: []string{"clip.mp4"}},
	}}
	for start := 0; start < len(data); start += 8 {
		hash := sha1.Sum(data[start:min(start+8, len(data))])
		info.Pieces = append(info.Pieces, hash[:]...)
	}
	encoded, err := bencode.Marshal(info)
	if err != nil {
		t.Fatal(err)
	}
	mi := metainfo.MetaInfo{InfoBytes: encoded}
	var metadata bytes.Buffer
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
	rec := s.records[added.ID]
	f, err := os.CreateTemp(t.TempDir(), "clip")
	if err != nil {
		t.Fatal(err)
	}
	fd := int(f.Fd())
	body, _ = json.Marshal(ConfigureRequest{ID: added.ID, Descriptors: []*int{nil, &fd}, Selected: []int{1}})
	code, response = s.DispatchControl("POST", "/configure", string(body))
	_ = f.Close() // Only Go's duplicated descriptor remains alive.
	if code != 200 {
		t.Fatalf("configure: %d %s", code, response)
	}
	return s, rec, data
}

func writeVerifiedPiece(t *testing.T, s *EngineServer, rec *TorrentRecord, data []byte, index int) {
	t.Helper()
	info := rec.Torrent.Info()
	start := int64(index) * info.PieceLength
	end := min(start+info.PieceLength, int64(len(data)))
	st := s.storage.GetStorage(rec.InfoHash)
	p := &documentPiece{storage: st, piece: info.Piece(index)}
	if _, err := p.WriteAt(data[start:end], 0); err != nil {
		t.Fatal(err)
	}
	// Flush write-back to disk before verify so restore-verify hole detection
	// and peer hashing both see the same bytes.
	st.mu.Lock()
	if err := st.flushPieceLocked(index); err != nil {
		st.mu.Unlock()
		t.Fatal(err)
	}
	st.mu.Unlock()
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	if err := rec.Torrent.Piece(index).VerifyDataContext(ctx); err != nil {
		t.Fatal(err)
	}
	// VerifyDataContext finishes hashing before the asynchronous completion
	// marker necessarily publishes its state to readers.
	for !rec.Torrent.Piece(index).State().Complete {
		select {
		case <-ctx.Done():
			t.Fatalf("piece %d did not verify: %+v", index, rec.Torrent.Piece(index).State())
		case <-time.After(time.Millisecond):
		}
	}
}

func TestPlaybackWaitsForVerificationThenSupportsSeeking(t *testing.T) {
	s, rec, data := playbackFixture(t)
	reader, err := s.OpenPlayback(rec.ID, 1)
	if err != nil {
		t.Fatal(err)
	}
	defer reader.Close()
	result := make(chan []byte, 1)
	failure := make(chan error, 1)
	go func() {
		buf := make([]byte, 6)
		_, err := reader.ReadAt(buf, 0)
		if err != nil {
			failure <- err
			return
		}
		result <- buf
	}()
	select {
	case got := <-result:
		t.Fatalf("returned unverified bytes: %x", got)
	case err := <-failure:
		t.Fatal(err)
	case <-time.After(100 * time.Millisecond):
	}
	before := s.statusFingerprint()
	for index := 0; index < rec.Torrent.NumPieces(); index++ {
		writeVerifiedPiece(t, s, rec, data, index)
	}
	select {
	case got := <-result:
		if !bytes.Equal(got, data[5:11]) {
			t.Fatalf("wrong file offset: %x", got)
		}
	case err := <-failure:
		t.Fatal(err)
	case <-time.After(3 * time.Second):
		t.Fatal("verified read did not wake")
	}
	if before == s.statusFingerprint() {
		t.Fatal("final verification was not observable")
	}
	want := data[5:]
	for _, offset := range []int64{int64(len(want) - 4), 2, int64(len(want)), 0} {
		buf := make([]byte, 17)
		n, err := reader.ReadAt(buf, offset)
		if err != nil {
			t.Fatal(err)
		}
		expected := want[offset:min(offset+17, int64(len(want)))]
		if !bytes.Equal(buf[:n], expected) {
			t.Fatalf("seek %d: got %x want %x", offset, buf[:n], expected)
		}
	}
}

func largePlaybackFixture(t *testing.T, pieces int) (*EngineServer, *TorrentRecord, int64) {
	t.Helper()
	s := testEngine(t)
	pieceLen := int64(1024 * 1024)
	total := int64(pieces) * pieceLen
	data := bytes.Repeat([]byte("abcdefgh"), int(total/8))
	info := metainfo.Info{Name: "movie.mp4", Length: total, PieceLength: pieceLen}
	for start := int64(0); start < total; start += pieceLen {
		hash := sha1.Sum(data[start : start+pieceLen])
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
	body, _ := json.Marshal(AddRequest{Prepare: true, TorrentData: base64.StdEncoding.EncodeToString(metadata.Bytes())})
	code, response := s.DispatchControl("POST", "/add", string(body))
	if code != 200 {
		t.Fatalf("add: %d %s", code, response)
	}
	var added AddResponse
	if err := json.Unmarshal(response, &added); err != nil {
		t.Fatal(err)
	}
	rec := s.records[added.ID]
	f, err := os.CreateTemp(t.TempDir(), "movie")
	if err != nil {
		t.Fatal(err)
	}
	fd := int(f.Fd())
	body, _ = json.Marshal(ConfigureRequest{ID: added.ID, Descriptors: []*int{&fd}, Selected: []int{0}})
	code, response = s.DispatchControl("POST", "/configure", string(body))
	_ = f.Close()
	if code != 200 {
		t.Fatalf("configure: %d %s", code, response)
	}
	playBody, _ := json.Marshal(PlayRequest{ID: added.ID, FileIndex: intPtr(0)})
	code, response = s.DispatchControl("POST", "/play", string(playBody))
	if code != 200 {
		t.Fatalf("play: %d %s", code, response)
	}
	return s, rec, pieceLen
}

func TestPlaybackWindowFollowsSeekAndDemotesOldNow(t *testing.T) {
	s, rec, pieceLen := largePlaybackFixture(t, 48)
	reader, err := s.OpenPlayback(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer reader.Close()
	if got := reader.testPiecePriority(0); got != torrent.PiecePriorityNow {
		t.Fatalf("open should prioritize playhead tip, got %v", got)
	}

	mid := 10 * pieceLen // large jump → fast-seek: tip Now, short Readahead
	reader.testApplyOffset(mid)
	urgent, readahead, lookbehind, hotspots, fastSeek, headBoost, tailBoost := reader.testWindow()
	if urgent.begin != 10 || urgent.end != 12 ||
		readahead.begin != 12 || readahead.end != 18 ||
		lookbehind.begin != 9 || lookbehind.end != 10 ||
		!fastSeek || hotspots < 1 || headBoost || tailBoost {
		t.Fatalf("urgent=%v readahead=%v lookbehind=%v hotspots=%d fast=%v head=%v tail=%v",
			urgent, readahead, lookbehind, hotspots, fastSeek, headBoost, tailBoost)
	}
	if got := reader.testPiecePriority(10); got != torrent.PiecePriorityNow {
		t.Fatalf("tip should be Now, got %v", got)
	}
	if got := reader.testPiecePriority(15); got != torrent.PiecePriorityReadahead {
		t.Fatalf("forward should be Readahead, got %v", got)
	}
	if got := reader.testPiecePriority(0); got != torrent.PiecePriorityHigh {
		// Prior playhead kept as hotspot High, not Now.
		t.Fatalf("old tip should be hotspot High, got %v", got)
	}
	if got := reader.testPiecePriority(4); got != torrent.PiecePriorityNormal {
		t.Fatalf("old head boost outside hotspot should be Normal, got %v", got)
	}
	_ = rec
}

func TestPlaybackFastSeekSettlesThenExpands(t *testing.T) {
	s, rec, pieceLen := largePlaybackFixture(t, 64)
	reader, err := s.OpenPlayback(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer reader.Close()

	start := 20 * pieceLen
	reader.testApplyOffset(start)
	urgent, readahead, _, _, fastSeek, _, _ := reader.testWindow()
	if !fastSeek || urgent.begin != 20 || readahead.end-readahead.begin != 6 {
		t.Fatalf("expected fast window, urgent=%v readahead=%v fast=%v", urgent, readahead, fastSeek)
	}
	for i := 0; i < playbackSeekSettleReads; i++ {
		reader.testApplyOffset(start + int64(i+1)*pieceLen/2)
	}
	urgent, readahead, _, _, fastSeek, _, _ = reader.testWindow()
	if fastSeek {
		t.Fatal("expected settle out of fast-seek")
	}
	if readahead.end <= readahead.begin || (readahead.end-urgent.begin) < 20 {
		t.Fatalf("settled readahead too small: urgent=%v readahead=%v", urgent, readahead)
	}
}

func TestPlaybackHandlesSharePriorityOwnership(t *testing.T) {
	s, rec, pieceLen := largePlaybackFixture(t, 48)
	first, err := s.OpenPlayback(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	second, err := s.OpenPlayback(rec.ID, 0)
	if err != nil {
		first.Close()
		t.Fatal(err)
	}
	first.testApplyOffset(0)
	second.testApplyOffset(40 * pieceLen)
	second.priorityOwner.mu.Lock()
	gotSecond := second.priorityOwner.effective[40]
	second.priorityOwner.mu.Unlock()
	if gotSecond != torrent.PiecePriorityNow {
		t.Fatalf("second handle did not publish urgent demand: %v", gotSecond)
	}
	second.Close()
	second.priorityOwner.mu.Lock()
	gotSecond = second.priorityOwner.effective[40]
	gotFirst := second.priorityOwner.effective[0]
	second.priorityOwner.mu.Unlock()
	if gotSecond != 0 {
		t.Fatalf("closing second handle retained stale priority: %v", gotSecond)
	}
	if gotFirst != torrent.PiecePriorityNow {
		t.Fatalf("closing second handle erased first handle demand: %v", gotFirst)
	}
	first.Close()
	second.priorityOwner.mu.Lock()
	gotFinal := second.priorityOwner.effective[0]
	second.priorityOwner.mu.Unlock()
	if gotFinal != 0 {
		t.Fatalf("closing final handle retained priority: %v", gotFinal)
	}
}

func TestPlaybackSeekHotspotsWarmPriorTimestamps(t *testing.T) {
	s, rec, pieceLen := largePlaybackFixture(t, 64)
	reader, err := s.OpenPlayback(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer reader.Close()

	positions := []int64{5 * pieceLen, 25 * pieceLen, 45 * pieceLen}
	for _, off := range positions {
		reader.testApplyOffset(off)
	}
	_, _, _, hotspots, _, _, _ := reader.testWindow()
	if hotspots < 2 {
		t.Fatalf("expected seek hotspots for prior positions, got %d", hotspots)
	}
	if got := reader.testPiecePriority(5); got != torrent.PiecePriorityHigh {
		t.Fatalf("prior timestamp hotspot should stay High, got %v", got)
	}
}

func TestFilePieceRangeAndOverlapHelpers(t *testing.T) {
	s, rec, _ := playbackFixture(t)
	file := rec.Torrent.Files()[1]
	begin, end := filePieceRange(file, 0, 8)
	if begin != file.BeginPieceIndex() || end <= begin {
		t.Fatalf("range %d-%d for file pieces %d-%d", begin, end, file.BeginPieceIndex(), file.EndPieceIndex())
	}
	_ = s
	if !rangesOverlap(0, 5, 4, 8) || rangesOverlap(0, 3, 3, 5) {
		t.Fatal("rangesOverlap helper incorrect")
	}
}

func intPtr(v int) *int { return &v }

func TestPlaybackCloseCancelsBlockedRead(t *testing.T) {
	s, rec, _ := playbackFixture(t)
	reader, err := s.OpenPlayback(rec.ID, 1)
	if err != nil {
		t.Fatal(err)
	}
	done := make(chan error, 1)
	go func() { _, err := reader.ReadAt(make([]byte, 8), 0); done <- err }()
	reader.Close()
	select {
	case err := <-done:
		if err == nil {
			t.Fatal("closed read succeeded")
		}
	case <-time.After(time.Second):
		t.Fatal("close did not cancel the blocked read")
	}
}

func TestCompletedAndStreamingHTTPRanges(t *testing.T) {
	s, rec, data := playbackFixture(t)
	// The first two pieces are playable while the rest is still downloading.
	writeVerifiedPiece(t, s, rec, data, 0)
	writeVerifiedPiece(t, s, rec, data, 1)
	server := httptest.NewServer(http.HandlerFunc(s.handleStream))
	defer server.Close()
	client := server.Client()
	client.Timeout = 3 * time.Second
	url := server.URL + "/torrent/" + rec.ID + "/file/1"
	check := func(method, requestRange string, code int, want []byte) {
		t.Helper()
		req, _ := http.NewRequest(method, url, nil)
		if requestRange != "" {
			req.Header.Set("Range", requestRange)
		}
		resp, err := client.Do(req)
		if err != nil {
			t.Fatal(err)
		}
		defer resp.Body.Close()
		got, err := io.ReadAll(resp.Body)
		if err != nil {
			t.Fatal(err)
		}
		if resp.StatusCode != code {
			t.Fatalf("%s %s: %d %q", method, requestRange, resp.StatusCode, got)
		}
		if code < 400 && !bytes.Equal(got, want) {
			t.Fatalf("%s: got %x want %x", requestRange, got, want)
		}
	}
	check("HEAD", "", 200, nil)
	check("GET", "bytes=0-5", 206, data[5:11])
	for i := 2; i < rec.Torrent.NumPieces(); i++ {
		writeVerifiedPiece(t, s, rec, data, i)
	}
	check("GET", "bytes=-7", 206, data[len(data)-7:])
	check("GET", "", 200, data[5:])
	check("GET", "bytes=999-", 416, nil)
	code, body := s.DispatchControl("GET", "/torrent/"+rec.ID, "")
	var status TorrentView
	if err := json.Unmarshal(body, &status); err != nil {
		t.Fatal(err)
	}
	if code != 200 || !status.Done || status.Files[1].VerifiedBytes != status.Files[1].Length {
		t.Fatalf("not completed: %s", body)
	}
}

func TestTorrentStatusDoneUsesVerifiedNotOnlyLiveProgress(t *testing.T) {
	s, rec, data := playbackFixture(t)
	writeVerifiedPiece(t, s, rec, data, 0)
	code, body := s.DispatchControl("GET", "/torrent/"+rec.ID, "")
	var status TorrentView
	if err := json.Unmarshal(body, &status); err != nil {
		t.Fatal(err)
	}
	if code != 200 {
		t.Fatalf("status %d: %s", code, body)
	}
	if status.Done {
		t.Fatal("one verified piece must not complete a multi-file selection")
	}
	if status.VerifiedBytes <= 0 {
		t.Fatalf("expected verified bytes after piece 0, got %d", status.VerifiedBytes)
	}
	if status.Downloaded < status.VerifiedBytes {
		t.Fatalf("live downloaded %d < verified %d", status.Downloaded, status.VerifiedBytes)
	}
	if status.Files[0].VerifiedBytes <= 0 {
		t.Fatalf("file0 missing verified bytes: %+v", status.Files[0])
	}
	for i := 0; i < infoNumPieces(rec); i++ {
		writeVerifiedPiece(t, s, rec, data, i)
	}
	code, body = s.DispatchControl("GET", "/torrent/"+rec.ID, "")
	if err := json.Unmarshal(body, &status); err != nil {
		t.Fatal(err)
	}
	if code != 200 || !status.Done || status.VerifiedBytes < status.Length {
		t.Fatalf("expected verified completion: %s", body)
	}
	if status.Downloaded < status.VerifiedBytes {
		t.Fatalf("live downloaded %d < verified %d after done", status.Downloaded, status.VerifiedBytes)
	}
}

func infoNumPieces(rec *TorrentRecord) int {
	return rec.Torrent.Info().NumPieces()
}

func TestPlaybackRejectsUnselectedFilesAndUnverifiedCompletion(t *testing.T) {
	s, rec, data := playbackFixture(t)
	if reader, err := s.OpenPlayback(rec.ID, 0); err == nil {
		reader.Close()
		t.Fatal("opened unselected file")
	}
	writeVerifiedPiece(t, s, rec, data, 0)
	got := verifiedFileBytes(rec.Torrent)
	if got[0] != 5 || got[1] != 3 {
		t.Fatalf("shared boundary: %v", got)
	}
	code, body := s.DispatchControl("GET", "/torrent/"+rec.ID, "")
	var status TorrentView
	if err := json.Unmarshal(body, &status); err != nil {
		t.Fatal(err)
	}
	if code != 200 || status.Done || status.Files[1].VerifiedBytes >= status.Files[1].Length {
		t.Fatalf("premature completion: %s", body)
	}
	if rec.Torrent.Files()[1].Priority() != torrent.PiecePriorityNormal {
		t.Fatal("status changed download selection")
	}
}

func TestLocalPeerDownloadProducesPlayableMedia(t *testing.T) {
	data, err := os.ReadFile("testdata/playback.mp4")
	if err != nil {
		t.Fatal(err)
	}
	info := metainfo.Info{Name: "clip.mp4", Length: int64(len(data)), PieceLength: 16 * 1024}
	for start := 0; start < len(data); start += int(info.PieceLength) {
		hash := sha1.Sum(data[start:min(start+int(info.PieceLength), len(data))])
		info.Pieces = append(info.Pieces, hash[:]...)
	}
	encoded, err := bencode.Marshal(info)
	if err != nil {
		t.Fatal(err)
	}
	mi := metainfo.MetaInfo{InfoBytes: encoded}
	var metadata bytes.Buffer
	if err := mi.Write(&metadata); err != nil {
		t.Fatal(err)
	}
	newPeer := func(seed bool) (*EngineServer, *TorrentRecord) {
		s := testEngine(t, func(cfg *torrent.ClientConfig) {
			cfg.DisableTCP = false
			cfg.DisableIPv6 = true
			cfg.ListenHost = func(string) string { return "127.0.0.1" }
			cfg.ListenPort = 0
			cfg.DialForPeerConns, cfg.AcceptPeerConnections = true, true
			cfg.Seed = seed
		})
		body, _ := json.Marshal(AddRequest{Prepare: true, TorrentData: base64.StdEncoding.EncodeToString(metadata.Bytes())})
		code, response := s.DispatchControl("POST", "/add", string(body))
		if code != 200 {
			t.Fatalf("add: %s", response)
		}
		var added AddResponse
		if err := json.Unmarshal(response, &added); err != nil {
			t.Fatal(err)
		}
		rec := s.records[added.ID]
		f, err := os.CreateTemp(t.TempDir(), "clip")
		if err != nil {
			t.Fatal(err)
		}
		fd := int(f.Fd())
		body, _ = json.Marshal(ConfigureRequest{ID: rec.ID, Descriptors: []*int{&fd}, Selected: []int{0}})
		code, response = s.DispatchControl("POST", "/configure", string(body))
		_ = f.Close()
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
	reader, err := leech.OpenPlayback(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer reader.Close()
	if rec.Torrent.AddClientPeer(seed.client) == 0 {
		t.Fatal("no local seed address")
	}
	got := make([]byte, len(data))
	finished := make(chan error, 1)
	go func() { _, err := reader.ReadAt(got, 0); finished <- err }()
	select {
	case err := <-finished:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(15 * time.Second):
		reader.Close()
		t.Fatal("local peer download stalled")
	}
	if !bytes.Equal(got, data) {
		t.Fatal("downloaded media was corrupted")
	}
	// Decode the actual bytes returned to a player when FFmpeg is available.
	if ffmpeg, err := exec.LookPath("ffmpeg"); err == nil {
		path := filepath.Join(t.TempDir(), "received.mp4")
		if err := os.WriteFile(path, got, 0600); err != nil {
			t.Fatal(err)
		}
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		if output, err := exec.CommandContext(ctx, ffmpeg, "-v", "error", "-xerror", "-i", path, "-f", "null", "-").CombinedOutput(); err != nil {
			t.Fatalf("media decoder rejected downloaded bytes: %v %s", err, output)
		}
	}
}
