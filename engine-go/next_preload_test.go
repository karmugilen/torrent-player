package main

import (
	"bytes"
	"encoding/json"
	"testing"
	"time"
)

func playWithNext(t *testing.T, s *EngineServer, id string, file int, next *int) {
	t.Helper()
	request, _ := json.Marshal(PlayRequest{ID: id, FileIndex: &file, NextFileIndex: next})
	if code, response := s.DispatchControl("POST", "/play", string(request)); code != 200 {
		t.Fatalf("play: %d %s", code, response)
	}
}

func TestDirectPlayPreloadsNextEpisodeNearTheEnd(t *testing.T) {
	if testing.Short() {
		t.Skip("local peer transfer")
	}
	const episode = int64(8 << 20)
	data, newPeer := localFilesSwarm(t, "Show", []int64{episode, episode})
	seed, _ := newPeer(true)
	leech, rec := newPeer(false)
	if rec.Torrent.AddClientPeer(seed.client) == 0 {
		t.Fatal("no seed address")
	}
	read := func(p *playbackReader, base, offset int64) {
		t.Helper()
		got := make([]byte, 256<<10)
		done := make(chan error, 1)
		go func() {
			_, err := p.ReadAt(got, offset)
			done <- err
		}()
		select {
		case err := <-done:
			if err != nil {
				t.Fatal(err)
			}
		case <-time.After(20 * time.Second):
			t.Fatalf("read at %d stalled", offset)
		}
		if !bytes.Equal(got, data[base+offset:base+offset+int64(len(got))]) {
			t.Fatalf("incorrect bytes at %d", offset)
		}
	}
	preload := func() *NextPreloadView { return idleView(t, leech, rec.ID).NextPreload }

	// Without Play next nothing is planned; naming the same file is ignored.
	playWithNext(t, leech, rec.ID, 0, nil)
	if np := preload(); np != nil {
		t.Fatalf("preload without Play next: %+v", np)
	}
	zero, one := 0, 1
	playWithNext(t, leech, rec.ID, 0, &zero)
	if np := preload(); np != nil {
		t.Fatalf("preload of the playing file: %+v", np)
	}
	playWithNext(t, leech, rec.ID, 0, &one)
	if np := preload(); np == nil || np.FileIndex != 1 || np.Active {
		t.Fatalf("planned preload: %+v", np)
	}

	first, err := leech.OpenPlayback(rec.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	// Player probes of the head and container tail are not the end of the episode.
	read(first, 0, 0)
	read(first, 0, episode-(256<<10))
	if np := preload(); np.Active {
		t.Fatal("tail probe started the preload")
	}
	read(first, 0, 3<<20)
	if np := preload(); np.Active {
		t.Fatal("preload started mid-episode")
	}
	// A jump to the end is a scrub; continued playback there is the end.
	near := episode*9/10 + (64 << 10)
	read(first, 0, near)
	time.Sleep(playbackScrubDebounce + 50*time.Millisecond)
	read(first, 0, near+(256<<10))
	deadline := time.Now().Add(20 * time.Second)
	for np := preload(); !np.Ready; np = preload() {
		if time.Now().After(deadline) {
			t.Fatalf("next episode never warmed: %+v", np)
		}
		time.Sleep(50 * time.Millisecond)
	}

	// The warmed head and tail are durable startup pieces.
	st := leech.storage.GetStorage(rec.InfoHash)
	files := rec.Torrent.Files()
	next := files[1]
	headBegin, headEnd := filePieceRange(next, 0, directStartupHead)
	tailBegin, tailEnd := filePieceRange(next, next.Length()-directStartupTail, next.Length())
	waitSaved := time.Now().Add(10 * time.Second)
	for {
		st.mu.Lock()
		saved := true
		for p := headBegin; p < headEnd; p++ {
			saved = saved && st.startupSaved[p]
		}
		for p := tailBegin; p < tailEnd; p++ {
			saved = saved && st.startupSaved[p]
		}
		st.mu.Unlock()
		if saved {
			break
		}
		if time.Now().After(waitSaved) {
			t.Fatal("next episode pieces were not saved to the startup cache")
		}
		time.Sleep(20 * time.Millisecond)
	}
	// The body of the next episode is left for its own playback.
	if mid := files[1].BeginPieceIndex() + int((4<<20)/rec.Torrent.Info().PieceLength); rec.Torrent.Piece(mid).State().Complete {
		t.Fatal("preload fetched beyond the head and tail")
	}
	first.Close()

	// Play next starts from the warm cache even with every peer gone.
	rec.Torrent.SetMaxEstablishedConns(0)
	waitConns(t, rec, func(n int) bool { return n == 0 })
	playWithNext(t, leech, rec.ID, 1, nil)
	if np := preload(); np != nil {
		t.Fatalf("preload survived the next play: %+v", np)
	}
	second, err := leech.OpenPlayback(rec.ID, 1)
	if err != nil {
		t.Fatal(err)
	}
	defer second.Close()
	read(second, episode, 0)
	read(second, episode, next.Length()-(256<<10))
}
