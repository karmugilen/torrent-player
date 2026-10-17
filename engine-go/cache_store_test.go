package main

import (
	"encoding/json"
	"net"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/anacrolix/torrent"
)

func TestCacheStatusAndClearCategories(t *testing.T) {
	dir := t.TempDir()
	s := testEngineAt(t, dir)
	_ = os.MkdirAll(filepath.Join(dir, ".startup", "abc"), 0700)
	_ = os.WriteFile(filepath.Join(dir, ".startup", "abc", "0.piece"), []byte("startup"), 0600)
	_ = os.MkdirAll(filepath.Join(dir, ".cache", "resume", "abc"), 0700)
	_ = os.WriteFile(filepath.Join(dir, ".cache", "resume", "abc", "window.bin"), []byte("resume-bytes"), 0600)
	s.peerHintMu.Lock()
	s.peerHints = map[string][]peerHint{
		"0123456789abcdef0123456789abcdef01234567": {{
			Addr: "1.2.3.4:6881", Transport: "tcp", LastSuccess: time.Now(), Expires: time.Now().Add(time.Hour),
		}},
	}
	s.persistPeerHintsLocked()
	s.peerHintMu.Unlock()
	s.dhtMu.Lock()
	s.dhtContacts = []dhtContact{{Addr: "8.8.8.8:6881", Family: "ipv4", LastGood: time.Now(), Expires: time.Now().Add(time.Hour)}}
	s.persistDHTContactsLocked()
	s.dhtMu.Unlock()

	code, body := s.DispatchControl("GET", "/cache", "")
	if code != 200 {
		t.Fatalf("get cache: %d %s", code, body)
	}
	var status cacheStatusResponse
	if err := json.Unmarshal(body, &status); err != nil {
		t.Fatal(err)
	}
	byID := map[string]cacheCategoryView{}
	for _, c := range status.Categories {
		byID[c.ID] = c
	}
	for _, id := range []string{"startup", "resume", "peers", "dht"} {
		if byID[id].Bytes <= 0 {
			t.Fatalf("expected bytes for %s: %+v", id, byID[id])
		}
	}

	code, body = s.DispatchControl("POST", "/cache/clear", `{"categories":["peers","dht","startup","resume"]}`)
	if code != 200 {
		t.Fatalf("clear: %d %s", code, body)
	}
	if err := json.Unmarshal(body, &status); err != nil {
		t.Fatal(err)
	}
	for _, c := range status.Categories {
		if c.Bytes != 0 {
			t.Fatalf("category %s still has bytes after clear: %+v", c.ID, c)
		}
	}
}

func TestPeerHintsPersistPublicOnlyAndExpire(t *testing.T) {
	dir := t.TempDir()
	s := testEngineAt(t, dir)
	hash := "0123456789abcdef0123456789abcdef01234567"
	s.rememberPeerHint(hash, torrent.PeerInfo{
		Addr: &net.TCPAddr{IP: net.ParseIP("9.9.9.9"), Port: 6881}, Source: torrent.PeerSourceTracker,
	}, false)
	s.rememberPeerHint(hash, torrent.PeerInfo{
		Addr: &net.TCPAddr{IP: net.ParseIP("9.9.9.8"), Port: 6881}, Source: torrent.PeerSourceTracker,
	}, true) // private must be ignored

	s.peerHintMu.Lock()
	if len(s.peerHints[hash]) != 1 {
		t.Fatalf("expected one public hint, got %#v", s.peerHints[hash])
	}
	s.peerHints[hash][0].Expires = time.Now().Add(-time.Minute)
	s.persistPeerHintsLocked()
	s.peerHintMu.Unlock()

	s2 := testEngineAt(t, dir)
	s2.loadPeerHints()
	s2.peerHintMu.Lock()
	defer s2.peerHintMu.Unlock()
	if len(s2.peerHints[hash]) != 0 {
		t.Fatalf("expired hints should not load: %#v", s2.peerHints[hash])
	}
}

func TestDHTContactsRoundTripBounded(t *testing.T) {
	dir := t.TempDir()
	s := testEngineAt(t, dir)
	for i := 0; i < dhtContactMaxPerFamily+5; i++ {
		ip := net.IPv4(1, 2, byte(i/256), byte(i))
		s.rememberDHTContact(net.JoinHostPort(ip.String(), "6881"), "ipv4")
	}
	s.dhtMu.Lock()
	if len(s.dhtContacts) > dhtContactMaxPerFamily {
		t.Fatalf("ipv4 contacts unbounded: %d", len(s.dhtContacts))
	}
	s.dhtMu.Unlock()

	s2 := testEngineAt(t, dir)
	s2.loadDHTContacts()
	s2.dhtMu.Lock()
	defer s2.dhtMu.Unlock()
	if len(s2.dhtContacts) == 0 {
		t.Fatal("expected persisted DHT contacts")
	}
}

func TestResumeWindowPersistsVerifiedBytesOnly(t *testing.T) {
	dir := t.TempDir()
	s := testEngineAt(t, dir)
	rec := &TorrentRecord{InfoHash: "0123456789abcdef0123456789abcdef01234567"}
	payload := []byte("verified-resume-window-bytes-0123456789")
	s.saveResumeWindow(rec, 0, 1024, int64(len(payload)), payload)
	meta, data, ok := s.loadResumeWindow(rec.InfoHash)
	if !ok {
		t.Fatal("resume window missing")
	}
	if meta.Offset != 1024 || meta.FileIndex != 0 || string(data) != string(payload) {
		t.Fatalf("unexpected resume window meta=%+v data=%q", meta, data)
	}
	// Corrupt length must miss safely.
	_ = os.WriteFile(filepath.Join(resumeWindowDir(dir, rec.InfoHash), "window.bin"), []byte("x"), 0600)
	if _, _, ok := s.loadResumeWindow(rec.InfoHash); ok {
		t.Fatal("corrupt resume window must not load")
	}
}

func TestForgetPeerHintsOnRemovePath(t *testing.T) {
	dir := t.TempDir()
	s := testEngineAt(t, dir)
	hash := "abcdef0123456789abcdef0123456789abcdef01"
	s.rememberPeerHint(hash, torrent.PeerInfo{
		Addr: &net.TCPAddr{IP: net.ParseIP("5.6.7.8"), Port: 1}, Source: torrent.PeerSourceTracker,
	}, false)
	s.forgetPeerHints(hash)
	if _, err := os.Stat(peerHintsPath(dir)); err == nil {
		raw, _ := os.ReadFile(peerHintsPath(dir))
		var file peerHintFile
		_ = json.Unmarshal(raw, &file)
		if len(file.Torrents[hash]) != 0 {
			t.Fatalf("hint remained after forget: %#v", file.Torrents[hash])
		}
	}
}
