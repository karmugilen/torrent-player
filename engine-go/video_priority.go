package main

import (
	"mime"
	"path/filepath"
	"strings"

	"github.com/anacrolix/torrent"
)

// applyVideoStartupHints applies soft startup hints for selected video files.
// Head (~5MB) and tail (~3MB) pieces are raised to PiecePriorityNow so container
// metadata (such as moov atoms) downloads urgently, while the file remains at
// PiecePriorityNormal so mid-file pieces can download concurrently without gating.
func (s *EngineServer) applyVideoStartupHints(rec *TorrentRecord) {
	c := s.ensurePlaybackCoordinator(rec.Torrent)
	if !s.payloadTransfersAllowed.Load() || rec.Paused || !rec.Configured || rec.Checking || rec.removed || rec.Error != nil {
		c.setHints(nil)
		return
	}
	t := rec.Torrent
	info := t.Info()
	if info == nil || info.PieceLength <= 0 {
		c.setHints(nil)
		return
	}
	selected := make(map[int]bool, len(rec.Selected))
	for _, index := range rec.Selected {
		selected[index] = true
	}
	demand := make(map[int]torrent.PiecePriority)
	for i, f := range t.Files() {
		if !selected[i] || !isVideoFileName(f.DisplayPath()) {
			continue
		}
		if (rec.DirectPlay || rec.FileOnly) && (rec.FocusedFile == nil || i != *rec.FocusedFile) {
			continue
		}
		length := f.Length()
		if length <= 0 {
			continue
		}
		head, tail := videoStartupHeadBytes, videoStartupTailBytes
		if rec.DirectPlay {
			head, tail = directStartupHead, directStartupTail
		}
		if s.isGhostProEnabled() && !rec.DirectPlay {
			pieceLen := info.PieceLength
			head = max(head, playbackBoostHeadBytes, 2*pieceLen)
			tail = max(tail, playbackBoostTailBytes, 2*pieceLen)
		}
		raiseSpan(demand, spanFromFile(f, 0, minInt64(length, head)), torrent.PiecePriorityNow)
		if length > tail {
			raiseSpan(demand, spanFromFile(f, length-tail, length), torrent.PiecePriorityNow)
		}
	}
	if st := s.directStreamStorage(rec); st != nil {
		// Save every selected video's metadata in the background, below
		// playback and startup demand.
		for _, p := range st.startupFetchPieces(s.openPlaybackReaders(t) > 0) {
			if demand[p] < torrent.PiecePriorityNormal {
				demand[p] = torrent.PiecePriorityNormal
			}
		}
	}
	raiseNextPreload(rec, demand)
	c.setHints(demand)
}

func isVideoFileName(name string) bool {
	ext := strings.ToLower(filepath.Ext(name))
	if ext == "" {
		return false
	}
	if strings.HasPrefix(mime.TypeByExtension(ext), "video/") {
		return true
	}
	// mime may omit some torrent-common containers.
	switch ext {
	case ".mkv", ".ts", ".m2ts", ".mts", ".vob", ".m4v", ".f4v", ".mpg", ".mpeg", ".m2v":
		return true
	default:
		return false
	}
}
