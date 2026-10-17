package main

import (
	"context"
	"fmt"
	"sort"
	"sync"
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
	st := s.storage.GetStorage(rec.InfoHash)
	pending := make([]int, 0, len(ordered))
	trusted := 0
	for _, index := range ordered {
		if st != nil && st.PieceComplete(index) {
			trusted++
			continue
		}
		pending = append(pending, index)
	}
	s.publishStorageCompletions(rec)
	if len(pending) == 0 {
		rec.Checking, rec.CheckedPieces, rec.CheckTotal = false, trusted, len(ordered)
		if st != nil {
			st.MarkVerified()
			st.UnfreezeTrusted()
		}
		return
	}
	ctx, cancel := context.WithCancel(context.Background())
	job := &savedDataCheck{cancel: cancel, done: make(chan struct{}), wake: make(chan struct{}, 1)}
	rec.verification = job
	rec.Checking, rec.CheckedPieces, rec.CheckTotal = true, trusted, len(ordered)
	go s.checkSavedData(ctx, rec, job, pending)
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
	// Pipeline independent checks through all available CPU workers. A single
	// dispatcher owns pause/resume and progress; workers only verify pieces.
	workerCtx, cancelWorkers := context.WithCancel(ctx)
	workers := min(backgroundWorkers(), len(pieces))
	jobs := make(chan int)
	results := make(chan error, workers)
	var work sync.WaitGroup
	for w := 0; w < workers; w++ {
		work.Add(1)
		go func() {
			defer work.Done()
			for {
				select {
				case <-workerCtx.Done():
					return
				case index := <-jobs:
					if workerCtx.Err() != nil {
						return
					}
					var err error
					if st != nil && st.PieceComplete(index) {
						rec.Torrent.Piece(index).UpdateCompletion()
					} else {
						err = rec.Torrent.Piece(index).VerifyDataContext(workerCtx)
					}
					select {
					case results <- err:
					case <-workerCtx.Done():
						return
					}
				}
			}
		}()
	}
	defer func() { cancelWorkers(); work.Wait() }()
	lastNotify := time.Now()
	next, checked := 0, 0
	for checked < len(pieces) {
		rec.mu.RLock()
		paused := rec.Paused
		rec.mu.RUnlock()
		var ready chan<- int
		var index int
		if !paused && next < len(pieces) {
			ready, index = jobs, pieces[next]
		}
		select {
		case <-ctx.Done():
			return
		case <-job.wake:
			// Re-evaluate the latest user intent before dispatching more work.
		case ready <- index:
			next++
		case err := <-results:
			if ctx.Err() != nil {
				return
			}
			if err != nil {
				// Join the complete pipeline before changing verification mode.
				cancelWorkers()
				work.Wait()
				rec.mu.Lock()
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
			checked++
			rec.mu.Lock()
			rec.CheckedPieces++
			rec.mu.Unlock()
			if time.Since(lastNotify) >= 250*time.Millisecond {
				s.notifyChange()
				lastNotify = time.Now()
			}
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

func (s *EngineServer) publishStorageCompletions(rec *TorrentRecord) {
	if rec == nil || rec.Torrent == nil || rec.Torrent.Info() == nil {
		return
	}
	n := rec.Torrent.NumPieces()
	for i := 0; i < n; i++ {
		rec.Torrent.Piece(i).UpdateCompletion()
	}
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
		if active && selected[i] && !rec.DirectPlay {
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
