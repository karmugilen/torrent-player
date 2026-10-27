# wt-policy port notes

Pure Rust port of `engine-go` playback/peer policy. No dependency on `wt-core`.
Time is injected (`Instant`) so tests never sleep. The only I/O surface is
`BypassStream` (tokio `AsyncRead` + `AsyncWrite` write-split wrapper).

## Ported

| Go source | Rust | Notes |
|---|---|---|
| `adaptive_buffer.go` | `AdaptiveBuffer` | Same EWMA / burst-aware / damping |
| `arrival_model.go` | `ArrivalModelPersist` | Path + save gate only; neural net stays in wt-core |
| `video_priority.go` | `video_startup_hints`, `is_video_file_name` | Explicit file/context inputs |
| `playback_grid.go` | `split_urgent`, `playback_tiers`, `range_start`, … | Telemetry builders |
| `playback_resume.go` | `ResumeRange` | Timer expiry via `tick_expire(now)` |
| `next_preload.go` | `NextPreload` | Plan / should_start / activate / view |
| `peer_exploration.go` | `PeerExploration` | `PeerKey = u64` instead of `*PeerConn` |
| `ghost_mode.go` | `GhostDialController` | Dial-rate feedback |
| `ghost_pro.go` | `GhostProSwarm`, wanted/overlap helpers | Victim choice + piece sets |
| `bypass.go` | `BypassStream`, `BypassController`, `split_write_sync` | Async wrapper + sync helper |
| `transfer_rates.go` | `TransferRates` | Per-torrent EWMA |
| `direct_play_idle.go` | `IdleDirectPlay`, `playback_window_filling` | Rest / wake / drop-ready |
| `claude_seek.go` | `SeekLatencyStats`, drop-chokers, tuning | Stats + decisions |
| `seek_log.go` | `SeekLog`, `StreamMeter` | Format/rotate decisions; engine does I/O |

## Differs from Go

- Peer identities are opaque `PeerKey` values assigned by the engine (no anacrolix types).
- Resume / idle / exploration use injected `Instant` instead of `time.AfterFunc` goroutines.
- `SeekLog` does not open files; it formats CSV and reports rotation.
- Arrival model JSON blob load/save is performed by the engine; this crate only decides path/when.
- Video mime detection uses a fixed extension table equivalent to Go's `mime` + torrent extras (no `/etc/mime.types`).
- Ghost Mode "clock went backwards" reset is unreachable with monotonic `Instant`; other reset paths are covered.

## Tests

Unit ports of the policy cores from:

- `adaptive_buffer_test.go`
- `video_priority_test.go` (`TestIsVideoFileName` + head/tail hint logic)
- `playback_grid_test.go` (`TestRangeStartParsesSingleRanges` + tier split)
- `playback_resume_test.go` (probe vs body position, generation expiry)
- `next_preload_test.go` (plan / start gate / ready)
- `peer_exploration_test.go` (pure `choose` cases)
- `ghost_mode_test.go` (dial controller traces)
- `ghost_pro_test.go` (contiguous wanted + victim choice)
- `bypass_test.go` (controller + split + loopback)
- `transfer_rates_test.go`
- `direct_play_idle_test.go` (`playbackWindowFilling` + rest/wake/drop)
- Policy parts of `idle_buffer_test.go` (buffer-filling blocks rest)

Full EngineServer integration cases (local swarm, `/play` HTTP, JNI) remain in `wt-engine`.

## Remains

- Engine wiring: applying demand maps, dial hooks, rest peer address cache.
- Seek A/B experiment RNG lives in the engine (policy exposes `pick_seek_variant`).
