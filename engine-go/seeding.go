package main

import (
	"encoding/json"
	"os"
	"path/filepath"
)

type seedAccount struct {
	Uploaded   int64 `json:"uploaded"`
	Downloaded int64 `json:"downloaded"`
	Seconds    int64 `json:"seconds"`
}

func (s *EngineServer) loadSeedAccounts() {
	s.seedAccounts = make(map[string]seedAccount)
	b, err := os.ReadFile(filepath.Join(s.downloadDir, ".seed-accounts.json"))
	if err == nil {
		_ = json.Unmarshal(b, &s.seedAccounts)
	}
	if s.seedAccounts == nil {
		s.seedAccounts = make(map[string]seedAccount)
	}
}

func (s *EngineServer) seedAccount(hash string) seedAccount {
	s.seedMu.Lock()
	defer s.seedMu.Unlock()
	return s.seedAccounts[hash]
}

func seedTotals(rec *TorrentRecord) seedAccount {
	stats := rec.Torrent.Stats()
	return seedAccount{Uploaded: rec.seedBase.Uploaded + stats.BytesWrittenData.Int64(), Downloaded: rec.seedBase.Downloaded + stats.BytesReadData.Int64(), Seconds: rec.seedBase.Seconds}
}

// Caller owns rec.mu. Normal downloading keeps reciprocal uploads; completed
// selections seed only when enabled, permitted by Android policy and below caps.
func (s *EngineServer) applySeedPolicy(rec *TorrentRecord, active bool, tick bool) {
	was := rec.Seeding
	rec.Seeding = false
	if !active || rec.DirectPlay || rec.Torrent.Info() == nil || len(rec.Selected) == 0 {
		if was {
			s.notifyChange()
		}
		return
	}
	t := rec.Torrent
	files := t.Files()
	stats := t.Stats()
	verified := rec.verifiedFileBytesCached(t, stats.PiecesComplete)
	var size int64
	complete := true
	for _, i := range rec.Selected {
		if i < 0 || i >= len(files) {
			complete = false
			continue
		}
		size += files[i].Length()
		if verified[i] < files[i].Length() {
			complete = false
		}
	}
	if !complete || size <= 0 {
		t.AllowDataUpload()
		if was {
			s.notifyChange()
		}
		return
	}
	account := seedTotals(rec)
	denominator := max(size, account.Downloaded)
	ratio := float64(account.Uploaded) / float64(max(denominator, 1))
	allowed := s.seedingEnabled.Load() && s.seedingAllowed.Load() && ratio < float64(s.seedRatioMilli.Load())/1000 && account.Seconds < s.seedTimeMinutes.Load()*60
	if allowed {
		t.AllowDataUpload()
		rec.Seeding = true
		if tick {
			rec.seedBase.Seconds++
		}
	} else {
		t.DisallowDataUpload()
	}
	if was != rec.Seeding {
		s.notifyChange()
	}
}

func (s *EngineServer) seedTick() {
	s.mu.RLock()
	records := make([]*TorrentRecord, 0, len(s.records))
	for _, r := range s.records {
		records = append(records, r)
	}
	s.mu.RUnlock()
	for _, rec := range records {
		rec.mu.Lock()
		active := s.payloadTransfersAllowed.Load() && rec.Configured && !rec.Paused && !rec.Checking && !rec.removed && rec.Error == nil
		s.applySeedPolicy(rec, active, true)
		account := seedTotals(rec)
		s.seedMu.Lock()
		s.seedAccounts[rec.InfoHash] = account
		s.seedMu.Unlock()
		rec.mu.Unlock()
	}
}

func (s *EngineServer) saveSeedAccounts() {
	s.seedSaveMu.Lock()
	defer s.seedSaveMu.Unlock()
	s.seedMu.Lock()
	b, err := json.Marshal(s.seedAccounts)
	s.seedMu.Unlock()
	if err != nil {
		return
	}
	path := filepath.Join(s.downloadDir, ".seed-accounts.json")
	if os.WriteFile(path+".tmp", b, 0600) == nil {
		_ = os.Rename(path+".tmp", path)
	}
}
