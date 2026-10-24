package main

import (
	"encoding/json"
	"testing"
	"time"

	"github.com/anacrolix/torrent"
)

// Hybrid (btih + btmh) magnets with trackers aborted the whole app: the
// library half-registered the v2 tracker keys and panicked inside its unlock.
func TestAddHybridMagnetWithTrackersKeepsEngineAlive(t *testing.T) {
	s := testEngine(t, func(cfg *torrent.ClientConfig) { cfg.DisableTrackers = false })
	magnet := "magnet:?xt=urn:btih:643bf2b0362ff2548c0d34c56762e74e2302f88f" +
		"&xt=urn:btmh:1220deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef" +
		"&dn=hybrid&tr=http%3A%2F%2F127.0.0.1%3A1%2Fannounce&tr=udp%3A%2F%2F127.0.0.1%3A1%2Fannounce"
	request, _ := json.Marshal(AddRequest{TorrentID: magnet, Prepare: true})
	code, response := s.DispatchControl("POST", "/add", string(request))
	if code != 200 {
		t.Fatalf("add: %d %s", code, response)
	}
	var added AddResponse
	if err := json.Unmarshal(response, &added); err != nil {
		t.Fatal(err)
	}
	// Tracker updates run on client unlocks; give announces and refreshes time.
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		if code, response := s.DispatchControl("GET", "/torrent/"+added.ID, ""); code != 200 {
			t.Fatalf("torrent: %d %s", code, response)
		}
		time.Sleep(100 * time.Millisecond)
	}
}
