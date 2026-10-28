# Handoff: Rust peer-count parity vs Go (Bunny)

**For:** Codex (continue this fix)  
**Date:** 2026-10-28  
**Branch:** `karmugilen/rust-engine`  
**HEAD:** `4faea3ad497b5c0997b8b19364d5030997dac3fa` (`4faea3a`)  
**Repo path:** `/home/kar/orca/workspaces/torrent-player/rust-engine`  
**Engine code:** `engine-rs/` (workspace crates under `engine-rs/crates/`)

---

## Goal

Rust established peers on Big Buck Bunny (60s) must **match or beat** Go (anacrolix + Webtor).  
Port Go behaviour from `engine-go/patches/torrent/`. **No blind constant tuning.**

Secondary: keep download speed ≥ Go (~6 MiB/s on current swarm). Speed is already matched.

---

## Current HEAD commits (this retain phase)

Base before retain work: `5e1559b` (clippy div_ceil / prior peer-pool merges).

| SHA | Summary |
|-----|---------|
| `823e757` | Unhelpful post-HS close (`reset`/`decode`/`eof`, 0 useful bytes) → same dial fail backoff as `dial_finished(false)` |
| `e73d9bd` | Go `sendInitialMessages` parity; stop focus-wake request flood (`isLowOnRequests`) |
| `3568801` | `dial_ok` must **not** clear fail_count/backoff; clear only via `note_peer_helpful` on useful chunk |
| `c36e75b` | Dial race **first-wins** like Go `getFirst`; drop 750ms TCP-prefer |
| `f3b4932` | Stop requeueing failed dials (Go PopMax + mid-run rediscovery only) |
| `4faea3a` | Gate unused `should_requeue_failed_dial` under `cfg(test)` |

Earlier already on branch (pre-retain): dial requeue/fail-order (`7268b03`), discovery mid-run feed (`b3b314e`), chokers/worstBad (`bcc99d2`), uTP SACK strip (`42e958b`), TCP-prefer (later reverted by `c36e75b`), PEX replace/listing, etc.

---

## Latest Bunny numbers (retain4)

Artifacts: `/tmp/peer-parity-reports/bunny-retain4.jsonl`  
Runs: `~/.cache/wt-tmp/peers-{go,rust}-bunny-retain4-{1,2,3}/`

| | Go mean | Rust mean |
|--|---------|-----------|
| **max** peers | 39.7 | 26.7 |
| **end** `conn_n` | 28.3 | 26.7 |
| avg MiB/s | 6.19 | 5.99 |

Per-run **ends:** Go `45/22/18`, Rust `25/27/28` — Rust beat Go on 2/3 ends.  
Go still wins **max** via early mid-run spikes (run1 hit 53).

Prior campaigns: `/tmp/peer-parity-reports/bunny-retain{,2,3}.jsonl`, `retain-SUMMARY.md`, `BRIEF.md`.

---

## Why Rust still has fewer peers (ranked)

Evidence from retain4 host.log / swarm samples:

1. **~40% of `dialOk` are junk DHT peers** on ports `:20027`/`:20041`/`:20081` (Chinese ISP DHT). They handshake then `frame decode error: Connection reset` / write reset. Each burns a half-open slot. Multi-redial churn was fixed (fail_count survives `dial_ok`); **first** dial still costs a slot.

2. **~565–577 dial timeouts / 60s** with half-open stuck at **24/24**. Handshake success ≈ 50–60 → ~9% conversion. Large DHT `poolSize` (~180–220) keeps `reducedDialTimeout` near **min**.

3. **Early ramp gap** (esp. run1): Go `12→45` by t=25s while Rust `1→20`. That hole defines the max-peer gap even when end counts converge.

4. **Fewer uTP established** on spike runs: Go run1 `23 uTP + 22 TCP`; Rust ~`9–12 uTP + 15–16 TCP`. First-wins helped vs old TCP-prefer; still behind Go’s best uTP runs.

5. **PEX barely dials:** `pexReceived` 1–2, `pexDialed` 0–1. Almost all dials are `DhtGetPeers`.

Not the limit: max-peers 55, pool emptiness, or speed.

---

## Key code map

| Area | Path |
|------|------|
| Dial pool / fail backoff / unhelpful close | `engine-rs/crates/wt-core/src/torrent.rs` (`take_dial_candidates`, `dial_finished`, `maybe_penalize_unhelpful_close`, `clear_dial_penalty`, `dial_pending_ready`) |
| `peer_closed` + helpful hook | `engine-rs/crates/wt-core/src/torrent_impls.rs` |
| Dialer loop (no requeue) | `engine-rs/crates/wt-core/src/client.rs` (~dial task) |
| Transport race first-wins | `engine-rs/crates/wt-core/src/conn/dialer.rs` |
| Post-HS messages / request wake | `engine-rs/crates/wt-core/src/peer_conn.rs` (`start`, `on_wake`, `note_peer_helpful`) |
| Discovery mid-run feed | `engine-rs/crates/wt-discovery/` + client want_peers |
| Go reference | `engine-go/patches/torrent/` (`client.go` dialPool/getFirst/sendInitialMessages, `prioritized-peers.go`, `torrent.go` dropConnection) |

