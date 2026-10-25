package main

import (
	"os"
	"path/filepath"
	"time"
)

// The block arrival model (patches/torrent/webtor_arrival.go) are saved so
// a restart does not start it over. Android often stops the app without
// Close, so they are also saved once a minute while it learns.

const arrivalSaveEvery = time.Minute

func (s *EngineServer) arrivalModelPath() string {
	return filepath.Join(s.downloadDir, ".cache", "arrival-model.json")
}

func (s *EngineServer) arrivalStats() ArrivalStats {
	if s.client == nil {
		return ArrivalStats{}
	}
	a := s.client.WebtorArrivalStats()
	return ArrivalStats{
		Samples: a.Samples, NetworkErrMs: a.NetworkErrMs, Using: a.Using,
	}
}

func (s *EngineServer) loadArrivalModel() {
	if s.client == nil {
		return
	}
	if b, err := os.ReadFile(s.arrivalModelPath()); err == nil {
		// A file from an older model shape is ignored; learning starts over.
		_ = s.client.SetWebtorArrivalState(b)
	}
}

func (s *EngineServer) saveArrivalModel() {
	if s.client == nil {
		return
	}
	b, err := s.client.WebtorArrivalState()
	if err != nil || len(b) == 0 {
		return
	}
	path := s.arrivalModelPath()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return
	}
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, b, 0o644); err != nil {
		return
	}
	_ = os.Rename(tmp, path)
}

// arrivalSaveLoop saves the models once a minute when they learned more.
func (s *EngineServer) arrivalSaveLoop() {
	ticker := time.NewTicker(arrivalSaveEvery)
	defer ticker.Stop()
	var saved int64
	for {
		select {
		case <-s.done:
			return
		case <-ticker.C:
		}
		if n := s.client.WebtorArrivalStats().Samples; n != saved {
			s.saveArrivalModel()
			saved = n
		}
	}
}
