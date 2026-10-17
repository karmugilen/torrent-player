package main

import (
	"container/heap"
	"encoding/json"
	"net/url"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"
)

// Browser payloads stay small even when the on-disk JSON store is huge.
const apiDefaultLimit = 150
const apiMaxLimit = 600

type viewQuery struct {
	Source  string
	terms   []string
	Q       string
	Sort    string
	Kind    string
	Trend   string
	Pending bool
	Located bool
	Country string
	Limit   int
	Offset  int
	TopOnly bool
}

func parseViewQuery(values url.Values) viewQuery {
	limit, _ := strconv.Atoi(values.Get("limit"))
	if limit <= 0 {
		limit = apiDefaultLimit
	}
	if limit > apiMaxLimit {
		limit = apiMaxLimit
	}
	offset, _ := strconv.Atoi(values.Get("offset"))
	if offset < 0 {
		offset = 0
	}
	if offset > 20_000 {
		offset = 20_000
	}
	return viewQuery{
		Q:       strings.TrimSpace(values.Get("q")),
		Source:  values.Get("source"),
		Sort:    strings.TrimSpace(values.Get("sort")),
		Kind:    strings.TrimSpace(values.Get("kind")),
		Trend:   strings.TrimSpace(values.Get("trend")),
		Pending: values.Get("pending") == "1" || values.Get("pending") == "true",
		Located: values.Get("located") == "1" || values.Get("located") == "true",
		Country: strings.ToUpper(strings.TrimSpace(values.Get("country"))),
		Limit:   limit,
		Offset:  offset,
		TopOnly: values.Get("top") == "1" || values.Get("top") == "true",
	}
}

func trendScoreObs(r observation, now time.Time) int64 {
	peers := r.Peers
	if peers < 0 {
		peers = 0
	}
	seeds := 0
	if r.Seeds != nil && *r.Seeds > 0 {
		seeds = *r.Seeds
	}
	located := 0
	for c := range r.Countries {
		if c != "" && c != "ZZ" && c != "Unlocated" {
			located++
		}
	}
	stamp := r.LastSeen
	if stamp.IsZero() {
		stamp = r.CheckedAt
	}
	if stamp.IsZero() {
		stamp = r.FirstSeen
	}
	ageH := now.Sub(stamp).Hours()
	if ageH < 0 {
		ageH = 0
	}
	fresh := 72 - ageH
	if fresh < 0 {
		fresh = 0
	}
	hits := r.SampleHits
	if hits > 10 {
		hits = 10
	}
	resolved := int64(0)
	if r.Resolved {
		resolved = 15
	}
	return int64(seeds)*5 + int64(peers)*2 + int64(located)*4 + int64(fresh) + resolved + int64(hits)
}

func withinTrend(r observation, trend string, now time.Time) bool {
	if trend == "" || trend == "all" {
		return true
	}
	stamp := r.LastSeen
	if !r.CheckedAt.IsZero() {
		stamp = r.CheckedAt
	}
	if stamp.IsZero() {
		stamp = r.FirstSeen
	}
	if stamp.IsZero() {
		return false
	}
	age := now.Sub(stamp)
	if trend == "today" {
		return age <= 24*time.Hour
	}
	if trend == "week" {
		return age <= 7*24*time.Hour
	}
	return true
}

func hasLocated(r observation) bool {
	for c := range r.Countries {
		if c != "" && c != "ZZ" && c != "Unlocated" {
			return true
		}
	}
	return false
}

func matchesView(r observation, q viewQuery, now time.Time) bool {
	if !q.Pending && !r.Resolved {
		return false
	}
	if q.Kind != "" && q.Kind != "all" && r.Kind != q.Kind {
		return false
	}
	if !withinTrend(r, q.Trend, now) {
		return false
	}
	if q.Located && !hasLocated(r) {
		return false
	}
	if q.Country != "" {
		if _, ok := r.Countries[q.Country]; !ok {
			return false
		}
	}
	if q.Source != "" && q.Source != "all" {
		switch q.Source {
		case "observed":
			if r.SampleHits == 0 {
				return false
			}
		case "verified":
			if r.Source != "peer" && r.Source != "cache" {
				return false
			}
		case "indexed":
			if r.Source != "dump" && r.Source != "catalog" {
				return false
			}
		default:
			if r.Source != q.Source {
				return false
			}
		}
	}
	if q.Q != "" {
		if validHash(q.Q) {
			return r.Hash == q.Q
		}
		text := normalizeSearchText(r.Name + " " + strings.Join(r.Files, " "))
		for _, term := range q.terms {
			if !strings.Contains(text, term) {
				return false
			}
		}
	}

	return true
}

func seedCountObs(r observation) int {
	if r.Seeds == nil {
		return -1
	}
	return *r.Seeds
}

func viewLess(a, b observation, q viewQuery, now time.Time) bool {
	// true if a should rank above b
	switch q.Sort {
	case "name":
		an, bn := a.Name, b.Name
		if an == "" {
			an = a.Hash
		}
		if bn == "" {
			bn = b.Hash
		}
		if an != bn {
			return an < bn
		}
	case "recent":
		at, bt := a.CheckedAt, b.CheckedAt
		if at.IsZero() {
			at = a.LastSeen
		}
		if bt.IsZero() {
			bt = b.LastSeen
		}
		if !at.Equal(bt) {
			return at.After(bt)
		}
	case "seeds":
		as, bs := seedCountObs(a), seedCountObs(b)
		if as != bs {
			return as > bs
		}
		if a.Peers != b.Peers {
			return a.Peers > b.Peers
		}
	case "trending":
		ta, tb := trendScoreObs(a, now), trendScoreObs(b, now)
		if ta != tb {
			return ta > tb
		}
	default:
		if q.Trend != "" && q.Trend != "all" {
			ta, tb := trendScoreObs(a, now), trendScoreObs(b, now)
			if ta != tb {
				return ta > tb
			}
		}
		if a.Peers != b.Peers {
			return a.Peers > b.Peers
		}
		as, bs := seedCountObs(a), seedCountObs(b)
		if as != bs {
			return as > bs
		}
	}
	ta, tb := trendScoreObs(a, now), trendScoreObs(b, now)
	if ta != tb {
		return ta > tb
	}
	if !a.LastSeen.Equal(b.LastSeen) {
		return a.LastSeen.After(b.LastSeen)
	}
	return a.Hash < b.Hash
}

