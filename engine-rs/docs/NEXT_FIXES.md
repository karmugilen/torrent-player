# Next fixes: Rust engine vs Go on big swarms (Codex brief)

Worktree: `/home/kar/orca/workspaces/torrent-player/rust-engine`, branch `karmugilen/rust-engine`.
Latest commit when written: `4173061`. Reference implementation: `engine-go/patches/torrent/`
(anacrolix/torrent v1.61.0 + Webtor patches) and `engine-go/server.go`.
Rule: port Go's behaviour exactly. Do not tune constants by trial and error.

## Where we are (28 Oct, separate 30 s runs, same network)

| Torrent | Metric | Go | Rust | Rust vs Go |
|---|---|---|---|---|
| Ubuntu 24.04.3 | metadata | 8.0 s | 3.5 s | faster |
| Ubuntu | peers | 13 → 20 | 6 → 16 | fewer |
| Ubuntu | speed avg | 2.87 MiB/s | 1.24 MiB/s | **43 % of Go** |
| Ubuntu | speed trend | rises 1.8 → 3.9 | **falls 1.6 → 0.9** | decays |
| Sintel | peers | 9 → 20 | 23 → 32 | more |
| Sintel | speed avg | 1.58 MiB/s | 3.34 MiB/s | 2.1x faster |

