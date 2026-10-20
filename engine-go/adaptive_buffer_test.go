package main

import (
	"bytes"
	"encoding/json"
	"testing"
	"time"
)

// Exercise the same /play -> reader interface used by content URI and HTTP
// playback. Changing demand must affect actual scheduled look-ahead.
func TestAdaptiveBufferAddsHeadroomForVariableDemand(t *testing.T) {
	variable := []int64{256 << 10, 1792 << 10}
	plain := measuredPlaybackLookahead(t, variable, bufferExperiment{}, false)
	adaptive := measuredPlaybackLookahead(t, variable, bufferExperiment{adaptive: true}, false)
	if adaptive <= plain {
		t.Fatalf("variable demand needs extra look-ahead: adaptive=%d default=%d", adaptive, plain)
	}
	settled := measuredPlaybackLookahead(t, variable, bufferExperiment{adaptive: true}, true)
	if settled >= adaptive {
		t.Fatalf("stable demand should release extra headroom: stable=%d variable=%d", settled, adaptive)
	}
}

func TestBurstAwareBufferRetainsHeadroomAcrossLongerBursts(t *testing.T) {
	pattern := []int64{256 << 10, 1792 << 10}
	adaptive := measuredPlaybackLookahead(t, pattern, bufferExperiment{adaptive: true}, true)
	burstAware := measuredPlaybackLookahead(t, pattern, bufferExperiment{burst: true}, true)
	if burstAware <= adaptive {
		t.Fatalf("longer burst history needs headroom: burst=%d adaptive=%d", burstAware, adaptive)
	}
}

type bufferExperiment struct{ adaptive, burst bool }

func measuredPlaybackLookahead(t *testing.T, pattern []int64, experiment bufferExperiment, settle bool) int64 {
	t.Helper()
	s, rec, pieceLen := largePlaybackFixture(t, 72)
	body, _ := json.Marshal(map[string]any{"id": rec.ID, "fileIndex": 0, "adaptiveBuffer": experiment.adaptive, "burstAwareBuffer": experiment.burst})
	if code, response := s.DispatchControl("POST", "/play", string(body)); code != 200 {
		t.Fatalf("play: %d %s", code, response)
	}
	reader, err := s.OpenPlayback(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer reader.Close()
	clock := time.Unix(100, 0)
	reader.nowFn = func() time.Time { return clock }
	offset := 10 * pieceLen
	st := s.storage.GetStorage(rec.InfoHash)
	payload := bytes.Repeat([]byte("abcdefgh"), int(pieceLen/8))
	count := 20
	if settle {
		count += 20
	}
	for sample := 0; sample < count; sample++ {
		delta := pattern[sample%len(pattern)]
		if settle && sample >= 20 {
			delta = 1 << 20
		}
		offset += delta
		piece := &documentPiece{storage: st, piece: rec.Torrent.Info().Piece(int(offset / pieceLen))}
		if _, err := piece.WriteAt(payload, 0); err != nil {
			t.Fatal(err)
		}
		if err := piece.MarkComplete(); err != nil {
			t.Fatal(err)
		}
		clock = clock.Add(time.Second)
		if n, err := reader.ReadAt(make([]byte, 1), offset); err != nil || n != 1 {
			t.Fatalf("read: %d %v", n, err)
		}
	}
	urgent, ahead, _, _, _, _, _ := reader.testWindow()
	return int64(max(urgent.end, ahead.end)-urgent.begin) * pieceLen
}
