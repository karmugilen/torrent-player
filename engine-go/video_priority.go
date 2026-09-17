package main

import (
	"mime"
	"path/filepath"
	"strings"

	"github.com/anacrolix/torrent"
)

// applyVideoStartupHints implements moov-first downloading for selected video
// files. Until head (~5MB) and tail (~3MB) pieces are complete, the file is set
// to PiecePriorityNone to hold mid-file pieces from downloading, while only the
// head and tail pieces are raised to PiecePriorityNow. Once all head and tail
// pieces are complete (or if the file is so small that head/tail cover the entire
// file), the file is restored to PiecePriorityNormal so the rest can download.
// If active playback is open on the file, mid-hold is skipped.
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

		headBegin, headEnd := filePieceRange(f, 0, minInt64(length, videoStartupHeadBytes))
		var tailBegin, tailEnd int
		hasTail := length > videoStartupTailBytes
		if hasTail {
			tailBegin, tailEnd = filePieceRange(f, length-videoStartupTailBytes, length)
		}

		// Active playback skips mid-hold so streaming / seeking is not starved.
		if s.hasActivePlaybackForFile(t, f) {
			f.SetPriority(torrent.PiecePriorityNormal)
			continue
		}

		coversWholeFile := headEnd >= f.EndPieceIndex() || (hasTail && headEnd >= tailBegin)
		tipsComplete := isPieceRangeComplete(t, headBegin, headEnd) && (!hasTail || isPieceRangeComplete(t, tailBegin, tailEnd))

		if coversWholeFile || tipsComplete {
			f.SetPriority(torrent.PiecePriorityNormal)
			if !tipsComplete {
				raisePieceRange(t, headBegin, headEnd, torrent.PiecePriorityNow)
				if hasTail {
					raisePieceRange(t, tailBegin, tailEnd, torrent.PiecePriorityNow)
				}
			}
			continue
		}

		// Hold mid-file: file priority None stops anacrolix from pulling pieces
		// outside the explicit Now tips.
		f.SetPriority(torrent.PiecePriorityNone)
		raisePieceRange(t, headBegin, headEnd, torrent.PiecePriorityNow)
		if hasTail {
			raisePieceRange(t, tailBegin, tailEnd, torrent.PiecePriorityNow)
		}
	}
}

func (s *EngineServer) hasActivePlaybackForFile(t *torrent.Torrent, f *torrent.File) bool {
	s.playbackMu.Lock()
	defer s.playbackMu.Unlock()
	if s.playbackOwners == nil {
		return false
	}
	coordinator := s.playbackOwners[t]
	if coordinator == nil {
		return false
	}
	coordinator.mu.Lock()
	defer coordinator.mu.Unlock()
	for reader := range coordinator.owners {
		if reader.file == f || (reader.file != nil && f != nil && reader.file.Path() == f.Path()) {
			return true
		}
	}
	return false
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

func isPieceRangeComplete(t *torrent.Torrent, begin, end int) bool {
	for p := begin; p < end; p++ {
		if !t.Piece(p).State().Complete {
			return false
		}
	}
	return true
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
