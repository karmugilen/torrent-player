package torrent

import "strings"

// WebtorPipelineDiagnostics is a consistent snapshot under the client lock.
type WebtorPipelineDiagnostics struct {
	PeerChoking        bool    `json:"peerChoking"`
	AmInterested       bool    `json:"amInterested"`
	PendingRequests    int     `json:"pendingRequests"`
	DownloadRate       float64 `json:"downloadRate"`
	RTTMs              float64 `json:"rttMs"`
	PipelineLimit      int     `json:"pipelineLimit"`
	NominalMaxRequests int     `json:"nominalMaxRequests"`
	PeerMaxRequests    int     `json:"peerMaxRequests"`
	PeakRequests       int     `json:"peakRequests"`
}

func (c *PeerConn) WebtorPipelineDiagnostics() WebtorPipelineDiagnostics {
	c.t.cl.lock()
	defer c.t.cl.unlock()
	return WebtorPipelineDiagnostics{
		c.peerChoking, c.requestState.Interested, int(c.requestState.Requests.GetCardinality()),
		c.downloadRate(), float64(c.webtorRTT.estimate) / 1e6, int(c.webtorPipelineLimit),
		int(c.nominalMaxRequests()), int(c.PeerMaxRequests), int(c.peakRequests),
	}
}

// WebtorSwarmDiagnostics combines live gauges with cumulative per-torrent counters.
type WebtorSwarmDiagnostics struct {
	TransportFailures map[string]uint64 `json:"transportFailures"`
	PoolSize          int               `json:"poolSize"`
	Dialing           int               `json:"dialing"`
	Established       int               `json:"established"`
	DialOK            uint64            `json:"dialOk"`
	DialFailures      map[string]uint64 `json:"dialFailures"`
	Closes            map[string]uint64 `json:"closes"`
	PexReceived       uint64            `json:"pexReceived"`
	PexDialed         uint64            `json:"pexDialed"`
}

func webtorFailureKind(err error) string {
	if err == nil {
		return "eof"
	}
	s := strings.ToLower(err.Error())
	switch {
	case strings.Contains(s, "refused"):
		return "refused"
	case strings.Contains(s, "timeout"), strings.Contains(s, "timed out"), strings.Contains(s, "deadline"):
		return "timeout"
	case strings.Contains(s, "mse"), strings.Contains(s, "obfuscation"):
		return "mse"
	case strings.Contains(s, "handshake"):
		return "handshake"
	case strings.Contains(s, "eof"):
		return "eof"
	case strings.Contains(s, "decode"):
		return "decode"
	case strings.Contains(s, "choker"):
		return "choker"
	case strings.Contains(s, "reset"):
		return "reset"
	default:
		return "other"
	}
}

func webtorCountReason(counts *map[string]uint64, err error) {
	if *counts == nil {
		*counts = make(map[string]uint64)
	}
	(*counts)[webtorFailureKind(err)]++
}

func (t *Torrent) WebtorSwarmDiagnostics() WebtorSwarmDiagnostics {
	t.cl.lock()
	defer t.cl.unlock()
	s := t.webtorDiagnostics
	s.PoolSize, s.Dialing, s.Established = t.peers.Len(), len(t.halfOpen), len(t.conns)
	// The response is serialized after unlocking; copy maps to avoid races.
	s.DialFailures = make(map[string]uint64, len(t.webtorDiagnostics.DialFailures))
	for k, v := range t.webtorDiagnostics.DialFailures {
		s.DialFailures[k] = v
	}
	s.TransportFailures = make(map[string]uint64, len(t.webtorDiagnostics.TransportFailures))
	for k, v := range t.webtorDiagnostics.TransportFailures {
		s.TransportFailures[k] = v
	}
	s.Closes = make(map[string]uint64, len(t.webtorDiagnostics.Closes))
	for k, v := range t.webtorDiagnostics.Closes {
		s.Closes[k] = v
	}
	return s
}
