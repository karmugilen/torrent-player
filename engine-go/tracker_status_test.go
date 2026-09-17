package main

import (
	"context"
	"encoding/json"
	"errors"
	"log/slog"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/anacrolix/torrent"
	"github.com/anacrolix/torrent/metainfo"
	"github.com/anacrolix/torrent/tracker"
)

func TestBuildTrackerViewsMarksOriginalAndStatus(t *testing.T) {
	rec := &TorrentRecord{
		OriginalTrackers: [][]string{
			{"udp://original.example.org:1337/announce"},
			{"https://backup.example.org/announce"},
		},
	}
	live := map[string]trackerLiveStatus{
		"udp://original.example.org:1337/announce": {Status: trackerStatusWorking, Message: "3 peers"},
		"https://extra.example.org/announce":       {Status: trackerStatusError, Message: "timeout"},
	}
	// Simulate metainfo that includes a supplemental tracker by using OriginalTrackers only
	// when Torrent is nil — then inject supplemental via a fake list through live keys.
	got := buildTrackerViews(rec, live)
	if len(got) != 2 {
		t.Fatalf("got %d trackers: %+v", len(got), got)
	}
	if !got[0].Original || got[0].Status != trackerStatusWorking || got[0].Message != "3 peers" {
		t.Fatalf("original tracker: %+v", got[0])
	}
	if !got[1].Original || got[1].Status != trackerStatusWaiting {
		t.Fatalf("backup tracker: %+v", got[1])
	}
}

func TestGetTorrentIncludesTrackers(t *testing.T) {
	s := testEngine(t)
	mi, err := metainfo.LoadFromFile("testdata/tiny.torrent")
	if err != nil {
		t.Fatal(err)
	}
	tr, err := s.client.AddTorrent(mi)
	if err != nil {
		t.Fatal(err)
	}
	rec := &TorrentRecord{
		ID:               "tiny",
		InfoHash:         tr.InfoHash().HexString(),
		Torrent:          tr,
		OriginalTrackers: [][]string{{"udp://tracker.opentrackr.org:1337/announce"}},
	}
	s.records[rec.ID] = rec
	s.setTrackerStatus(rec.InfoHash, "udp://tracker.opentrackr.org:1337/announce", trackerStatusWorking, "2 peers")

	w := httptest.NewRecorder()
	s.handleGetTorrent(w, httptest.NewRequest("GET", "/torrent/tiny", nil), rec.ID)
	var view TorrentView
	if err := json.Unmarshal(w.Body.Bytes(), &view); err != nil {
		t.Fatal(err)
	}
	if len(view.Trackers) == 0 {
		t.Fatalf("expected trackers in status: %s", w.Body.String())
	}
	tr0 := view.Trackers[0]
	if tr0.URL != "udp://tracker.opentrackr.org:1337/announce" {
		t.Fatalf("url: %+v", tr0)
	}
	if tr0.Status != trackerStatusWorking || tr0.Message != "2 peers" || !tr0.Original {
		t.Fatalf("tracker fields: %+v", tr0)
	}
}

func TestAnnounceSlogUpdatesTrackerStatus(t *testing.T) {
	s := &EngineServer{trackerStatus: make(map[string]map[string]trackerLiveStatus), eventCh: make(chan struct{}, 1)}
	h := &trackerStatusHandler{s: s, next: slog.DiscardHandler}
	ih := [20]byte{1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20}
	url := "udp://tracker.example.org:1337/announce"
	r := slog.NewRecord(time.Now(), slog.LevelDebug, "announced", 0)
	r.AddAttrs(
		slog.Any("url", url),
		slog.Any("short infohash", ih),
		slog.Any("err", error(nil)),
		slog.Any("resp", tracker.AnnounceResponse{Peers: make([]tracker.Peer, 4)}),
	)
	if err := h.Handle(context.Background(), r); err != nil {
		t.Fatal(err)
	}
	snap := s.trackerStatusSnapshot(infoHashAttr(ih))
	st := snap[url]
	if st.Status != trackerStatusWorking || st.Message != "4 peers" {
		t.Fatalf("status: %+v", st)
	}

	rErr := slog.NewRecord(time.Now(), slog.LevelError, "announced", 0)
	rErr.AddAttrs(
		slog.Any("url", url),
		slog.Any("short infohash", ih),
		slog.Any("err", context.Canceled),
	)
	_ = h.Handle(context.Background(), rErr)
	st = s.trackerStatusSnapshot(infoHashAttr(ih))[url]
	if st.Status != trackerStatusError || st.Message == "" {
		t.Fatalf("error status: %+v", st)
	}
}

