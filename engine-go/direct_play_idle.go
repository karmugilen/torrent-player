package main

import (
	"sync"
	"sync/atomic"
	"time"

	"github.com/anacrolix/torrent"
)

// Idle Direct Play. With an idle limit set, a configured, unpaused Direct Play
// torrent that has had no playback read for directPlayRestAfter "rests": its
// peer connections close and no new ones open, while RAM pieces and the known
// peer list stay. The next playback read wakes it at the normal connection
// limit. Once it has rested for the full idle limit with no player attached,
// the view reports idleDropReady and Android drops the session.
const (
	directPlayRestAfter = 2 * time.Minute
	restPeerLimit       = 80
)

type directPlayIdleState struct {
	lastActivity atomic.Int64 // unix nanoseconds; 0 until first observed
	reading      atomic.Int32
	resting      atomic.Bool
	dropReady    atomic.Bool
	// The library forgets connected peers' addresses, so resting keeps the
	// dialable ones for the wake-up.
	peersMu   sync.Mutex
	restPeers []torrent.PeerInfo
	restLimit int // connection limit replaced by the rest
}

func (st *directPlayIdleState) touch(now time.Time) { st.lastActivity.Store(now.UnixNano()) }

// beginPlaybackRead marks rec active and wakes it before the read waits for
// pieces. The returned func ends the read.
func (s *EngineServer) beginPlaybackRead(rec *TorrentRecord) func() {
	st := &rec.playbackIdle
	st.reading.Add(1)
	st.touch(time.Now())
	st.dropReady.Store(false)
	s.wakeDirectPlayFromRead(rec)
	return func() {
		st.touch(time.Now())
		st.reading.Add(-1)
	}
}

// wakeDirectPlayFromRead wakes rec without waiting for rec.mu. Pause, remove
// and preview close hold rec.mu while they join a reader's I/O, so a read
// that waited for the lock here would deadlock the torrent. The read itself
// waits for pieces, which arrive once the wake has run.
func (s *EngineServer) wakeDirectPlayFromRead(rec *TorrentRecord) {
	if rec.playbackIdle.resting.Load() {
		go s.wakeDirectPlay(rec)
	}
}

// wakeDirectPlay restores rec's peer connections if it was resting. It takes
// rec.mu; never call it from a read path (see wakeDirectPlayFromRead).
func (s *EngineServer) wakeDirectPlay(rec *TorrentRecord) bool {
	if !rec.playbackIdle.resting.CompareAndSwap(true, false) {
		return false
	}
	st := &rec.playbackIdle
	st.peersMu.Lock()
	known, limit := st.restPeers, st.restLimit
	st.restPeers = nil
	st.peersMu.Unlock()
	rec.mu.RLock()
	removed := rec.removed
	known = append(known, rec.reconnectPeers...)
	rec.mu.RUnlock()
	if removed {
		return false
	}
	rec.Torrent.AddPeers(known)
	s.mu.RLock()
	if s.maxPeers > 0 {
		limit = s.maxPeers
	}
	s.mu.RUnlock()
	rec.Torrent.SetMaxEstablishedConns(max(limit, 1))
	s.notifyChange()
	return true
}

// restDirectPlay closes rec's peer connections, keeping their dial addresses.
func restDirectPlay(rec *TorrentRecord) {
	var known []torrent.PeerInfo
	for _, pc := range rec.Torrent.PeerConns() {
		if len(known) >= restPeerLimit {
			break
		}
		if addr, ok := pc.CacheDialAddr(); ok && addr != nil {
			known = append(known, torrent.PeerInfo{Addr: addr, Source: pc.Discovery, Trusted: true})
		}
	}
	limit := rec.Torrent.SetMaxEstablishedConns(0)
	rec.playbackIdle.peersMu.Lock()
	rec.playbackIdle.restPeers, rec.playbackIdle.restLimit = known, limit
	rec.playbackIdle.peersMu.Unlock()
}

func (s *EngineServer) openPlaybackReaders(t *torrent.Torrent) int {
	s.playbackMu.Lock()
	owner := s.playbackOwners[t]
	s.playbackMu.Unlock()
	if owner == nil {
		return 0
	}
	owner.mu.Lock()
	defer owner.mu.Unlock()
	return len(owner.readers)
}

func (s *EngineServer) directPlayIdleLimit() time.Duration {
	return time.Duration(s.idleDropMinutes.Load()) * time.Minute
}

func (s *EngineServer) directPlayIdleTick(now time.Time) {
	limit := s.directPlayIdleLimit()
	restAfter := min(directPlayRestAfter, limit)
	s.mu.RLock()
	records := make([]*TorrentRecord, 0, len(s.records))
	for _, rec := range s.records {
		records = append(records, rec)
	}
	s.mu.RUnlock()
	for _, rec := range records {
		rec.mu.RLock()
		eligible := rec.DirectPlay && rec.Configured && !rec.Paused && !rec.removed && rec.Error == nil
		rec.mu.RUnlock()
		st := &rec.playbackIdle
		if !eligible || limit <= 0 {
			s.wakeDirectPlay(rec)
			st.dropReady.Store(false)
			st.lastActivity.Store(0)
			continue
		}
		if st.lastActivity.Load() == 0 {
			st.touch(now)
		}
		if st.reading.Load() > 0 {
			continue
		}
		idle := now.Sub(time.Unix(0, st.lastActivity.Load()))
		if idle >= restAfter && st.resting.CompareAndSwap(false, true) {
			restDirectPlay(rec)
		}
		st.dropReady.Store(st.resting.Load() && idle >= limit && s.openPlaybackReaders(rec.Torrent) == 0)
	}
}
