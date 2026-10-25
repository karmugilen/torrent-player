package torrent

import (
	"encoding/json"
	"math"
	"math/rand"
	"time"
)

// Webtor learned arrival estimate. Peer choice for the playhead, hedging and
// slow-peer yielding all rest on one guess: when would a block asked of this
// peer now arrive. expectedArrival guesses round trip + queue and block at
// the peer's lifetime rate. Every block that arrives tells how wrong that
// was, a few hundred times a second while a video plays, so a small network
// learns the real relation on the phone itself: one hidden layer of
// arrivalHidden tanh units over log inputs, predicting log time, so a 20 s
// stall is one bad guess, not a lesson.
//
// x adds what the formula ignores: the peer's recent rate (its lifetime
// average lags when it speeds up or slows down), how long since its last
// block (a peer going quiet) and since it unchoked us (a fresh unchoke
// bursts, then slows). With LearnedArrival on, the network alone gives the
// estimate, from the first block; the formula is not scored or fallen back
// to. Linear, attention, boosted-tree, mutating, spread-predicting and
// first-principles contenders were tried on the phone; none beat this
// network by enough to pay for its work.

const (
	// x: bias, round trip, queue/rate, block/rate, jitter, queue/recent
	// rate, seconds since the peer's last block, seconds since it unchoked
	// us. All but the bias are seconds.
	arrivalFeatures = 8
	arrivalHidden   = 8

	// arrivalNetStep is the network's gradient step, in log time.
	arrivalNetStep = 0.005
	// arrivalScoreAlpha weighs the latest block in the running error score.
	arrivalScoreAlpha = 0.01
	// arrivalPeerAlpha weighs the latest block in a peer's jitter and recent
	// rate.
	arrivalPeerAlpha = 0.1
	// arrivalIdleCap and arrivalUnchokeCap bound the time inputs, seconds.
	arrivalIdleCap    = 10
	arrivalUnchokeCap = 30
)

// arrivalNet is a one-hidden-layer network from log inputs to log seconds.
type arrivalNet struct {
	W1 [arrivalHidden][arrivalFeatures]float64 // input 0 is the bias
	W2 [arrivalHidden]float64
	B2 float64
}

func (n *arrivalNet) init() {
	rng := rand.New(rand.NewSource(1))
	for h := range n.W1 {
		for i := range n.W1[h] {
			n.W1[h][i] = 0.3 * rng.NormFloat64()
		}
	}
	n.B2 = math.Log(0.5)
}

// netInputs maps x to log inputs, so seconds from 1 ms to minutes are on
// one scale.
func netInputs(x [arrivalFeatures]float64) (z [arrivalFeatures]float64) {
	z[0] = 1
	for i := 1; i < arrivalFeatures; i++ {
		z[i] = math.Log1p(10 * max(x[i], 0))
	}
	return
}

// forward returns log seconds and the hidden activations.
func (n *arrivalNet) forward(z [arrivalFeatures]float64) (y float64, h [arrivalHidden]float64) {
	y = n.B2
	for j := range h {
		var a float64
		for i, v := range z {
			a += n.W1[j][i] * v
		}
		h[j] = math.Tanh(a)
		y += n.W2[j] * h[j]
	}
	return
}

func (n *arrivalNet) predict(x [arrivalFeatures]float64) float64 {
	y, _ := n.forward(netInputs(x))
	return math.Exp(min(max(y, math.Log(0.001)), math.Log(60)))
}

// learn moves the network toward log(actual) by one gradient step.
func (n *arrivalNet) learn(x [arrivalFeatures]float64, actual float64) {
	z := netInputs(x)
	y, h := n.forward(z)
	err := min(max(math.Log(max(actual, 0.001))-y, -2), 2)
	for j := range h {
		// The hidden unit's share of the error, before its output weight moves.
		back := err * n.W2[j] * (1 - h[j]*h[j])
		n.W2[j] += arrivalNetStep * err * h[j]
		for i, v := range z {
			n.W1[j][i] += arrivalNetStep * back * v
		}
	}
	n.B2 += arrivalNetStep * err
}

// arrivalModel is shared by every torrent of a client. Client lock.
type arrivalModel struct {
	Net arrivalNet
	// Samples is the blocks learned from; NetErr is the running mean
	// absolute error, seconds.
	Samples int64
	NetErr  float64
	ready   bool
}

func (m *arrivalModel) init() {
	if m.ready {
		return
	}
	m.ready = true
	if m.Samples == 0 {
		m.Net.init()
	}
}

// arrivalSample is what a request was sent with, to learn from its block.
type arrivalSample struct {
	sent     time.Time
	x        [arrivalFeatures]float64
	estimate float64 // seconds
}

// arrivalPeer is a peer's learned state. Zero is unset.
type arrivalPeer struct {
	jitter    float64 // mean absolute error of the estimate in use, seconds
	recent    float64 // recent rate, bytes per second
	lastBlock time.Time
}

// webtorNoteBlock updates c's recent rate from a block that just arrived.
// Client lock.
func (c *PeerConn) webtorNoteBlock() {
	p := &c.webtorArrival
	now := time.Now()
	if !p.lastBlock.IsZero() {
		// Rate over the gap since the previous block, at least 1 ms.
		inst := float64(c.t.chunkSize) / max(now.Sub(p.lastBlock).Seconds(), 0.001)
		if p.recent == 0 {
			p.recent = inst
		} else {
			p.recent += arrivalPeerAlpha * (inst - p.recent)
		}
	}
	p.lastBlock = now
}

