package main

import (
	"encoding/json"
	"testing"
	"time"
)

func TestTorrentRatesAndETAUseTheirOwnSamples(t *testing.T) {
	s := testEngine(t)
	start := time.Unix(100, 0)
	var ids []string
	for _, hash := range []string{"0123456789012345678901234567890123456789", "abcdefabcdefabcdefabcdefabcdefabcdefabcd"} {
		code, body := s.DispatchControl("POST", "/add", `{"torrentId":"magnet:?xt=urn:btih:`+hash+`","prepare":true}`)
		if code != 200 {
			t.Fatalf("add: %d %s", code, body)
		}
		var added AddResponse
		if err := json.Unmarshal([]byte(body), &added); err != nil {
			t.Fatal(err)
		}
		ids = append(ids, added.ID)
	}
	for i, id := range ids {
		rec := s.records[id]
		rec.mu.Lock()
		rec.rates = transferRates{lastSample: start}
		rec.rates.sample(start.Add(time.Second), int64(i*1000), int64(i*100), false)
		rec.mu.Unlock()
	}
	// Global traffic must never appear on the first, idle torrent.
	s.downloadSpeed, s.uploadSpeed = 1000, 100
	for i, id := range ids {
		code, body := s.DispatchControl("GET", "/torrent/"+id, "")
		var view TorrentView
		if code != 200 || json.Unmarshal([]byte(body), &view) != nil {
			t.Fatalf("status: %d %s", code, body)
		}
		if view.DownloadSpeed != int64(i*1000) || view.UploadSpeed != int64(i*100) {
			t.Fatalf("torrent %d copied a different torrent's rate: %+v", i, view)
		}
	}
	before := s.statusFingerprint()
	rec := s.records[ids[1]]
	rec.mu.Lock()
	rec.rates.sample(start.Add(2*time.Second), 1000, 100, false)
	rec.mu.Unlock()
	if s.statusFingerprint() == before {
		t.Fatal("per-torrent rate decay must wake Android even if the combined rate is unchanged")
	}
	code, body := s.DispatchControl("POST", "/pause", `{"id":"`+ids[1]+`"}`)
	if code != 200 {
		t.Fatalf("pause: %d %s", code, body)
	}
	_, body = s.DispatchControl("GET", "/torrent/"+ids[1], "")
	var paused TorrentView
	if err := json.Unmarshal([]byte(body), &paused); err != nil {
		t.Fatal(err)
	}
	if paused.DownloadSpeed != 0 || paused.UploadSpeed != 0 || paused.TimeRemaining != nil {
		t.Fatalf("paused torrent retained live rate/ETA: %+v", paused)
	}
}

func TestTransferRatesHandleIrregularSamplesPauseAndCounterReset(t *testing.T) {
	start := time.Unix(100, 0)
	var rates transferRates
	rates.sample(start, 1000, 100, false)
	rates.sample(start.Add(2*time.Second), 5000, 300, false)
	if rates.down != 2000 || rates.up != 100 {
		t.Fatalf("rate must use elapsed time, got %+v", rates)
	}
	rates.sample(start.Add(2100*time.Millisecond), 5100, 310, false)
	if rates.downloaded != 5000 || rates.down != 2000 {
		t.Fatal("sub-interval read consumed a sample")
	}
	rates.sample(start.Add(3*time.Second), 5100, 310, true)
	if rates.down != 0 || rates.up != 0 {
		t.Fatal("pause retained rate")
	}
	rates.sample(start.Add(4*time.Second), 6100, 410, false)
	if rates.down != 1000 || rates.up != 100 {
		t.Fatal("resume used pre-pause history")
	}
	rates.sample(start.Add(5*time.Second), 0, 0, false)
	if rates.down != 0 || rates.up != 0 {
		t.Fatal("reset produced a negative rate")
	}
}
