package main

import (
	"github.com/anacrolix/torrent"
)

// playbackDetailMax bounds the per-piece window sent to the grid.
const playbackDetailMax = 2048

// urgentNowExtraPieces is how many pieces past the playhead's 2 MiB are Now.
var urgentNowExtraPieces = 0

// splitUrgent splits an urgent span into the pieces under the playhead, which
// are Now, and the rest of the fast tier, which is Next. Every Now piece
// competes for the same peer slots, and Ghost Pro's fast tier can be dozens of
// pieces, so only the pieces holding the playhead's next 2 MiB (plus
// urgentNowExtraPieces) are Now.
func splitUrgent(urgent pieceSpan, pieceLen int64) (now, next pieceSpan) {
	nowEnd := urgent.end
	if pieceLen > 0 {
		nowEnd = min(nowEnd, urgent.begin+int((playbackUrgentBytes+pieceLen-1)/pieceLen)+urgentNowExtraPieces)
	}
	return pieceSpan{urgent.begin, nowEnd}, pieceSpan{nowEnd, urgent.end}
}

// playbackFocusWindow is the window of the reader at the playhead, or the
// widest open window when none has started playing.
func (s *EngineServer) playbackFocusWindow(t *torrent.Torrent) (playbackWindowState, bool) {
	s.playbackMu.Lock()
	c := s.playbackOwners[t]
	s.playbackMu.Unlock()
	if c == nil {
		return playbackWindowState{}, false
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.closed {
		return playbackWindowState{}, false
	}
	if window, ok := c.windows[c.playing]; ok && c.playing != nil {
		return window, true
	}
	var best playbackWindowState
	found, width := false, -1
	for _, window := range c.windows {
		span := unionSpan(window.urgent, window.readahead, window.lookahead)
		if w := span.end - span.begin; w > width {
			best, found, width = window, true, w
		}
	}
	return best, found
}

// addPlaybackTelemetry adds every open reader's tiers and playhead, and the
// focus window piece by piece, so the grid can show each tier at true size.
func (s *EngineServer) addPlaybackTelemetry(resp *PieceTelemetryResponse, t *torrent.Torrent, st *DocumentTorrentStorage) {
	pieceLen := t.Info().PieceLength
	for _, window := range s.playbackWindows(t) {
		// This block (U), then the next blocks in order (N).
		now, _ := splitUrgent(window.urgent, pieceLen)
		next := pieceSpan{now.end, unionSpan(window.urgent, window.readahead, window.lookahead).end}
		for _, tier := range []struct {
			span  pieceSpan
			label string
		}{{now, "U"}, {next, "N"}} {
			if !tier.span.empty() {
				resp.Playback = append(resp.Playback, PlaybackPieceRange{tier.span.begin, tier.span.end - 1, tier.label})
			}
		}
		if !window.urgent.empty() {
			resp.Playheads = append(resp.Playheads, window.urgent.begin)
		}
	}
	window, ok := s.playbackFocusWindow(t)
	if !ok || window.urgent.empty() {
		return
	}
	span := unionSpan(window.urgent, window.readahead, window.lookahead)
	span.end = min(span.end, span.begin+playbackDetailMax, t.NumPieces())
	if span.empty() {
		return
	}
	states := make([]byte, 0, span.end-span.begin)
	if st != nil {
		st.mu.Lock()
		for i := span.begin; i < span.end; i++ {
			states = append(states, st.pieceStateLocked(i))
		}
		st.mu.Unlock()
	} else {
		for i := span.begin; i < span.end; i++ {
			state := t.Piece(i).State()
			switch {
			case state.Complete:
				states = append(states, 'v')
			case state.Partial || state.Checking || state.Hashing:
				states = append(states, 'r')
			default:
				states = append(states, 'm')
			}
		}
	}
	resp.PlaybackDetail = &PlaybackDetail{Start: span.begin, States: string(states), Playhead: window.urgent.begin}
}

// pieceStateLocked is one Direct Play piece for the grid: in RAM and verified,
// saved on storage only, partly received, or missing.
func (st *DocumentTorrentStorage) pieceStateLocked(index int) byte {
	inRAM := len(st.cache[index]) > 0 && !st.streamLost[index]
	switch {
	case inRAM && st.completed[index]:
		return 'v'
	case st.startupSaved[index]:
		return 's'
	case inRAM && len(st.streamRanges[index]) > 0:
		return 'r'
	default:
		return 'm'
	}
}
