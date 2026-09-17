package main

import (
	"bytes"
	"crypto/sha1"
	"encoding/base64"
	"encoding/json"
	"os"
	"reflect"
	"testing"
	"unsafe"

	"github.com/anacrolix/torrent"
	"github.com/anacrolix/torrent/bencode"
	"github.com/anacrolix/torrent/metainfo"
)

func TestIsVideoFileName(t *testing.T) {
	cases := map[string]bool{
		"clip.mp4":    true,
		"Show.MKV":    true,
		"a/b/c.webm":  true,
		"notes.txt":   false,
		"archive.zip": false,
		"track.flac":  false,
		"movie":       false,
		"sample.m4v":  true,
	}
	for name, want := range cases {
		if got := isVideoFileName(name); got != want {
			t.Fatalf("%q: got %v want %v", name, got, want)
		}
	}
}

func TestVideoStartupHintsRaiseHeadAndTail(t *testing.T) {
	s, rec, pieceLen, _ := configuredLargeVideo(t, 48)
	tr := rec.Torrent
	file := tr.Files()[0]
	headBegin, headEnd := filePieceRange(file, 0, videoStartupHeadBytes)
	tailBegin, tailEnd := filePieceRange(file, file.Length()-videoStartupTailBytes, file.Length())
	middle := headEnd

	if headBegin != 0 || headEnd != int(videoStartupHeadBytes/pieceLen) {
		t.Fatalf("head range [%d,%d) pieceLen=%d", headBegin, headEnd, pieceLen)
	}
	if tailEnd-tailBegin != int(videoStartupTailBytes/pieceLen) {
		t.Fatalf("tail range [%d,%d)", tailBegin, tailEnd)
	}
	if middle >= tailBegin {
		t.Fatalf("fixture too small for a middle piece: headEnd=%d tailBegin=%d", headEnd, tailBegin)
	}

	rec.mu.Lock()
	s.applyTransferState(rec)
	rec.mu.Unlock()

	// Soft prepare: file remains Normal so mid-file pieces are allowed to download,
	// while head and tail pieces are raised to Now for urgent metadata.
	if got := file.Priority(); got != torrent.PiecePriorityNormal {
		t.Fatalf("file priority=%v want Normal", got)
	}
	if got := rawPiecePriority(tr.Piece(headBegin)); got != torrent.PiecePriorityNow {
		t.Fatalf("head piece %d priority=%v want Now", headBegin, got)
	}
	if got := rawPiecePriority(tr.Piece(headEnd - 1)); got != torrent.PiecePriorityNow {
		t.Fatalf("head end piece %d priority=%v want Now", headEnd-1, got)
	}
	if got := rawPiecePriority(tr.Piece(tailBegin)); got != torrent.PiecePriorityNow {
		t.Fatalf("tail piece %d priority=%v want Now", tailBegin, got)
	}
	if got := rawPiecePriority(tr.Piece(tailEnd - 1)); got != torrent.PiecePriorityNow {
		t.Fatalf("tail end piece %d priority=%v want Now", tailEnd-1, got)
	}
	if got := rawPiecePriority(tr.Piece(middle)); got == torrent.PiecePriorityNow {
		t.Fatalf("middle piece %d should not be Now, got %v", middle, got)
	}
}

