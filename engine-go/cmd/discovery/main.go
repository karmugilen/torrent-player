// Local, metadata-only public DHT discovery dashboard.
package main

import (
	"context"
	_ "embed"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/oschwald/maxminddb-golang"
)

//go:embed ui.html
var page string

type observation struct {
	Hash      string         `json:"hash"`
	Name      string         `json:"name"`
	Kind      string         `json:"kind"`
	Size      int64          `json:"size"`
	Files     []string       `json:"files"`
	Peers     int            `json:"peers"`
	Seeds     *int           `json:"seeds"`
	Countries map[string]int `json:"countries"`
	FirstSeen time.Time      `json:"firstSeen"`
	LastSeen  time.Time      `json:"lastSeen"`
	CheckedAt time.Time      `json:"checkedAt"`
	Resolved  bool           `json:"resolved"`
}
type state struct {
	Started       time.Time     `json:"started"`
	Updated       time.Time     `json:"updated"`
	Running       bool          `json:"running"`
	Status        string        `json:"status"`
	Queries       int           `json:"queries"`
	Responses     int           `json:"responses"`
	SamplingNodes int           `json:"samplingNodes"`
	Hashes        int           `json:"hashes"`
	Attempts      int           `json:"attempts"`
	Resolved      int           `json:"resolved"`
	PayloadBytes  int64         `json:"payloadBytes"`
	Records       []observation `json:"records"`
}
type dashboard struct {
	mu        sync.Mutex
	state     state
	records   map[string]observation
	cachePath string
	geo       *maxminddb.Reader
	cancel    context.CancelFunc
	done      chan struct{}
	control   sync.Mutex
}

