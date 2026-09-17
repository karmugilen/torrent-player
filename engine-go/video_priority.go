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
	if !s.payloadTransfersAllowed.Load() || rec.Paused || !rec.Configured || rec.Checking || rec.removed || rec.Error != nil {
		return
	}
	t := rec.Torrent
	info := t.Info()
	if info == nil || info.PieceLength <= 0 {
		return
	}
	selected := make(map[int]bool, len(rec.Selected))
	for _, index := range rec.Selected {
		selected[index] = true
	}
	for i, f := range t.Files() {
		if !selected[i] || !isVideoFileName(f.DisplayPath()) {
			continue
		}
		length := f.Length()
		if length <= 0 {
			continue
		}

		raiseFileByteRange(t, f, 0, minInt64(length, videoStartupHeadBytes), torrent.PiecePriorityNow)
		if length > videoStartupTailBytes {
			raiseFileByteRange(t, f, length-videoStartupTailBytes, length, torrent.PiecePriorityNow)
		}
	}
}

func raiseFileByteRange(t *torrent.Torrent, file *torrent.File, start, end int64, priority torrent.PiecePriority) {
	begin, endPiece := filePieceRange(file, start, end)
	raisePieceRange(t, begin, endPiece, priority)
}

func raisePieceRange(t *torrent.Torrent, begin, end int, priority torrent.PiecePriority) {
	for p := begin; p < end; p++ {
		t.Piece(p).SetPriority(priority)
	}
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
