package main

import (
	"testing"
	"time"

	"github.com/anacrolix/torrent"
)

// A player seeking from 1:23 to 4:33 to 7:44 can leave a reader open at each
// earlier spot. Only the spot it reads now is fetched; the earlier ones stop.
func TestOnlyTheLatestSeekPositionIsFetched(t *testing.T) {
	s, rec, _, data := configuredLargeVideo(t, 64)
	rec.mu.Lock()
	rec.DirectPlay = true
	rec.mu.Unlock()
	const mib = int64(1 << 20)
	for _, piece := range []int{10, 30, 50} {
		writeVerifiedPiece(t, s, rec, data, piece)
	}
	open := func() *playbackReader {
		p, err := s.OpenPlayback(rec.ID, 0)
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(p.Close)
		return p
	}
	read := func(p *playbackReader, piece int64) {
		if n, err := p.ReadAt(make([]byte, 4), piece*mib); err != nil || n != 4 {
			t.Fatalf("read at piece %d: %d %v", piece, n, err)
		}
	}
	fetchedFrom := func() []int {
		var heads []int
		for _, w := range s.playbackWindows(rec.Torrent) {
			heads = append(heads, w.urgent.begin)
		}
		return heads
	}

	first, second, third := open(), open(), open()
	read(first, 10)
	read(second, 30)
	if heads := fetchedFrom(); len(heads) != 1 || heads[0] != 30 {
		t.Fatalf("after the seek to piece 30, fetching from %v", heads)
	}
	if got := rawPiecePriority(rec.Torrent.Piece(11)); got >= torrent.PiecePriorityNext {
		t.Fatalf("the earlier spot is still wanted at %v", got)
	}
	read(third, 50)
	if heads := fetchedFrom(); len(heads) != 1 || heads[0] != 50 {
		t.Fatalf("after the seek to piece 50, fetching from %v", heads)
	}
	for _, piece := range []int{11, 31} {
		if got := rawPiecePriority(rec.Torrent.Piece(piece)); got >= torrent.PiecePriorityNext {
			t.Fatalf("earlier spot piece %d is still wanted at %v", piece, got)
		}
	}

	// Closing an earlier reader does not hold its old spot for resume.
	second.Close()
	c := third.priorityOwner
	c.mu.Lock()
	held := len(c.resumeDemand)
	c.mu.Unlock()
	if held != 0 {
		t.Fatalf("closing the old reader held %d pieces of its spot", held)
	}

	// A displaced reader that reads again takes its window back.
	read(first, 10)
	if heads := fetchedFrom(); len(heads) != 1 || heads[0] != 10 {
		t.Fatalf("reading again at piece 10, fetching from %v", heads)
	}
}

// A read still waiting at the old spot must not bring it back: every block
// that arrived woke it and it re-published its window, so the old spots kept
// downloading. It ends, and the old piece is no longer wanted at all.
func TestBlockedReadAtAnEarlierSpotStopsAndStaysStopped(t *testing.T) {
	s, rec, _, data := configuredLargeVideo(t, 64)
	rec.mu.Lock()
	rec.DirectPlay = true
	rec.mu.Unlock()
	const mib = int64(1 << 20)
	writeVerifiedPiece(t, s, rec, data, 30)
	open := func() *playbackReader {
		p, err := s.OpenPlayback(rec.ID, 0)
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(p.Close)
		return p
	}
	fetchedFrom := func() []int {
		var heads []int
		for _, w := range s.playbackWindows(rec.Torrent) {
			heads = append(heads, w.urgent.begin)
		}
		return heads
	}
	old, current := open(), open()
	done := make(chan error, 1)
	go func() {
		_, err := old.ReadAt(make([]byte, 4), 20*mib) // piece 20 never arrives
		done <- err
	}()
	deadline := time.Now().Add(5 * time.Second)
	for {
		if heads := fetchedFrom(); len(heads) == 1 && heads[0] == 20 {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("old read never published piece 20: %v", fetchedFrom())
		}
		time.Sleep(5 * time.Millisecond)
	}
	if n, err := current.ReadAt(make([]byte, 4), 30*mib); err != nil || n != 4 {
		t.Fatalf("seek read: %d %v", n, err)
	}
	select {
	case err := <-done:
		if err == nil {
			t.Fatal("the abandoned read returned data it never received")
		}
	case <-time.After(5 * time.Second):
		t.Fatal("the read at the old spot kept waiting")
	}
	time.Sleep(50 * time.Millisecond)
	if heads := fetchedFrom(); len(heads) != 1 || heads[0] != 30 {
		t.Fatalf("fetching from %v, want only piece 30", heads)
	}
	if got := rawPiecePriority(rec.Torrent.Piece(20)); got >= torrent.PiecePriorityNext {
		t.Fatalf("old spot piece 20 still wanted at %v", got)
	}
	if got := rec.Torrent.Piece(20).State().Priority; got >= torrent.PiecePriorityNext {
		t.Fatalf("old spot piece 20 still wanted by the library reader at %v", got)
	}
}
