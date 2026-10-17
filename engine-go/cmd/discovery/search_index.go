package main

import (
	"strings"
	"unicode"
)

// Cap posting lists so ultra-common tokens (e.g. "1080") do not explode RAM.
// Over-cap tokens fall back to a scanned match for that query.
const searchPostingCap = 40_000

func tokenizeName(name string) []string {
	name = strings.ToLower(strings.TrimSpace(name))
	if name == "" {
		return nil
	}
	parts := strings.FieldsFunc(name, func(r rune) bool {
		return !unicode.IsLetter(r) && !unicode.IsDigit(r)
	})
	out := make([]string, 0, len(parts))
	seen := map[string]bool{}
	for _, p := range parts {
		if len(p) < 2 || len(p) > 48 {
			continue
		}
		if seen[p] {
			continue
		}
		seen[p] = true
		out = append(out, p)
		if len(out) >= 16 {
			break
		}
	}
	return out
}

func (d *dashboard) ensureSearchIndexLocked() {
	if d.searchTok != nil {
		return
	}
	d.searchTok = map[string][]string{}
	d.searchCommon = map[string]bool{}
}

func (d *dashboard) indexNameLocked(hash, name string) {
	d.ensureSearchIndexLocked()
	for _, tok := range tokenizeName(name) {
		if d.searchCommon[tok] {
			// Keep the capped posting list; stop growing it.
			continue
		}
		list := d.searchTok[tok]
		if len(list) >= searchPostingCap {
			d.searchCommon[tok] = true
			continue
		}
		d.searchTok[tok] = append(list, hash)
	}
}

// searchCandidateHashes returns hashes likely matching q via the token index.
// ok=false means fall back to a full store scan for this query.
func (d *dashboard) searchCandidateHashes(q string) (hashes []string, ok bool) {
	q = strings.TrimSpace(strings.ToLower(q))
	if q == "" {
		return nil, false
	}
	// Exact infohash lookup skips the name index.
	if validHash(q) {
		return []string{q}, true
	}
	tokens := tokenizeName(q)
	if len(tokens) == 0 {
		return nil, false
	}
	d.ensureSearchIndexLocked()
	// Prefer the rarest token that still has a posting list (common tokens keep
	// a capped list so queries like "1080" stay indexed instead of full-scan).
	bestTok := ""
	bestLen := int(^uint(0) >> 1)
	for _, tok := range tokens {
		list := d.searchTok[tok]
		if len(list) == 0 {
			continue
		}
		if len(list) < bestLen {
			bestLen = len(list)
			bestTok = tok
			hashes = list
		}
	}
	if bestTok == "" {
		// No posting lists (unknown token or empty index) — scan the store.
		return nil, false
	}
	return hashes, true
}

func rebuildSearchIndex(d *dashboard) {
	d.mu.Lock()
	defer d.mu.Unlock()
	d.searchTok = map[string][]string{}
	d.searchCommon = map[string]bool{}
	for hash, rec := range d.records {
		if rec.Resolved && rec.Name != "" {
			d.indexNameLocked(hash, rec.Name)
		}
	}
}
