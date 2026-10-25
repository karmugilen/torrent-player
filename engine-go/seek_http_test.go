package main

import (
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"slices"
	"strconv"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/anacrolix/torrent"
)

// streamResponse is the 2.1.41 writer: each read sent at once, in reads of up
// to 256 KiB. Phone playback felt slower with it, and it went back out.
type streamResponse struct{ http.ResponseWriter }

func (w streamResponse) Write(b []byte) (int, error) { return flushOnly(w).Write(b) }

func (w streamResponse) ReadFrom(r io.Reader) (int64, error) {
	return io.CopyBuffer(struct{ io.Writer }{w}, r, make([]byte, 256<<10))
}

// flushOnly sends each write at once but copies through io.Copy's 32 KiB.
type flushOnly struct{ http.ResponseWriter }

func (w flushOnly) Write(b []byte) (int, error) {
	n, err := w.ResponseWriter.Write(b)
	if err == nil {
		err = http.NewResponseController(w.ResponseWriter).Flush()
	}
	return n, err
}

// bigOnly copies in 256 KiB reads without flushing.
type bigOnly struct{ http.ResponseWriter }

func (w bigOnly) ReadFrom(r io.Reader) (int64, error) {
	return io.CopyBuffer(struct{ io.Writer }{w.ResponseWriter}, r, make([]byte, 256<<10))
}

// TestSeekHTTPComparison serves the simulated swarm over HTTP as the phone's
// player reads it, with each response writer interleaved: time to the first
// byte, to 64 KiB and to 1 MiB after a seek to a cold spot, and the rate of a
// cold 8 MiB stream. Run with SEEK_SEARCH=http (SEEK_ROUNDS sets rounds).
func TestSeekHTTPComparison(t *testing.T) {
	if os.Getenv("SEEK_SEARCH") != "http" {
		t.Skip("set SEEK_SEARCH=http to compare HTTP response writers")
	}
	writers := map[string]func(http.ResponseWriter) http.ResponseWriter{
		"old":   func(w http.ResponseWriter) http.ResponseWriter { return w },
		"new":   func(w http.ResponseWriter) http.ResponseWriter { return streamResponse{w} },
		"flush": func(w http.ResponseWriter) http.ResponseWriter { return flushOnly{w} },
		"big":   func(w http.ResponseWriter) http.ResponseWriter { return bigOnly{w} },
	}
	names := []string{"old", "new", "flush", "big"}
	rounds := 10
	if n, err := strconv.Atoi(os.Getenv("SEEK_ROUNDS")); err == nil {
		rounds = n
	}
	const pieceLen, fileLen = 256 << 10, 192 << 20
	sw := newSeekSwarm(t, pieceLen, fileLen, 64<<20, 16)
	sw.reader.Close()
	current := "old"
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		playback, err := sw.leech.openPlaybackAt(sw.rec.ID, 0, rangeStart(r.Header.Get("Range")))
		if err != nil {
			http.Error(w, err.Error(), http.StatusConflict)
			return
		}
		defer playback.Close()
		w.Header().Set("Content-Type", "video/x-matroska")
		http.ServeContent(writers[current](w), r, "movie.mkv", time.Time{}, &playbackHTTPReader{playback: playback})
	}))
	defer server.Close()
	st := sw.leech.storage.GetStorage(sw.rec.InfoHash)
	coldSpan := func(span int64) int64 {
		for tries := 0; ; tries++ {
			off := sw.rng.Int63n(fileLen-span-pieceLen) / pieceLen * pieceLen
			cold := true
			st.mu.Lock()
			for p := int(off / pieceLen); p <= int((off+span)/pieceLen); p++ {
				if st.completed[p] || st.cache[p] != nil {
					cold = false
					break
				}
			}
			st.mu.Unlock()
			if cold || tries > 500 {
				// A point inside the piece, as a player's seek lands.
				return off + sw.rng.Int63n(pieceLen)
			}
		}
	}
	type result struct{ first, k64, m1, rate []float64 }
	results := map[string]*result{}
	for _, n := range names {
		results[n] = &result{}
	}
	get := func(off int64) (*http.Response, time.Time) {
		req, _ := http.NewRequest("GET", server.URL, nil)
		req.Header.Set("Range", fmt.Sprintf("bytes=%d-", off))
		start := time.Now()
		resp, err := server.Client().Do(req)
		if err != nil {
			t.Fatal(err)
		}
		return resp, start
	}
	ms := func(d time.Duration) float64 { return float64(d.Microseconds()) / 1000 }
	for round := 0; round < rounds; round++ {
		for _, i := range sw.rng.Perm(len(names)) {
			name := names[i]
			current = name
			res := results[name]
			for k := 0; k < 3; k++ {
				resp, start := get(coldSpan(1 << 20))
				buf := make([]byte, 1)
				if _, err := io.ReadFull(resp.Body, buf); err != nil {
					t.Fatal(err)
				}
				res.first = append(res.first, ms(time.Since(start)))
				if _, err := io.ReadFull(resp.Body, make([]byte, 64<<10-1)); err != nil {
					t.Fatal(err)
				}
				res.k64 = append(res.k64, ms(time.Since(start)))
				if _, err := io.ReadFull(resp.Body, make([]byte, 1<<20-64<<10)); err != nil {
					t.Fatal(err)
				}
				res.m1 = append(res.m1, ms(time.Since(start)))
				resp.Body.Close()
				time.Sleep(300 * time.Millisecond)
			}
			const span = 8 << 20
			resp, start := get(coldSpan(span))
			if _, err := io.CopyN(io.Discard, resp.Body, span); err != nil {
				t.Fatal(err)
			}
			res.rate = append(res.rate, span/time.Since(start).Seconds()/(1<<20))
			resp.Body.Close()
		}
	}
	median := func(v []float64) float64 { slices.Sort(v); return v[len(v)/2] }
	p90 := func(v []float64) float64 { slices.Sort(v); return v[len(v)*9/10] }
	for _, n := range names {
		r := results[n]
		t.Logf("%-5s first byte median=%.0fms | 64KiB median=%.0fms p90=%.0fms | 1MiB median=%.0fms p90=%.0fms | stream %.2f MiB/s (n=%d)",
			n, median(r.first), median(r.k64), p90(r.k64), median(r.m1), p90(r.m1), median(r.rate), len(r.k64))
	}
}

