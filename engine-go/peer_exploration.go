package main

import (
	"time"

	"github.com/anacrolix/torrent"
)

// Keep productive connections and slowly explore alternatives by releasing
// at most one long-idle slot. Discovery and dialing remain owned by torrent.
type peerExplorationState struct {
	previous                 map[*torrent.PeerConn]peerExplorationProgress
	nextSample, nextRotation time.Time
}

type peerExplorationProgress struct {
	useful, uploaded int64
	idleSince        time.Time
}

type peerExplorationObservation struct {
	peer             *torrent.PeerConn
	useful, uploaded int64
	protected        bool
}

func (s *peerExplorationState) choose(now time.Time, peers []peerExplorationObservation, capacity, pending int) *torrent.PeerConn {
	if now.Before(s.nextSample) {
		return nil
	}
	s.nextSample = now.Add(10 * time.Second)
	next := make(map[*torrent.PeerConn]peerExplorationProgress, len(peers))
	var victim *torrent.PeerConn
	var oldest time.Time
	for _, observation := range peers {
		previous, known := s.previous[observation.peer]
		idleSince := previous.idleSince
		if !known || observation.useful != previous.useful || observation.uploaded != previous.uploaded {
			idleSince = now
		}
		next[observation.peer] = peerExplorationProgress{observation.useful, observation.uploaded, idleSince}
		if !observation.protected && now.Sub(idleSince) >= 90*time.Second && (victim == nil || idleSince.Before(oldest)) {
			victim, oldest = observation.peer, idleSince
		}
	}
	s.previous = next
	if capacity < 2 || len(peers) < 2 || len(peers) < capacity || pending <= 0 || now.Before(s.nextRotation) {
		return nil
	}
	if victim != nil {
		s.nextRotation = now.Add(30 * time.Second)
		delete(s.previous, victim)
	}
	return victim
}

func (s *EngineServer) peerExplorationTick(now time.Time) {
	if !s.peerExploration.Load() {
		return
	}
	s.mu.RLock()
	records := make([]*TorrentRecord, 0, len(s.records))
	for _, rec := range s.records {
		records = append(records, rec)
	}
	capacity := s.maxPeers
	s.mu.RUnlock()
	for _, rec := range records {
		rec.mu.Lock()
		if !s.peerExploration.Load() || !s.payloadTransfersAllowed.Load() || !rec.Configured || rec.Paused || rec.Checking || rec.removed || rec.Error != nil || rec.Seeding || len(rec.Selected) == 0 {
			rec.peerExplorer = nil
			rec.mu.Unlock()
			continue
		}
		if rec.peerExplorer == nil {
			rec.peerExplorer = &peerExplorationState{}
		}
		if now.Before(rec.peerExplorer.nextSample) {
			rec.mu.Unlock()
			continue
		}
		stats := rec.Torrent.Stats()
		observations := make([]peerExplorationObservation, 0, stats.ActivePeers)
		for _, peer := range rec.Torrent.PeerConns() {
			progress := peer.Stats()
			observations = append(observations, peerExplorationObservation{
				peer: peer, useful: progress.BytesReadUsefulData.Int64(), uploaded: progress.BytesWrittenData.Int64(),
				protected: stats.ConnectedSeeders <= 1 && progress.RemotePieceCount >= rec.Torrent.NumPieces(),
			})
		}
		if victim := rec.peerExplorer.choose(now, observations, capacity, stats.PendingPeers); victim != nil {
			// A peer may have transferred data after its sample was collected.
			// Recheck immediately before releasing its slot.
			latest := victim.Stats()
			for _, observed := range observations {
				if observed.peer == victim && latest.BytesReadUsefulData.Int64() == observed.useful && latest.BytesWrittenData.Int64() == observed.uploaded {
					_ = victim.Close()
					s.notifyChange()
					break
				}
			}
		}
		rec.mu.Unlock()
	}
}
