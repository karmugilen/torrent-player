package main

import (
	"context"
	"encoding/hex"
	"fmt"
	"log/slog"
	"reflect"
	"strings"

	"github.com/anacrolix/torrent"
	"github.com/anacrolix/torrent/tracker"
)

const (
	trackerStatusWaiting = "waiting"
	trackerStatusWorking = "working"
	trackerStatusError   = "error"
)

type trackerLiveStatus struct {
	Status  string
	Message string
}

// trackerStatusHandler captures regular HTTP/UDP announce results from the
// torrent client's slog output. Websocket trackers report via StatusUpdated.
type trackerStatusHandler struct {
	s    *EngineServer
	next slog.Handler

	// slog keeps Logger.With attributes in the Handler returned by WithAttrs;
	// they are deliberately not copied into the Record passed to Handle. The
	// torrent logger binds the announce URL and short infohash this way, so we
	// retain a private, immutable copy of that context for diagnostics.
	bound  []trackerBoundAttr
	groups []string
}

type trackerBoundAttr struct {
	attr   slog.Attr
	groups []string
}

func (h *trackerStatusHandler) Enabled(ctx context.Context, level slog.Level) bool {
	return level >= slog.LevelDebug
}

func (h *trackerStatusHandler) Handle(ctx context.Context, r slog.Record) error {
	if r.Message == "announced" {
		h.s.noteAnnounceRecord(h.announceAttrs(r))
	}
	if h.next != nil && h.next.Enabled(ctx, r.Level) {
		return h.next.Handle(ctx, r)
	}
	return nil
}

func (h *trackerStatusHandler) WithAttrs(attrs []slog.Attr) slog.Handler {
	next := h.next
	if next != nil {
		next = next.WithAttrs(attrs)
	}
	bound := make([]trackerBoundAttr, len(h.bound), len(h.bound)+len(attrs))
	copy(bound, h.bound)
	for _, attr := range attrs {
		bound = append(bound, trackerBoundAttr{
			attr:   attr,
			groups: append([]string(nil), h.groups...),
		})
	}
	return &trackerStatusHandler{
		s:      h.s,
		next:   next,
		bound:  bound,
		groups: append([]string(nil), h.groups...),
	}
}

func (h *trackerStatusHandler) WithGroup(name string) slog.Handler {
	if name == "" {
		return h
	}
	next := h.next
	if next != nil {
		next = next.WithGroup(name)
	}
	groups := append(append([]string(nil), h.groups...), name)
	return &trackerStatusHandler{
		s:      h.s,
		next:   next,
		bound:  append([]trackerBoundAttr(nil), h.bound...),
		groups: groups,
	}
}

// announceAttrs resolves the logger context and record attributes into the
// small set of fields used by tracker diagnostics. Bound attrs retain the
// groups that were active when they were added; record attrs use the groups
// active on this handler. A leaf-key match is intentional: the torrent
// library's real logger places its identity in a "torrent" group while the
// announce fields are bound at the outer level.
func (h *trackerStatusHandler) announceAttrs(r slog.Record) []slog.Attr {
	attrs := make([]slog.Attr, 0, len(h.bound)+r.NumAttrs())
	for _, bound := range h.bound {
		attrs = appendResolvedTrackerAttrs(attrs, bound.attr, bound.groups)
	}
	r.Attrs(func(attr slog.Attr) bool {
		attrs = appendResolvedTrackerAttrs(attrs, attr, h.groups)
		return true
	})
	return attrs
}

func appendResolvedTrackerAttrs(dst []slog.Attr, attr slog.Attr, groups []string) []slog.Attr {
	attr.Value = attr.Value.Resolve()
	if attr.Equal(slog.Attr{}) {
		return dst
	}
	if attr.Value.Kind() == slog.KindGroup {
		group := attr.Key
		nextGroups := groups
		if group != "" {
			nextGroups = append(append([]string(nil), groups...), group)
		}
		for _, child := range attr.Value.Group() {
			dst = appendResolvedTrackerAttrs(dst, child, nextGroups)
		}
		return dst
	}
	// Keep the group-qualified key for downstream callers while allowing the
	// parser to identify fields inside the library's torrent group.
	key := attr.Key
	if len(groups) > 0 {
		key = strings.Join(append(append([]string(nil), groups...), attr.Key), ".")
	}
	return append(dst, slog.Attr{Key: key, Value: attr.Value})
}

func (s *EngineServer) noteAnnounceRecord(attrs []slog.Attr) {
	var url, infoHash string
	var announceErr error
	var peers int
	var havePeers bool
	for _, a := range attrs {
		// Group-qualified keys are retained for downstream behavior, but this
		// parser consumes the leaf name so real torrent.slogGroup attrs do not
		// hide the announce fields.
		key := a.Key
		if i := strings.LastIndexByte(key, '.'); i >= 0 {
			key = key[i+1:]
		}
		switch key {
		case "url":
			url = fmt.Sprint(a.Value.Any())
		case "short infohash":
			infoHash = infoHashAttr(a.Value.Any())
		case "err":
			if v := a.Value.Any(); v != nil {
				if err, ok := v.(error); ok {
					announceErr = err
				} else if s := fmt.Sprint(v); s != "" && s != "<nil>" {
					announceErr = fmt.Errorf("%s", s)
				}
			}
		case "resp":
			peers, havePeers = peerCountAttr(a.Value.Any())
		}
	}
	if url == "" || infoHash == "" {
		return
	}
	if announceErr != nil {
		s.setTrackerStatus(infoHash, url, trackerStatusError, announceErr.Error())
		return
	}
	msg := ""
	if havePeers {
		msg = formatPeerCount(peers)
	}
	s.setTrackerStatus(infoHash, url, trackerStatusWorking, msg)
}