func TestVideoStartupHintsSkipNonVideoAndPaused(t *testing.T) {
	s := testEngine(t)
	pieceLen := int64(1024 * 1024)
	videoLen := int64(20) * pieceLen
	textLen := int64(4) * pieceLen
	total := videoLen + textLen
	data := bytes.Repeat([]byte("abcdefgh"), int(total/8))
	info := metainfo.Info{
		Name:        "pack",
		PieceLength: pieceLen,
		Files: []metainfo.FileInfo{
			{Length: videoLen, Path: []string{"clip.mp4"}},
			{Length: textLen, Path: []string{"notes.txt"}},
		},
	}
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
	videoFile, err := os.CreateTemp(t.TempDir(), "clip")
	if err != nil {
		t.Fatal(err)
	}
	textFile, err := os.CreateTemp(t.TempDir(), "notes")
	if err != nil {
		t.Fatal(err)
	}
	vfd, tfd := int(videoFile.Fd()), int(textFile.Fd())
	body, _ = json.Marshal(ConfigureRequest{
		ID:          added.ID,
		Descriptors: []*int{&vfd, &tfd},
		Selected:    []int{0, 1},
	})
	code, response = s.DispatchControl("POST", "/configure", string(body))
	_ = videoFile.Close()
	_ = textFile.Close()
	if code != 200 {
		t.Fatalf("configure: %d %s", code, response)
	}

	rec := s.records[added.ID]
	tr := rec.Torrent
	video := tr.Files()[0]
	text := tr.Files()[1]

	rec.mu.Lock()
	s.applyTransferState(rec)
	rec.mu.Unlock()

	if got := rawPiecePriority(tr.Piece(video.BeginPieceIndex())); got != torrent.PiecePriorityNow {
		t.Fatalf("video head priority=%v want Now", got)
	}
	textMiddle := (text.BeginPieceIndex() + text.EndPieceIndex()) / 2
	if got := rawPiecePriority(tr.Piece(textMiddle)); got == torrent.PiecePriorityNow {
		t.Fatalf("non-video piece should not get startup Now, got %v", got)
	}
	if text.Priority() != torrent.PiecePriorityNormal {
		t.Fatalf("selected non-video file priority=%v want Normal", text.Priority())
	}
	if video.Priority() != torrent.PiecePriorityNormal {
		t.Fatalf("selected video file priority=%v want Normal", video.Priority())
	}

	code, response = s.DispatchControl("POST", "/pause", `{"id":"`+added.ID+`"}`)
	if code != 200 {
		t.Fatalf("pause: %d %s", code, response)
	}
	if video.Priority() != torrent.PiecePriorityNone {
		t.Fatalf("paused video file priority=%v want None", video.Priority())
	}
}

func TestVideoStartupHintsPlaybackCoexistence(t *testing.T) {
	s, rec, _, _ := configuredLargeVideo(t, 48)
	file := rec.Torrent.Files()[0]
	headPiece := file.BeginPieceIndex()

	rec.mu.Lock()
	s.applyTransferState(rec)
	rec.mu.Unlock()

	if got := file.Priority(); got != torrent.PiecePriorityNormal {
		t.Fatalf("before playback: file priority=%v want Normal", got)
	}
	if got := rawPiecePriority(rec.Torrent.Piece(headPiece)); got != torrent.PiecePriorityNow {
		t.Fatalf("before playback: head piece priority=%v want Now", got)
	}

	reader, err := s.OpenPlayback(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer reader.Close()

	rec.mu.Lock()
	s.applyTransferState(rec)
	rec.mu.Unlock()

	// Active playback coexists with soft hints: file remains Normal and head piece remains Now
	if got := file.Priority(); got != torrent.PiecePriorityNormal {
		t.Fatalf("during playback: file priority=%v want Normal", got)
	}
	if got := rawPiecePriority(rec.Torrent.Piece(headPiece)); got != torrent.PiecePriorityNow {
		t.Fatalf("during playback: head piece priority=%v want Now", got)
	}
}

// rawPiecePriority reads the piece-level override SetPriority stores. State().Priority
// is the effective value and returns None while hashing / completion is unknown.
func rawPiecePriority(p *torrent.Piece) torrent.PiecePriority {
	field := reflect.ValueOf(p).Elem().FieldByName("priority")
	return torrent.PiecePriority(reflect.NewAt(field.Type(), unsafe.Pointer(field.UnsafeAddr())).Elem().Uint())
}

func configuredLargeVideo(t *testing.T, pieces int) (*EngineServer, *TorrentRecord, int64, []byte) {
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
	return s, s.records[added.ID], pieceLen, data
}
