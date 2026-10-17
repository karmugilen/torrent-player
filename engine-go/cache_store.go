package main

import (
	"crypto/sha1"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
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
	Version     int       `json:"version"`
	InfoHash    string    `json:"infoHash"`
	FileIndex   int       `json:"fileIndex"`
	FileName    string    `json:"fileName"`
	FileLength  int64     `json:"fileLength"`
	PieceLength int64     `json:"pieceLength"`
	Offset      int64     `json:"offset"`
	Length      int64     `json:"length"`
	Checksum    string    `json:"checksum"`
	Generation  uint64    `json:"generation"`
	SavedAt     time.Time `json:"savedAt"`
	Expires     time.Time `json:"expires"`
}

const resumeWindowVersion = 2
const peerHintMaxLoadBytes = int64(1 << 20)
const dhtContactMaxLoadBytes = int64(512 << 10)

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
	if err := s.clearCacheCategories(req.Categories); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": err.Error()})
		return
	}
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
	type pair struct {
		hash string
		t    *torrent.Torrent
		st   *DocumentTorrentStorage
	}
	s.mu.RLock()
	items := make([]pair, 0, len(s.records))
	for _, rec := range s.records {
		items = append(items, pair{hash: rec.InfoHash, t: rec.Torrent, st: s.storage.GetStorage(rec.InfoHash)})
	}
	s.mu.RUnlock()

	s.playbackMu.Lock()
	owners := make(map[*torrent.Torrent]*playbackPriorityCoordinator, len(s.playbackOwners))
	for t, c := range s.playbackOwners {
		owners[t] = c
	}
	s.playbackMu.Unlock()

	for _, item := range items {
		if item.st != nil {
			item.st.mu.Lock()
			stream := item.st.stream && !item.st.previewTemp
			base := item.st.baseDir
			hash := item.st.infoHash.HexString()
			item.st.mu.Unlock()
			if stream {
				startup += dirBytes(filepath.Join(base, ".startup", hash))
			}
		}
		if coord := owners[item.t]; coord != nil {
			coord.mu.Lock()
			active := len(coord.readers) > 0 || len(coord.resumeDemand) > 0
			coord.mu.Unlock()
			if active {
				resume += dirBytes(resumeWindowDir(s.downloadDir, item.hash))
			}
		}
	}
	return startup, resume
}

func (s *EngineServer) clearCacheCategories(categories []string) error {
	if len(categories) == 0 {
		return fmt.Errorf("no categories")
	}
	want := make(map[string]bool, len(categories))
	for _, c := range categories {
		switch c {
		case "startup", "resume", "peers", "dht":
			want[c] = true
		default:
			return fmt.Errorf("invalid cache category %q", c)
		}
	}
	s.cacheClearGen++
	gen := s.cacheClearGen
	base := s.downloadDir
	var firstErr error
	note := func(err error) {
		if err != nil && !os.IsNotExist(err) && firstErr == nil {
			firstErr = err
		}
	}
	if want["peers"] {
		s.peerHintMu.Lock()
		s.peerHintGen++
		s.peerHints = nil
		note(os.Remove(peerHintsPath(base)))
		s.peerHintMu.Unlock()
	}
	if want["dht"] {
		s.dhtMu.Lock()
		s.dhtGen++
		s.dhtContacts = nil
		note(os.Remove(dhtContactsPath(base)))
		s.dhtMu.Unlock()
	}
	if want["startup"] {
		note(s.clearInactiveStartupCaches())
	}
	if want["resume"] {
		note(s.clearInactiveResumeWindows(gen))
	}
	return firstErr
}

func (s *EngineServer) clearInactiveStartupCaches() error {
	root := filepath.Join(s.downloadDir, ".startup")
	entries, err := os.ReadDir(root)
	if err != nil {
		if os.IsNotExist(err) {
			return nil
		}
		return err
	}
	protected := make(map[string]bool)
	s.mu.RLock()
	for _, rec := range s.records {
		if st := s.storage.GetStorage(rec.InfoHash); st != nil && st.StreamMode() {
			protected[rec.InfoHash] = true
		}
	}
	s.mu.RUnlock()
	var firstErr error
	for _, e := range entries {
		if !e.IsDir() || protected[e.Name()] {
			continue
		}
		if err := os.RemoveAll(filepath.Join(root, e.Name())); err != nil && firstErr == nil {
			firstErr = err
		}
	}
	return firstErr
}