func infoHashAttr(v any) string {
	switch x := v.(type) {
	case string:
		return strings.ToLower(x)
	case [20]byte:
		return hex.EncodeToString(x[:])
	case []byte:
		if len(x) == 20 {
			return hex.EncodeToString(x)
		}
	}
	// slog may wrap arrays; try reflect for [20]uint8
	rv := reflect.ValueOf(v)
	if rv.Kind() == reflect.Array && rv.Len() == 20 && rv.Type().Elem().Kind() == reflect.Uint8 {
		b := make([]byte, 20)
		for i := 0; i < 20; i++ {
			b[i] = byte(rv.Index(i).Uint())
		}
		return hex.EncodeToString(b)
	}
	return strings.ToLower(fmt.Sprint(v))
}

func peerCountAttr(v any) (int, bool) {
	if v == nil {
		return 0, false
	}
	switch x := v.(type) {
	case tracker.AnnounceResponse:
		return len(x.Peers), true
	case *tracker.AnnounceResponse:
		if x == nil {
			return 0, false
		}
		return len(x.Peers), true
	}
	rv := reflect.ValueOf(v)
	if rv.Kind() == reflect.Pointer {
		if rv.IsNil() {
			return 0, false
		}
		rv = rv.Elem()
	}
	if rv.Kind() == reflect.Struct {
		if f := rv.FieldByName("Peers"); f.IsValid() && f.Kind() == reflect.Slice {
			return f.Len(), true
		}
	}
	return 0, false
}

func formatPeerCount(n int) string {
	if n == 1 {
		return "1 peer"
	}
	return fmt.Sprintf("%d peers", n)
}

func (s *EngineServer) setTrackerStatus(infoHash, url, status, message string) {
	url = strings.TrimSpace(url)
	infoHash = strings.ToLower(strings.TrimSpace(infoHash))
	if s == nil || url == "" || infoHash == "" {
		return
	}
	next := trackerLiveStatus{Status: status, Message: message}
	s.trackerStatusMu.Lock()
	if s.trackerStatus == nil {
		s.trackerStatus = make(map[string]map[string]trackerLiveStatus)
	}
	byURL := s.trackerStatus[infoHash]
	if byURL == nil {
		byURL = make(map[string]trackerLiveStatus)
		s.trackerStatus[infoHash] = byURL
	}
	changed := byURL[url] != next
	if changed {
		byURL[url] = next
	}
	s.trackerStatusMu.Unlock()
	if changed {
		s.notifyChange()
	}
}

func (s *EngineServer) clearTrackerStatus(infoHash string) {
	if s == nil {
		return
	}
	infoHash = strings.ToLower(strings.TrimSpace(infoHash))
	if infoHash == "" {
		return
	}
	s.trackerStatusMu.Lock()
	delete(s.trackerStatus, infoHash)
	s.trackerStatusMu.Unlock()
}

func (s *EngineServer) trackerStatusSnapshot(infoHash string) map[string]trackerLiveStatus {
	s.trackerStatusMu.RLock()
	defer s.trackerStatusMu.RUnlock()
	infoHash = strings.ToLower(strings.TrimSpace(infoHash))
	if infoHash == "" {
		return nil
	}
	src := s.trackerStatus[infoHash]
	if len(src) == 0 {
		return nil
	}
	out := make(map[string]trackerLiveStatus, len(src))
	for k, v := range src {
		out[k] = v
	}
	return out
}

func (s *EngineServer) onStatusUpdated(e torrent.StatusUpdatedEvent) {
	switch e.Event {
	case torrent.TrackerAnnounceSuccessful:
		s.setTrackerStatus(e.InfoHash, e.Url, trackerStatusWorking, "")
	case torrent.TrackerConnected:
		if e.Error != nil {
			s.setTrackerStatus(e.InfoHash, e.Url, trackerStatusError, e.Error.Error())
		} else {
			// This describes tracker transport state only. It does not include a
			// peer count; a successful announce is recorded separately below.
			s.setTrackerStatus(e.InfoHash, e.Url, trackerStatusWorking, "")
		}
	case torrent.TrackerAnnounceError, torrent.TrackerDisconnected:
		msg := ""
		if e.Error != nil {
			msg = e.Error.Error()
		}
		s.setTrackerStatus(e.InfoHash, e.Url, trackerStatusError, msg)
	}
}

func buildTrackerViews(rec *TorrentRecord, live map[string]trackerLiveStatus) []TrackerView {
	if rec == nil {
		return nil
	}
	original := make(map[string]struct{})
	for _, tier := range rec.OriginalTrackers {
		for _, u := range tier {
			u = strings.TrimSpace(u)
			if u != "" {
				original[u] = struct{}{}
			}
		}
	}

	var tiers [][]string
	if rec.Torrent != nil {
		mi := rec.Torrent.Metainfo()
		tiers = mi.UpvertedAnnounceList()
	}
	if len(tiers) == 0 {
		tiers = rec.OriginalTrackers
	}

	seen := make(map[string]struct{})
	out := make([]TrackerView, 0, 16)
	for tierIdx, tier := range tiers {
		for _, raw := range tier {
			u := strings.TrimSpace(raw)
			if u == "" {
				continue
			}
			if _, dup := seen[u]; dup {
				continue
			}
			seen[u] = struct{}{}
			_, isOriginal := original[u]
			view := TrackerView{
				URL:      u,
				Tier:     tierIdx,
				Status:   trackerStatusWaiting,
				Original: isOriginal,
			}
			if st, ok := live[u]; ok {
				view.Status = st.Status
				view.Message = st.Message
			}
			out = append(out, view)
		}
	}
	return out
}
