package main

import (
	"bytes"
	"encoding/json"
	"testing"

	"github.com/anacrolix/torrent"
)

func TestPlaybackCloseReopenPreservesResumeAcrossMetadataProbes(t *testing.T) {
	for _, direct := range []bool{false, true} {
		name := "download"
		if direct {
			name = "direct"
		}
		t.Run(name, func(t *testing.T) {
			s, rec, pieceLen := largePlaybackFixtureMode(t, 24, direct)
			data := bytes.Repeat([]byte("abcdefgh"), int(24*pieceLen/8))
			for _, index := range []int{0, 10, 11} {
				writeVerifiedPiece(t, s, rec, data, index)
			}
			open := func() *playbackReader {
				p, err := s.OpenPlayback(rec.ID, 0)
				if err != nil {
					t.Fatal(err)
				}
				t.Cleanup(p.Close)
				return p
			}
			read := func(p *playbackReader, offset int64) {
				buf := make([]byte, 64<<10)
				n, err := p.ReadAt(buf, offset)
				if err != nil || n != len(buf) || !bytes.Equal(buf, data[offset:offset+int64(len(buf))]) {
					t.Fatalf("read at %d: n=%d err=%v", offset, n, err)
				}
			}
			first := open()
			read(first, 10*pieceLen)
			first.Close()
			c := first.priorityOwner
			if priority := rawPiecePriority(rec.Torrent.Piece(10)); priority < torrent.PiecePriorityHigh {
				t.Fatalf("resume buffer lost its demand: %v", priority)
			}
			position := c.resumePosition(first.file)
			probe := open()
			read(probe, 0)
			probe.Close()
			if got := c.resumePosition(first.file); got != position {
				t.Fatalf("header probe replaced resume: %d != %d", got, position)
			}
			reopened := open()
			urgent, _, _, _, _, _, _ := reopened.testWindow()
			if urgent.begin != 10 {
				t.Fatalf("reopen starts at piece %d", urgent.begin)
			}
			// No peer is available; these reads must reuse verified local/RAM data.
			read(reopened, 10*pieceLen)
			reopened.Close()
			c.mu.Lock()
			generation := c.resumeGeneration
			c.mu.Unlock()
			c.expireResume(generation - 1)
			c.mu.Lock()
			retained := len(c.resumeDemand)
			c.mu.Unlock()
			if retained == 0 {
				t.Fatal("stale timer expired a newer hold")
			}
			c.expireResume(generation)
			c.mu.Lock()
			remaining := len(c.resumeDemand)
			c.mu.Unlock()
			if remaining != 0 {
				t.Fatal("expired resume demand remains pinned")
			}
			hint := int64(14 * pieceLen)
			hintBody, _ := json.Marshal(PlayRequest{ID: rec.ID, FileIndex: intPtr(0), Offset: &hint})
			hintCode, hintResponse := s.DispatchControl("POST", "/play", string(hintBody))
			if hintCode != 200 {
				t.Fatalf("resume hint: %d %s", hintCode, hintResponse)
			}
			hinted := open()
			urgent, _, _, _, _, _, _ = hinted.testWindow()
			if urgent.begin != 14 {
				t.Fatal("saved-position prefetch hint was ignored")
			}
			hinted.Close()
			c.setHints(nil) // Transfer pause also releases the extra hold.
			c.holdReaderResume(reopened)
			c.mu.Lock()
			pausedDemand := len(c.resumeDemand)
			c.mu.Unlock()
			if pausedDemand != 0 {
				t.Fatal("pause retained a resume hold")
			}
			zero := int64(0)
			body, _ := json.Marshal(PlayRequest{ID: rec.ID, FileIndex: intPtr(0), Offset: &zero})
			code, response := s.DispatchControl("POST", "/play", string(body))
			if code != 200 {
				t.Fatalf("restart: %d %s", code, response)
			}
			restart := open()
			urgent, _, _, _, _, _, _ = restart.testWindow()
			if urgent.begin != 0 {
				t.Fatal("Play from start reused the old body position")
			}
			restart.Close()
			// Teardown invalidates the timer before dropping its torrent.
			c.prepareResume(restart.file, &hint)
			s.closePlaybackReaders(rec.Torrent)
			c.holdReaderResume(reopened)
			c.mu.Lock()
			defer c.mu.Unlock()
			if c.resumeTimer != nil || len(c.resumeDemand) != 0 {
				t.Fatal("resume survived shutdown")
			}
		})
	}
}
