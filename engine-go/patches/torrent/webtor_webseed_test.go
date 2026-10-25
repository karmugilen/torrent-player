package torrent

import (
	"crypto/sha1"
	"math/rand"
	"net/http"
	"net/http/httptest"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/go-quicktest/qt"

	"github.com/anacrolix/torrent/bencode"
	"github.com/anacrolix/torrent/metainfo"
)

// A seek to another web seed slice starts a request there at once, not on the
// 5 s replanning timer.
func TestWebseedStartsAtAnUrgentPieceAtOnce(t *testing.T) {
	oldChunk := webseedRequestChunkSize
	webseedRequestChunkSize = 1 << 20
	var cl *Client
	t.Cleanup(func() {
		// Runs after the client closed: its web seed goroutines read the size
		// until their requests are gone.
		for deadline := time.Now().Add(5 * time.Second); time.Now().Before(deadline); time.Sleep(10 * time.Millisecond) {
			cl.lock()
			active := len(cl.activeWebseedRequests)
			cl.unlock()
			if active == 0 {
				break
			}
		}
		webseedRequestChunkSize = oldChunk
	})

	const pieceLen, fileLen = 256 << 10, 8 << 20
	data := make([]byte, fileLen)
	rand.New(rand.NewSource(1)).Read(data)
	info := metainfo.Info{Name: "movie.mkv", PieceLength: pieceLen, Length: fileLen}
	for off := 0; off < fileLen; off += pieceLen {
		h := sha1.Sum(data[off : off+pieceLen])
		info.Pieces = append(info.Pieces, h[:]...)
	}
	infoBytes, err := bencode.Marshal(info)
	qt.Assert(t, qt.IsNil(err))

	// A slow server keeps each request running for seconds.
	var mu sync.Mutex
	var starts []int64
	sawRequest := make(chan struct{}, 64)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		spec := strings.TrimPrefix(r.Header.Get("Range"), "bytes=")
		first, last, _ := strings.Cut(spec, "-")
		begin, _ := strconv.ParseInt(first, 10, 64)
		end, _ := strconv.ParseInt(last, 10, 64)
		mu.Lock()
		starts = append(starts, begin)
		mu.Unlock()
		sawRequest <- struct{}{}
		w.Header().Set("Content-Range", "bytes "+first+"-"+last+"/"+strconv.Itoa(fileLen))
		w.Header().Set("Content-Length", strconv.FormatInt(end-begin+1, 10))
		w.WriteHeader(http.StatusPartialContent)
		for pos := begin; pos <= end; pos += 16 << 10 {
			if _, err := w.Write(data[pos:min(pos+16<<10, end+1)]); err != nil {
				return
			}
			w.(http.Flusher).Flush()
			select {
			case <-r.Context().Done():
				return
			case <-time.After(200 * time.Millisecond):
			}
		}
	}))
	t.Cleanup(server.Close)

	cl = newTestingClient(t)
	tor, _ := cl.AddTorrentOpt(AddTorrentOpts{InfoHash: metainfo.HashBytes(infoBytes), InfoBytes: infoBytes})
	tor.AddWebSeeds([]string{server.URL + "/"})
	tor.Piece(0).SetPriority(PiecePriorityNow)
	select {
	case <-sawRequest:
	case <-time.After(10 * time.Second):
		t.Fatal("no web seed request for the first piece")
	}

	// Seek: piece 20 is in slice 5.
	seek := time.Now()
	tor.Piece(20).SetPriority(PiecePriorityNow)
	want := int64(20 * pieceLen)
	for {
		mu.Lock()
		found := false
		for _, s := range starts {
			found = found || (s >= 5<<20 && s <= want)
		}
		mu.Unlock()
		if found {
			break
		}
		select {
		case <-sawRequest:
		case <-time.After(time.Until(seek.Add(2 * time.Second))):
			t.Fatalf("no web seed request at the playhead within 2s; requests began at %v", starts)
		}
	}
}
