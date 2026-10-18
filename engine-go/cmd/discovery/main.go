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

// retainedTorrentLimit 0 means the JSON store keeps every observation (no count cap).
// Age expiry on load still drops stale unresolved rows and old named rows.
const retainedTorrentLimit = 0

// Bound opportunistic DHT source hints so they cannot grow without limit.
const sourceHintLimit = 20_000

type observation struct {
	Hash            string         `json:"hash"`
	Name            string         `json:"name"`
	Kind            string         `json:"kind"`
	Size            int64          `json:"size"`
	Files           []string       `json:"files"`
	Peers           int            `json:"peers"`
	Seeds           *int           `json:"seeds"`
	Countries       map[string]int `json:"countries"`
	FirstSeen       time.Time      `json:"firstSeen"`
	LastSeen        time.Time      `json:"lastSeen"`
	CheckedAt       time.Time      `json:"checkedAt"`
	Resolved        bool           `json:"resolved"`
	ResolveAttempts int            `json:"resolveAttempts"`
	NextAttempt     time.Time      `json:"nextAttempt"`
	Source          string         `json:"source"`
	SampleHits      int            `json:"sampleHits"`
	ResolutionMS    int64          `json:"resolutionMs"`
}
type state struct {
	Started                time.Time      `json:"started"`
	Updated                time.Time      `json:"updated"`
	Running                bool           `json:"running"`
	Status                 string         `json:"status"`
	Queries                int            `json:"queries"`
	Responses              int            `json:"responses"`
	SamplingNodes          int            `json:"samplingNodes"`
	Hashes                 int            `json:"hashes"`
	RetentionLimit         int            `json:"retentionLimit"`
	ActiveSamplers         int            `json:"activeSamplers"`
	HashSamples            int64          `json:"hashSamples"`
	Attempts               int            `json:"attempts"`
	Resolved               int            `json:"resolved"`
	Unresolved             int            `json:"unresolved"`
	Active                 int            `json:"active"`
	Pending                int            `json:"pending"`
	Workers                int            `json:"workers"`
	Completed              int            `json:"completed"`
	LookupsPerMinute       int            `json:"lookupsPerMinute"`
	LiveNamesPerMinute     int            `json:"liveNamesPerMinute"`
	IndexNamesPerMinute    int            `json:"indexNamesPerMinute"`
	VerifiedNamesPerMinute int            `json:"verifiedNamesPerMinute"`
	Successes              int            `json:"successes"`
	CacheHits              int            `json:"cacheHits"`
	CacheRequests          int64          `json:"cacheRequests"`
	CacheMissReuses        int64          `json:"cacheMissReuses"`
	CacheActive            int            `json:"cacheActive"`
	CacheCapacity          int            `json:"cacheCapacity"`
	CatalogHits            int            `json:"catalogHits"`
	DumpHits               int            `json:"dumpHits"`
	DumpLookups            int            `json:"dumpLookups"`
	PeerHintUses           int            `json:"peerHintUses"`
	EmptyLookups           int            `json:"emptyLookups"`
	PeerHits               int            `json:"peerHits"`
	AverageLookupMS        int64          `json:"averageLookupMs"`
	PayloadBytes           int64          `json:"payloadBytes"`
	DumpReady              bool           `json:"dumpReady"`
	DumpRows               int            `json:"dumpRows"`
	MatchCount             int            `json:"matchCount"`
	CountryTotals          map[string]int `json:"countryTotals"`
	Records                []observation  `json:"records"`
}
type dashboard struct {
	mu      sync.RWMutex
	state   state
	records map[string]observation
	// pendingSet holds unresolved hashes for O(pending) job picks instead of
	// scanning every named row in the unlimited store.
	pendingSet      map[string]struct{}
	pendingRevision uint64
	searchTok       map[string][]string
	searchCommon    map[string]bool
	cachePath       string
	dumpPath        string
	dumpMu          sync.RWMutex
	dump            *dumpIndex
	geo             *maxminddb.Reader
	cancel          context.CancelFunc
	done            chan struct{}
	control         sync.Mutex
	totalLookupTime time.Duration
	nativeSearch    nativeSearchIndex
	nameRates       [60]nameRateBucket
	persistMu       sync.Mutex
	metadataCaches  *metadataCaches
}

const namedRetention = 30 * 24 * time.Hour
const unresolvedRetention = 24 * time.Hour

func newDashboard(cachePath string, geo *maxminddb.Reader) *dashboard {
	d := &dashboard{
		records:      map[string]observation{},
		pendingSet:   map[string]struct{}{},
		searchTok:    map[string][]string{},
		searchCommon: map[string]bool{},
		nativeSearch: newNativeSearch(),
		cachePath:    cachePath,
		dumpPath:     filepath.Join(filepath.Dir(cachePath), "torrents.csv"),
		geo:          geo,
		state:        state{Started: time.Now().UTC(), Status: "Starting discovery"},
	}
	if b, e := os.ReadFile(cachePath); e == nil {
		var saved []observation
		if json.Unmarshal(b, &saved) == nil {
			now := time.Now()
			for _, r := range saved {
				if !validHash(r.Hash) {
					continue
				}
				// Named rows stay in the JSON store for weeks so search and
				// trending survive restarts. Unresolved DHT noise expires sooner.
				keepFor := unresolvedRetention
				if r.Resolved && strings.TrimSpace(r.Name) != "" {
					keepFor = namedRetention
				}
				if now.Sub(r.LastSeen) < keepFor {
					if r.Countries == nil {
						r.Countries = map[string]int{}
					}
					d.records[r.Hash] = r
					if !r.Resolved {
						d.pendingSet[r.Hash] = struct{}{}
					} else if strings.TrimSpace(r.Name) != "" {
						d.indexNameLocked(r.Hash, r.Name+" "+strings.Join(r.Files, " "))
					}
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
	// No count cap: the store grows with discovered and catalog/dump names.
	// Invalid hashes are rejected on load; age expiry runs in newDashboard.
	if retainedTorrentLimit <= 0 {
		return
	}
	if len(d.records) <= retainedTorrentLimit {
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
		if (all[i].Peers > 0) != (all[j].Peers > 0) {
			return all[i].Peers > 0
		}
		return all[i].LastSeen.After(all[j].LastSeen)
	})
	for _, r := range all[retainedTorrentLimit:] {
		delete(d.records, r.Hash)
	}
}
func (d *dashboard) snapshot() state {
	return d.snapshotForAPI(viewQuery{Limit: apiDefaultLimit, Sort: "trending"})
}
func (d *dashboard) persist()           { d.persistAll() }
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
		b, _ := json.Marshal(d.snapshotForAPI(parseViewQuery(r.URL.Query())))
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
		_ = json.NewEncoder(w).Encode(d.snapshotForAPI(parseViewQuery(r.URL.Query())))
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
		_ = json.NewEncoder(w).Encode(d.snapshotForAPI(viewQuery{Limit: apiDefaultLimit, Sort: "trending"}))
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
