package main

import (
	"fmt"
	"sort"
	"strings"
	"time"

	"github.com/anacrolix/torrent"
)

// ghostProPeerProgress tracks the transfer and idle history of a peer
// observed under Ghost Pro mode.
type ghostProPeerProgress struct {
	useful, uploaded int64
	idleSince        time.Time
}

// ghostProSwarmState retains peer progress history and enforces rotation
// deadbands for intelligent bitfield-aware slot recycling.
type ghostProSwarmState struct {
	previous                 map[*torrent.PeerConn]ghostProPeerProgress
	nextSample, nextRotation time.Time
}

// ghostProPeerObservation contains point-in-time metrics and bitfield
// overlap data for a connected peer.
type ghostProPeerObservation struct {
	peer              *torrent.PeerConn
	useful, uploaded  int64
	overlapPieces     int
	contiguousOverlap int
	isSeeder          bool
}

// chooseVictim evaluates connected peers against our active piece demand and
// returns the best candidate for disconnection to free a slot for fresh dials.
//
// Prefer zero overlap with the current playback path after 15s idle, then
// zero selection overlap, then the lowest-overlap peer idle for at least 60s.
// Seeders and actively transferring peers are protected.
func (s *ghostProSwarmState) chooseVictim(
	now time.Time,
	peers []ghostProPeerObservation,
	capacity, pending int,
	playbackActive bool,
) *torrent.PeerConn {
	if now.Before(s.nextSample) {
		return nil
	}
	s.nextSample = now.Add(10 * time.Second)
	next := make(map[*torrent.PeerConn]ghostProPeerProgress, len(peers))

	var zeroOverlapVictim *torrent.PeerConn
	var zeroOldest time.Time
	var pathVictim *torrent.PeerConn
	var pathOldest time.Time

	var idleVictim *torrent.PeerConn
	var idleOldest time.Time
	minOverlap := int(^uint(0) >> 1)

	for _, obs := range peers {
		prev, known := s.previous[obs.peer]
		idleSince := prev.idleSince
		if !known || obs.useful != prev.useful || obs.uploaded != prev.uploaded {
			idleSince = now
		}
		next[obs.peer] = ghostProPeerProgress{
			useful:    obs.useful,
			uploaded:  obs.uploaded,
			idleSince: idleSince,
		}

		if obs.isSeeder {
			continue // Never evict seeders who possess all pieces.
		}

		idleDuration := now.Sub(idleSince)
		if playbackActive && obs.contiguousOverlap == 0 && idleDuration >= 15*time.Second {
			if pathVictim == nil || idleSince.Before(pathOldest) {
				pathVictim, pathOldest = obs.peer, idleSince
			}
		}

		// Candidate 1: Zero pieces in common with our active playback demand.
		if obs.overlapPieces == 0 && idleDuration >= 15*time.Second {
			if zeroOverlapVictim == nil || idleSince.Before(zeroOldest) {
				zeroOverlapVictim = obs.peer
				zeroOldest = idleSince
			}
		}

		// Candidate 2: Long-idle peer (>= 60s) with lowest overlap.
		if idleDuration >= 60*time.Second {
			overlap := obs.overlapPieces
			if playbackActive {
				overlap = obs.contiguousOverlap
			}
			if idleVictim == nil || overlap < minOverlap || (overlap == minOverlap && idleSince.Before(idleOldest)) {
				idleVictim = obs.peer
				idleOldest = idleSince
				minOverlap = overlap
			}
		}
	}

	s.previous = next

	if capacity < 2 || len(peers) < 2 || len(peers) < capacity || pending <= 0 || now.Before(s.nextRotation) {
		return nil
	}

	victim := pathVictim
	if victim == nil {
		victim = zeroOverlapVictim
	}
	if victim == nil {
		victim = idleVictim
	}

	if victim != nil {
		s.nextRotation = now.Add(20 * time.Second)
		delete(s.previous, victim)
	}

	return victim
}

// computeWantedPieces identifies uncompleted pieces within the torrent's
// active selection (or focused file for Direct Play).
func (rec *TorrentRecord) computeWantedPieces() map[int]bool {
	t := rec.Torrent
	if t == nil {
		return nil
	}
	info := t.Info()
	if info == nil || info.PieceLength <= 0 {
		return nil
	}
	files := t.Files()
	if len(files) == 0 {
		return nil
	}

	wanted := make(map[int]bool)
	selectedSet := make(map[int]bool, len(rec.Selected))
	for _, idx := range rec.Selected {
		selectedSet[idx] = true
	}

	for i, f := range files {
		if !selectedSet[i] {
			continue
		}
		if (rec.DirectPlay || rec.FileOnly) && (rec.FocusedFile == nil || i != *rec.FocusedFile) {
			continue
		}
		span := spanFromFile(f, 0, f.Length())
		numPieces := t.NumPieces()
		for p := span.begin; p < span.end && p < numPieces; p++ {
			if p >= 0 && !t.Piece(p).State().Complete {
				wanted[p] = true
			}
		}
	}
	return wanted
}

