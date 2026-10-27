# wt-core `webtor/` port notes

Ports runtime seek decisions from `engine-go/patches/torrent/webtor_urgent.go`
(and the NoBalanceSteal gate in `requesting.go`) as pure functions over
injected traits. Selection helpers already live in `strategy/webtor.rs`.

## Ported

- `shouldStealUrgent`, focus steal divisor, NoBalanceSteal / balance gate
- `fasterFocusPeer` (rate, QueueAware, ArrivalAware / ArrivalRatio)
- `expectedArrival` (+ LearnedArrival via `ArrivalPredictor` → `arrival.rs`)
- `rttEstimator` (idle-peer probe, EWMA /8; estimate starts at 0 like Go)
- `pendingRequests`, `urgentPastCancels`
- `requestFocusIndex` / `SetRequestFocus` encode
- `RefocusRequests` cancel predicate + peer sort key
- `NudgeUrgentRequests` peer selection predicate
- HOL window / overdue / `webtorRescueHeadOfLine` decisions
- Hedge decisions + answered bookkeeping helper
- `SlowPeerYield` (`webtorYieldToFasterPeers`)
- `WebtorDropChokers` selection (≤2)
- `webtorHasPlayingPieceWeLack`, `WebtorPeerCounts`
- PeakMode → next peak / nominal
- Counters: rescues, hedges, chokers dropped
- Injectable `Clock` / `ManualClock` for deterministic tests

## Differs

- No live `PeerConn` / lock integration here — agy-2 wires call sites
- `picker::should_steal_urgent` still exists (agy-2); canonical clock-aware
  API is `webtor::should_steal_urgent`
- Go `peer_conn::RttEstimator` currently defaults estimate to 200 ms; this
  module matches Go zero-until-sampled

## Remains

- agy-2 should call these from refill / nudge / choke loops (replace
  duplicated logic in `torrent.rs` / `picker.rs` when convenient)
- Integration tests that need a full swarm still live against `TorrentInner`
- Hash-first-for-Now (`getPieceToHash`) is grok-6 hashing ownership
