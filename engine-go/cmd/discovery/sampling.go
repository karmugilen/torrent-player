package main

import (
	"context"
	"encoding/hex"
	"net"
	"time"

	"github.com/anacrolix/dht/v2/krpc"
)

const samplingConcurrency = 16

// Free slots are refilled independently: a dead node cannot hold up replies
// from healthy nodes or force an entire batch to wait for its timeout.
func (s *sampler) sampleBatch(ctx context.Context) {
	if ctx.Err() != nil {
		return
	}
	now := time.Now()
	var batch []krpc.NodeInfo
	s.mu.Lock()
nodes:
	for addr, node := range s.nodes {
		if now.Before(s.nextSample[addr]) {
			continue
		}
		select {
		case s.samplingSlots <- struct{}{}:
			s.nextSample[addr] = now.Add(10 * time.Minute)
			batch = append(batch, node)
		default:
			break nodes
		}
	}
	s.mu.Unlock()
	for _, node := range batch {
		s.samplingWork.Add(1)
		go func(node krpc.NodeInfo) {
			defer s.samplingWork.Done()
			defer func() { <-s.samplingSlots }()
			s.sampleNode(ctx, node)
		}(node)
	}
}

func (s *sampler) sampleNode(ctx context.Context, node krpc.NodeInfo) {
	addr := &net.UDPAddr{IP: node.Addr.IP, Port: node.Addr.Port}
	reply := s.lookup(ctx, addr, "sample_infohashes", krpc.MsgArgs{Target: randomID()})
	if reply == nil || reply.Samples == nil {
		// A responsive node without BEP 51 can still route us toward new nodes.
		if reply != nil && ctx.Err() == nil {
			s.lookup(ctx, addr, "find_node", krpc.MsgArgs{Target: randomID()})
		}
		return
	}
	interval := 10 * time.Minute
	if reply.Interval != nil {
		interval = time.Duration(max(60, min(21600, *reply.Interval))) * time.Second
	}
	s.mu.Lock()
	s.nextSample[node.Addr.String()] = time.Now().Add(interval)
	s.mu.Unlock()
	for _, hash := range *reply.Samples {
		s.rememberSource(hex.EncodeToString(hash[:]), node)
	}
	now := time.Now().UTC()
	s.d.mu.Lock()
	s.d.state.SamplingNodes++
	s.d.state.HashSamples += int64(len(*reply.Samples))
	s.d.state.Status = "Sampling public torrents and resolving names in parallel"
	for _, hash := range *reply.Samples {
		h := hex.EncodeToString(hash[:])
		rec, ok := s.d.records[h]
		if !ok {
			rec = observation{Hash: h, FirstSeen: now, Countries: map[string]int{}}
		}
		rec.LastSeen = now
		rec.SampleHits++
		s.d.records[h] = rec
	}
	s.d.pruneLocked()
	s.d.mu.Unlock()
}

// Explore fresh random regions of the DHT even when known sample providers
// have long advertised intervals. This never bypasses a provider's interval.
func (s *sampler) refreshRoutes(ctx context.Context) {
	if ctx.Err() != nil || time.Now().Before(s.nextRefresh) {
		return
	}
	s.mu.Lock()
	var node krpc.NodeInfo
	found := false
	for _, candidate := range s.nodes {
		node = candidate
		found = true
		break
	}
	s.mu.Unlock()
	if !found {
		// Retry a cold bootstrap instead of leaving discovery empty after a
		// temporary DNS/router failure. Resolver jobs cannot block the first
		// bootstrap because it runs before those workers are started.
		select {
		case s.samplingSlots <- struct{}{}:
		default:
			return
		}
		s.nextRefresh = time.Now().Add(10 * time.Second)
		s.samplingWork.Add(1)
		go func() { defer s.samplingWork.Done(); defer func() { <-s.samplingSlots }(); s.bootstrapNodes(ctx) }()
		return
	}
	select {
	case s.samplingSlots <- struct{}{}:
	default:
		return
	}
	s.nextRefresh = time.Now().Add(3 * time.Second)
	s.samplingWork.Add(1)
	go func() {
		defer s.samplingWork.Done()
		defer func() { <-s.samplingSlots }()
		s.lookup(ctx, &net.UDPAddr{IP: node.Addr.IP, Port: node.Addr.Port}, "find_node", krpc.MsgArgs{Target: randomID()})
	}()
}