func TestAnnounceSlogLoggerContextAndGroupsAreRetained(t *testing.T) {
	s := &EngineServer{trackerStatus: make(map[string]map[string]trackerLiveStatus)}
	logger := slog.New(&trackerStatusHandler{s: s, next: slog.DiscardHandler})
	ih1 := [20]byte{1}
	ih2 := [20]byte{2}
	url1 := "udp://one.example.org:1337/announce"
	url2 := "udp://two.example.org:1337/announce"

	// This mirrors the torrent library: URL and short infohash are bound with
	// Logger.With, the torrent identity is a bound group, and resp/err are on
	// the announce record itself. The second logger proves that context cannot
	// bleed between torrents.
	logger.With(
		"short infohash", ih1,
		"url", url1,
	).WithGroup("announce").With(
		slog.Group("torrent", slog.String("name", "one")),
	).LogAttrs(context.Background(), slog.LevelDebug, "announced",
		slog.Any("resp", tracker.AnnounceResponse{Peers: make([]tracker.Peer, 2)}),
		slog.Any("err", error(nil)),
	)
	logger.With(
		"short infohash", ih2,
		"url", url2,
	).WithGroup("announce").With(
		slog.Group("torrent", slog.String("name", "two")),
	).LogAttrs(context.Background(), slog.LevelDebug, "announced",
		slog.Any("resp", tracker.AnnounceResponse{}),
		slog.Any("err", errors.New("tracker unavailable")),
	)

	first := s.trackerStatusSnapshot(infoHashAttr(ih1))[url1]
	if first.Status != trackerStatusWorking || first.Message != "2 peers" {
		t.Fatalf("first torrent status: %+v", first)
	}
	second := s.trackerStatusSnapshot(infoHashAttr(ih2))[url2]
	if second.Status != trackerStatusError || second.Message != "tracker unavailable" {
		t.Fatalf("second torrent status: %+v", second)
	}
	if got := s.trackerStatusSnapshot(infoHashAttr(ih1))[url2]; got != (trackerLiveStatus{}) {
		t.Fatalf("second torrent leaked into first: %+v", got)
	}
	if got := s.trackerStatusSnapshot(""); got != nil {
		t.Fatalf("unexpected unassociated status: %+v", got)
	}
}

func TestTrackerStatusDoesNotCreateGlobalFallback(t *testing.T) {
	s := &EngineServer{trackerStatus: make(map[string]map[string]trackerLiveStatus)}
	s.setTrackerStatus("", "udp://tracker.example.org:1337/announce", trackerStatusWorking, "1 peer")
	if got := s.trackerStatusSnapshot(""); got != nil {
		t.Fatalf("unassociated tracker status: %+v", got)
	}
	if got := s.trackerStatusSnapshot("some-torrent"); got != nil {
		t.Fatalf("global status borrowed by torrent: %+v", got)
	}
}

func TestTrackerStatusNormalizesIdentityAndURL(t *testing.T) {
	s := &EngineServer{trackerStatus: make(map[string]map[string]trackerLiveStatus)}
	s.setTrackerStatus("  ABCDEF  ", "  https://tracker.example.org/announce  ", trackerStatusWorking, "0 peers")
	got := s.trackerStatusSnapshot("abcdef")
	if got["https://tracker.example.org/announce"] != (trackerLiveStatus{Status: trackerStatusWorking, Message: "0 peers"}) {
		t.Fatalf("normalized status: %+v", got)
	}
	if s.trackerStatusSnapshot(" ") != nil {
		t.Fatal("blank identity returned a status map")
	}
}

func TestTrackerStatusForwardsBoundAttributesToDownstreamHandler(t *testing.T) {
	var output strings.Builder
	next := slog.NewTextHandler(&output, &slog.HandlerOptions{Level: slog.LevelDebug})
	s := &EngineServer{trackerStatus: make(map[string]map[string]trackerLiveStatus)}
	logger := slog.New(&trackerStatusHandler{s: s, next: next})
	ih := [20]byte{7}
	logger.With("short infohash", ih, "url", "udp://tracker.example.org:1337/announce").
		WithGroup("announce").Log(context.Background(), slog.LevelDebug, "announced", "resp", tracker.AnnounceResponse{}, "err", error(nil))
	line := output.String()
	for _, want := range []string{"\"short infohash\"=", "url=udp://tracker.example.org:1337/announce", "announce.resp=", "announce.err="} {
		if !strings.Contains(line, want) {
			t.Fatalf("downstream output missing %q in %q", want, line)
		}
	}
}