func (s *EngineServer) clearInactiveResumeWindows(clearGen uint64) error {
	root := filepath.Join(s.downloadDir, ".cache", "resume")
	entries, err := os.ReadDir(root)
	if err != nil {
		if os.IsNotExist(err) {
			return nil
		}
		return err
	}
	protected := make(map[string]bool)
	s.playbackMu.Lock()
	owners := make(map[*torrent.Torrent]*playbackPriorityCoordinator, len(s.playbackOwners))
	for t, coord := range s.playbackOwners {
		owners[t] = coord
	}
	s.playbackMu.Unlock()
	s.mu.RLock()
	for _, rec := range s.records {
		coord := owners[rec.Torrent]
		if coord == nil {
			continue
		}
		coord.mu.Lock()
		active := len(coord.readers) > 0 || len(coord.resumeDemand) > 0
		coord.mu.Unlock()
		if active {
			protected[rec.InfoHash] = true
		}
	}
	s.mu.RUnlock()
	var firstErr error
	for _, e := range entries {
		if !e.IsDir() || protected[e.Name()] {
			continue
		}
		if clearGen != 0 && s.cacheClearGen != clearGen {
			break
		}
		if err := os.RemoveAll(filepath.Join(root, e.Name())); err != nil && firstErr == nil {
			firstErr = err
		}
	}
	return firstErr
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
	ip := net.ParseIP(host)
	if ip == nil || ip.IsUnspecified() || ip.IsLoopback() || ip.IsMulticast() {
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
	gen := s.peerHintGen
	if err := os.WriteFile(tmp, data, 0600); err != nil {
		return
	}
	if s.peerHintGen != gen {
		_ = os.Remove(tmp)
		return
	}
	_ = os.Rename(tmp, path)
}

func (s *EngineServer) loadPeerHints() {
	path := peerHintsPath(s.downloadDir)
	info, err := os.Stat(path)
	if err != nil || info.Size() > peerHintMaxLoadBytes {
		return
	}
	data, err := os.ReadFile(path)
	if err != nil || int64(len(data)) > peerHintMaxLoadBytes {
		return
	}
	var file peerHintFile
	if json.Unmarshal(data, &file) != nil || file.Torrents == nil {
		return
	}
	now := time.Now()
	out := make(map[string][]peerHint)
	count := 0
	for hash, hints := range file.Torrents {
		if !isHexInfoHash(hash) || count >= peerHintMaxTorrents {
			continue
		}
		kept := make([]peerHint, 0, len(hints))
		for _, h := range hints {
			host, port, err := net.SplitHostPort(h.Addr)
			if err != nil || net.ParseIP(host) == nil {
				continue
			}
			p, err := strconv.Atoi(port)
			if err != nil || p <= 0 || p > 65535 {
				continue
			}
			if h.Expires.After(now) && len(kept) < peerHintMaxPerTorrent {
				kept = append(kept, h)
			}
		}
		if len(kept) > 0 {
			out[hash] = kept
			count++
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
	s.rememberDHTContactAt(addr, family, time.Time{})
}

// rememberDHTContactAt stores a validated UDP endpoint. lastGood zero means
// "seen in routing table, not yet confirmed responsive" and is stored as the
// zero time so restore treats the contact as needing revalidation.
func (s *EngineServer) rememberDHTContactAt(addr string, family string, lastGood time.Time) {
	host, port, err := net.SplitHostPort(addr)
	if err != nil {
		// Bare IP without port is not a usable DHT endpoint.
		return
	}
	ip := net.ParseIP(host)
	if ip == nil || ip.IsUnspecified() || ip.IsLoopback() || ip.IsMulticast() {
		return
	}
	p, err := strconv.Atoi(port)
	if err != nil || p <= 0 || p > 65535 {
		return
	}
	addr = net.JoinHostPort(ip.String(), strconv.Itoa(p))
	if family == "" {
		if ip.To4() != nil {
			family = "ipv4"
		} else {
			family = "ipv6"
		}
	}
	now := time.Now()
	contact := dhtContact{Addr: addr, Family: family, LastGood: lastGood, Expires: now.Add(dhtContactTTL)}
	s.dhtMu.Lock()
	defer s.dhtMu.Unlock()
	list := s.dhtContacts
	replaced := false
	for i := range list {
		if list[i].Addr == contact.Addr {
			// Preserve a stronger LastGood if the new evidence is empty.
			if contact.LastGood.IsZero() && !list[i].LastGood.IsZero() {
				contact.LastGood = list[i].LastGood
			}
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
	gen := s.dhtGen
	if err := os.WriteFile(tmp, data, 0600); err != nil {
		return
	}
	if s.dhtGen != gen {
		_ = os.Remove(tmp)
		return
	}
	_ = os.Rename(tmp, path)
}

func (s *EngineServer) loadDHTContacts() {
	path := dhtContactsPath(s.downloadDir)
	info, err := os.Stat(path)
	if err != nil || info.Size() > dhtContactMaxLoadBytes {
		return
	}
	data, err := os.ReadFile(path)
	if err != nil || int64(len(data)) > dhtContactMaxLoadBytes {
		return
	}
	var file dhtContactFile
	if json.Unmarshal(data, &file) != nil {
		return
	}
	now := time.Now()
	kept := make([]dhtContact, 0, len(file.Contacts))
	for _, c := range file.Contacts {
		host, port, err := net.SplitHostPort(c.Addr)
		if err != nil || net.ParseIP(host) == nil {
			continue
		}
		p, err := strconv.Atoi(port)
		if err != nil || p <= 0 || p > 65535 {
			continue
		}
		if c.Expires.After(now) {
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
			// Routing-table presence is not proof of a recent response.
			s.rememberDHTContactAt(
				net.JoinHostPort(ni.Addr.IP.String(), strconv.Itoa(ni.Addr.Port)),
				family,
				time.Time{},
			)
		}
	}
}

// --- CACHE-04 resume window ---

func resumeChecksum(data []byte) string {
	sum := sha1.Sum(data)
	return hex.EncodeToString(sum[:])
}

func (s *EngineServer) saveResumeWindow(rec *TorrentRecord, fileIndex int, offset, length int64, data []byte) {
	s.saveResumeWindowDetailed(rec, fileIndex, "", 0, 0, offset, length, data, 0)
}

func (s *EngineServer) saveResumeWindowDetailed(
	rec *TorrentRecord,
	fileIndex int,
	fileName string,
	fileLength, pieceLength, offset, length int64,
	data []byte,
	generation uint64,
) {
	if rec == nil || length <= 0 || int64(len(data)) != length || length > resumeWindowMaxBytes {
		return
	}
	dir := resumeWindowDir(s.downloadDir, rec.InfoHash)
	if err := os.MkdirAll(dir, 0700); err != nil {
		return
	}
	// Bound total resume cache size.
	if dirBytes(filepath.Join(s.downloadDir, ".cache", "resume"))+length > resumeWindowTotalMax {
		_ = s.clearInactiveResumeWindows(0)
		if dirBytes(filepath.Join(s.downloadDir, ".cache", "resume"))+length > resumeWindowTotalMax {
			return
		}
	}
	if fileName == "" && rec.Torrent != nil {
		files := rec.Torrent.Files()
		if fileIndex >= 0 && fileIndex < len(files) {
			fileName = files[fileIndex].DisplayPath()
			fileLength = files[fileIndex].Length()
		}
		if info := rec.Torrent.Info(); info != nil {
			pieceLength = info.PieceLength
		}
	}
	meta := resumeWindowMeta{
		Version:     resumeWindowVersion,
		InfoHash:    rec.InfoHash,
		FileIndex:   fileIndex,
		FileName:    fileName,
		FileLength:  fileLength,
		PieceLength: pieceLength,
		Offset:      offset,
		Length:      length,
		Checksum:    resumeChecksum(data),
		Generation:  generation,
		SavedAt:     time.Now(),
		Expires:     time.Now().Add(resumeWindowTTL),
	}
	raw, err := json.Marshal(meta)
	if err != nil {
		return
	}
	tmpDir, err := os.MkdirTemp(dir, "write-*")
	if err != nil {
		return
	}
	defer os.RemoveAll(tmpDir)
	tmpData := filepath.Join(tmpDir, "window.bin")
	tmpMeta := filepath.Join(tmpDir, "window.json")
	if err := os.WriteFile(tmpData, data, 0600); err != nil {
		return
	}
	if err := os.WriteFile(tmpMeta, raw, 0600); err != nil {
		return
	}
	finalData := filepath.Join(dir, "window.bin")
	finalMeta := filepath.Join(dir, "window.json")
	if err := os.Rename(tmpData, finalData); err != nil {
		return
	}
	if err := os.Rename(tmpMeta, finalMeta); err != nil {
		// Keep previous good meta if present; drop inconsistent data.
		_ = os.Remove(finalData)
		return
	}
}

func (s *EngineServer) loadResumeWindow(infoHash string) (resumeWindowMeta, []byte, bool) {
	dir := resumeWindowDir(s.downloadDir, infoHash)
	metaPath := filepath.Join(dir, "window.json")
	info, err := os.Stat(metaPath)
	if err != nil || info.Size() > 64<<10 {
		return resumeWindowMeta{}, nil, false
	}
	raw, err := os.ReadFile(metaPath)
	if err != nil {
		return resumeWindowMeta{}, nil, false
	}
	var meta resumeWindowMeta
	if json.Unmarshal(raw, &meta) != nil || meta.InfoHash != infoHash || meta.Length <= 0 || meta.Length > resumeWindowMaxBytes {
		return resumeWindowMeta{}, nil, false
	}
	if meta.Version != 0 && meta.Version != resumeWindowVersion {
		return resumeWindowMeta{}, nil, false
	}
	if !meta.Expires.After(time.Now()) {
		_ = os.RemoveAll(dir)
		return resumeWindowMeta{}, nil, false
	}
	binPath := filepath.Join(dir, "window.bin")
	binInfo, err := os.Stat(binPath)
	if err != nil || binInfo.Size() != meta.Length || binInfo.Size() > resumeWindowMaxBytes {
		return resumeWindowMeta{}, nil, false
	}
	data, err := os.ReadFile(binPath)
	if err != nil || int64(len(data)) != meta.Length {
		return resumeWindowMeta{}, nil, false
	}
	if meta.Checksum != "" && resumeChecksum(data) != meta.Checksum {
		return resumeWindowMeta{}, nil, false
	}
	return meta, data, true
}

func (s *EngineServer) deleteResumeWindow(infoHash string) {
	_ = os.RemoveAll(resumeWindowDir(s.downloadDir, infoHash))
}

// applyResumeWindowToStorage injects a validated resume window into storage so
// OpenPlayback can reuse already-verified bytes without an extra fetch.
func (s *EngineServer) applyResumeWindowToStorage(rec *TorrentRecord, fileIndex int) (offset int64, ok bool) {
	if rec == nil || rec.Torrent == nil || rec.Torrent.Info() == nil {
		return 0, false
	}
	meta, data, loaded := s.loadResumeWindow(rec.InfoHash)
	if !loaded || meta.FileIndex != fileIndex {
		return 0, false
	}
	files := rec.Torrent.Files()
	if fileIndex < 0 || fileIndex >= len(files) {
		return 0, false
	}
	file := files[fileIndex]
	info := rec.Torrent.Info()
	if meta.FileName != "" && meta.FileName != file.DisplayPath() {
		return 0, false
	}
	if meta.FileLength != 0 && meta.FileLength != file.Length() {
		return 0, false
	}
	if meta.PieceLength != 0 && meta.PieceLength != info.PieceLength {
		return 0, false
	}
	if meta.Offset < 0 || meta.Offset+meta.Length > file.Length() {
		return 0, false
	}
	st := s.storage.GetStorage(rec.InfoHash)
	if st == nil {
		return 0, false
	}
	abs := file.Offset() + meta.Offset
	if !st.injectVerifiedBytes(abs, data) {
		return 0, false
	}
	return meta.Offset, true
}

// peerDialEndpoint returns a validated dial/listen address for caching.
// Inbound RemoteAddr may be a transient source port; prefer PeerListenPort.
func peerDialEndpoint(pc *torrent.PeerConn) net.Addr {
	if pc == nil || pc.RemoteAddr == nil {
		return nil
	}
	host, portStr, err := net.SplitHostPort(pc.RemoteAddr.String())
	if err != nil {
		return nil
	}
	ip := net.ParseIP(host)
	if ip == nil || ip.IsUnspecified() || ip.IsLoopback() || ip.IsMulticast() {
		return nil
	}
	port, err := strconv.Atoi(portStr)
	if err != nil || port <= 0 || port > 65535 {
		return nil
	}
	// Prefer the peer's advertised listen port when present (inbound source
	// ports are not dialable endpoints).
	if pc.PeerListenPort > 0 && pc.PeerListenPort <= 65535 {
		port = pc.PeerListenPort
	}
	network := strings.ToLower(pc.Network)
	if strings.Contains(network, "udp") || strings.Contains(network, "utp") {
		return &net.UDPAddr{IP: ip, Port: port}
	}
	return &net.TCPAddr{IP: ip, Port: port}
}
