package main

import (
	"fmt"
	"testing"
	"time"
)

// Model the real backlog: most hashes are waiting for their retry deadline,
// while workers repeatedly choose from a small number of ready arrivals.
func BenchmarkResolverDispatch(b *testing.B) {
	d := &dashboard{records: map[string]observation{}, pendingSet: map[string]struct{}{}}
	now := time.Now()
	for i := 0; i < 100_000; i++ {
		hash := fmt.Sprintf("%040x", i)
		d.records[hash] = observation{Hash: hash, FirstSeen: now, NextAttempt: now.Add(time.Hour)}
		d.pendingSet[hash] = struct{}{}
	}
	s := &sampler{d: d}
	b.ReportAllocs()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		// Simulate the aggregate polling clock of the worker pool, including
		// periodic backlog refresh rather than timing only a frozen cache hit.
		s.nextJobOf(now.Add(time.Duration(i)*time.Millisecond), jobCache)
	}
}
