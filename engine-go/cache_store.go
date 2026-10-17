package main

import (
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"time"

	"github.com/anacrolix/torrent"
)

const (
	peerHintTTL            = 45 * time.Minute
	peerHintMaxPerTorrent  = 32
	peerHintMaxTorrents    = 64
	dhtContactTTL          = 24 * time.Hour
	dhtContactMaxPerFamily = 32
	resumeWindowTTL        = 2 * time.Hour
	resumeWindowMaxBytes   = int64(4 << 20)
	resumeWindowTotalMax   = int64(16 << 20)
)

type cacheCategoryView struct {
	ID               string `json:"id"`
	Bytes            int64  `json:"bytes"`
	ReclaimableBytes int64  `json:"reclaimableBytes"`
}

type cacheStatusResponse struct {
	Categories  []cacheCategoryView `json:"categories"`
	MemoryBytes int64               `json:"memoryBytes"`
}

type peerHint struct {
	Addr        string    `json:"addr"`
	Source      string    `json:"source"`
	Transport   string    `json:"transport"`
	LastSuccess time.Time `json:"lastSuccess"`
	Expires     time.Time `json:"expires"`
}

type peerHintFile struct {
	Torrents map[string][]peerHint `json:"torrents"`
}

type dhtContact struct {
	Addr     string    `json:"addr"`
	Family   string    `json:"family"`
	LastGood time.Time `json:"lastGood"`
	Expires  time.Time `json:"expires"`
}

type dhtContactFile struct {
	Contacts []dhtContact `json:"contacts"`
}

type resumeWindowMeta struct {
	InfoHash  string    `json:"infoHash"`
	FileIndex int       `json:"fileIndex"`
	Offset    int64     `json:"offset"`
	Length    int64     `json:"length"`
	SavedAt   time.Time `json:"savedAt"`
	Expires   time.Time `json:"expires"`
}

func peerHintsPath(base string) string {
	return filepath.Join(base, ".cache", "peer-hints.json")
}

func dhtContactsPath(base string) string {
	return filepath.Join(base, ".cache", "dht-contacts.json")
}

func resumeWindowDir(base, hash string) string {
	return filepath.Join(base, ".cache", "resume", hash)
}

func dirBytes(path string) int64 {
	var total int64
	_ = filepath.Walk(path, func(_ string, info os.FileInfo, err error) error {
		if err != nil || info == nil || info.IsDir() {
			return nil
		}
		total += info.Size()
		return nil
	})
	return total
}

func (s *EngineServer) handleGetCache(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, http.StatusOK, s.cacheStatus())
}

