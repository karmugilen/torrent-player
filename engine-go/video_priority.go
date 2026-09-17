package main

import (
	"mime"
	"path/filepath"
	"strings"

	"github.com/anacrolix/torrent"
)

// applyVideoStartupHints raises the first and last pieces of selected video
// files so players can open sooner (container header + end-of-file moov).
// Called from applyTransferState while transfers are active. Uses High so an
// open playback reader (Now/Readahead) still outranks these background hints.
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
		raiseFileByteRange(t, f, 0, minInt64(length, videoStartupHeadBytes), torrent.PiecePriorityHigh)
		if length > videoStartupTailBytes {
			raiseFileByteRange(t, f, length-videoStartupTailBytes, length, torrent.PiecePriorityHigh)
		}
	}
}

func raiseFileByteRange(t *torrent.Torrent, file *torrent.File, start, end int64, priority torrent.PiecePriority) {
	begin, endPiece := filePieceRange(file, start, end)
	for p := begin; p < endPiece; p++ {
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
