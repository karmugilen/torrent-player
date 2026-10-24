package main

import (
	"slices"

	"github.com/anacrolix/torrent"
)

// Play next warm-up. With Play next on, Android names the episode it would
// play after the current one. Once playback reaches nextPreloadAt of the
// current file, the engine fetches the next episode's head and tail at High
// priority, below every playback window, so the next episode starts from
// verified bytes. Direct Play saves them in the durable startup cache.
const nextPreloadAt = 0.9

type nextEpisodePreload struct {
	current, next int
	active        bool
	pieces        map[int]bool // fetched pieces; nil until active
}

type NextPreloadView struct {
	FileIndex int  `json:"fileIndex"`
	Active    bool `json:"active"`
	Ready     bool `json:"ready"`
}

// directStreamStorage returns the RAM stream storage of a Direct Play record.
func (s *EngineServer) directStreamStorage(rec *TorrentRecord) *DocumentTorrentStorage {
	if !rec.DirectPlay || s.storage == nil {
		return nil
	}
	if st := s.storage.GetStorage(rec.InfoHash); st != nil && st.StreamMode() {
		return st
	}
	return nil
}

// setNextPreloadLocked replaces the warm-up plan for a new /play. Requires rec.mu.
func (s *EngineServer) setNextPreloadLocked(rec *TorrentRecord, current int, next *int) {
	if rec.nextPreload != nil && rec.nextPreload.active {
		if st := s.directStreamStorage(rec); st != nil {
			st.setPreloadFile(-1)
		}
	}
	rec.nextPreload = nil
	if next == nil || *next == current {
		return
	}
	files := rec.Torrent.Files()
	if *next < 0 || *next >= len(files) || !slices.Contains(rec.Selected, *next) || !isVideoFileName(files[*next].DisplayPath()) {
		return
	}
	rec.nextPreload = &nextEpisodePreload{current: current, next: *next}
}

// startNextPreload begins warming the next episode once playback of current
// nears its end. Readers call it asynchronously; it runs at most once per /play.
func (s *EngineServer) startNextPreload(rec *TorrentRecord, current int) {
	rec.mu.Lock()
	defer rec.mu.Unlock()
	np := rec.nextPreload
	if np == nil || np.active || np.current != current || rec.removed || rec.Paused || !rec.Configured || rec.Error != nil {
		return
	}
	info := rec.Torrent.Info()
	files := rec.Torrent.Files()
	if info == nil || np.next >= len(files) {
		return
	}
	np.active = true
	if st := s.directStreamStorage(rec); st != nil {
		// Only pieces the startup cache can keep; others would be evicted from RAM.
		np.pieces = st.setPreloadFile(np.next)
	} else {
		f := files[np.next]
		pieces := make(map[int]bool)
		head, tail := videoStartupHeadBytes, videoStartupTailBytes
		for _, span := range []pieceSpan{
			spanFromFile(f, 0, minInt64(f.Length(), head)),
			spanFromFile(f, max(0, f.Length()-tail), f.Length()),
		} {
			for p := span.begin; p < span.end; p++ {
				pieces[p] = true
			}
		}
		np.pieces = pieces
	}
	s.applyVideoStartupHints(rec)
	s.notifyChange()
}

// raiseNextPreload adds active warm-up pieces to the startup hints.
func raiseNextPreload(rec *TorrentRecord, demand map[int]torrent.PiecePriority) {
	np := rec.nextPreload
	if np == nil || !np.active {
		return
	}
	for p := range np.pieces {
		if demand[p] < torrent.PiecePriorityHigh {
			demand[p] = torrent.PiecePriorityHigh
		}
	}
}

// nextPreloadView reports the warm-up for the status API. Requires rec.mu.
func nextPreloadView(rec *TorrentRecord) *NextPreloadView {
	np := rec.nextPreload
	if np == nil {
		return nil
	}
	view := &NextPreloadView{FileIndex: np.next, Active: np.active}
	if np.active && len(np.pieces) > 0 {
		view.Ready = true
		for p := range np.pieces {
			if !rec.Torrent.Piece(p).State().Complete {
				view.Ready = false
				break
			}
		}
	}
	return view
}

// maybeStartNextPreload is called by a reader after each window update.
// Opening probes and scrubs never count as reaching the end.
func (p *playbackReader) maybeStartNextPreloadLocked(offset int64) {
	if p.preloadAsked || p.openPhase || p.scrubbing || p.record == nil || p.length <= 0 {
		return
	}
	if float64(offset) < float64(p.length)*nextPreloadAt {
		return
	}
	srv := p.engine()
	if srv == nil {
		return
	}
	p.preloadAsked = true
	go srv.startNextPreload(p.record, p.fileIndex)
}
