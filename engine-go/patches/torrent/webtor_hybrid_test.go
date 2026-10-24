package torrent

import (
	"testing"

	g "github.com/anacrolix/generics"
	"github.com/go-quicktest/qt"

	infohash_v2 "github.com/anacrolix/torrent/types/infohash-v2"
)

// A hybrid magnet (btih + btmh) with a regular tracker must register both
// short infohashes, so the tracker announce dispatcher can find the torrent
// under each. Before the fix the v2 key was half-added and the next tracker
// state update panicked inside Client.unlock, aborting the process.
func TestHybridMagnetTrackersRegisterBothShortInfohashes(t *testing.T) {
	cfg := TestingConfig(t)
	cfg.DisableTrackers = false
	cl, err := NewClient(cfg)
	qt.Assert(t, qt.IsNil(err))
	defer cl.Close()
	v2 := infohash_v2.FromHexString("deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef")
	tor, _ := cl.AddTorrentOpt(AddTorrentOpts{
		InfoHash:   testingTorrentInfoHash,
		InfoHashV2: g.Some(v2),
		Storage:    &storageClient{},
	})
	tor.AddTrackers([][]string{{"http://127.0.0.1:1/announce", "udp://127.0.0.1:1/announce"}})

	cl.lock()
	qt.Check(t, qt.Equals(cl.torrentsByShortHash[*v2.ToShort()], tor))
	for key := range tor.regularTrackerAnnounceState {
		qt.Check(t, qt.IsTrue(cl.regularTrackerAnnounceDispatcher.announceData.ContainsKey(key)), qt.Commentf("%v", key))
	}
	// What a DHT announce or completion does on unlock; panicked before.
	tor.deferUpdateRegularTrackerAnnouncing()
	cl.unlock()

	tor.Drop()
	cl.lock()
	_, stale := cl.torrentsByShortHash[*v2.ToShort()]
	cl.unlock()
	qt.Check(t, qt.IsFalse(stale))
}
