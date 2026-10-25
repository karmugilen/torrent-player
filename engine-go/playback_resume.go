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
	// A player that seeks closes the old reader after opening the new one.
	// Holding the old position then would keep fetching where it no longer is.
	for r := range c.readers {
		if r != p && r.file == p.file {
			return
		}
	}
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
	head, tail := videoStartupHeadBytes, videoStartupTailBytes
	if p.boost && p.startupHeadBytes > 0 {
		head, tail = p.startupHeadBytes, p.startupTailBytes
	}
	if p.priorityOwner == nil || offset < head || offset >= p.length-tail {
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

// A player closes its stream on every seek. Saving the resume window each time
// hashed and fsynced up to 4 MiB while holding cacheMu, which the next open
// waits for, and wrote flash on every seek. The save now runs once seeking has
// settled for resumePersistDelay; engine shutdown runs it at once.
const resumePersistDelay = 2 * time.Second

type pendingResumeSave struct {
	timer     *time.Timer
	rec       *TorrentRecord
	fileIndex int
	offset    int64
}

func (s *EngineServer) scheduleResumePersist(rec *TorrentRecord, fileIndex int, offset int64) {
	s.cacheMu.Lock()
	defer s.cacheMu.Unlock()
	if s.resumeSaves == nil {
		s.resumeSaves = make(map[string]*pendingResumeSave)
	}
	if old := s.resumeSaves[rec.InfoHash]; old != nil {
		old.timer.Stop()
	}
	save := &pendingResumeSave{rec: rec, fileIndex: fileIndex, offset: offset}
	save.timer = time.AfterFunc(resumePersistDelay, func() {
		s.cacheMu.Lock()
		current := s.resumeSaves[rec.InfoHash] == save
		if current {
			delete(s.resumeSaves, rec.InfoHash)
		}
		s.cacheMu.Unlock()
		if current && !s.shuttingDown.Load() {
			s.persistVerifiedResumeWindow(rec, fileIndex, offset)
		}
	})
	s.resumeSaves[rec.InfoHash] = save
}

// flushResumeSaves runs every pending save now.
func (s *EngineServer) flushResumeSaves() {
	s.cacheMu.Lock()
	pending := make([]*pendingResumeSave, 0, len(s.resumeSaves))
	for hash, save := range s.resumeSaves {
		if save.timer.Stop() {
			pending = append(pending, save)
		}
		delete(s.resumeSaves, hash)
	}
	s.cacheMu.Unlock()
	for _, save := range pending {
		s.persistVerifiedResumeWindow(save.rec, save.fileIndex, save.offset)
	}
}

// forgetResumeLocked drops the known window and any pending save of infoHash,
// or of every torrent when infoHash is empty.
func (s *EngineServer) forgetResumeLocked(infoHash string) {
	for hash, save := range s.resumeSaves {
		if infoHash == "" || hash == infoHash {
			save.timer.Stop()
			delete(s.resumeSaves, hash)
		}
	}
	for hash := range s.resumeKnown {
		if infoHash == "" || hash == infoHash {
			delete(s.resumeKnown, hash)
		}
	}
}

func (s *EngineServer) rememberResumeLocked(meta resumeWindowMeta) {
	if s.resumeKnown == nil {
		s.resumeKnown = make(map[string]resumeWindowMeta)
	}
	s.resumeKnown[meta.InfoHash] = meta
}

// resumeWindowInRAM reports the known resume window of fileIndex when every
// piece of it is already verified in storage.
func (s *EngineServer) resumeWindowInRAM(rec *TorrentRecord, fileIndex int) (int64, bool) {
	s.cacheMu.Lock()
	meta, ok := s.resumeKnown[rec.InfoHash]
	s.cacheMu.Unlock()
	files := rec.Torrent.Files()
	if !ok || meta.FileIndex != fileIndex || fileIndex < 0 || fileIndex >= len(files) || !meta.Expires.After(time.Now()) {
		return 0, false
	}
	file := files[fileIndex]
	if meta.Offset < 0 || meta.Length <= 0 || meta.Offset+meta.Length > file.Length() {
		return 0, false
	}
	st := s.storage.GetStorage(rec.InfoHash)
	if st == nil {
		return 0, false
	}
	begin, end := filePieceRange(file, meta.Offset, meta.Offset+meta.Length)
	st.mu.Lock()
	defer st.mu.Unlock()
	for i := begin; i < end; i++ {
		if !st.completed[i] {
			return 0, false
		}
	}
	return meta.Offset, true
}
