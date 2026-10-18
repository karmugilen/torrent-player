package main

import (
	"context"
	"encoding/csv"
	"io"
	"net/http"
	"os"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"time"
)

// Collaborative torrents.csv dump (same project as torrents-csv.com). One local
// download gives hash→name lookup with no peer upload. Sorted by infohash.
const dumpURL = "https://codeberg.org/heretic/torrents-csv-data/raw/branch/main/torrents.csv"
const dumpMaxAge = 24 * time.Hour

// Bulk-apply chunk size for undelayed local dump seeding.
const dumpSeedChunk = 2500

type dumpIndex struct {
	mu       sync.RWMutex
	rows     map[string]dumpRow // Go fallback; empty when rust==true
	rust     bool
	rustRows int
	// A miss remains a miss until the next CSV is loaded. Keep the cache on
	// this index so replacing it automatically makes every hash eligible again.
	checked         map[string]struct{}
	pendingRevision uint64
	scanned         bool
}

type dumpRow struct {
	Name     string
	Size     int64
	Seeders  int
	Leechers int
}

func (d *dashboard) setDumpIndex(idx *dumpIndex) {
	d.dumpMu.Lock()
	d.dump = idx
	d.dumpMu.Unlock()
}

func (d *dashboard) dumpIndex() *dumpIndex {
	d.dumpMu.RLock()
	defer d.dumpMu.RUnlock()
	return d.dump
}

func (d *dashboard) ingestDump(ctx context.Context) {
	idx := &dumpIndex{rows: map[string]dumpRow{}}
	load := func(force bool) *dumpIndex {
		if err := d.ensureDumpFile(ctx, force); err != nil {
			if idx.len() == 0 {
				d.setStatus("Local torrents.csv dump unavailable; using catalog and quiet DHT")
			}
			return idx
		}
		next, err := loadDumpIndex(d.dumpPath)
		if err != nil {
			d.setStatus("Local torrents.csv dump failed to load")
			return idx
		}
		idx = next
		d.setDumpIndex(idx)
		d.mu.Lock()
		d.state.DumpReady = true
		d.state.DumpRows = idx.len()
		d.mu.Unlock()
		backend := "go"
		if idx.rust {
			backend = "rust"
		}
		d.setStatus("Local torrents.csv dump loaded (" + backend + ") · resolving pending hashes")
		d.resolvePendingFromDump(idx)
		return idx
	}
	idx = load(false)

	var work sync.WaitGroup
	work.Add(1)
	go func() {
		defer work.Done()
		d.seedFromDump(ctx, idx)
	}()
	work.Add(1)
	go func() {
		defer work.Done()
		d.matchPendingFromDump(ctx)
	}()

	daily := time.NewTicker(time.Hour)
	defer daily.Stop()
	for {
		select {
		case <-ctx.Done():
			work.Wait()
			return
		case <-daily.C:
			if st, err := os.Stat(d.dumpPath); err != nil || time.Since(st.ModTime()) >= dumpMaxAge {
				d.setStatus("Updating local torrents.csv dump…")
				idx = load(true)
				d.seedFromDump(ctx, idx)
			}
		}
	}
}

// seedFromDump imports every seeded dump row into the JSON store as fast as
// possible. No pacing delays — runs async until the pass completes.
func (d *dashboard) seedFromDump(ctx context.Context, idx *dumpIndex) {
	if idx == nil || idx.len() == 0 || ctx.Err() != nil {
		return
	}
	if idx.rust && rustDumpEnabled() {
		d.setStatus("Local dump seeding names at full speed (rust)…")
		totalAdded := rustSeedAll(d)
		d.setStatus("Local dump seed complete · " + strconv.Itoa(totalAdded) + " names added this pass")
		d.persist()
		return
	}
	idx.mu.RLock()
	keys := make([]string, 0, len(idx.rows))
	for hash, row := range idx.rows {
		if row.Seeders > 0 {
			keys = append(keys, hash)
		}
	}
	idx.mu.RUnlock()
	if len(keys) == 0 {
		return
	}
	d.setStatus("Local dump seeding names at full speed…")
	totalAdded := 0
	for i := 0; i < len(keys); i += dumpSeedChunk {
		if ctx.Err() != nil {
			return
		}
		if latest := d.dumpIndex(); latest != nil {
			idx = latest
		}
		end := i + dumpSeedChunk
		if end > len(keys) {
			end = len(keys)
		}
		totalAdded += d.applyDumpRowsBatch(keys[i:end], idx, true)
		if (i/dumpSeedChunk)%8 == 0 {
			d.setStatus("Local dump seeding… " + strconv.Itoa(totalAdded) + " new names")
			runtime.Gosched()
		}
	}
	d.setStatus("Local dump seed complete · " + strconv.Itoa(totalAdded) + " names added this pass")
	d.persist()
}

