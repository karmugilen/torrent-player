package main

import (
	"context"
	"sync"
	"sync/atomic"
	"time"

	"github.com/anacrolix/dht/v2"
	"github.com/anacrolix/torrent/metainfo"
)

type peerPipelineStats struct {
	active, lookups, chunks              atomic.Int64
	queued, metadata, connected, dialing atomic.Int64
	lookupCount, lookupMS, peerLookups   atomic.Int64
}

const lookupPeerTarget = 12
const lookupPeerSettle = 750 * time.Millisecond

// DHT traversal has its own admission and deadline. A slow traversal cannot
// hold up known peer dialing or publisher metadata. Results stream to the
// consumer as they arrive; cancellation joins this worker before dropping t.
func (s *sampler) streamPeerLookup(ctx context.Context, hash metainfo.Hash, delay bool, work *sync.WaitGroup) (<-chan dht.PeersValues, <-chan struct{}) {
	results := make(chan dht.PeersValues, 8)
	started := make(chan struct{})
	work.Add(1)
	go func() {
		defer work.Done()
		defer close(results)
		if delay {
			timer := time.NewTimer(350 * time.Millisecond)
			defer timer.Stop()
			select {
			case <-timer.C:
			case <-ctx.Done():
				return
			}
		}
		if s.peerLookups != nil {
			if s.pipeline != nil {
				s.pipeline.queued.Add(1)
			}
			select {
			case s.peerLookups <- struct{}{}:
				defer func() { <-s.peerLookups }()
			case <-ctx.Done():
				if s.pipeline != nil {
					s.pipeline.queued.Add(-1)
				}
				return
			}
			if s.pipeline != nil {
				s.pipeline.queued.Add(-1)
			}
		}
		close(started)
		lookupStart := time.Now()
		if s.pipeline != nil {
			s.pipeline.lookups.Add(1)
			defer func() {
				s.pipeline.lookups.Add(-1)
				s.pipeline.lookupCount.Add(1)
				s.pipeline.lookupMS.Add(time.Since(lookupStart).Milliseconds())
			}()
		}
		lookupCtx, cancel := context.WithCancel(ctx)
		defer cancel()
		emptyTimer := time.NewTimer(3 * time.Second)
		defer emptyTimer.Stop()
		var emptyDeadline <-chan time.Time = emptyTimer.C
		foundPeers := false
		endpoints := make(map[string]bool)
		var settleTimer *time.Timer
		var settled <-chan time.Time
		defer func() {
			if settleTimer != nil {
				settleTimer.Stop()
			}
		}()
		ann, err := s.server.AnnounceTraversal(hash, dht.Scrape())
		if err != nil {
			return
		}
		defer ann.Close()
		for {
			select {
			case <-lookupCtx.Done():
				return
			case <-settled:
				return
			case <-emptyDeadline:
				// A peer reply gets a short gathering window below; empty
				// searches end without occupying metadata-fetch capacity.
				if !foundPeers {
					return
				}
				emptyDeadline = nil
			case peers, ok := <-ann.Peers:
				if !ok {
					return
				}
				for _, peer := range peers.Peers {
					if publicIP(peer.IP) && peer.Port > 0 {
						endpoints[peer.String()] = true
					}
				}
				if !foundPeers && len(endpoints) > 0 {
					foundPeers = true
					if s.pipeline != nil {
						s.pipeline.peerLookups.Add(1)
					}
					settleTimer = time.NewTimer(lookupPeerSettle)
					settled = settleTimer.C
				}
				select {
				case results <- peers:
				case <-lookupCtx.Done():
					return
				}
				// Dialing/metadata now runs independently. More DHT replies for
				// this swarm must not monopolize a scarce traversal slot.
				if len(endpoints) >= lookupPeerTarget {
					return
				}
			}
		}
	}()
	return results, started
}