type rankedHeap struct {
	items []observation
	q     viewQuery
	now   time.Time
}

func (h rankedHeap) Len() int { return len(h.items) }
func (h rankedHeap) Less(i, j int) bool {
	// Min-heap of the best items: root is the worst of the current best set.
	return viewLess(h.items[j], h.items[i], h.q, h.now)
}
func (h rankedHeap) Swap(i, j int) { h.items[i], h.items[j] = h.items[j], h.items[i] }
func (h *rankedHeap) Push(x any)   { h.items = append(h.items, x.(observation)) }
func (h *rankedHeap) Pop() any {
	n := len(h.items)
	x := h.items[n-1]
	h.items = h.items[:n-1]
	return x
}

func copyObservation(r observation) observation {
	copy := r
	copy.Files = append([]string(nil), r.Files...)
	if r.Seeds != nil {
		seeds := *r.Seeds
		copy.Seeds = &seeds
	}
	if r.Countries != nil {
		countryCopy := map[string]int{}
		for c, n := range r.Countries {
			countryCopy[c] = n
		}
		copy.Countries = countryCopy
	}
	return copy
}

// snapshotForAPI returns stats for the full store plus a small ranked window for
// the browser. The on-disk JSON keeps every observation.
func (d *dashboard) snapshotForAPI(q viewQuery) state {
	// Readers share the store via RLock so dump seed / resolvers can still write
	// between polls without waiting on a full exclusive scan.
	q.Q = normalizeSearchQuery(q.Q)
	q.terms = strings.Fields(normalizeSearchText(q.Q))
	if q.Limit <= 0 {
		q.Limit = apiDefaultLimit
	}
	d.mu.RLock()
	defer d.mu.RUnlock()
	now := time.Now().UTC()
	s := d.state
	s.Updated = now
	d.applyNameRatesLocked(&s, now)
	s.Hashes = len(d.records)
	s.RetentionLimit = retainedTorrentLimit
	if s.Completed > 0 {
		s.AverageLookupMS = d.totalLookupTime.Milliseconds() / int64(s.Completed)
	}
	s.Unresolved = len(d.pendingSet)
	s.CountryTotals = map[string]int{}
	// Search skips the full meta walk; browse rebuilds country totals.
	if q.Q == "" {
		s.Resolved = 0
		for _, r := range d.records {
			if r.Resolved {
				s.Resolved++
			}
			for c, n := range r.Countries {
				s.CountryTotals[c] += n
			}
		}
	} else {
		s.Resolved = s.Hashes - s.Unresolved
		if s.Resolved < 0 {
			s.Resolved = 0
		}
	}
	limit := q.Limit
	if q.TopOnly || (q.Trend != "" && q.Trend != "all") {
		if limit > 10 {
			limit = 10
		}
		q.Offset = 0
	}
	need := limit + q.Offset
	h := &rankedHeap{q: q, now: now, items: make([]observation, 0, need+1)}
	matchCount := 0
	push := func(r observation) {
		if !matchesView(r, q, now) {
			return
		}
		matchCount++
		// Store shallow candidates while holding the read lock. Deep-copy
		// only the chosen window, not every matching row in the catalog.
		item := r
		if h.Len() < need {
			heap.Push(h, item)
			return
		}
		if viewLess(item, h.items[0], q, now) {
			h.items[0] = item
			heap.Fix(h, 0)
		}
	}
	if q.Q != "" {
		if hashes, indexed := d.searchCandidateHashes(q.Q); indexed {
			for _, hash := range hashes {
				r, ok := d.records[hash]
				if !ok {
					continue
				}
				push(r)
			}
		} else {
			for _, r := range d.records {
				push(r)
			}
		}
	} else {
		for _, r := range d.records {
			push(r)
		}
	}
	s.MatchCount = matchCount
	sort.Slice(h.items, func(i, j int) bool {
		return viewLess(h.items[i], h.items[j], q, now)
	})
	// Always emit a non-nil slice so JSON clients see [] instead of null.
	s.Records = []observation{}
	if q.Offset < len(h.items) {
		end := q.Offset + limit
		if end > len(h.items) {
			end = len(h.items)
		}
		for _, r := range h.items[q.Offset:end] {
			s.Records = append(s.Records, copyObservation(r))
		}
	}
	return s
}

// persistAll writes the full observation map to discovery.json without building
// a browser-sized API payload.
func (d *dashboard) persistAll() {
	d.persistMu.Lock()
	defer d.persistMu.Unlock()
	d.mu.Lock()
	records := make([]observation, 0, len(d.records))
	for _, r := range d.records {
		records = append(records, copyObservation(r))
	}
	path := d.cachePath
	d.mu.Unlock()
	b, err := json.Marshal(records)
	if err != nil {
		return
	}
	if os.MkdirAll(filepath.Dir(path), 0700) != nil {
		return
	}
	tmp := path + ".tmp"
	if os.WriteFile(tmp, b, 0600) == nil {
		_ = os.Rename(tmp, path)
	}
}
