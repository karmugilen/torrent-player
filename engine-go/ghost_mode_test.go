package main

import (
	"encoding/json"
	"net"
	"testing"
	"time"

	"github.com/anacrolix/torrent"
	"golang.org/x/time/rate"
)

type ghostTrace struct {
	policy ghostDialController
	now    time.Time
	input  ghostObservation
	base   int
}

func newGhostTrace(base int) *ghostTrace {
	g := &ghostTrace{now: time.Unix(1000, 0), input: ghostObservation{cohort: "one:1", opportunity: true}, base: base}
	g.policy.observe(g.now, g.input, g.base)
	return g
}

func (g *ghostTrace) step(speed int64) int {
	g.now = g.now.Add(10 * time.Second)
	g.input.usefulBytes += speed * 10
	return g.policy.observe(g.now, g.input, g.base)
}

func TestGhostModeKeepsMeasuredGainsAndLimitsDialing(t *testing.T) {
	g := newGhostTrace(30)
	if g.step(100) != 30 || g.step(100) != 40 {
		t.Fatal("must collect two stable windows before the first +10/s probe")
	}
	if g.step(120) != 40 || g.step(120) != 40 || g.policy.kept != 40 {
		t.Fatal("must retain a probe only after two improved throughput windows")
	}
	if g.step(120) != 40 || g.step(120) != 50 {
		t.Fatal("next probe must wait for another stable baseline")
	}
	nearCeiling := newGhostTrace(75)
	if nearCeiling.step(100) != 75 || nearCeiling.step(100) != 80 {
		t.Fatal("probe exceeded the shared 80/s ceiling")
	}
	fast := newGhostTrace(80)
	for i := 0; i < 20; i++ {
		if fast.step(100) != 80 {
			t.Fatal("Ghost must respect Fast peer search's existing ceiling")
		}
	}
}

func TestGhostModeRollsBackFailedProbesAndCoolsDown(t *testing.T) {
	for _, speed := range []int64{100, 60} {
		g := newGhostTrace(30)
		g.step(100)
		g.step(100)
		current := g.step(speed)
		if speed == 100 {
			if current != 40 {
				t.Fatal("a neutral trial needs a second window")
			}
			current = g.step(speed)
		}
		if current != 30 {
			t.Fatal("failed probe must restore the previous rate")
		}
		for i := 0; i < 5; i++ {
			if g.step(100) != 30 {
				t.Fatal("failed probe retried before the one-minute cooldown")
			}
		}
		if g.step(100) != 40 {
			t.Fatal("expected a new stable trial after cooldown")
		}
	}
}

func TestGhostModeDoesNotProbeBurstsOrZeroThroughput(t *testing.T) {
	g := newGhostTrace(30)
	for _, speed := range []int64{0, 100, 1000, 10, 1000, 0, 0} {
		if g.step(speed) != 30 {
			t.Fatal("unstable or absent throughput must not trigger a probe")
		}
	}
}

func TestGhostModeResetsForChangedConditions(t *testing.T) {
	for _, condition := range []string{"no spare slots", "cohort changed", "counter reset", "long gap", "clock reset", "download capped"} {
		t.Run(condition, func(t *testing.T) {
			g := newGhostTrace(30)
			g.step(100)
			g.step(100)
			g.now = g.now.Add(10 * time.Second)
			g.input.usefulBytes += 1000
			switch condition {
			case "no spare slots":
				g.input.opportunity = false
			case "cohort changed":
				g.input.cohort = "different:2"
			case "counter reset":
				g.input.usefulBytes = 0
			case "long gap":
				g.now = g.now.Add(time.Minute)
			case "clock reset":
				g.now = time.Unix(1, 0)
			case "download capped":
				g.input.downloadLimit = 100
			}
			if got := g.policy.observe(g.now, g.input, g.base); got != 30 {
				t.Fatalf("stale trial survived %s: %d", condition, got)
			}
		})
	}
}