func newDashboard(cachePath string, geo *maxminddb.Reader) *dashboard {
	d := &dashboard{records: map[string]observation{}, cachePath: cachePath, geo: geo, state: state{Started: time.Now().UTC(), Status: "Starting discovery"}}
	if b, e := os.ReadFile(cachePath); e == nil {
		var saved []observation
		if json.Unmarshal(b, &saved) == nil {
			now := time.Now()
			for _, r := range saved {
				if validHash(r.Hash) && now.Sub(r.LastSeen) < 24*time.Hour {
					d.records[r.Hash] = r
				}
			}
		}
	}
	d.pruneLocked()
	return d
}
func validHash(h string) bool {
	if len(h) != 40 {
		return false
	}
	for _, c := range h {
		if !strings.ContainsRune("0123456789abcdef", c) {
			return false
		}
	}
	return true
}
func (d *dashboard) pruneLocked() {
	if len(d.records) <= 600 {
		return
	}
	all := make([]observation, 0, len(d.records))
	for _, r := range d.records {
		all = append(all, r)
	}
	sort.Slice(all, func(i, j int) bool {
		if all[i].Resolved != all[j].Resolved {
			return all[i].Resolved
		}
		return all[i].LastSeen.After(all[j].LastSeen)
	})
	for _, r := range all[600:] {
		delete(d.records, r.Hash)
	}
}
func (d *dashboard) snapshot() state {
	d.mu.Lock()
	defer d.mu.Unlock()
	s := d.state
	s.Updated = time.Now().UTC()
	s.Hashes = len(d.records)
	s.Records = make([]observation, 0, len(d.records))
	s.Resolved = 0
	for _, r := range d.records {
		if r.Resolved {
			s.Resolved++
		}
		r.Files = append([]string(nil), r.Files...)
		countryCopy := map[string]int{}
		for c, n := range r.Countries {
			countryCopy[c] = n
		}
		r.Countries = countryCopy
		s.Records = append(s.Records, r)
	}
	sort.Slice(s.Records, func(i, j int) bool {
		a, b := s.Records[i], s.Records[j]
		if a.Peers != b.Peers {
			return a.Peers > b.Peers
		}
		if a.Resolved != b.Resolved {
			return a.Resolved
		}
		return a.LastSeen.After(b.LastSeen)
	})
	return s
}
func (d *dashboard) persist() {
	s := d.snapshot()
	b, e := json.Marshal(s.Records)
	if e != nil {
		return
	}
	if os.MkdirAll(filepath.Dir(d.cachePath), 0700) != nil {
		return
	}
	tmp := d.cachePath + ".tmp"
	if os.WriteFile(tmp, b, 0600) == nil {
		_ = os.Rename(tmp, d.cachePath)
	}
}
func (d *dashboard) setStatus(s string) { d.mu.Lock(); d.state.Status = s; d.mu.Unlock() }
func (d *dashboard) setRunning(parent context.Context, on bool) {
	d.control.Lock()
	defer d.control.Unlock()
	if !on {
		if d.cancel != nil {
			d.cancel()
			<-d.done
			d.cancel = nil
		}
		d.mu.Lock()
		d.state.Running = false
		d.state.Status = "Discovery paused"
		d.mu.Unlock()
		d.persist()
		return
	}
	if d.cancel != nil {
		select {
		case <-d.done:
			d.cancel()
			d.cancel = nil
		default:
			return
		}
	}
	ctx, cancel := context.WithCancel(parent)
	d.cancel = cancel
	d.done = make(chan struct{})
	d.mu.Lock()
	d.state.Running = true
	d.state.Status = "Connecting to public DHT"
	d.mu.Unlock()
	go func() {
		defer close(d.done)
		if e := d.collect(ctx); e != nil && !errors.Is(e, context.Canceled) {
			d.setStatus("Discovery unavailable: " + e.Error())
		}
		d.mu.Lock()
		d.state.Running = false
		d.mu.Unlock()
	}()
}
func (d *dashboard) country(ip net.IP) string {
	if d.geo == nil {
		return "ZZ"
	}
	var r struct {
		Country struct {
			Code string `maxminddb:"iso_code"`
		} `maxminddb:"country"`
	}
	if d.geo.Lookup(ip, &r) != nil || r.Country.Code == "" {
		return "ZZ"
	}
	return r.Country.Code
}
func (d *dashboard) handler(world []byte) http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/" {
			http.NotFound(w, r)
			return
		}
		if r.Method != "GET" {
			w.WriteHeader(405)
			return
		}
		b, _ := json.Marshal(d.snapshot())
		html := strings.ReplaceAll(page, "__WORLD_MAP__", string(world))
		html = strings.ReplaceAll(html, "__INITIAL_STATE__", string(b))
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		w.Header().Set("Cache-Control", "no-store")
		_, _ = io.WriteString(w, html)
	})
	mux.HandleFunc("/api/state", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != "GET" {
			w.WriteHeader(405)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		w.Header().Set("Cache-Control", "no-store")
		_ = json.NewEncoder(w).Encode(d.snapshot())
	})
	mux.HandleFunc("/api/control", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != "POST" {
			w.WriteHeader(405)
			return
		}
		origin := r.Header.Get("Origin")
		if origin != "" && origin != "http://"+r.Host {
			http.Error(w, "Origin not allowed", 403)
			return
		}
		if r.Header.Get("Sec-Fetch-Site") == "cross-site" {
			http.Error(w, "Origin not allowed", 403)
			return
		}
		var body struct {
			Running *bool `json:"running"`
		}
		if json.NewDecoder(http.MaxBytesReader(w, r.Body, 128)).Decode(&body) != nil || body.Running == nil {
			http.Error(w, "Expected running boolean", 400)
			return
		}
		d.setRunning(context.Background(), *body.Running)
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(d.snapshot())
	})
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("X-Content-Type-Options", "nosniff")
		w.Header().Set("Referrer-Policy", "no-referrer")
		w.Header().Set("X-Frame-Options", "DENY")
		mux.ServeHTTP(w, r)
	})
}
func main() {
	bind := flag.String("listen", "127.0.0.1:18181", "local dashboard address")
	assets := flag.String("assets", "../android/app/src/main/assets", "offline map/country assets")
	cacheRoot, _ := os.UserCacheDir()
	cache := flag.String("cache", filepath.Join(cacheRoot, "torrent-player", "discovery.json"), "observations cache")
	flag.Parse()
	host, _, e := net.SplitHostPort(*bind)
	if e != nil || !net.ParseIP(host).IsLoopback() {
		fmt.Fprintln(os.Stderr, "Dashboard must listen on a loopback IP")
		os.Exit(1)
	}
	world, e := os.ReadFile(filepath.Join(*assets, "world-map.json"))
	if e != nil {
		fmt.Fprintln(os.Stderr, "World map:", e)
		os.Exit(1)
	}
	geo, e := maxminddb.Open(filepath.Join(*assets, "geoip-country-2026-10.mmdb"))
	if e != nil {
		fmt.Fprintln(os.Stderr, "Country lookup unavailable; unknown locations stay unlocated")
	} else {
		defer geo.Close()
	}
	d := newDashboard(*cache, geo)
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	listener, e := net.Listen("tcp", *bind)
	if e != nil {
		fmt.Fprintln(os.Stderr, e)
		os.Exit(1)
	}
	server := &http.Server{Handler: d.handler(world), ReadHeaderTimeout: 5 * time.Second, WriteTimeout: 20 * time.Second, IdleTimeout: 60 * time.Second}
	go func() {
		<-ctx.Done()
		d.setRunning(ctx, false)
		end, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		_ = server.Shutdown(end)
	}()
	d.setRunning(ctx, true)
	fmt.Printf("Torrent discovery: http://%s\n", *bind)
	if e = server.Serve(listener); e != nil && !errors.Is(e, http.ErrServerClosed) {
		fmt.Fprintln(os.Stderr, e)
		os.Exit(1)
	}
}
