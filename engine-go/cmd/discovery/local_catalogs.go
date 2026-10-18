package main

import (
	"compress/gzip"
	"context"
	"encoding/csv"
	"io"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"
)

type catalogStamp struct {
	size     int64
	modified time.Time
	rows     int
}

// Additional datasets join the same name store and native search index. Stream
// them instead of keeping one extra Go hash map per CSV. Unchanged files are
// skipped between scans; startup reimports labels pruned by age retention.
func (d *dashboard) ingestLocalCatalogs(ctx context.Context) {
	_ = os.MkdirAll(d.catalogDir, 0700)
	loaded := map[string]catalogStamp{}
	timer := time.NewTimer(0)
	defer timer.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-timer.C:
		}
		entries, err := os.ReadDir(d.catalogDir)
		if err == nil {
			seen := map[string]bool{}
			for _, entry := range entries {
				if ctx.Err() != nil {
					return
				}
				if entry.IsDir() || (!strings.HasSuffix(entry.Name(), ".csv") && !strings.HasSuffix(entry.Name(), ".csv.gz")) {
					continue
				}
				if len(seen) >= 16 {
					break
				}
				path := filepath.Join(d.catalogDir, entry.Name())
				seen[path] = true
				info, err := entry.Info()
				if err != nil {
					continue
				}
				previous, exists := loaded[path]
				if exists && previous.size == info.Size() && previous.modified.Equal(info.ModTime()) {
					continue
				}
				rows, err := d.importNamedCSV(ctx, path)
				if err != nil {
					if ctx.Err() != nil {
						return
					}
					d.mu.Lock()
					d.state.LocalCatalogErrors++
					d.mu.Unlock()
					continue
				}
				loaded[path] = catalogStamp{size: info.Size(), modified: info.ModTime(), rows: rows}
			}
			rows := 0
			for path, stamp := range loaded {
				if !seen[path] {
					delete(loaded, path)
				} else {
					rows += stamp.rows
				}
			}
			d.mu.Lock()
			d.state.LocalCatalogFiles = len(loaded)
			d.state.LocalCatalogRows = rows
			d.mu.Unlock()
		}
		timer.Reset(5 * time.Minute)
	}
}

func (d *dashboard) importNamedCSV(ctx context.Context, path string) (int, error) {
	f, err := os.Open(path)
	if err != nil {
		return 0, err
	}
	defer f.Close()
	var input io.Reader = f
	if strings.HasSuffix(path, ".gz") {
		compressed, err := gzip.NewReader(f)
		if err != nil {
			return 0, err
		}
		defer compressed.Close()
		input = compressed
	}
	r := csv.NewReader(input)
	r.ReuseRecord = true
	r.FieldsPerRecord = -1
	header, err := r.Read()
	if err != nil {
		return 0, err
	}
	hashColumn, nameColumn, sizeColumn := -1, -1, -1
	for i, value := range header {
		switch strings.ToLower(strings.TrimSpace(strings.TrimPrefix(value, "\ufeff"))) {
		case "infohash", "hash", "info_hash":
			hashColumn = i
		case "name", "title":
			nameColumn = i
		case "size_bytes", "size":
			sizeColumn = i
		}
	}
	if hashColumn < 0 || nameColumn < 0 {
		return 0, io.ErrUnexpectedEOF
	}
	rows := 0
	batch := make([]namePatch, 0, 2500)
	flush := func() {
		now := time.Now().UTC()
		d.mu.Lock()
		for _, patch := range batch {
			if rec, ok := d.records[patch.Hash]; !ok || !rec.Resolved {
				d.applyNameLocked(patch, now)
			}
		}
		d.mu.Unlock()
		batch = batch[:0]
	}
	for {
		if ctx.Err() != nil {
			return rows, ctx.Err()
		}
		row, err := r.Read()
		if err == io.EOF {
			flush()
			return rows, nil
		}
		if err != nil {
			return rows, err
		}
		if hashColumn >= len(row) || nameColumn >= len(row) {
			continue
		}
		hash, name := normalizeInfoHash(row[hashColumn]), cleanMetadataText(row[nameColumn], 512)
		if hash == "" || name == "" {
			continue
		}
		var size int64
		if sizeColumn >= 0 && sizeColumn < len(row) {
			size, _ = strconv.ParseInt(row[sizeColumn], 10, 64)
		}
		batch = append(batch, namePatch{Hash: hash, Name: name, Size: max(0, size), Source: "catalog"})
		rows++
		if len(batch) == cap(batch) {
			flush()
		}
	}
}
