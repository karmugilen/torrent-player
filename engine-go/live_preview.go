package main

import (
	"net/http"
	"strconv"
)

// A small read-only snapshot lets thumbnails follow real cached playback data.
func (s *EngineServer) handlePreviewState(w http.ResponseWriter, r *http.Request, id string) {
	s.mu.RLock()
	rec := s.records[id]
	s.mu.RUnlock()
	if rec == nil {
		writeJSON(w, 404, map[string]string{"error": "torrent not found"})
		return
	}
	rec.mu.RLock()
	defer rec.mu.RUnlock()
	index, err := strconv.Atoi(r.URL.Query().Get("fileIndex"))
	files := rec.Torrent.Files()
	if err != nil || index < 0 || index >= len(files) {
		writeJSON(w, 400, map[string]string{"error": "invalid file index"})
		return
	}
	file := files[index]
	var offset int64
	active := false
	s.playbackMu.Lock()
	owner := s.playbackOwners[rec.Torrent]
	s.playbackMu.Unlock()
	if owner != nil {
		owner.mu.Lock()
		if owner.resumeFile == file {
			offset = owner.resumeOffset
		}
		for reader := range owner.readers {
			if reader.file == file {
				active = true
				break
			}
		}
		owner.mu.Unlock()
	}
	var signature uint64
	if st := s.storage.GetStorage(rec.InfoHash); st != nil {
		st.mu.Lock()
		for piece, complete := range st.completed {
			at := int64(piece) * st.chunkLength
			if complete && at < file.Offset()+file.Length() && at+st.info.Piece(piece).Length() > file.Offset() {
				signature ^= uint64(piece+1) * 0x9e3779b97f4a7c15
			}
		}
		st.mu.Unlock()
	}
	writeJSON(w, http.StatusOK, struct {
		Active     bool   `json:"active"`
		ByteOffset int64  `json:"byteOffset"`
		Signature  string `json:"signature"`
	}{active, offset, strconv.FormatUint(signature, 16)})
}
