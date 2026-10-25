package main

import (
	"bufio"
	"fmt"
	"io"
	"math/rand/v2"
	"os"
	"sync"
	"time"

	"github.com/anacrolix/torrent"
)

// seekLog appends one line per HTTP playback request to a CSV file in the
// app's external files folder, where adb can read it, so seek changes are
// judged on the phone. The time to 2 MiB is about what a player buffers
// before it resumes after a seek.
type seekLog struct {
	mu   sync.Mutex
	path string
	tag  string
}

const (
	seekLogMaxBytes = 2 << 20
	seekLogHeader   = "time,build,torrent,file,kind,offset_mib,position_pct,first_ms,mib1_ms,mib2_ms,total_ms,bytes,in_ram,peers,unchoked_us,down_kib_s,variant,rescues,mode,piece_kib\n"
)

func (l *seekLog) setPath(path, tag string) {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.path, l.tag = path, tag
	// A log from an older build has other columns: keep it beside the new one.
	if f, err := os.Open(path); err == nil {
		header, _ := bufio.NewReader(f).ReadString('\n')
		f.Close()
		if header != seekLogHeader {
			_ = os.Rename(path, path+".old")
		}
	}
}

// pickSeekVariant chooses seek rules at random for a range request when the
// A/B test is on and names them for the log: u (blocks returned as they
// arrive) or v (verified pieces only) for download playback, and f8 or f128
// for the blocks after the playhead sent to the soonest peers (FocusBlocks,
// 128 KiB or 2 MiB). Off, or for a request without a range, it is "default".
func (s *EngineServer) pickSeekVariant(start int64) (name string, unverified bool) {
	if !s.seekExperiment.Load() || start < 0 {
		return "default", false
	}
	tuning := s.baseSeekTuning()
	if rand.IntN(2) == 1 {
		tuning.FocusBlocks = 128
	}
	torrent.SetSeekTuning(tuning)
	unverified = rand.IntN(2) == 1
	mode := "v"
	if unverified {
		mode = "u"
	}
	return fmt.Sprintf("%s-f%d", mode, tuning.FocusBlocks), unverified
}

type seekRecord struct {
	at              time.Time
	torrent         string
	file            int
	kind            string
	offset, length  int64
	first, m1, m2   time.Duration
	total           time.Duration
	bytes           int64
	inRAM           bool
	peers, unchoked int
	downBytesSec    int64
	variant         string
	rescues         int64
	directPlay      bool
	pieceBytes      int64
}

func (l *seekLog) write(r seekRecord) {
	l.mu.Lock()
	defer l.mu.Unlock()
	if l.path == "" {
		return
	}
	if st, err := os.Stat(l.path); err == nil && st.Size() > seekLogMaxBytes {
		_ = os.Rename(l.path, l.path+".1")
	}
	f, err := os.OpenFile(l.path, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o644)
	if err != nil {
		return
	}
	defer f.Close()
	if st, err := f.Stat(); err == nil && st.Size() == 0 {
		_, _ = f.WriteString(seekLogHeader)
	}
	ms := func(d time.Duration) string {
		if d <= 0 {
			return ""
		}
		return fmt.Sprint(d.Milliseconds())
	}
	pct := 0.0
	if r.length > 0 {
		pct = float64(r.offset) * 100 / float64(r.length)
	}
	inRAM := 0
	if r.inRAM {
		inRAM = 1
	}
	mode := "download"
	if r.directPlay {
		mode = "direct"
	}
	_, _ = fmt.Fprintf(f, "%s,%s,%s,%d,%s,%.1f,%.1f,%s,%s,%s,%d,%d,%d,%d,%d,%d,%s,%d,%s,%d\n",
		r.at.Format(time.RFC3339), l.tag, r.torrent, r.file, r.kind,
		float64(r.offset)/(1<<20), pct, ms(r.first), ms(r.m1), ms(r.m2), r.total.Milliseconds(),
		r.bytes, inRAM, r.peers, r.unchoked, r.downBytesSec>>10, r.variant, r.rescues, mode, r.pieceBytes>>10)
}

// streamMeter times the bytes a response hands to the player from start.
type streamMeter struct {
	r             io.ReadSeeker
	start         time.Time
	first, m1, m2 time.Duration
	n             int64
}

func (m *streamMeter) Read(b []byte) (int, error) {
	n, err := m.r.Read(b)
	if n > 0 {
		at := time.Since(m.start)
		before := m.n
		m.n += int64(n)
		if before == 0 {
			m.first = at
		}
		if before < 1<<20 && m.n >= 1<<20 {
			m.m1 = at
		}
		if before < 2<<20 && m.n >= 2<<20 {
			m.m2 = at
		}
	}
	return n, err
}

func (m *streamMeter) Seek(offset int64, whence int) (int64, error) {
	return m.r.Seek(offset, whence)
}
