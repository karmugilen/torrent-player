package main

import (
	"sync/atomic"
	"time"
)

type sourceMeter struct {
	requests, hits, misses, failures, cancelled, milliseconds atomic.Int64
	latency                                                   [8]atomic.Int64
}
type sourceState struct {
	Name      string `json:"name"`
	Requests  int64  `json:"requests"`
	Hits      int64  `json:"hits"`
	Misses    int64  `json:"misses"`
	Failures  int64  `json:"failures"`
	Cancelled int64  `json:"cancelled"`
	AverageMS int64  `json:"averageMs"`
	P95MS     int64  `json:"p95Ms"`
}

var latencyBounds = [8]int64{100, 250, 500, 1000, 2500, 5000, 12000, 60000}

func (m *sourceMeter) record(start time.Time, outcome string) {
	m.requests.Add(1)
	switch outcome {
	case "hit":
		m.hits.Add(1)
	case "miss":
		m.misses.Add(1)
	case "cancelled":
		m.cancelled.Add(1)
	default:
		m.failures.Add(1)
	}
	ms := time.Since(start).Milliseconds()
	m.milliseconds.Add(ms)
	for i, bound := range latencyBounds {
		if ms <= bound || i == len(latencyBounds)-1 {
			m.latency[i].Add(1)
			break
		}
	}
}
func (m *sourceMeter) snapshot(name string) sourceState {
	s := sourceState{Name: name, Requests: m.requests.Load(), Hits: m.hits.Load(), Misses: m.misses.Load(), Failures: m.failures.Load(), Cancelled: m.cancelled.Load()}
	if s.Requests > 0 {
		s.AverageMS = m.milliseconds.Load() / s.Requests
	}
	var count, total int64
	var buckets [8]int64
	for i := range buckets {
		buckets[i] = m.latency[i].Load()
		total += buckets[i]
	}
	for i, n := range buckets {
		count += n
		if total > 0 && count*100 >= total*95 {
			s.P95MS = latencyBounds[i]
			break
		}
	}
	return s
}
