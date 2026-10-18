package main

import (
	"strings"
	"time"
)

// namePatch is the small interface for naming an observation. Dump, catalog,
// cache, and peer adapters produce patches; applyNameLocked owns merge policy.
type namePatch struct {
	Hash         string
	Name         string
	Source       string
	Size         int64
	Seeds        *int
	PeerBump     int
	Files        []string
	Kind         string
	ResolutionMS int64
}

// applyNameLocked merges a name into the store. Caller must hold d.mu for write.
// Returns true when this call newly resolves a previously unnamed hash.
func (d *dashboard) applyNameLocked(p namePatch, now time.Time) bool {
	if !validHash(p.Hash) || p.Name == "" {
		return false
	}
	rec, exists := d.records[p.Hash]
	if !exists {
		rec = observation{Hash: p.Hash, FirstSeen: now, Countries: map[string]int{}}
	}
	wasResolved := rec.Resolved
	verified := func(source string) bool { return source == "cache" || source == "peer" }
	// In-flight exact metadata outranks a catalog label that arrived first.
	// No new network job is scheduled just to upgrade an already named row.
	upgrade := verified(p.Source) && !verified(rec.Source)
	if wasResolved && rec.Name != "" && !upgrade {
		if p.PeerBump > rec.Peers {
			rec.Peers = p.PeerBump
		}
		if p.Seeds != nil {
			rec.Seeds = p.Seeds
		}
		if p.Size > 0 && rec.Size == 0 {
			rec.Size = p.Size
		}
		if rec.Source == "" && p.Source != "" {
			rec.Source = p.Source
		}
		rec.LastSeen = now
		d.records[p.Hash] = rec
		return false
	}
	rec.Name = p.Name
	if rec.NameHint != "" && !wasResolved && d.state.FeedHints > 0 {
		d.state.FeedHints--
	}
	rec.NameHint = ""
	rec.Resolved = true
	rec.Source = p.Source
	rec.CheckedAt = now
	rec.LastSeen = now
	rec.NextAttempt = time.Time{}
	if p.ResolutionMS > 0 {
		rec.ResolutionMS = p.ResolutionMS
	}
	if p.Size > 0 {
		rec.Size = p.Size
	}
	if p.PeerBump > rec.Peers {
		rec.Peers = p.PeerBump
	}
	if p.Seeds != nil {
		rec.Seeds = p.Seeds
	}
	if len(p.Files) > 0 {
		rec.Files = append([]string(nil), p.Files...)
	}
	if p.Kind != "" {
		rec.Kind = p.Kind
	} else {
		rec.Kind = catalogKind(p.Name)
	}
	d.records[p.Hash] = rec
	d.markResolvedLocked(p.Hash)
	d.indexNameLocked(p.Hash, rec.Name+" "+strings.Join(rec.Files, " "))
	if wasResolved {
		return false
	}
	d.state.Successes++
	d.recordNameRateLocked(p.Source, exists && rec.SampleHits > 0, now)
	switch p.Source {
	case "dump":
		d.state.DumpHits++
	case "catalog":
		d.state.CatalogHits++
	case "cache":
		d.state.CacheHits++
	case "peer":
		d.state.PeerHits++
	}
	return true
}

func (d *dashboard) markPendingLocked(hash string) {
	if d.pendingSet == nil {
		d.pendingSet = map[string]struct{}{}
	}
	if rec, ok := d.records[hash]; ok && rec.Resolved {
		return
	}
	if _, exists := d.pendingSet[hash]; !exists {
		d.pendingSet[hash] = struct{}{}
		d.pendingRevision++
	}
}

func (d *dashboard) markResolvedLocked(hash string) {
	if _, exists := d.pendingSet[hash]; exists {
		delete(d.pendingSet, hash)
		d.pendingRevision++
	}
}

// upsertSampleLocked records a DHT sighting. Returns whether the hash still
// needs a name (caller may try dump lookup after releasing the lock).
func (d *dashboard) upsertSampleLocked(hash string, now time.Time) bool {
	rec, ok := d.records[hash]
	if !ok {
		rec = observation{Hash: hash, FirstSeen: now, Countries: map[string]int{}}
	}
	rec.LastSeen = now
	rec.SampleHits++
	d.records[hash] = rec
	if rec.Resolved {
		return false
	}
	d.markPendingLocked(hash)
	return true
}
