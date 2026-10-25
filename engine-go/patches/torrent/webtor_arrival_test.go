package torrent

import (
	"math/rand"
	"testing"
	"time"

	"github.com/go-quicktest/qt"
)

// The network starts from nothing, learns from arrivals that queues drain
// slower than the formula says and that a peer is fast just after it
// unchokes us, and is used when switched on.
// Saved state restores it.
func TestArrivalModelLearnsFromArrivedBlocks(t *testing.T) {
	cl, tor := newTricksTorrent(t)
	defer func() {
		for c := range tor.conns {
			delete(tor.conns, c)
		}
	}()
	tor.cl.lock()
	normal := newRatedPeer(t, cl, tor, 256<<10)
	slow := newRatedPeer(t, cl, tor, 64<<10)
	for _, c := range []*PeerConn{normal, slow} {
		c.webtorRTT.estimate = 100 * time.Millisecond
	}
	_, ok := normal.arrivalInputs(10)
	qt.Assert(t, qt.IsTrue(ok))

	tuning := DefaultSeekTuning
	tuning.LearnedArrival = true
	SetSeekTuning(tuning)
	defer SetSeekTuning(DefaultSeekTuning)
	rng := rand.New(rand.NewSource(1))
	r := RequestIndex(0)
	for i := 0; i < 6000; i++ {
		for _, c := range []*PeerConn{normal, slow} {
			queued := uint64(rng.Intn(40))
			c.webtorUnchokedAt = time.Now().Add(-time.Duration(rng.Float64() * 10 * float64(time.Second)))
			x, _ := c.arrivalInputs(queued)
			// The truth: queues drain 1.6x slower, and the first 2 s after
			// an unchoke go twice as fast.
			actual := x[1] + 1.6*x[2] + x[3]
			if x[7] < 2 {
				actual /= 2
			}
			actual *= 1 + 0.1*rng.NormFloat64()
			c.webtorNoteRequestSent(r, queued)
			s := c.webtorArrivalSamples[r]
			s.sent = time.Now().Add(-time.Duration(actual * float64(time.Second)))
			c.webtorArrivalSamples[r] = s
			c.webtorLearnArrival(r)
			r++
		}
	}
	// The formula is off by 1.6 s on the same truth.
	qt.Check(t, qt.IsTrue(cl.webtorArrival.NetErr < 0.8), qt.Commentf("the network beats the formula"))
	qt.Check(t, qt.IsTrue(cl.useLearnedArrival()))
	// A quarter of the rate behind the same round trip: 0.35 s against 0.16 s.
	t.Logf("expected: slow %.0f ms, normal %.0f ms", slow.expectedArrival()*1000, normal.expectedArrival()*1000)
	qt.Check(t, qt.IsTrue(slow.expectedArrival() > 1.5*normal.expectedArrival()), qt.Commentf("peer choice sees the slow peer"))
	SetSeekTuning(DefaultSeekTuning)
	qt.Check(t, qt.IsFalse(cl.useLearnedArrival()), qt.Commentf("off when switched off"))
	tor.cl.unlock()

	st := cl.WebtorArrivalStats()
	t.Logf("network off by %.0f ms over %d blocks", st.NetworkErrMs, st.Samples)

	saved, err := cl.WebtorArrivalState()
	qt.Assert(t, qt.IsNil(err))
	cl2 := newTestingClient(t)
	qt.Assert(t, qt.IsNil(cl2.SetWebtorArrivalState(saved)))
	qt.Check(t, qt.Equals(cl2.WebtorArrivalStats(), cl.WebtorArrivalStats()), qt.Commentf("restored after a restart"))
	qt.Check(t, qt.IsNotNil(cl2.SetWebtorArrivalState([]byte("not json"))))
}

func abs(f float64) float64 {
	if f < 0 {
		return -f
	}
	return f
}
