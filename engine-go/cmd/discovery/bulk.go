package main

import (
	"encoding/base32"
	"encoding/hex"
	"encoding/json"
	"io"
	"net/http"
	"strings"
	"time"

	"github.com/anacrolix/torrent/metainfo"
)

const bulkHashLimit = 50_000

type nameMatch struct {
	Hash     string `json:"hash"`
	Name     string `json:"name"`
	Size     int64  `json:"size"`
	Source   string `json:"source"`
	Verified bool   `json:"verified"`
}

type bulkResolution struct {
	Requested        int         `json:"requested"`
	Unique           int         `json:"unique"`
	Matched          int         `json:"matched"`
	Invalid          int         `json:"invalid"`
	Duplicates       int         `json:"duplicates"`
	Results          []nameMatch `json:"results"`
	Missing          []string    `json:"missing"`
	ElapsedMS        float64     `json:"elapsedMs"`
	MatchesPerMinute int64       `json:"matchesPerMinute"`
}

func normalizeInfoHash(value string) string {
	value = strings.TrimSpace(value)
	if strings.HasPrefix(strings.ToLower(value), "magnet:?") {
		m, err := metainfo.ParseMagnetUri(value)
		if err != nil || m.InfoHash.IsZero() {
			return ""
		}
		return m.InfoHash.HexString()
	}
	value = strings.TrimPrefix(strings.ToLower(value), "urn:btih:")
	if validHash(value) && value != "0000000000000000000000000000000000000000" {
		return value
	}
	if len(value) == 32 {
		b, err := base32.StdEncoding.WithPadding(base32.NoPadding).DecodeString(strings.ToUpper(value))
		if err == nil && len(b) == 20 {
			return normalizeInfoHash(hex.EncodeToString(b))
		}
	}
	return ""
}

// resolveHashes returns reusable labels, without inventing new discoveries or
// scheduling network jobs. The same seam powers the website and throughput test.
func (d *dashboard) resolveHashes(input []string) bulkResolution {
	started := time.Now()
	out := bulkResolution{Requested: len(input), Results: []nameMatch{}, Missing: []string{}}
	seen := make(map[string]struct{}, len(input))
	hashes := make([]string, 0, len(input))
	for _, raw := range input {
		hash := normalizeInfoHash(raw)
		if hash == "" {
			out.Invalid++
			continue
		}
		if _, ok := seen[hash]; ok {
			out.Duplicates++
			continue
		}
		seen[hash] = struct{}{}
		hashes = append(hashes, hash)
	}
	out.Unique = len(hashes)
	missing := make([]string, 0, len(hashes))
	// Short read-lock windows allow collector writes between batches.
	for start := 0; start < len(hashes); start += 512 {
		d.mu.RLock()
		for _, hash := range hashes[start:min(start+512, len(hashes))] {
			r, ok := d.records[hash]
			if ok && r.Resolved && r.Name != "" {
				out.Results = append(out.Results, nameMatch{Hash: hash, Name: r.Name, Size: r.Size, Source: r.Source, Verified: r.Source == "peer" || r.Source == "cache"})
			} else {
				missing = append(missing, hash)
			}
		}
		d.mu.RUnlock()
	}
	idx := d.dumpIndex()
	hits := map[string]dumpRow{}
	if idx != nil && len(missing) > 0 {
		if idx.rust {
			for _, it := range rustLookupMany(missing) {
				hits[it.hash] = it.row
			}
		} else {
			idx.mu.RLock()
			for _, hash := range missing {
				if row, ok := idx.rows[hash]; ok {
					hits[hash] = row
				}
			}
			idx.mu.RUnlock()
		}
	}
	for _, hash := range missing {
		if row, ok := hits[hash]; ok && row.Name != "" {
			out.Results = append(out.Results, nameMatch{Hash: hash, Name: row.Name, Size: row.Size, Source: "dump"})
		} else {
			out.Missing = append(out.Missing, hash)
		}
	}
	out.Matched = len(out.Results)
	elapsed := time.Since(started)
	out.ElapsedMS = float64(elapsed) / float64(time.Millisecond)
	if elapsed > 0 {
		out.MatchesPerMinute = int64(float64(out.Matched) * float64(time.Minute) / float64(elapsed))
	}
	return out
}

func (d *dashboard) serveResolve(w http.ResponseWriter, r *http.Request) {
	if r.Method != "POST" {
		w.WriteHeader(http.StatusMethodNotAllowed)
		return
	}
	if origin := r.Header.Get("Origin"); (origin != "" && origin != "http://"+r.Host) || r.Header.Get("Sec-Fetch-Site") == "cross-site" {
		http.Error(w, "Origin not allowed", http.StatusForbidden)
		return
	}
	select {
	case d.bulkSlots <- struct{}{}:
		defer func() { <-d.bulkSlots }()
	default:
		http.Error(w, "Another batch is running; retry shortly", http.StatusTooManyRequests)
		return
	}
	var body struct {
		Hashes []string `json:"hashes"`
	}
	decoder := json.NewDecoder(http.MaxBytesReader(w, r.Body, 4<<20))
	if decoder.Decode(&body) != nil || len(body.Hashes) == 0 || len(body.Hashes) > bulkHashLimit {
		http.Error(w, "Expected 1–50000 hashes or magnets (maximum 4 MiB)", http.StatusBadRequest)
		return
	}
	var trailing any
	if decoder.Decode(&trailing) != io.EOF {
		http.Error(w, "Expected one JSON object", http.StatusBadRequest)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	_ = json.NewEncoder(w).Encode(d.resolveHashes(body.Hashes))
}