// matchPendingFromDump keeps matching new DHT hashes against the dump with no
// multi-second wait between passes.
func (d *dashboard) matchPendingFromDump(ctx context.Context) {
	for {
		if ctx.Err() != nil {
			return
		}
		idx := d.dumpIndex()
		if idx == nil {
			select {
			case <-ctx.Done():
				return
			case <-time.After(200 * time.Millisecond):
			}
			continue
		}
		added := d.resolvePendingFromDump(idx)
		if added == 0 {
			select {
			case <-ctx.Done():
				return
			case <-time.After(250 * time.Millisecond):
			}
			continue
		}
		// Keep going immediately while there is work.
		runtime.Gosched()
	}
}

func (d *dashboard) ensureDumpFile(ctx context.Context, force bool) error {
	if !force {
		if st, err := os.Stat(d.dumpPath); err == nil && st.Size() > 1024 && time.Since(st.ModTime()) < dumpMaxAge {
			return nil
		}
	}
	if rustDumpEnabled() {
		d.setStatus("Downloading local torrents.csv dump (~175MB, rust)…")
		if err := rustEnsureDump(d.dumpPath, force); err == nil {
			return nil
		}
		// Fall through to Go download on rust failure.
	}
	d.setStatus("Downloading local torrents.csv dump (~175MB)…")
	tmp := d.dumpPath + ".tmp"
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, dumpURL, nil)
	if err != nil {
		return err
	}
	req.Header.Set("User-Agent", "TorrentAtlas/1.0 (metadata-only; local dump)")
	client := &http.Client{Timeout: 30 * time.Minute}
	resp, err := client.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return io.ErrUnexpectedEOF
	}
	out, err := os.OpenFile(tmp, os.O_CREATE|os.O_TRUNC|os.O_WRONLY, 0600)
	if err != nil {
		return err
	}
	_, copyErr := io.Copy(out, resp.Body)
	closeErr := out.Close()
	if copyErr != nil {
		_ = os.Remove(tmp)
		return copyErr
	}
	if closeErr != nil {
		_ = os.Remove(tmp)
		return closeErr
	}
	if err := os.Rename(tmp, d.dumpPath); err != nil {
		_ = os.Remove(tmp)
		return err
	}
	return nil
}

func (idx *dumpIndex) len() int {
	if idx == nil {
		return 0
	}
	if idx.rust {
		if idx.rustRows > 0 {
			return idx.rustRows
		}
		return rustDumpLen()
	}
	return len(idx.rows)
}

func (idx *dumpIndex) lookup(hash string) (dumpRow, bool) {
	if idx == nil {
		return dumpRow{}, false
	}
	if idx.rust {
		return rustLookup(hash)
	}
	idx.mu.RLock()
	row, ok := idx.rows[hash]
	idx.mu.RUnlock()
	return row, ok
}

func loadDumpIndex(path string) (*dumpIndex, error) {
	if rustDumpEnabled() {
		if idx, err := rustLoadDump(path); err == nil {
			return idx, nil
		}
	}
	return loadDumpIndexGo(path)
}

func loadDumpIndexGo(path string) (*dumpIndex, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	r := csv.NewReader(f)
	r.ReuseRecord = true
	r.LazyQuotes = true
	r.FieldsPerRecord = -1
	header, err := r.Read()
	if err != nil {
		return nil, err
	}
	col := map[string]int{}
	for i, name := range header {
		col[strings.ToLower(strings.TrimSpace(name))] = i
	}
	hi, okH := col["infohash"]
	ni, okN := col["name"]
	if !okH || !okN {
		return nil, io.ErrUnexpectedEOF
	}
	si, hasSize := col["size_bytes"]
	sei, hasSeeds := col["seeders"]
	li, hasLeech := col["leechers"]
	idx := &dumpIndex{rows: make(map[string]dumpRow, 1_200_000)}
	for {
		rec, err := r.Read()
		if err == io.EOF {
			break
		}
		if err != nil {
			continue
		}
		if hi >= len(rec) || ni >= len(rec) {
			continue
		}
		hash := strings.ToLower(strings.TrimSpace(rec[hi]))
		if !validHash(hash) {
			continue
		}
		name := cleanMetadataText(rec[ni], 512)
		if name == "" {
			continue
		}
		row := dumpRow{Name: name}
		if hasSize && si < len(rec) {
			row.Size, _ = strconv.ParseInt(strings.TrimSpace(rec[si]), 10, 64)
		}
		if hasSeeds && sei < len(rec) {
			row.Seeders, _ = strconv.Atoi(strings.TrimSpace(rec[sei]))
		}
		if hasLeech && li < len(rec) {
			row.Leechers, _ = strconv.Atoi(strings.TrimSpace(rec[li]))
		}
		idx.rows[hash] = row
	}
	return idx, nil
}