func TestStatusUpdatedConnectionErrorUsesErrorWhenIdentityIsPresent(t *testing.T) {
	s := &EngineServer{trackerStatus: make(map[string]map[string]trackerLiveStatus)}
	s.onStatusUpdated(torrent.StatusUpdatedEvent{
		Event:    torrent.TrackerConnected,
		Url:      "wss://tracker.example.org",
		InfoHash: "abcdef",
		Error:    errors.New("connection failed"),
	})
	got := s.trackerStatusSnapshot("abcdef")["wss://tracker.example.org"]
	if got.Status != trackerStatusError || got.Message != "connection failed" {
		t.Fatalf("connection status: %+v", got)
	}
	// A connection event has no announce response and therefore must not claim
	// any peers. A clean connection is still only transport state.
	s.onStatusUpdated(torrent.StatusUpdatedEvent{
		Event:    torrent.TrackerConnected,
		Url:      "wss://tracker-clean.example.org",
		InfoHash: "abcdef",
	})
	got = s.trackerStatusSnapshot("abcdef")["wss://tracker-clean.example.org"]
	if got.Status != trackerStatusWorking || got.Message != "" {
		t.Fatalf("clean connection status: %+v", got)
	}
	s.onStatusUpdated(torrent.StatusUpdatedEvent{
		Event:    torrent.TrackerDisconnected,
		Url:      "wss://tracker.example.org",
		InfoHash: "abcdef",
		Error:    errors.New("connection failed"),
	})
	got = s.trackerStatusSnapshot("abcdef")["wss://tracker.example.org"]
	if got.Status != trackerStatusError || got.Message != "connection failed" {
		t.Fatalf("disconnect status: %+v", got)
	}
}

func TestBuildTrackerViewsMarksAttachedTrackerSupplemental(t *testing.T) {
	s := testEngine(t)
	mi, err := metainfo.LoadFromFile("testdata/tiny.torrent")
	if err != nil {
		t.Fatal(err)
	}
	mi.Announce = ""
	mi.AnnounceList = nil
	tr, err := s.client.AddTorrent(mi)
	if err != nil {
		t.Fatal(err)
	}
	tr.AddTrackers([][]string{{"https://supplement.example.org/announce"}})
	rec := &TorrentRecord{Torrent: tr}
	views := buildTrackerViews(rec, map[string]trackerLiveStatus{
		"https://supplement.example.org/announce": {Status: trackerStatusWorking, Message: "0 peers"},
	})
	if len(views) != 1 {
		t.Fatalf("got %d tracker views: %+v", len(views), views)
	}
	if views[0].Original {
		t.Fatalf("attached tracker incorrectly marked original: %+v", views[0])
	}
	if views[0].Status != trackerStatusWorking || views[0].Message != "0 peers" {
		t.Fatalf("tracker status: %+v", views[0])
	}
}

func TestBuildTrackerViewsAttachedSupplementAppearsOnce(t *testing.T) {
	s := testEngine(t)
	mi, err := metainfo.LoadFromFile("testdata/tiny.torrent")
	if err != nil {
		t.Fatal(err)
	}
	mi.Announce = ""
	mi.AnnounceList = [][]string{{"https://original.example.org/announce"}}
	tr, err := s.client.AddTorrent(mi)
	if err != nil {
		t.Fatal(err)
	}
	tr.AddTrackers([][]string{{"https://original.example.org/announce", "https://supplement.example.org/announce"}})
	rec := &TorrentRecord{
		Torrent:          tr,
		OriginalTrackers: [][]string{{"https://original.example.org/announce"}},
	}
	views := buildTrackerViews(rec, nil)
	if len(views) != 2 {
		t.Fatalf("got %d tracker views: %+v", len(views), views)
	}
	if !views[0].Original || views[1].Original {
		t.Fatalf("tracker origins: %+v", views)
	}
}

func TestStatusUpdatedCallback(t *testing.T) {
	s := &EngineServer{trackerStatus: make(map[string]map[string]trackerLiveStatus), eventCh: make(chan struct{}, 1)}
	s.onStatusUpdated(torrent.StatusUpdatedEvent{
		Event:    torrent.TrackerAnnounceSuccessful,
		Url:      "wss://tracker.example.org",
		InfoHash: "abcdef",
	})
	st := s.trackerStatusSnapshot("abcdef")["wss://tracker.example.org"]
	if st.Status != trackerStatusWorking {
		t.Fatalf("%+v", st)
	}
	s.onStatusUpdated(torrent.StatusUpdatedEvent{
		Event:    torrent.TrackerAnnounceError,
		Url:      "wss://tracker.example.org",
		InfoHash: "abcdef",
		Error:    errors.New("boom"),
	})
	st = s.trackerStatusSnapshot("abcdef")["wss://tracker.example.org"]
	if st.Status != trackerStatusError || st.Message != "boom" {
		t.Fatalf("%+v", st)
	}
}