// arrivalInputs is x for a block asked of c now, behind queued blocks. ok
// is false while c has no measured rate.
func (c *PeerConn) arrivalInputs(queued uint64) (x [arrivalFeatures]float64, ok bool) {
	rate := c.downloadRate()
	if rate <= 0 || c.t.chunkSize <= 0 {
		return x, false
	}
	rtt := c.webtorRTT.estimate.Seconds()
	if rtt <= 0 {
		rtt = 0.2
	}
	p := &c.webtorArrival
	block := float64(c.t.chunkSize) / rate
	recent := p.recent
	if recent <= 0 {
		recent = rate
	}
	idle := 0.0
	if !p.lastBlock.IsZero() {
		idle = min(time.Since(p.lastBlock).Seconds(), arrivalIdleCap)
	}
	unchoked := 0.0
	if !c.peerChoking && !c.webtorUnchokedAt.IsZero() {
		unchoked = min(time.Since(c.webtorUnchokedAt).Seconds(), arrivalUnchokeCap)
	}
	x = [arrivalFeatures]float64{
		1, rtt, float64(queued) * block, block, p.jitter,
		float64(queued) * float64(c.t.chunkSize) / recent, idle, unchoked,
	}
	return x, true
}

// learnedArrival is the network's estimate for x from c. Client lock.
func (c *PeerConn) learnedArrival(x [arrivalFeatures]float64) float64 {
	m := &c.t.cl.webtorArrival
	m.init()
	return m.Net.predict(x)
}

// useLearnedArrival reports whether estimates come from the network.
func (cl *Client) useLearnedArrival() bool {
	return currentSeekTuning().LearnedArrival
}

// webtorNoteRequestSent records r's inputs, sent behind queued blocks.
// Client lock.
func (c *PeerConn) webtorNoteRequestSent(r RequestIndex, queued uint64) {
	x, ok := c.arrivalInputs(queued)
	if !ok {
		return
	}
	if c.webtorArrivalSamples == nil {
		c.webtorArrivalSamples = make(map[RequestIndex]arrivalSample)
	}
	m := &c.t.cl.webtorArrival
	m.init()
	c.webtorArrivalSamples[r] = arrivalSample{
		sent: time.Now(), x: x, estimate: m.Net.predict(x),
	}
}

// webtorLearnArrival scores the network's estimate for r's block, which
// just arrived from c, and teaches it. Client lock.
func (c *PeerConn) webtorLearnArrival(r RequestIndex) {
	c.webtorNoteBlock()
	s, ok := c.webtorArrivalSamples[r]
	if !ok {
		return
	}
	delete(c.webtorArrivalSamples, r)
	actual := time.Since(s.sent).Seconds()
	m := &c.t.cl.webtorArrival
	m.init()
	d := math.Abs(s.estimate - actual)
	if m.Samples == 0 {
		m.NetErr = d
	} else {
		m.NetErr += arrivalScoreAlpha * (d - m.NetErr)
	}
	m.Samples++
	p := &c.webtorArrival
	p.jitter += arrivalPeerAlpha * (d - p.jitter)
	m.Net.learn(s.x, actual)
}

// webtorBlockOverdue reports whether r, held by c, is late by its learned
// estimate: past it by twice c's usual error, at least min. ok is false
// when no model is in use or there is no estimate for r. Client lock.
func (c *PeerConn) webtorBlockOverdue(r RequestIndex, min time.Duration) (overdue, ok bool) {
	if !c.t.cl.useLearnedArrival() {
		return false, false
	}
	s, have := c.webtorArrivalSamples[r]
	if !have {
		return false, false
	}
	late := time.Duration((s.estimate + 2*c.webtorArrival.jitter) * float64(time.Second))
	return time.Since(s.sent) >= max(late, min), true
}

// ArrivalStats is how well block arrival is predicted.
type ArrivalStats struct {
	Samples      int64
	NetworkErrMs float64
	// Using is whether LearnedArrival uses the network now.
	Using bool
}

// WebtorArrivalStats reports the learned arrival model.
func (cl *Client) WebtorArrivalStats() ArrivalStats {
	cl.rLock()
	defer cl.rUnlock()
	m := cl.webtorArrival
	return ArrivalStats{Samples: m.Samples, NetworkErrMs: m.NetErr * 1000, Using: cl.useLearnedArrival()}
}

// WebtorArrivalState is the learned arrival model, to save across runs.
func (cl *Client) WebtorArrivalState() ([]byte, error) {
	cl.rLock()
	defer cl.rUnlock()
	m := cl.webtorArrival
	if m.Samples == 0 {
		return nil, nil
	}
	return json.Marshal(m)
}

// SetWebtorArrivalState restores a model saved by WebtorArrivalState. Saved
// state from another shape of model is ignored.
func (cl *Client) SetWebtorArrivalState(b []byte) error {
	var m arrivalModel
	if err := json.Unmarshal(b, &m); err != nil {
		return err
	}
	if m.Samples <= 0 {
		return nil
	}
	m.ready = true
	cl.lock()
	defer cl.unlock()
	cl.webtorArrival = m
	return nil
}
