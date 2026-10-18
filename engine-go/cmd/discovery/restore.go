package main

import (
	"encoding/json"
	"os"
	"strings"
	"time"
)

// Decode one record at a time: startup no longer retains both a full JSON byte
// buffer and a second million-record slice while building the live store.
func (d *dashboard) restoreRecords() {
	f, err := os.Open(d.cachePath)
	if err != nil {
		return
	}
	defer f.Close()
	decoder := json.NewDecoder(f)
	if token, err := decoder.Token(); err != nil || token != json.Delim('[') {
		return
	}
	now := time.Now()
	for decoder.More() {
		var r observation
		if decoder.Decode(&r) != nil {
			return
		}
		if !validHash(r.Hash) {
			continue
		}
		keepFor := unresolvedRetention
		if r.Resolved && strings.TrimSpace(r.Name) != "" {
			keepFor = namedRetention
		}
		if now.Sub(r.LastSeen) >= keepFor {
			continue
		}
		if r.Countries == nil {
			r.Countries = map[string]int{}
		}
		d.records[r.Hash] = r
		if !r.Resolved {
			d.pendingSet[r.Hash] = struct{}{}
		}
		if r.Name != "" || r.NameHint != "" {
			d.indexNameLocked(r.Hash, r.Name+" "+r.NameHint+" "+strings.Join(r.Files, " "))
		}
		if !r.Resolved && r.NameHint != "" {
			d.state.FeedHints++
		}
	}
}
