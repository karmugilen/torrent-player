package main

import (
	"slices"
	"sync"
	"time"

	"github.com/anacrolix/torrent"
)

// Claude optimized seek mode. Three parts, all off unless the mode is on:
//   - urgent blocks outstanding longer than claudeSeekStealAfter (750 ms) move to a
//     peer that is currently delivering (patched library, Client-wide);
//   - while a playback read is blocked, peers with spare request slots that
//     hold an urgent piece are nudged every claudeSeekNudgeEvery so the move
//     happens without waiting for their pipeline to drain;
//   - stalled reads refresh their urgent request after 3 s instead of 8 s,
//     keeping the existing 45 s total wait.
//
// Seek latency stats are recorded in every mode so on and off can be compared.
const seekStatsWindow = 20

// Seek tuning; variables so the seek search can try other values.
var (
	claudeSeekStealAfter = 500 * time.Millisecond
	claudeSeekNudgeEvery = 100 * time.Millisecond
)

var claudeSeekReadAttempts = []time.Duration{3 * time.Second, 6 * time.Second, 12 * time.Second, 24 * time.Second}

type SeekStats struct {
	Count    int   `json:"count"`
	LastMs   int64 `json:"lastMs"`
	MedianMs int64 `json:"medianMs"`
	Stalls   int   `json:"stalls"`
}

// seekLatencyStats keeps the recent seek-to-first-byte times since the last reset.
type seekLatencyStats struct {
	mu     sync.Mutex
	recent []time.Duration
	count  int
	stalls int
}

func (s *seekLatencyStats) recordSeek(d time.Duration) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.count++
	s.recent = append(s.recent, max(d, 0))
	if len(s.recent) > seekStatsWindow {
		s.recent = s.recent[len(s.recent)-seekStatsWindow:]
	}
}

func (s *seekLatencyStats) recordStall() {
	s.mu.Lock()
	s.stalls++
	s.mu.Unlock()
}

func (s *seekLatencyStats) reset() {
	s.mu.Lock()
	s.recent, s.count, s.stalls = nil, 0, 0
	s.mu.Unlock()
}

func (s *seekLatencyStats) snapshot() SeekStats {
	s.mu.Lock()
	defer s.mu.Unlock()
	out := SeekStats{Count: s.count, Stalls: s.stalls}
	if n := len(s.recent); n > 0 {
		out.LastMs = s.recent[n-1].Milliseconds()
		sorted := slices.Clone(s.recent)
		slices.Sort(sorted)
		out.MedianMs = sorted[n/2].Milliseconds()
	}
	return out
}

func (s *EngineServer) isClaudeSeekEnabled() bool {
	return s.claudeSeek.Load()
}

// setClaudeSeekLocked applies the mode to the client and starts a fresh
// comparison window. Caller holds s.mu.
func (s *EngineServer) setClaudeSeekLocked(enabled bool) {
	if s.claudeSeek.Swap(enabled) == enabled {
		return
	}
	after := time.Duration(0)
	if enabled {
		after = claudeSeekStealAfter
	}
	if s.client != nil {
		s.client.SetUrgentRequestStealing(after)
	}
	s.seekStats.reset()
}

// baseSeekTuning is the shipped seek rules with the user's experimental
// switches applied.
func (s *EngineServer) baseSeekTuning() torrent.SeekTuning {
	tuning := torrent.DefaultSeekTuning
	tuning.Hedge = s.hedgeRequests.Load()
	tuning.SlowPeerYield = s.slowPeerYield.Load()
	tuning.LearnedArrival = s.learnedArrival.Load()
	return tuning
}

// chokeTimeout is how long a peer may keep us choked while we want its data
// before dropChokersLoop frees its slot (WebTorrent's CHOKE_TIMEOUT).
const chokeTimeout = 5 * time.Second

// dropChokersLoop drops, once a second, peers that keep us choked, when the
// setting is on.
func (s *EngineServer) dropChokersLoop() {
	ticker := time.NewTicker(time.Second)
	defer ticker.Stop()
	for {
		select {
		case <-s.done:
			return
		case <-ticker.C:
		}
		if !s.dropChokers.Load() {
			continue
		}
		s.mu.RLock()
		torrents := make([]*torrent.Torrent, 0, len(s.records))
		for _, rec := range s.records {
			if rec.Torrent != nil {
				torrents = append(torrents, rec.Torrent)
			}
		}
		s.mu.RUnlock()
		for _, t := range torrents {
			t.WebtorDropChokers(chokeTimeout)
		}
	}
}

func (p *playbackReader) engine() *EngineServer {
	if p.priorityOwner == nil {
		return nil
	}
	return p.priorityOwner.server
}

// startUrgentNudger nudges t's peers until the returned stop is called.
func startUrgentNudger(t *torrent.Torrent, every time.Duration) (stop func()) {
	done := make(chan struct{})
	go func() {
		ticker := time.NewTicker(every)
		defer ticker.Stop()
		for {
			select {
			case <-done:
				return
			case <-ticker.C:
				t.NudgeUrgentRequests()
			}
		}
	}()
	return func() { close(done) }
}