func TestGhostModeSettingRestoresLatestBaseAndPreservesOtherExperiments(t *testing.T) {
	s := testEngine(t, func(cfg *torrent.ClientConfig) {
		cfg.DialRateLimiter = rate.NewLimiter(defaultDialRate, defaultDialRate)
	})
	request := func(method, body string) SettingsResponse {
		t.Helper()
		code, response := s.DispatchControl(method, "/settings", body)
		if code != 200 {
			t.Fatalf("settings: %d %s", code, response)
		}
		var result SettingsResponse
		if err := json.Unmarshal(response, &result); err != nil {
			t.Fatal(err)
		}
		return result
	}
	if request("GET", "").GhostMode {
		t.Fatal("Ghost must default off")
	}
	if !request("POST", `{"ghostMode":true,"peerExploration":true}`).GhostMode {
		t.Fatal("Ghost did not enable")
	}
	// Drive the same policy and limiter used by the engine sampler to a trial.
	probe := func() {
		t.Helper()
		g := newGhostTrace(s.baseDialRateLocked())
		g.step(100)
		s.setGhostDialRateLocked(g.step(100))
		s.ghostDial = g.policy
	}
	probe()
	if got := request("GET", ""); got.DialRatePerSec != 30 || got.EffectiveDialRatePerSec != 40 {
		t.Fatalf("configured and trial rates must be distinct: %+v", got)
	}
	if got := request("POST", `{"maxPeers":55}`); !got.GhostMode || got.EffectiveDialRatePerSec != 30 {
		t.Fatal("connection changes must discard the old trial without disabling Ghost")
	}
	probe()
	if got := request("POST", `{"dialRatePerSec":80}`); !got.GhostMode || got.EffectiveDialRatePerSec != 80 {
		t.Fatal("Fast peer search did not replace the base rate")
	}
	if got := request("POST", `{"ghostMode":false}`); got.GhostMode || got.EffectiveDialRatePerSec != 80 || !got.PeerExploration {
		t.Fatal("switching off must restore the latest base and preserve Peer exploration")
	}
	request("POST", `{"dialRatePerSec":30,"ghostMode":true}`)
	probe()
	if got := request("POST", `{"ghostMode":false}`); got.EffectiveDialRatePerSec != 30 {
		t.Fatal("switching off did not restore normal dialing")
	}
}

func TestGhostModeSamplerHonorsPauseAndPayloadGate(t *testing.T) {
	s, rec, _ := playbackFixtureMode(t, true)
	s.dialLimiter = rate.NewLimiter(defaultDialRate, defaultDialRate)
	s.maxPeers = 55
	s.ghostMode = true
	rec.Torrent.AddPeers([]torrent.PeerInfo{{Addr: &net.TCPAddr{IP: net.ParseIP("127.0.0.1"), Port: 1}}})
	now := time.Unix(1000, 0)
	s.ghostModeTick(now)
	if s.ghostDial.cohort == "" {
		t.Fatal("active torrent was not sampled")
	}
	for _, pause := range []bool{true, false} {
		rec.Paused = pause
		s.payloadTransfersAllowed.Store(pause)
		s.setGhostDialRateLocked(40)
		s.ghostModeTick(now.Add(10 * time.Second))
		if got := limiterRate(s.dialLimiter); got != 30 {
			t.Fatalf("paused=%v payload=%v retained probe: %d", pause, pause, got)
		}
	}
}

func TestGhostModeCanToggleWhileSamplerRuns(t *testing.T) {
	s, _, _ := playbackFixtureMode(t, true)
	s.dialLimiter = rate.NewLimiter(defaultDialRate, defaultDialRate)
	s.maxPeers = 55
	done := make(chan struct{})
	go func() {
		defer close(done)
		for i := 0; i < 200; i++ {
			s.ghostModeTick(time.Unix(1000+int64(i), 0))
		}
	}()
	for i := 0; i < 20; i++ {
		if code, response := s.DispatchControl("POST", "/settings", `{"ghostMode":true}`); code != 200 {
			t.Fatalf("enable: %d %s", code, response)
		}
		if code, response := s.DispatchControl("POST", "/settings", `{"ghostMode":false}`); code != 200 {
			t.Fatalf("disable: %d %s", code, response)
		}
	}
	<-done
	if got := limiterRate(s.dialLimiter); got != defaultDialRate {
		t.Fatalf("sampler overwrote disabled base rate: %d", got)
	}
}
