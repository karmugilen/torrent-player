package main

import (
	"context"
	"fmt"
	"net"
	"sync/atomic"
	"testing"
	"time"

	"github.com/anacrolix/dht/v2/krpc"
)

func TestSamplingRefillsFreeSlotsWithoutWaitingForSlowNodes(t *testing.T) {
	d := testDashboard(t)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	slowStarted, releaseSlow := make(chan struct{}), make(chan struct{})
	var calls atomic.Int32
	s := &sampler{d: d, nodes: map[string]krpc.NodeInfo{}, nextSample: map[string]time.Time{}, samplingSlots: make(chan struct{}, 2)}
	s.lookup = func(ctx context.Context, a *net.UDPAddr, q string, args krpc.MsgArgs) *krpc.Return {
		calls.Add(1)
		if a.Port == 1 {
			close(slowStarted)
			select {
			case <-ctx.Done():
			case <-releaseSlow:
			}
		}
		interval := int64(120)
		samples := krpc.CompactInfohashes{{byte(a.Port)}}
		reply := &krpc.Return{}
		reply.Samples = &samples
		reply.Interval = &interval
		return reply
	}
	for port := 1; port <= 2; port++ {
		node := krpc.NodeInfo{Addr: krpc.NodeAddr{IP: net.ParseIP("8.8.8.8"), Port: port}}
		s.nodes[node.Addr.String()] = node
	}
	returned := make(chan struct{})
	go func() { s.sampleBatch(ctx); close(returned) }()
	select {
	case <-returned:
	case <-time.After(time.Second):
		close(releaseSlow)
		t.Fatal("slow DHT node blocked sampling scheduler")
	}
	<-slowStarted
	deadline := time.Now().Add(time.Second)
	for len(s.samplingSlots) == 2 && time.Now().Before(deadline) {
		time.Sleep(time.Millisecond)
	}
	node := krpc.NodeInfo{Addr: krpc.NodeAddr{IP: net.ParseIP("8.8.4.4"), Port: 3}}
	s.mu.Lock()
	s.nodes[node.Addr.String()] = node
	s.mu.Unlock()
	s.sampleBatch(ctx)
	for calls.Load() < 3 && time.Now().Before(deadline) {
		time.Sleep(time.Millisecond)
	}
	close(releaseSlow)
	s.samplingWork.Wait()
	if calls.Load() != 3 {
		t.Fatal("free sampling slot was not refilled")
	}
	if d.snapshot().HashSamples != 3 {
		t.Fatal("sample receipt count missing")
	}
	s.sampleBatch(ctx)
	s.samplingWork.Wait()
	if calls.Load() != 3 {
		t.Fatal("sampling ignored advertised interval")
	}
	if d.snapshot().Hashes != 3 {
		t.Fatalf("lost sampled hashes: %s", fmt.Sprint(d.snapshot().Hashes))
	}
}