func (d *dashboard) applyDumpRowsBatch(hashes []string, idx *dumpIndex, seedOnly bool) int {
	if len(hashes) == 0 || idx == nil {
		return 0
	}
	items := make([]dumpMatch, 0)
	if idx.rust {
		// One FFI call per batch; native workers search the mmap in parallel.
		for start := 0; start < len(hashes); start += 50_000 {
			end := min(len(hashes), start+50_000)
			items = append(items, rustLookupMany(hashes[start:end])...)
		}
	} else {
		for _, hash := range hashes {
			if row, ok := idx.lookup(hash); ok {
				items = append(items, dumpMatch{hash: hash, row: row})
			}
		}
	}
	if len(items) == 0 {
		return 0
	}
	now := time.Now().UTC()
	added := 0
	d.mu.Lock()
	defer d.mu.Unlock()
	for _, it := range items {
		if seedOnly {
			if _, exists := d.records[it.hash]; exists {
				continue
			}
		}
		var seeds *int
		peerBump := 0
		if it.row.Seeders > 0 || it.row.Leechers > 0 {
			peerBump = it.row.Seeders + it.row.Leechers
			s := it.row.Seeders
			seeds = &s
		}
		if d.applyNameLocked(namePatch{
			Hash: it.hash, Name: it.row.Name, Source: "dump", Size: it.row.Size,
			Seeds: seeds, PeerBump: peerBump, Kind: catalogKind(it.row.Name),
		}, now) {
			added++
		}
	}
	if added > 0 {
		d.state.Status = "Local dump seeding names at full speed…"
	}
	return added
}

// applyDumpRow names a hash from the local dump. When seedOnly is set, skip
// hashes already present so DHT rows keep their peer geography.
func (d *dashboard) applyDumpRow(hash string, row dumpRow, seedOnly bool) bool {
	now := time.Now().UTC()
	d.mu.Lock()
	defer d.mu.Unlock()
	if seedOnly {
		if _, exists := d.records[hash]; exists {
			return false
		}
	}
	var seeds *int
	peerBump := 0
	if row.Seeders > 0 || row.Leechers > 0 {
		peerBump = row.Seeders + row.Leechers
		s := row.Seeders
		seeds = &s
	}
	if d.applyNameLocked(namePatch{
		Hash: hash, Name: row.Name, Source: "dump", Size: row.Size,
		Seeds: seeds, PeerBump: peerBump, Kind: catalogKind(row.Name),
	}, now) {
		d.state.Status = "Resolved name from local torrents.csv dump"
		return true
	}
	return false
}

func (d *dashboard) tryResolveHashFromDump(hash string) bool {
	idx := d.dumpIndex()
	if idx == nil {
		return false
	}
	d.mu.Lock()
	if rec, ok := d.records[hash]; ok && rec.Resolved {
		d.mu.Unlock()
		return false
	}
	idx.mu.Lock()
	if idx.checked == nil {
		idx.checked = map[string]struct{}{}
	}
	_, checked := idx.checked[hash]
	if !checked {
		idx.checked[hash] = struct{}{}
		d.state.DumpLookups++
	}
	idx.mu.Unlock()
	d.mu.Unlock()
	if checked {
		return false
	}
	row, ok := idx.lookup(hash)
	if !ok {
		return false
	}
	return d.applyDumpRow(hash, row, false)
}

func (d *dashboard) resolvePendingFromDump(idx *dumpIndex) int {
	if idx == nil {
		return 0
	}
	d.mu.Lock()
	idx.mu.Lock()
	if idx.scanned && idx.pendingRevision == d.pendingRevision {
		idx.mu.Unlock()
		d.mu.Unlock()
		return 0
	}
	idx.scanned, idx.pendingRevision = true, d.pendingRevision
	if idx.checked == nil {
		idx.checked = map[string]struct{}{}
	}
	pending := make([]string, 0)
	for hash := range d.pendingSet {
		if _, checked := idx.checked[hash]; checked {
			continue
		}
		idx.checked[hash] = struct{}{}
		pending = append(pending, hash)
	}
	d.state.DumpLookups += len(pending)
	idx.mu.Unlock()
	d.mu.Unlock()
	if len(pending) == 0 {
		return 0
	}
	return d.applyDumpRowsBatch(pending, idx, false)
}

type dumpMatch struct {
	hash string
	row  dumpRow
}