func (s *EngineServer) handleClearCache(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Categories []string `json:"categories"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid json"})
		return
	}
	s.clearCacheCategories(req.Categories)
	writeJSON(w, http.StatusOK, s.cacheStatus())
}

func (s *EngineServer) cacheStatus() cacheStatusResponse {
	base := s.downloadDir
	startupBytes := dirBytes(filepath.Join(base, ".startup"))
	resumeBytes := dirBytes(filepath.Join(base, ".cache", "resume"))
	peerBytes := fileSize(peerHintsPath(base))
	dhtBytes := fileSize(dhtContactsPath(base))

	activeStartup, activeResume := s.activeCacheProtection()
	startupReclaim := startupBytes - activeStartup
	if startupReclaim < 0 {
		startupReclaim = 0
	}
	resumeReclaim := resumeBytes - activeResume
	if resumeReclaim < 0 {
		resumeReclaim = 0
	}

	var memory int64
	s.mu.RLock()
	for _, rec := range s.records {
		if st := s.storage.GetStorage(rec.InfoHash); st != nil {
			st.mu.Lock()
			memory += st.cacheBytes
			st.mu.Unlock()
		}
	}
	s.mu.RUnlock()

	return cacheStatusResponse{
		Categories: []cacheCategoryView{
			{ID: "startup", Bytes: startupBytes, ReclaimableBytes: startupReclaim},
			{ID: "resume", Bytes: resumeBytes, ReclaimableBytes: resumeReclaim},
			{ID: "peers", Bytes: peerBytes, ReclaimableBytes: peerBytes},
			{ID: "dht", Bytes: dhtBytes, ReclaimableBytes: dhtBytes},
		},
		MemoryBytes: memory,
	}
}

func fileSize(path string) int64 {
	info, err := os.Stat(path)
	if err != nil {
		return 0
	}
	return info.Size()
}

func (s *EngineServer) activeCacheProtection() (startup, resume int64) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	for _, rec := range s.records {
		st := s.storage.GetStorage(rec.InfoHash)
		if st == nil {
			continue
		}
		st.mu.Lock()
		if st.stream && !st.previewTemp {
			startup += dirBytes(filepath.Join(st.baseDir, ".startup", st.infoHash.HexString()))
		}
		st.mu.Unlock()
		if coord := s.playbackOwners[rec.Torrent]; coord != nil {
			coord.mu.Lock()
			if len(coord.readers) > 0 || len(coord.resumeDemand) > 0 {
				resume += dirBytes(resumeWindowDir(s.downloadDir, rec.InfoHash))
			}
			coord.mu.Unlock()
		}
	}
	return startup, resume
}

func (s *EngineServer) clearCacheCategories(categories []string) {
	if len(categories) == 0 {
		return
	}
	want := make(map[string]bool, len(categories))
	for _, c := range categories {
		want[c] = true
	}
	activeStartup, activeResume := s.activeCacheProtection()
	_ = activeStartup
	_ = activeResume
	base := s.downloadDir
	if want["peers"] {
		_ = os.Remove(peerHintsPath(base))
		s.peerHintMu.Lock()
		s.peerHints = nil
		s.peerHintMu.Unlock()
	}
	if want["dht"] {
		_ = os.Remove(dhtContactsPath(base))
	}
	if want["startup"] {
		s.clearInactiveStartupCaches()
	}
	if want["resume"] {
		s.clearInactiveResumeWindows()
	}
}

func (s *EngineServer) clearInactiveStartupCaches() {
	root := filepath.Join(s.downloadDir, ".startup")
	entries, err := os.ReadDir(root)
	if err != nil {
		return
	}
	protected := make(map[string]bool)
	s.mu.RLock()
	for _, rec := range s.records {
		if st := s.storage.GetStorage(rec.InfoHash); st != nil && st.StreamMode() {
			protected[rec.InfoHash] = true
		}
	}
	s.mu.RUnlock()
	for _, e := range entries {
		if !e.IsDir() || protected[e.Name()] {
			continue
		}
		_ = os.RemoveAll(filepath.Join(root, e.Name()))
	}
}

func (s *EngineServer) clearInactiveResumeWindows() {
	root := filepath.Join(s.downloadDir, ".cache", "resume")
	entries, err := os.ReadDir(root)
	if err != nil {
		return
	}
	protected := make(map[string]bool)
	s.playbackMu.Lock()
	for t, coord := range s.playbackOwners {
		coord.mu.Lock()
		active := len(coord.readers) > 0 || len(coord.resumeDemand) > 0
		coord.mu.Unlock()
		if !active {
			continue
		}
		s.mu.RLock()
		for _, rec := range s.records {
			if rec.Torrent == t {
				protected[rec.InfoHash] = true
			}
		}
		s.mu.RUnlock()
	}
	s.playbackMu.Unlock()
	for _, e := range entries {
		if !e.IsDir() || protected[e.Name()] {
			continue
		}
		_ = os.RemoveAll(filepath.Join(root, e.Name()))
	}
}

// --- CACHE-02 peer hints ---

func (s *EngineServer) rememberPeerHint(infoHash string, peer torrent.PeerInfo, private bool) {
	if private || peer.Addr == nil {
		return
	}
	host, port, err := net.SplitHostPort(peer.Addr.String())
	if err != nil || net.ParseIP(host) == nil || port == "0" {
		return
	}
	transport := "tcp"
	if _, ok := peer.Addr.(*net.UDPAddr); ok {
		transport = "utp"
	}
	now := time.Now()
	hint := peerHint{
		Addr:        net.JoinHostPort(host, port),
		Source:      fmt.Sprint(peer.Source),
		Transport:   transport,
		LastSuccess: now,
		Expires:     now.Add(peerHintTTL),
	}
	s.peerHintMu.Lock()
	defer s.peerHintMu.Unlock()
	if s.peerHints == nil {
		s.peerHints = make(map[string][]peerHint)
	}
	list := s.peerHints[infoHash]
	replaced := false
	for i := range list {
		if list[i].Addr == hint.Addr && list[i].Transport == hint.Transport {
			list[i] = hint
			replaced = true
			break
		}
	}
	if !replaced {
		list = append(list, hint)
	}
	sort.Slice(list, func(i, j int) bool { return list[i].LastSuccess.After(list[j].LastSuccess) })
	if len(list) > peerHintMaxPerTorrent {
		list = list[:peerHintMaxPerTorrent]
	}
	s.peerHints[infoHash] = list
	for len(s.peerHints) > peerHintMaxTorrents {
		var oldest string
		var oldestTime time.Time
		first := true
		for hash, hints := range s.peerHints {
			if hash == infoHash || len(hints) == 0 {
				continue
			}
			if first || hints[0].LastSuccess.Before(oldestTime) {
				oldest, oldestTime, first = hash, hints[0].LastSuccess, false
			}
		}
		if oldest == "" {
			break
		}
		delete(s.peerHints, oldest)
	}
	s.persistPeerHintsLocked()
}

func (s *EngineServer) persistPeerHintsLocked() {
	path := peerHintsPath(s.downloadDir)
	_ = os.MkdirAll(filepath.Dir(path), 0700)
	payload := peerHintFile{Torrents: s.peerHints}
	data, err := json.Marshal(payload)
	if err != nil {
		return
	}
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, data, 0600); err != nil {
		return
	}
	_ = os.Rename(tmp, path)
}

func (s *EngineServer) loadPeerHints() {
	data, err := os.ReadFile(peerHintsPath(s.downloadDir))
	if err != nil {
		return
	}
	var file peerHintFile
	if json.Unmarshal(data, &file) != nil || file.Torrents == nil {
		return
	}
	now := time.Now()
	out := make(map[string][]peerHint)
	for hash, hints := range file.Torrents {
		if !isHexInfoHash(hash) {
			continue
		}
		kept := hints[:0:0]
		for _, h := range hints {
			if h.Expires.After(now) && h.Addr != "" {
				kept = append(kept, h)
			}
		}
		if len(kept) > 0 {
			out[hash] = kept
		}
	}
	s.peerHintMu.Lock()
	s.peerHints = out
	s.peerHintMu.Unlock()
}

func isHexInfoHash(v string) bool {
	if len(v) != 40 {
		return false
	}
	for _, c := range v {
		if !((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F')) {
			return false
		}
	}
	return true
}

func (s *EngineServer) applyPeerHints(rec *TorrentRecord) {
	if rec == nil || rec.Torrent == nil || rec.Torrent.Info() == nil {
		return
	}
	info := rec.Torrent.Info()
	if info.Private != nil && *info.Private {
		return
	}
	s.peerHintMu.Lock()
	hints := append([]peerHint(nil), s.peerHints[rec.InfoHash]...)
	s.peerHintMu.Unlock()
	now := time.Now()
	peers := make([]torrent.PeerInfo, 0, len(hints))
	for _, h := range hints {
		if !h.Expires.After(now) {
			continue
		}
		addr, err := net.ResolveTCPAddr("tcp", h.Addr)
		if err != nil {
			continue
		}
		peers = append(peers, torrent.PeerInfo{Addr: addr, Source: torrent.PeerSourceTracker})
	}
	if len(peers) > 0 {
		rec.Torrent.AddPeers(peers)
	}
}

func (s *EngineServer) forgetPeerHints(infoHash string) {
	s.peerHintMu.Lock()
	defer s.peerHintMu.Unlock()
	if s.peerHints == nil {
		return
	}
	delete(s.peerHints, infoHash)
	s.persistPeerHintsLocked()
}

// --- CACHE-03 DHT contacts ---

func (s *EngineServer) rememberDHTContact(addr string, family string) {
	ip := net.ParseIP(addr)
	if ip == nil {
		host, _, err := net.SplitHostPort(addr)
		if err != nil {
			return
		}
		ip = net.ParseIP(host)
		if ip == nil {
			return
		}
		addr = net.JoinHostPort(host, "6881")
	}
	if family == "" {
		if ip.To4() != nil {
			family = "ipv4"
		} else {
			family = "ipv6"
		}
	}
	now := time.Now()
	contact := dhtContact{Addr: addr, Family: family, LastGood: now, Expires: now.Add(dhtContactTTL)}
	s.dhtMu.Lock()
	defer s.dhtMu.Unlock()
	list := s.dhtContacts
	replaced := false
	for i := range list {
		if list[i].Addr == contact.Addr {
			list[i] = contact
			replaced = true
			break
		}
	}
	if !replaced {
		list = append(list, contact)
	}
	var v4, v6 []dhtContact
	for _, c := range list {
		if !c.Expires.After(now) {
			continue
		}
		if c.Family == "ipv6" {
			v6 = append(v6, c)
		} else {
			v4 = append(v4, c)
		}
	}
	sort.Slice(v4, func(i, j int) bool { return v4[i].LastGood.After(v4[j].LastGood) })
	sort.Slice(v6, func(i, j int) bool { return v6[i].LastGood.After(v6[j].LastGood) })
	if len(v4) > dhtContactMaxPerFamily {
		v4 = v4[:dhtContactMaxPerFamily]
	}
	if len(v6) > dhtContactMaxPerFamily {
		v6 = v6[:dhtContactMaxPerFamily]
	}
	s.dhtContacts = append(v4, v6...)
	s.persistDHTContactsLocked()
}

func (s *EngineServer) persistDHTContactsLocked() {
	path := dhtContactsPath(s.downloadDir)
	_ = os.MkdirAll(filepath.Dir(path), 0700)
	data, err := json.Marshal(dhtContactFile{Contacts: s.dhtContacts})
	if err != nil {
		return
	}
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, data, 0600); err != nil {
		return
	}
	_ = os.Rename(tmp, path)
}

func (s *EngineServer) loadDHTContacts() {
	data, err := os.ReadFile(dhtContactsPath(s.downloadDir))
	if err != nil {
		return
	}
	var file dhtContactFile
	if json.Unmarshal(data, &file) != nil {
		return
	}
	now := time.Now()
	kept := make([]dhtContact, 0, len(file.Contacts))
	for _, c := range file.Contacts {
		if c.Expires.After(now) && c.Addr != "" {
			kept = append(kept, c)
		}
	}
	s.dhtMu.Lock()
	s.dhtContacts = kept
	s.dhtMu.Unlock()
}

func (s *EngineServer) warmDHTContacts() {
	s.dhtMu.Lock()
	contacts := append([]dhtContact(nil), s.dhtContacts...)
	s.dhtMu.Unlock()
	if len(contacts) == 0 || s.client == nil {
		return
	}
	nodes := make([]string, 0, len(contacts))
	for _, c := range contacts {
		nodes = append(nodes, c.Addr)
	}
	s.client.AddDhtNodes(nodes)
}

func (s *EngineServer) snapshotDHTContacts() {
	if s.client == nil {
		return
	}
	for _, ds := range s.client.DhtServers() {
		w, ok := ds.(torrent.AnacrolixDhtServerWrapper)
		if !ok {
			continue
		}
		for _, ni := range w.Nodes() {
			if ni.Addr.IP == nil || ni.Addr.Port <= 0 {
				continue
			}
			family := "ipv4"
			if ni.Addr.IP.To4() == nil {
				family = "ipv6"
			}
			s.rememberDHTContact(net.JoinHostPort(ni.Addr.IP.String(), fmt.Sprintf("%d", ni.Addr.Port)), family)
		}
	}
}

// --- CACHE-04 resume window ---

func (s *EngineServer) saveResumeWindow(rec *TorrentRecord, fileIndex int, offset, length int64, data []byte) {
	if rec == nil || length <= 0 || int64(len(data)) != length || length > resumeWindowMaxBytes {
		return
	}
	dir := resumeWindowDir(s.downloadDir, rec.InfoHash)
	if err := os.MkdirAll(dir, 0700); err != nil {
		return
	}
	// Bound total resume cache size.
	if dirBytes(filepath.Join(s.downloadDir, ".cache", "resume"))+length > resumeWindowTotalMax {
		s.clearInactiveResumeWindows()
		if dirBytes(filepath.Join(s.downloadDir, ".cache", "resume"))+length > resumeWindowTotalMax {
			return
		}
	}
	meta := resumeWindowMeta{
		InfoHash:  rec.InfoHash,
		FileIndex: fileIndex,
		Offset:    offset,
		Length:    length,
		SavedAt:   time.Now(),
		Expires:   time.Now().Add(resumeWindowTTL),
	}
	raw, _ := json.Marshal(meta)
	tmpMeta := filepath.Join(dir, "window.json.tmp")
	tmpData := filepath.Join(dir, "window.bin.tmp")
	if err := os.WriteFile(tmpData, data, 0600); err != nil {
		return
	}
	if err := os.WriteFile(tmpMeta, raw, 0600); err != nil {
		_ = os.Remove(tmpData)
		return
	}
	_ = os.Rename(tmpData, filepath.Join(dir, "window.bin"))
	_ = os.Rename(tmpMeta, filepath.Join(dir, "window.json"))
}

func (s *EngineServer) loadResumeWindow(infoHash string) (resumeWindowMeta, []byte, bool) {
	dir := resumeWindowDir(s.downloadDir, infoHash)
	raw, err := os.ReadFile(filepath.Join(dir, "window.json"))
	if err != nil {
		return resumeWindowMeta{}, nil, false
	}
	var meta resumeWindowMeta
	if json.Unmarshal(raw, &meta) != nil || meta.InfoHash != infoHash || meta.Length <= 0 || meta.Length > resumeWindowMaxBytes {
		return resumeWindowMeta{}, nil, false
	}
	if !meta.Expires.After(time.Now()) {
		_ = os.RemoveAll(dir)
		return resumeWindowMeta{}, nil, false
	}
	data, err := os.ReadFile(filepath.Join(dir, "window.bin"))
	if err != nil || int64(len(data)) != meta.Length {
		return resumeWindowMeta{}, nil, false
	}
	return meta, data, true
}

func (s *EngineServer) deleteResumeWindow(infoHash string) {
	_ = os.RemoveAll(resumeWindowDir(s.downloadDir, infoHash))
}