Already fixed (do not undo): `8d2f401` (DHT EADDRINUSE disabled all discovery), `9ca6b7e`
(public tracker supplement, DHT lookups to completion), the overnight Grok commits (uTP BEP 29,
dial race, PEX, soft errors) and `4173061` (Go per-peer download rate = useful bytes / expecting
time, RTT starts at 0, request caps at Go's bound, KnownSwarm includes connections).

## Fix 1 (most likely cause of the Ubuntu speed gap): hard-coded `nominal_max: 64` in peer snapshots

- File: `engine-rs/crates/wt-core/src/torrent_impls.rs`, around line 575, in the function that builds
  `WebtorPeerSnap` for OTHER peers (`webtor_peer_snap`).
- Problem: it always sets `nominal_max: 64`. The HOL rescue, hedge and steal decisions compare
  `pending >= nominal_max` (`webtor/hol.rs` ~121 and ~229, `webtor/decisions.rs` ~204, ~238 and ~455),
  so every peer with 64 requests in flight is treated as "full". Measured: the best Ubuntu peer sat at
  exactly 64 pending requests while the speed decayed.
- Go: uses each peer's real `nominalMaxRequests()` (`peerconn.go:1598`) =
  `max(1, min(PeerMaxRequests, peakRequests*2, maxLocalToRemoteRequests))`, then min with the
  playback pipeline limit when it is set.
- Fix: put each peer's real nominal max (and the inputs it needs) into `PeerConnInfo`/the snapshot
  (`stats.rs` `PeerConnInfo`, filled in `peer_conn.rs` `peer_conn_info()`), and use it in
  `webtor_peer_snap` instead of 64.

## Fix 2: peer `reqq` is clamped to 250

- File: `engine-rs/crates/wt-core/src/peer_conn.rs` ~1612-1615:
  `self.state.max_requests = (reqq as usize).clamp(1, DEFAULT_PEER_MAX_REQUESTS)` (250).
- Go `peerconn.go:1091`: `if d.Reqq != 0 { c.PeerMaxRequests = d.Reqq }`, with no 250 ceiling.
  qBittorrent and Transmission often advertise 500 or more.
- Fix: take reqq as-is (clamp only to `MAX_LOCAL_TO_REMOTE_REQUESTS`, already defined in peer_conn.rs).

## Fix 3: speed still decays during a run on Ubuntu

After Fix 1 and Fix 2, re-measure. If it still falls, print every 5 s for the top 5 peers in BOTH engines:
`rate, rtt, pipeline limit, pending, peer_choking, downloaded` (Go: `Peer.WebtorRequestInfo()`,
`webtorPipelineLimit`, `webtorRTT.estimate`). Compare where Rust's pending/pipeline drops and Go's does not.
Check, against Go:
- `compute_playback_pipeline_limit` (`picker.rs`) vs Go `computePlaybackPipelineLimit`
  (`webtor_urgent.go:355`). Same formula; check that the inputs match (rate is now Go's; RTT probe is the same).
- The peak update `next_peak_requests` and the `PEAK_REQUESTS_FLOOR = 8` floor in `peer_conn.rs`
  (Go has no floor and starts at 0). Remove the floor if Go-equal behaviour works.
- SlowPeerYield / HOL steal: requests stolen from a peer that is still delivering.
- The Direct Play RAM cache (stream mode, 64 MiB in the test). Busy-cache write errors must re-request,
  not drop (already done; verify there is no stall when the cache is full).

## Fix 4: Ubuntu peer count stops at ~16 (Go keeps growing to 20+)

Compare these against Go and fix any difference:
- `torrent.rs` `wants_conns` (max_established - conns - dialing) vs Go `wantConns`/`openNewConns`.
- The dial pool `take_dial_candidates` order and the 5 s requeue/backoff vs Go `peers.PopMax` and
  `t.peers` priority (Go uses BEP 40 priority).
- Half-open limits and dial timeouts in `server.rs` (Go: 24 per torrent, 48 total, 8 s nominal, 2 s minimum,
  5 s handshake). codex-engine reported these match; re-check what the core actually uses.
- Tracker announces: `num_want` 200 while wanting peers (matches Go); check re-announce intervals and
  that supplemental trackers stay "working".
- PEX: count peers received via PEX and how many we dial, Go vs Rust.
- Print counters every 5 s for both engines: pool size, dialing, dial ok, dial fail by reason
  (timeout / refused / MSE / handshake), established, closed by reason.

## Fix 5 (smaller, found on the phone): WebRTC DTLS panic

- Phone log: `panicked at generic-array-0.14.7/src/lib.rs:572:9: assertion left == right failed, left: 32, right: 16`
  during the WebRTC DTLS handshake (webrtc 0.12, webrtc-dtls 0.11.0) after "Unsupported Extension Type" warnings.
- It kills only that WebTorrent peer. Fix by upgrading webrtc or restricting the DTLS cipher suites so it cannot panic.

## Fix 6: one Rust Sintel run produced no result

The first separate Rust Sintel run returned no result line (the second worked: 3.34 MiB/s). Re-run it 5 times;
if it reproduces, read `~/.cache/wt-tmp/peers-rust-sintel/host.log` for a panic or hang.

## Tests to run after each fix (all must pass)

Build environment (never build under /tmp):
```
cd engine-rs
export CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/codex CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0 \
       CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
```

1. Core unit and e2e tests: `cargo test -p wt-core -- --test-threads=3` (currently 194 pass, 0 fail).
2. Core lint: `cargo clippy -p wt-core --all-targets -- -D warnings` (currently clean).
3. Engine tests: `cargo test -p wt-engine -- --test-threads=2`, especially:
   `--test engine_two_host` (two real engine processes, magnet + SHA-1 check),
   `--test engine_magnet_configure`, `--test engine_swarm_playback` (local swarm: the 4 MiB stall regression).
4. API parity with Go (must stay 30 MATCH / 0 DIFF / 0 CRITICAL): see `crates/wt-parity/README.md`, `api-diff`.
5. Release loopback throughput: `cargo test --release -p wt-core --test e2e_throughput -- --ignored --nocapture`
   (was 43 MiB/s; must not get slower).
6. Real swarms, Go vs Rust, run SEPARATELY (one engine at a time), 30 or 60 s each, 3 runs per torrent:
   ```
   cargo build --release -p wt-engine --bin engine-host
   python3 tools/smoke/peers_compare.py <target>/release/engine-host rust-ubuntu "<ubuntu magnet>" 60
   python3 tools/smoke/peers_compare.py /home/kar/.cache/wt-tmp/engine-go-host go-ubuntu "<ubuntu magnet>" 60
   ```
   Ubuntu magnet (in the script header):
   `magnet:?xt=urn:btih:d160b8d8ea35a5b4e52837468fc8f03d55cef1f7&dn=ubuntu&tr=https%3A%2F%2Ftorrent.ubuntu.com%2Fannounce&tr=https%3A%2F%2Fipv6.torrent.ubuntu.com%2Fannounce`
   Sintel magnet:
   `magnet:?xt=urn:btih:08ada5a7a6183aae1e09d831df6748d566095a10&dn=Sintel&tr=udp%3A%2F%2Fexplodie.org%3A6969&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337&tr=wss%3A%2F%2Ftracker.webtorrent.dev`
   Use a normal network: on strict networks (mobile hotspot/CGNAT) most TCP dials time out and even Go
   gets no metadata. If Go cannot get metadata, the run does not count.
7. Android: `bash engine-rs/build-android.sh`, then
   `./android/gradlew -p android :app:assembleRelease -Pwebtor.engine=rust -Pwebtor.localSigning=true`;
   check the APK has `lib/arm64-v8a/libengine.so` with 9 `Java_webtor_app_EngineHost_*` exports; install with
   `adb install -r`, and the log must show `Engine ready` with no `panic:` lines.

## Done when

- Ubuntu: Rust average speed >= Go and peers >= Go in 3 separate 60 s runs, with no decay in the speed trend.
- Sintel: Rust still >= Go.
- Every test in the list above passes.
- Results recorded in `engine-rs/docs/SMOKE.md` (table: label, metadata s, peers min->max, MiB/s avg and peak).
- Commit each fix separately with a clear message, own paths only.
