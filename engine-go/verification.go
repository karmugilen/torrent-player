package main

import (
	"context"
	"fmt"
	"sort"
	"time"

	"github.com/anacrolix/torrent"
)

type savedDataCheck struct {
	cancel context.CancelFunc
	done   chan struct{}
	wake   chan struct{}
}

// Called with rec.mu held. Configuration only attaches storage; potentially
// multi-minute hashing belongs to the torrent's lifetime, not the JNI request.
func (s *EngineServer) startSavedDataCheck(rec *TorrentRecord) {
	pieces := make(map[int]bool)
	for _, index := range rec.Selected {
		f := rec.Torrent.Files()[index]
		for p := f.BeginPieceIndex(); p < f.EndPieceIndex(); p++ {
			pieces[p] = true
		}
	}
	ordered := make([]int, 0, len(pieces))
	for p := range pieces {
		ordered = append(ordered, p)
	}
	sort.Ints(ordered)
	ctx, cancel := context.WithCancel(context.Background())
	job := &savedDataCheck{cancel: cancel, done: make(chan struct{}), wake: make(chan struct{}, 1)}
	rec.verification = job
	rec.Checking, rec.CheckedPieces, rec.CheckTotal = true, 0, len(ordered)
	go s.checkSavedData(ctx, rec, job, ordered)
}

func (s *EngineServer) checkSavedData(ctx context.Context, rec *TorrentRecord, job *savedDataCheck, pieces []int) {
	defer close(job.done)
	defer job.cancel()
	st := s.storage.GetStorage(rec.InfoHash)
	if st != nil {
		st.SetRestoreVerify(true)
		// Always clear before peer hashing can start, including cancel paths.
		defer st.SetRestoreVerify(false)
	}
	clearRestore := func() {
		if st != nil {
			st.SetRestoreVerify(false)
		}
	}
	lastNotify := time.Now()
	for _, index := range pieces {
		for {
			if ctx.Err() != nil {
				return
			}
			rec.mu.RLock()
			paused := rec.Paused
			rec.mu.RUnlock()
			if !paused {
				break
			}
			select {
			case <-ctx.Done():
				return
			case <-job.wake:
			}
		}
		// Removal/shutdown cancels and joins this worker before closing the
		// torrent, so no verification call can race a dropped torrent.
		trusted := st != nil && st.PieceComplete(index)
		var err error
		if trusted {
			// Storage already has a durable verified bit for this piece (usually
			// from a checkpoint). Refresh torrent completion without re-hashing.
			rec.Torrent.Piece(index).UpdateCompletion()
		} else {
			err = rec.Torrent.Piece(index).VerifyDataContext(ctx)
		}
		if ctx.Err() != nil {
			return
		}
		rec.mu.Lock()
		if err != nil {
			message := fmt.Sprintf("Could not check saved data: %v", err)
			rec.Error, rec.Paused, rec.Checking = &message, true, false
			clearRestore()
			if st != nil {
				st.UnfreezeTrusted()
			}
			s.applyTransferState(rec)
			rec.mu.Unlock()
			s.notifyChange()
			return
		}
		rec.CheckedPieces++
		rec.mu.Unlock()
		if time.Since(lastNotify) >= 250*time.Millisecond {
			s.notifyChange()
			lastNotify = time.Now()
		}
	}
	rec.mu.Lock()
	if !rec.removed && ctx.Err() == nil {
		rec.Checking = false
		rec.Generation++
		if st != nil {
			st.MarkVerified()
			st.UnfreezeTrusted()
		}
		clearRestore()
		// Honor pause and selection changes made while hashing was running.
		s.applyTransferState(rec)
	} else if st != nil {
		st.UnfreezeTrusted()
	}
	rec.mu.Unlock()
	s.notifyChange()
}

// Never hold rec.mu while joining the worker. It may be publishing progress.
func stopSavedDataCheck(rec *TorrentRecord) {
	rec.mu.Lock()
	rec.removed = true
	job := rec.verification
	if job != nil {
		job.cancel()
	}
	rec.mu.Unlock()
	if job != nil {
		<-job.done
	}
}

// Called with rec.mu held. Resume during a check records intent without letting
// incoming payload overwrite data that has not yet been checked.
func (s *EngineServer) applyTransferState(rec *TorrentRecord) {
	t := rec.Torrent
	// DisallowData* gates only torrent piece payload. It leaves the torrent in
	// the client, so trackers, DHT, PEX, handshakes, and magnet metadata
	// exchange continue while Android's global data policy is disabled.
	active := s.payloadTransfersAllowed.Load() && rec.Configured && !rec.Paused && !rec.Checking && !rec.removed && rec.Error == nil
	if !active {
		t.DisallowDataDownload()
		t.DisallowDataUpload()
	}
	// Files() waits for magnet metadata. A global setting must never turn a
	// pending magnet into a blocking control request; when metadata arrives,
	// configure/select will apply the same policy before payload is enabled.
	if t.Info() == nil {
		return
	}
	selected := make(map[int]bool, len(rec.Selected))
	for _, index := range rec.Selected {
		selected[index] = true
	}
	for i, f := range t.Files() {
		priority := torrent.PiecePriorityNone
		if active && selected[i] {
			priority = torrent.PiecePriorityNormal
		}
		f.SetPriority(priority)
	}
	if active {
		t.AllowDataDownload()
		t.AllowDataUpload()
		// Must run after base file SetPriority so soft startup hints (raising
		// head and tail pieces to Now priority for selected videos) apply on
		// top of the base Normal file priority.
		s.applyVideoStartupHints(rec)
	}
	s.applyBackgroundFocus(rec)
}