// TestSeekResumeComparison measures what the player waits for after a seek:
// the time until 1 and 2 MiB from the seek point have come over HTTP (players
// resume after 1-2.5 s of video), plus a cold 8 MiB stream. Variants are
// SEEK_TUNE specs ("name:Field=val,...;..."), each at every SEEK_CACHE size
// (MiB), interleaved. Run with SEEK_SEARCH=resume.
func TestSeekResumeComparison(t *testing.T) {
	if os.Getenv("SEEK_SEARCH") != "resume" {
		t.Skip("set SEEK_SEARCH=resume to compare time to resume after a seek")
	}
	type variant struct {
		name                  string
		cache                 int64
		tuning                torrent.SeekTuning
		unverified            bool
		first, m1, m2, stream []float64
	}
	caches := []int64{16, 48}
	if env := os.Getenv("SEEK_CACHE"); env != "" {
		caches = nil
		for _, f := range strings.Split(env, ",") {
			n, _ := strconv.ParseInt(f, 10, 64)
			caches = append(caches, n)
		}
	}
	var variants []*variant
	for _, spec := range strings.Split(os.Getenv("SEEK_TUNE"), ";") {
		// unverified=1: reads return blocks before their piece is verified.
		unverified := strings.Contains(spec, "unverified=1")
		spec = strings.NewReplacer(",unverified=1", "", "unverified=1,", "", "unverified=1", "").Replace(spec)
		name, tuning, _, _ := parseSeekTune(t, spec)
		for _, c := range caches {
			variants = append(variants, &variant{name: name, cache: c << 20, tuning: tuning, unverified: unverified})
		}
	}
	rounds := 8
	if n, err := strconv.Atoi(os.Getenv("SEEK_ROUNDS")); err == nil {
		rounds = n
	}
	// SEEK_PIECE_KB sets the piece size; large films use 1-4 MiB pieces.
	pieceLen, fileLen := int64(256<<10), int64(192<<20)
	if n, err := strconv.Atoi(os.Getenv("SEEK_PIECE_KB")); err == nil && n > 0 {
		pieceLen = int64(n) << 10
	}
	sw := newSeekSwarm(t, pieceLen, fileLen, 16<<20, 16)
	sw.reader.Close()
	defer torrent.SetSeekTuning(torrent.DefaultSeekTuning)
	var unverified atomic.Bool
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		playback, err := sw.leech.openPlaybackAt(sw.rec.ID, 0, rangeStart(r.Header.Get("Range")))
		if err != nil {
			http.Error(w, err.Error(), http.StatusConflict)
			return
		}
		defer playback.Close()
		playback.mu.Lock()
		playback.unverifiedReads = unverified.Load()
		playback.mu.Unlock()
		w.Header().Set("Content-Type", "video/x-matroska")
		http.ServeContent(w, r, "movie.mkv", time.Time{}, &playbackHTTPReader{playback: playback})
	}))
	defer server.Close()
	st := sw.leech.storage.GetStorage(sw.rec.InfoHash)
	coldAt := func(span int64) int64 {
		for tries := 0; ; tries++ {
			off := sw.rng.Int63n(fileLen-span-pieceLen) / pieceLen * pieceLen
			cold := true
			st.mu.Lock()
			for p := int(off / pieceLen); p <= int((off+span)/pieceLen); p++ {
				if st.completed[p] || st.cache[p] != nil {
					cold = false
					break
				}
			}
			st.mu.Unlock()
			if cold || tries > 500 {
				return off + sw.rng.Int63n(pieceLen)
			}
		}
	}
	get := func(off int64) (io.ReadCloser, time.Time) {
		req, _ := http.NewRequest("GET", server.URL, nil)
		req.Header.Set("Range", fmt.Sprintf("bytes=%d-", off))
		start := time.Now()
		resp, err := server.Client().Do(req)
		if err != nil {
			t.Fatal(err)
		}
		return resp.Body, start
	}
	ms := func(start time.Time) float64 { return float64(time.Since(start).Microseconds()) / 1000 }
	read := func(body io.Reader, n int) {
		if _, err := io.ReadFull(body, make([]byte, n)); err != nil {
			t.Fatal(err)
		}
	}
	for round := 0; round < rounds; round++ {
		for _, i := range sw.rng.Perm(len(variants)) {
			v := variants[i]
			torrent.SetSeekTuning(v.tuning)
			unverified.Store(v.unverified)
			if err := sw.leech.storage.SetCacheLimit(v.cache); err != nil {
				t.Fatal(err)
			}
			sw.leech.refreshPlaybackWindows(sw.rec.Torrent)
			for k := 0; k < 3; k++ {
				body, start := get(coldAt(2 << 20))
				read(body, 1)
				v.first = append(v.first, ms(start))
				read(body, 1<<20-1)
				v.m1 = append(v.m1, ms(start))
				read(body, 1<<20)
				v.m2 = append(v.m2, ms(start))
				body.Close()
				time.Sleep(300 * time.Millisecond)
			}
			const span = 8 << 20
			body, start := get(coldAt(span))
			if _, err := io.CopyN(io.Discard, body, span); err != nil {
				t.Fatal(err)
			}
			v.stream = append(v.stream, span/time.Since(start).Seconds()/(1<<20))
			body.Close()
		}
	}
	if os.Getenv("SEEK_RAW") == "1" {
		// One line per seek, to pool runs on fresh swarms.
		for _, v := range variants {
			for i := range v.m2 {
				t.Logf("RAW %s %.0f %.0f %.0f", v.name, v.first[i], v.m1[i], v.m2[i])
			}
		}
	}
	q := func(v []float64, f float64) float64 { slices.Sort(v); return v[int(float64(len(v)-1)*f)] }
	for _, v := range variants {
		t.Logf("cache=%3dMiB %-9s first=%4.0fms 1MiB=%4.0fms 2MiB median=%4.0fms p75=%4.0fms p90=%4.0fms stream=%.2f MiB/s n=%d",
			v.cache>>20, v.name, q(v.first, .5), q(v.m1, .5), q(v.m2, .5), q(v.m2, .75), q(v.m2, .9), q(v.stream, .5), len(v.m2))
	}
}
