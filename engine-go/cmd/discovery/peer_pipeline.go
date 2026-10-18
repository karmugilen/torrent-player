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
	active, lookups, chunks atomic.Int64
}

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
			select {
			case s.peerLookups <- struct{}{}:
				defer func() { <-s.peerLookups }()
			case <-ctx.Done():
				return
			}
		}
		close(started)
		if s.pipeline != nil {
			s.pipeline.lookups.Add(1)
			defer s.pipeline.lookups.Add(-1)
		}
		lookupCtx, cancel := context.WithTimeout(ctx, 3*time.Second)
		defer cancel()
		ann, err := s.server.AnnounceTraversal(hash, dht.Scrape())
		if err != nil {
			return
		}
		defer ann.Close()
		for {
			select {
			case <-lookupCtx.Done():
				return
			case peers, ok := <-ann.Peers:
				if !ok {
					return
				}
				select {
				case results <- peers:
				case <-lookupCtx.Done():
					return
				}
			}
		}
	}()
	return results, started
}
