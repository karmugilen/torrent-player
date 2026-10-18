package main

import (
	"net"
	"net/http"
	"sort"
	"strings"

	"github.com/anacrolix/torrent"
)

// Connections are read only on demand by the visible details screen. Candidate
// addresses from DHT/trackers are deliberately excluded: they are not connected.
type PeerConnectionView struct {
	Address             string `json:"address"`
	Transport           string `json:"transport"`
	Client              string `json:"client"`
	Downloaded          int64  `json:"downloaded"`
	Uploaded            int64  `json:"uploaded"`
	Country             string `json:"country,omitempty"`
	Source              string `json:"source"`
	VerifiedBytes       int64  `json:"verifiedBytes"`
	UrgentVerifiedBytes int64  `json:"urgentVerifiedBytes"`
	MetadataChunks      int64  `json:"metadataChunks"`
	BadPieces           int64  `json:"badPieces"`
}

type ConnectionsResponse struct {
	ID     string               `json:"id"`
	Paused bool                 `json:"paused"`
	Peers  []PeerConnectionView `json:"peers"`
}

func peerConnectionView(peer *torrent.PeerConn) PeerConnectionView {
	transport := peer.Network
	switch {
	case strings.HasPrefix(transport, "tcp"):
		transport = "TCP"
	case strings.HasPrefix(transport, "udp"), strings.HasPrefix(transport, "utp"):
		transport = "uTP"
	case strings.Contains(strings.ToLower(transport), "webrtc"):
		transport = "WebRTC"
	}
	address := "Unknown address"
	if peer.RemoteAddr != nil {
		address = peer.RemoteAddr.String()
	}
	stats := peer.Stats()
	client, _ := peer.PeerClientName.Load().(string)
	return PeerConnectionView{
		Address: address, Transport: transport, Client: client,
		Downloaded: stats.BytesReadData.Int64(), Uploaded: stats.BytesWrittenData.Int64(),
		Source: string(peer.Discovery), VerifiedBytes: stats.BytesReadVerified.Int64(), UrgentVerifiedBytes: stats.BytesReadUrgentVerified.Int64(), MetadataChunks: stats.MetadataChunksVerified.Int64(), BadPieces: stats.PiecesDirtiedBad.Int64(),
	}
}

func (s *EngineServer) handleConnections(w http.ResponseWriter, r *http.Request, id string) {
	s.mu.RLock()
	rec := s.records[id]
	s.mu.RUnlock()
	if rec == nil {
		writeJSON(w, http.StatusNotFound, map[string]string{"error": "torrent not found"})
		return
	}
	rec.mu.RLock()
	defer rec.mu.RUnlock()
	view := ConnectionsResponse{ID: id, Paused: rec.Paused, Peers: make([]PeerConnectionView, 0)}
	if !rec.Paused {
		for _, peer := range rec.Torrent.PeerConns() {
			item := peerConnectionView(peer)
			item.Country = s.peerCountry(item.Address)
			view.Peers = append(view.Peers, item)
		}
	}
	sort.Slice(view.Peers, func(i, j int) bool {
		if view.Peers[i].Address == view.Peers[j].Address {
			return view.Peers[i].Transport < view.Peers[j].Transport
		}
		return view.Peers[i].Address < view.Peers[j].Address
	})
	writeJSON(w, http.StatusOK, view)
}

func (s *EngineServer) peerCountry(address string) string {
	host, _, err := net.SplitHostPort(address)
	if err != nil {
		return ""
	}
	ip := net.ParseIP(host)
	if ip == nil || !ip.IsGlobalUnicast() || ip.IsPrivate() {
		return ""
	}
	s.geoMu.RLock()
	defer s.geoMu.RUnlock()
	if s.geoIP == nil {
		return ""
	}
	var record struct {
		Country struct {
			Code string `maxminddb:"iso_code"`
		} `maxminddb:"country"`
	}
	if err := s.geoIP.Lookup(ip, &record); err != nil {
		return ""
	}
	return record.Country.Code
}
