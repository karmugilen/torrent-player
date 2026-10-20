package main

import (
	"encoding/json"
	"testing"
	"time"
)

func TestDirectPlayPieceGridTracksRAMReplacement(t *testing.T) {
	for _, warmStartup := range []bool{false, true} {
		name := "body"
		if warmStartup {
			name = "startup saved on disk"
		}
		t.Run(name, func(t *testing.T) {
			s, rec, data := playbackFixtureMode(t, true)
			st := s.storage.GetStorage(rec.InfoHash)
			st.setCacheLimit(4 * st.chunkLength)
			st.mu.Lock()
			st.startupPieces = map[int]bool{0: warmStartup}
			// Delay the torrent client's asynchronous completion refresh. The
			// grid must still describe the RAM snapshot accurately right now.
			release := make(chan struct{})
			st.onStreamEvict = func(int) { <-release }
			st.mu.Unlock()
			defer close(release)
			for index := 0; index < 4; index++ {
				writeVerifiedPiece(t, s, rec, data, index)
			}
			if warmStartup {
				deadline := time.Now().Add(time.Second)
				for {
					st.mu.Lock()
					saved := st.startupSaved[0]
					st.mu.Unlock()
					if saved {
						break
					}
					if time.Now().After(deadline) {
						t.Fatal("startup piece was not saved")
					}
					time.Sleep(time.Millisecond)
				}
			}
			buckets := func() []PieceBucket {
				code, body := s.DispatchControl("GET", "/pieces/"+rec.ID+"?maxBuckets=256", "")
				if code != 200 {
					t.Fatalf("pieces: %d %s", code, body)
				}
				var view PieceTelemetryResponse
				if err := json.Unmarshal(body, &view); err != nil {
					t.Fatal(err)
				}
				return view.Buckets
			}
			if got := buckets()[0].SelectedVerified; got != 1 {
				t.Fatalf("resident verified piece is unfilled: %d", got)
			}
			before := s.statusFingerprint()
			putDirectPiece(t, st, data, 4)
			if s.statusFingerprint() == before {
				t.Fatal("RAM replacement must wake grid observers even with unchanged torrent completion")
			}
			st.mu.Lock()
			_, oldResident := st.cache[0]
			used, limit := st.cacheBytes, st.maxCacheBytes
			st.mu.Unlock()
			if oldResident || used > limit {
				t.Fatalf("cache did not replace old piece within budget: resident=%v %d/%d", oldResident, used, limit)
			}
			if !rec.Torrent.Piece(0).State().Complete {
				t.Fatal("test requires stale torrent completion for the evicted piece")
			}
			if warmStartup && !st.PieceComplete(0) {
				t.Fatal("disk startup copy must remain available after RAM eviction")
			}
			next := buckets()
			if next[0].Verified != 0 || next[0].Receiving != 0 {
				t.Fatalf("evicted piece still filled: %+v", next[0])
			}
			if next[4].SelectedVerified != 1 {
				t.Fatalf("new resident piece is not filled: %+v", next[4])
			}
		})
	}
}

func TestStreamPieceGridPartialReplacementAndClose(t *testing.T) {
	st, data := directStorageFixture(t, t.TempDir(), 8, 8)
	st.startupPieces = map[int]bool{}
	for index := 0; index < 4; index++ {
		putDirectPiece(t, st, data, index)
	}
	snapshot := func() []pieceRun {
		runs, err := st.streamPieceRuns(8)
		if err != nil {
			t.Fatal(err)
		}
		return runs
	}
	buckets := func(runs []pieceRun) []PieceBucket {
		view, err := aggregatePieceBuckets(runs, []pieceRange{{start: 0, end: 8}}, 8, 8)
		if err != nil {
			t.Fatal(err)
		}
		return view
	}
	old := snapshot()
	piece := &documentPiece{storage: st, piece: st.info.Piece(4)}
	if _, err := piece.WriteAt(data[32:36], 0); err != nil {
		t.Fatal(err)
	}
	partial := buckets(snapshot())
	if partial[0].Verified != 0 || partial[0].Receiving != 0 || partial[4].SelectedReceiving != 1 {
		t.Fatalf("partial replacement not reflected: %+v", partial)
	}
	// An already-running aggregation must keep its immutable earlier snapshot.
	if previous := buckets(old); previous[0].SelectedVerified != 1 || previous[4].Receiving != 0 {
		t.Fatalf("later writes mutated an earlier grid snapshot: %+v", previous)
	}
	if _, err := piece.WriteAt(data[36:40], 4); err != nil {
		t.Fatal(err)
	}
	if err := piece.MarkComplete(); err != nil {
		t.Fatal(err)
	}
	if filled := buckets(snapshot())[4]; filled.SelectedVerified != 1 || filled.Receiving != 0 {
		t.Fatalf("verified replacement not filled: %+v", filled)
	}
	if err := st.Close(); err != nil {
		t.Fatal(err)
	}
	for _, empty := range buckets(snapshot()) {
		if empty.Verified != 0 || empty.Receiving != 0 {
			t.Fatalf("closed RAM buffer still filled: %+v", empty)
		}
	}
}

func BenchmarkStreamPieceGridSnapshot(b *testing.B) {
	for _, changed := range []bool{false, true} {
		name := "unchanged"
		if changed {
			name = "cache changed"
		}
		b.Run(name, func(b *testing.B) {
			st := &DocumentTorrentStorage{
				cache: make(map[int][]byte), completed: make(map[int]bool),
			}
			for index := 1000; index < 1128; index++ {
				st.cache[index] = []byte{1}
				st.completed[index] = true
			}
			b.ReportAllocs()
			b.ResetTimer()
			for i := 0; i < b.N; i++ {
				if changed {
					st.mu.Lock()
					st.completed[1000] = !st.completed[1000]
					st.streamRevision++
					st.mu.Unlock()
				}
				if _, err := st.streamPieceRuns(200_000); err != nil {
					b.Fatal(err)
				}
			}
		})
	}
}
