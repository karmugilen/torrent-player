package main

import (
	"fmt"
	"math"
	"sort"
	"strings"
	"time"

	"golang.org/x/time/rate"
)

const defaultDialRate = 30

// ghostDialController probes spare dialing capacity, then compares useful
// payload throughput. This is feedback tuning, not a trained AI model. It
// never changes established connections, piece priorities or memory budgets.
type ghostDialController struct {
	cohort              string
	last                time.Time
	bytes               int64
	current, kept       int
	previous, reference float64
	trialTotal          float64
	trialSamples        int
	cooldown            time.Time
}

type ghostObservation struct {
	cohort        string
	usefulBytes   int64
	opportunity   bool // Waiting candidates and a free established-connection slot.
	downloadLimit int64
}

func (c *ghostDialController) observe(now time.Time, observation ghostObservation, base int) int {
	base = max(10, min(80, base))
	reset := func() int {
		*c = ghostDialController{cohort: observation.cohort, last: now, bytes: observation.usefulBytes, current: base, kept: base}
		return base
	}
	if !observation.opportunity || c.last.IsZero() || c.cohort != observation.cohort || observation.usefulBytes < c.bytes || now.Before(c.last) || now.Sub(c.last) > 30*time.Second {
		return reset()
	}
	elapsed := now.Sub(c.last)
	if elapsed < 10*time.Second {
		return c.current
	}
	speed := float64(observation.usefulBytes-c.bytes) / elapsed.Seconds()
	if observation.downloadLimit > 0 && speed >= 0.9*float64(observation.downloadLimit) {
		return reset() // Extra dialing cannot usefully beat the user's payload cap.
	}
	c.last, c.bytes = now, observation.usefulBytes
	backOff := func() int {
		c.current = c.kept
		c.reference, c.previous, c.trialTotal, c.trialSamples = 0, 0, 0, 0
		c.cooldown = now.Add(time.Minute)
		return c.current
	}
	if c.reference > 0 {
		c.trialTotal += speed
		c.trialSamples++
		// End a clearly harmful trial after one window; otherwise compare two.
		if speed < 0.8*c.reference {
			return backOff()
		}
		if c.trialSamples < 2 {
			return c.current
		}
		if c.trialTotal/float64(c.trialSamples) < 1.05*c.reference {
			return backOff()
		}
		c.kept = c.current
		c.reference, c.previous, c.trialTotal, c.trialSamples = 0, 0, 0, 0
		return c.current
	}
	if speed <= 0 {
		c.current, c.kept = base, base
		return backOff()
	}
	previous := c.previous
	c.previous = speed
	// Two reasonably stable windows avoid reacting to a single burst. The
	// ceiling is shared with Fast peer search, which already selects 80/s.
	if now.Before(c.cooldown) || c.current >= 80 || previous <= 0 || math.Abs(speed-previous) > 0.2*previous {
		return c.current
	}
	c.reference = (speed + previous) / 2
	c.current = min(80, c.kept+10)
	return c.current
}

// These helpers and the controller are protected by EngineServer.mu.
func (s *EngineServer) baseDialRateLocked() int {
	if s.dialRatePerSec == 0 {
		return defaultDialRate
	}
	return s.dialRatePerSec
}

func (s *EngineServer) setGhostDialRateLocked(perSecond int) {
	if s.dialLimiter != nil {
		s.dialLimiter.SetLimit(rate.Limit(perSecond))
		s.dialLimiter.SetBurst(perSecond)
	}
}

func (s *EngineServer) resetGhostLocked() {
	s.ghostGeneration++
	s.ghostDial = ghostDialController{}
	s.setGhostDialRateLocked(s.baseDialRateLocked())
}

func (s *EngineServer) ghostModeTick(now time.Time) {
	s.mu.RLock()
	if !s.ghostMode || s.dialLimiter == nil {
		s.mu.RUnlock()
		return
	}
	generation, capacity := s.ghostGeneration, s.maxPeers
	records := make([]*TorrentRecord, 0, len(s.records))
	for _, rec := range s.records {
		records = append(records, rec)
	}
	s.mu.RUnlock()
	var observation ghostObservation
	var cohort []string
	for _, rec := range records {
		rec.mu.RLock()
		if rec.Configured && !rec.Paused && !rec.Checking && !rec.removed && rec.Error == nil && !rec.Seeding && len(rec.Selected) > 0 {
			stats := rec.Torrent.Stats()
			if stats.PiecesComplete < rec.Torrent.NumPieces() {
				cohort = append(cohort, fmt.Sprintf("%s:%d", rec.ID, rec.Generation))
				observation.usefulBytes += stats.BytesReadUsefulData.Int64()
				observation.opportunity = observation.opportunity || (stats.PendingPeers > 0 && stats.ActivePeers < capacity)
			}
		}
		rec.mu.RUnlock()
	}
	sort.Strings(cohort)
	observation.cohort = strings.Join(cohort, ",")
	s.mu.Lock()
	defer s.mu.Unlock()
	// A switch, route or policy change invalidates observations collected
	// outside the map lock. Switching off always restores the latest base rate.
	if generation != s.ghostGeneration || !s.ghostMode {
		return
	}
	if !s.payloadTransfersAllowed.Load() {
		observation.opportunity = false
	}
	observation.downloadLimit = limiterRate(s.downloadLimiter)
	s.setGhostDialRateLocked(s.ghostDial.observe(now, observation, s.baseDialRateLocked()))
}