// Intersect each reader's published forward path with missing selected pieces.
// Keep disjoint readers separate: a union bounding range would include gaps
// between unrelated seeks. Metadata probes are not forward paths.
func contiguousWantedPieces(windows []playbackWindowState, wanted map[int]bool) map[int]bool {
	pieces := make(map[int]bool)
	for _, window := range windows {
		for _, span := range []pieceSpan{window.urgent, window.readahead, window.lookahead} {
			for i := span.begin; i < span.end; i++ {
				if wanted[i] {
					pieces[i] = true
				}
			}
		}
	}
	return pieces
}

// calculatePeerOverlap counts how many of our wanted pieces are in the peer's bitfield.
func calculatePeerOverlap(pc *torrent.PeerConn, wanted map[int]bool) int {
	if len(wanted) == 0 || pc == nil {
		return 0
	}
	bitmap := pc.PeerPieces()
	if bitmap == nil {
		return 0
	}
	overlap := 0
	for pieceIdx := range wanted {
		if bitmap.Contains(uint32(pieceIdx)) {
			overlap++
		}
	}
	return overlap
}

func (s *EngineServer) resetGhostProLocked() {
	s.ghostProGeneration++
	s.ghostProDial = ghostDialController{}
	s.setGhostDialRateLocked(s.baseDialRateLocked())
}

// ghostProModeTick executes the 1 Hz controller loop for Ghost Pro Mode.
// It manages two coordinated subsystems:
//  1. Closed-loop dial rate feedback tuning (scaling between base and 80/s).
//  2. Bitfield-aware peer recycling (freeing zero-overlap slots for fresh dials).
func (s *EngineServer) ghostProModeTick(now time.Time) {
	s.mu.RLock()
	if !s.ghostProMode || s.dialLimiter == nil {
		s.mu.RUnlock()
		return
	}
	generation, capacity := s.ghostProGeneration, s.maxPeers
	records := make([]*TorrentRecord, 0, len(s.records))
	for _, rec := range s.records {
		records = append(records, rec)
	}
	s.mu.RUnlock()

	var observation ghostObservation
	var cohort []string

	for _, rec := range records {
		rec.mu.Lock()
		if !rec.Configured || rec.Paused || rec.Checking || rec.removed || rec.Error != nil || rec.Seeding || len(rec.Selected) == 0 {
			rec.ghostPro = nil
			rec.mu.Unlock()
			continue
		}
		if rec.ghostPro == nil {
			rec.ghostPro = &ghostProSwarmState{}
		}

		stats := rec.Torrent.Stats()
		if stats.PiecesComplete < rec.Torrent.NumPieces() {
			cohort = append(cohort, fmt.Sprintf("%s:%d", rec.ID, rec.Generation))
			observation.usefulBytes += stats.BytesReadUsefulData.Int64()
			observation.opportunity = observation.opportunity || (stats.PendingPeers > 0 && stats.ActivePeers < capacity)
		}

		if now.Before(rec.ghostPro.nextSample) {
			rec.mu.Unlock()
			continue
		}

		wanted := rec.computeWantedPieces()
		if len(wanted) == 0 {
			rec.mu.Unlock()
			continue
		}
		peerConns := rec.Torrent.PeerConns()
		numPieces := rec.Torrent.NumPieces()
		contiguous := contiguousWantedPieces(s.playbackWindows(rec.Torrent), wanted)

		peerObs := make([]ghostProPeerObservation, 0, len(peerConns))
		for _, pc := range peerConns {
			pstats := pc.Stats()
			overlap := calculatePeerOverlap(pc, wanted)
			isSeeder := pstats.RemotePieceCount >= numPieces

			peerObs = append(peerObs, ghostProPeerObservation{
				peer:              pc,
				useful:            pstats.BytesReadUsefulData.Int64(),
				uploaded:          pstats.BytesWrittenData.Int64(),
				overlapPieces:     overlap,
				contiguousOverlap: calculatePeerOverlap(pc, contiguous),
				isSeeder:          isSeeder,
			})
		}

		victim := rec.ghostPro.chooseVictim(now, peerObs, capacity, stats.PendingPeers, len(contiguous) > 0)
		if victim != nil {
			s.mu.RLock()
			latest := victim.Stats()
			for _, observed := range peerObs {
				if s.ghostProMode && generation == s.ghostProGeneration && s.payloadTransfersAllowed.Load() &&
					latest.RemotePieceCount < numPieces && observed.peer == victim &&
					latest.BytesReadUsefulData.Int64() == observed.useful &&
					latest.BytesWrittenData.Int64() == observed.uploaded {
					_ = victim.Close()
					s.notifyChange()
					break
				}
			}
			s.mu.RUnlock()
		}
		rec.mu.Unlock()
	}

	sort.Strings(cohort)
	observation.cohort = strings.Join(cohort, ",")

	s.mu.Lock()
	defer s.mu.Unlock()

	if generation != s.ghostProGeneration || !s.ghostProMode {
		return
	}
	if !s.payloadTransfersAllowed.Load() {
		observation.opportunity = false
	}
	observation.downloadLimit = limiterRate(s.downloadLimiter)
	s.setGhostDialRateLocked(s.ghostProDial.observe(now, observation, s.baseDialRateLocked()))
}
