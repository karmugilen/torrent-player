package main

import (
	"time"

	"github.com/anacrolix/torrent"
)

// Keep one small resume range warm, within the existing storage budget. The
// position is only a scheduling hint: the external player still owns its seek.
const playbackResumeTTL = 90 * time.Second

func (c *playbackPriorityCoordinator) clearResumeLocked() {
	c.resumeGeneration++
	if c.resumeTimer != nil {
		c.resumeTimer.Stop()
		c.resumeTimer = nil
	}
	c.resumeDemand = nil
}

func (c *playbackPriorityCoordinator) resumePosition(file *torrent.File) int64 {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.resumeFile == file {
		return c.resumeOffset
	}
	return 0
}

func (c *playbackPriorityCoordinator) prepareResume(file *torrent.File, offset *int64) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.closed {
		return
	}
	if offset != nil {
		c.resumeReader = nil
		c.resumeFile = file
		c.resumeOffset = max(int64(0), min(*offset, max(int64(0), file.Length()-1)))
	} else if c.resumeFile != file {
		c.resumeFile, c.resumeOffset, c.resumeReader = file, 0, nil
	}
	c.holdResumeLocked(file, torrent.PiecePriorityNow)
}

func (c *playbackPriorityCoordinator) holdReaderResume(p *playbackReader) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.resumeReader == p {
		c.holdResumeLocked(p.file, torrent.PiecePriorityHigh)
	}
}

func (c *playbackPriorityCoordinator) holdResumeLocked(file *torrent.File, priority torrent.PiecePriority) {
	if c.closed || c.resumeSuspended || c.resumeFile != file {
		return
	}
	c.clearResumeLocked()
	if c.resumeOffset == 0 {
		c.applyLocked()
		return
	}
	begin, end := filePieceRange(file, c.resumeOffset, min(file.Length(), c.resumeOffset+playbackUrgentBytes))
	c.resumeDemand = make(map[int]torrent.PiecePriority, end-begin)
	for i := begin; i < end; i++ {
		c.resumeDemand[i] = priority
	}
	generation := c.resumeGeneration
	c.resumeTimer = time.AfterFunc(playbackResumeTTL, func() {
		c.expireResume(generation)
	})
	c.applyLocked()
}

func (c *playbackPriorityCoordinator) expireResume(generation uint64) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.closed || c.resumeGeneration != generation {
		return
	}
	c.clearResumeLocked()
	c.applyLocked()
}

func (p *playbackReader) noteResumeReadLocked(offset int64) {
	// Header/tail probes must not replace the useful body position. Small
	// files are already covered by startup caching and need no extra hold.
	if p.priorityOwner == nil || offset < videoStartupHeadBytes || offset >= p.length-videoStartupTailBytes {
		return
	}
	c := p.priorityOwner
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.closed {
		return
	}
	c.resumeFile, c.resumeOffset, c.resumeReader = p.file, offset, p
	if len(c.resumeDemand) != 0 {
		c.clearResumeLocked()
		c.applyLocked()
	}
}

// persistVerifiedResumeWindow stores already-verified bytes near a useful
// resume offset. It never starts background fetching. Direct Play RAM pieces
// are snapshotted from the stream cache; download mode uses file pread.
func (s *EngineServer) persistVerifiedResumeWindow(rec *TorrentRecord, fileIndex int, offset int64) {
	if rec == nil || s == nil || rec.Torrent == nil {
		return
	}
	s.cacheMu.Lock()
	clearGen := s.cacheClearGen
	s.cacheMu.Unlock()
	files := rec.Torrent.Files()
	if fileIndex < 0 || fileIndex >= len(files) {
		return
	}
	file := files[fileIndex]
	st := s.storage.GetStorage(rec.InfoHash)
	if st == nil {
		return
	}
	// Align to whole pieces so resume inject can mark completion safely.
	info := rec.Torrent.Info()
	if info == nil || info.PieceLength <= 0 {
		return
	}
	pieceLen := info.PieceLength
	abs := ((file.Offset() + max(offset, 0)) / pieceLen) * pieceLen
	if abs < file.Offset() {
		abs += pieceLen
	}
	aligned := abs - file.Offset()
	var length int64
	end := min(file.Offset()+file.Length(), abs+resumeWindowMaxBytes)
	for at := abs; at < end; {
		piece := info.Piece(int(at / pieceLen))
		if at+piece.Length() > end || !st.PieceComplete(piece.Index()) {
			break
		}
		length += piece.Length()
		at += piece.Length()
	}
	if length <= 0 {
		return
	}
	buf, ok := st.copyVerifiedRange(abs, length)
	if !ok {
		return
	}
	var generation uint64
	if c := s.ensurePlaybackCoordinator(rec.Torrent); c != nil {
		c.mu.Lock()
		generation = c.resumeGeneration
		c.mu.Unlock()
	}
	s.saveResumeWindowDetailed(rec, fileIndex, file.DisplayPath(), file.Length(), pieceLen, aligned, length, buf, generation, clearGen)
}
