package main

import "time"

// Each torrent owns its samples. The client-wide rates belong only to /stats;
// copying them into every /torrent response duplicates speed and corrupts ETA.
type transferRates struct {
	lastSample time.Time
	downloaded int64
	uploaded   int64
	down       int64
	up         int64
}

func (r *transferRates) sample(now time.Time, downloaded, uploaded int64, paused bool) {
	elapsed := now.Sub(r.lastSample).Seconds()
	if r.lastSample.IsZero() || downloaded < r.downloaded || uploaded < r.uploaded {
		r.down, r.up = 0, 0
	} else if elapsed >= 0.5 {
		r.down = smoothTransferRate(r.down, float64(downloaded-r.downloaded)/elapsed)
		r.up = smoothTransferRate(r.up, float64(uploaded-r.uploaded)/elapsed)
	} else {
		return
	}
	if paused {
		r.down, r.up = 0, 0
	}
	r.lastSample, r.downloaded, r.uploaded = now, downloaded, uploaded
}

func smoothTransferRate(previous int64, current float64) int64 {
	if previous == 0 {
		return int64(current)
	}
	return int64(0.6*float64(previous) + 0.4*current)
}

func (s *EngineServer) sampleTorrentRates(now time.Time) {
	s.mu.RLock()
	records := make([]*TorrentRecord, 0, len(s.records))
	for _, rec := range s.records {
		records = append(records, rec)
	}
	s.mu.RUnlock()
	for _, rec := range records {
		rec.mu.Lock()
		stats := rec.Torrent.Stats()
		rec.rates.sample(now, stats.BytesReadData.Int64(), stats.BytesWrittenData.Int64(), rec.Paused || rec.removed)
		rec.mu.Unlock()
	}
}
