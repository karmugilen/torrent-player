package main

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/anacrolix/torrent"
)

func TestRangeStartParsesSingleRanges(t *testing.T) {
	for header, want := range map[string]int64{
		"bytes=0-": 0, "bytes=1048576-": 1 << 20, "bytes=10-20": 10, " bytes=7- ": 7,
		"": -1, "bytes=-500": -1, "bytes=1-2,5-6": -1, "items=1-": -1, "bytes=x-": -1,
	} {
		if got := rangeStart(header); got != want {
			t.Errorf("rangeStart(%q) = %d, want %d", header, got, want)
		}
	}
}

// A seek opens a new HTTP range. Its window must start at that range, not at
// the saved resume position, so no requests or priority swaps go elsewhere.
func TestSeekRangeOpensTheWindowAtTheRange(t *testing.T) {
	s, rec := ghostProPlaybackFixture(t, 1<<20, 256, false)
	p, err := s.openPlaybackAt(rec.ID, 0, 100<<20)
	if err != nil {
		t.Fatal(err)
	}
	defer p.Close()
	if p.urgent.begin != 100 || p.openPhase {
		t.Fatalf("window opened at %+v openPhase=%v", p.urgent, p.openPhase)
	}
	if got := rawPiecePriority(rec.Torrent.Piece(100)); got != torrent.PiecePriorityNow {
		t.Fatalf("seek piece priority %v", got)
	}
	if got := rawPiecePriority(rec.Torrent.Piece(0)); got >= torrent.PiecePriorityNow {
		t.Fatalf("the start of the file was made urgent: %v", got)
	}
}

// The grid gets every tier at its true size, the playhead, and the playing
// window piece by piece, with or without Ghost Pro.
func TestPiecesReportsPlaybackTiersPlayheadAndDetail(t *testing.T) {
	for _, ghostPro := range []bool{false, true} {
		s, rec := ghostProPlaybackFixture(t, 1<<20, 256, false)
		if !ghostPro {
			if code, body := s.DispatchControl("POST", "/settings", `{"ghostProMode":false}`); code != 200 {
				t.Fatalf("settings: %d %s", code, body)
			}
		}
		p, err := s.openPlaybackAt(rec.ID, 0, 40<<20)
		if err != nil {
			t.Fatal(err)
		}
		p.mu.Lock()
		p.finishFocusLocked()
		p.mu.Unlock()
		code, raw := s.DispatchControl("GET", "/pieces/"+rec.ID, "")
		if code != 200 {
			t.Fatalf("pieces: %d %s", code, raw)
		}
		var resp PieceTelemetryResponse
		if err := json.Unmarshal(raw, &resp); err != nil {
			t.Fatal(err)
		}
		tiers := map[string]PlaybackPieceRange{}
		for _, r := range resp.Playback {
			tiers[r.Priority] = r
		}
		if u := tiers["U"]; u.Start != 40 || u.End-u.Start+1 > 3 {
			t.Fatalf("ghostPro=%v urgent must be the playhead's few pieces: %+v", ghostPro, resp.Playback)
		}
		if n, ok := tiers["N"]; !ok || n.Start != tiers["U"].End+1 || len(resp.Playback) != 2 {
			t.Fatalf("ghostPro=%v next blocks must follow this block: %+v", ghostPro, resp.Playback)
		}
		for _, r := range resp.Playback {
			if r.Start < 40 {
				t.Fatalf("ghostPro=%v fetches behind the playhead: %+v", ghostPro, resp.Playback)
			}
		}
		if len(resp.Playheads) != 1 || resp.Playheads[0] != 40 {
			t.Fatalf("playheads %v", resp.Playheads)
		}
		d := resp.PlaybackDetail
		if d == nil || d.Playhead != 40 || d.Start > 40 || d.Start+len(d.States) <= 40 || strings.Trim(d.States, "vsrm") != "" {
			t.Fatalf("detail %+v", d)
		}
		p.Close()
	}
}