Unit tests to keep green:
- `cargo test -p wt-core --lib dial_parity`
- `cargo test -p wt-core --test peer_conn_handler_test`
- `cargo test -p wt-core --test conn_test dial_race`

---

## Binaries & measure loop

```bash
# Rust release host
cd /home/kar/orca/workspaces/torrent-player/rust-engine/engine-rs
export CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/codex CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0
cargo build --release -p wt-engine --bin engine-host
# → /home/kar/.cache/wt-targets/codex/release/engine-host

# Go host (existing)
# /home/kar/.cache/wt-tmp/engine-go-host

# Magnet
MAGNET=$(cat /tmp/bunny-magnet.txt)   # Big Buck Bunny smoke.sample

# One 60s run
pkill -f 'engine-host|engine-go-host' 2>/dev/null || true
cd /home/kar/orca/workspaces/torrent-player/rust-engine
python3 engine-rs/tools/smoke/peers_compare.py \
  /home/kar/.cache/wt-targets/codex/release/engine-host \
  rust-bunny-<label> "$MAGNET" 60

python3 engine-rs/tools/smoke/peers_compare.py \
  /home/kar/.cache/wt-tmp/engine-go-host \
  go-bunny-<label> "$MAGNET" 60
```

Results land in `~/.cache/wt-tmp/peers-<label>/{result.json,host.log,diagnostics.jsonl}`.  
Compare `peers_min_max`, last-sample / `conn_summary.n`, `swarm.closes`, `dialOk`, timeouts, and `conn_summary.transports`.

Prefer **3× interleaved** Go/Rust. Success: Rust mean **max** peers ≥ Go mean max (and speed not regress).

---

## Next work for Codex (suggested order)

### A. Stop burning half-open on known-junk DHT (highest leverage)

- After `dial_ok` + immediate unhelpful RST on the same addr, fail_count already escalates.
- Still dials junk **once** per backoff window because DHT re-adds them.
- Port Go spirit: stronger postponement / lower priority for contacts that repeatedly die with 0 useful bytes; or avoid treating “handshake then instant RST” as a full `dialOk` success for pool health.
- **Do not** hardcode IP lists. Prefer behaviour from fail history / source / connection lifetime already in `dial_fail_counts` + `maybe_penalize_unhelpful_close`.
- Tight unit test: after N unhelpful closes, rediscovery must not consume half-open until backoff (already partly true); extend so **fresh** DHT flood of same dead addrs cannot monopolize all 24 slots ahead of never-tried non-junk.

### B. Early dial conversion / reducedDialTimeout

- With `poolSize`≈200, `reduced_dial_timeout` stays near min → mass timeouts.
- Go `peers.Len()` shrinks after PopMax; we removed requeue (`f3b4932`) but DHT mid-run feed still keeps pending huge.
- Investigate matching Go’s pending signal for timeout only (not deleting discovery), without reintroducing the old requeue timeout clamp.
- Measure: timeouts/60s should drop and early `peers` at t=15–25 should rise toward Go.

### C. PEX dial path

- `pexDialed≈0` while `pexReceived≥1`. Trace ingest → `add_peers(PeerSource::Pex)` → `take_dial_candidates`. Same-addr replace vs DHT may leave source as DHT so `pex_dialed` never increments—or PEX addrs never become dialable.
- Go comparison: `engine-go/patches/torrent/` PEX add path.
- Unit/integration already exist around PEX replace (`dial_parity` pex tests); extend if production path differs from tests.

### D. uTP established parity (only if A–C insufficient)

- SACK strip is in (`42e958b`). First-wins is in (`c36e75b`).
- If Go still holds ~2× uTP on spike runs, compare dial race spare-wait (250ms), uTP connect timeout, and whether losers are drained too aggressively.
- Guard with piece-download check: uTP peers must have `downloaded_gt0` (old TCP-prefer was because uTP often stayed at 0 bytes).

---

## Rules (do not violate)

1. Port Go behaviour; no magic constants “tuned until Bunny looks good.”
2. Unique `CARGO_TARGET_DIR` if parallel agents (`/home/kar/.cache/wt-targets/codex` is fine for solo).
3. Prefer unit/integration; at most essential public Bunny runs.
4. Commit on `karmugilen/rust-engine` (or `agent/...` then cherry-pick) with clear messages.
5. Update `/tmp/peer-parity-reports/` when you measure.

---

## Quick verify before coding

```bash
cd /home/kar/orca/workspaces/torrent-player/rust-engine
git log -1 --oneline   # expect 4faea3a or descendant
cd engine-rs && export CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/codex
cargo test -p wt-core --lib dial_parity
cargo test -p wt-core --test peer_conn_handler_test
```

Read first:
- `/tmp/peer-parity-reports/retain-SUMMARY.md`
- `/tmp/peer-parity-reports/BRIEF.md`
- This file

Agent reports from prior phases: `retain-backoff.md`, `retain-protocol.md`, `dialdisc.md`, `discovery.md`, `chokers.md`.

---

## Done criteria

1. 3× Bunny: Rust mean **max** peers ≥ Go mean max (same magnet, separate processes).
2. Speed: Rust mean avg MiB/s not materially below Go.
3. host.log: junk DHT addrs no longer dominate half-open; timeouts materially down or sticky dial_ok up.
4. Tests above green; no reintroduction of focus-wake flood or `dial_ok` clearing fail_count.
