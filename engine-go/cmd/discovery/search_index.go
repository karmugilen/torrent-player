package main

import (
	"golang.org/x/text/unicode/norm"
	"net/url"
	"strings"
	"unicode"
)

// Cap posting lists so ultra-common tokens (e.g. "1080") do not explode RAM.
// Over-cap tokens fall back to a scanned match for that query.
const searchPostingCap = 40_000

type nativeSearchIndex interface {
	add(hash, name string)
	candidates(query string) ([]string, bool)
}

func tokenizeName(name string) []string {
	name = normalizeSearchText(name)
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
	if d.nativeSearch != nil {
		d.nativeSearch.add(hash, name)
		return
	}
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
	q = normalizeSearchQuery(q)
	if q == "" {
		return nil, false
	}
	// Exact infohash lookup skips the name index.
	if validHash(q) {
		return []string{q}, true
	}
	if d.nativeSearch != nil {
		if hashes, ready := d.nativeSearch.candidates(q); ready {
			return hashes, true
		}
	}
	tokens := tokenizeName(q)
	if len(tokens) == 0 {
		if text := strings.TrimSpace(normalizeSearchText(q)); text != "" {
			tokens = []string{text}
		} else {
			return nil, true
		}
	}
	// Prefer the rarest token that still has a posting list (common tokens keep
	// a capped list so queries like "1080" stay indexed instead of full-scan).
	bestTok := ""
	bestLen := int(^uint(0) >> 1)
	for _, tok := range tokens {
		list := d.searchTok[tok]
		if len(list) == 0 {
			// Prefix matches use the vocabulary, never a million-row scan.
			for word, postings := range d.searchTok {
				if strings.HasPrefix(word, tok) {
					list = append(list, postings...)
					if len(list) >= searchPostingCap {
						break
					}
				}
			}
		}
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
		return nil, len(d.searchTok) > 0
	}
	// Prefix postings can overlap; return each hash exactly once.
	seen := make(map[string]bool, len(hashes))
	unique := make([]string, 0, len(hashes))
	for _, hash := range hashes {
		if !seen[hash] {
			seen[hash] = true
			unique = append(unique, hash)
		}
	}
	return unique, true
}

func rebuildSearchIndex(d *dashboard) {
	d.mu.Lock()
	defer d.mu.Unlock()
	d.searchTok = map[string][]string{}
	d.searchCommon = map[string]bool{}
	d.nativeSearch = newNativeSearch()
	for hash, rec := range d.records {
		if rec.Resolved && rec.Name != "" {
			d.indexNameLocked(hash, rec.Name)
		}
	}
}

func normalizeSearchText(text string) string {
	text = norm.NFD.String(strings.ToLower(text))
	return strings.TrimSpace(strings.Map(func(r rune) rune {
		if unicode.Is(unicode.Mn, r) {
			return -1
		}
		if !unicode.IsLetter(r) && !unicode.IsDigit(r) {
			return ' '
		}
		return r
	}, text))
}
func normalizeSearchQuery(query string) string {
	query = strings.TrimSpace(strings.ToLower(query))
	if strings.HasPrefix(query, "magnet:?") {
		if magnet, err := url.Parse(query); err == nil {
			hash := strings.TrimPrefix(magnet.Query().Get("xt"), "urn:btih:")
			if validHash(hash) {
				return hash
			}
		}
	}
	return query
}
