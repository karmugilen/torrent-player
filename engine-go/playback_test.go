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
	p := &documentPiece{storage: s.storage.GetStorage(rec.InfoHash), piece: info.Piece(index)}
	if _, err := p.WriteAt(data[start:end], 0); err != nil {
		t.Fatal(err)
	}
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
	if code != 200 || !status.Done || status.Files[1].Progress != 1 {
		t.Fatalf("not completed: %s", body)
	}
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
	if code != 200 || status.Done || status.Files[1].Progress >= 1 {
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
