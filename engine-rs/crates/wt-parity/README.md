# wt-parity

Go-vs-Rust parity and performance harness for Webtor engine **host** binaries.

It drives any engine that speaks the same localhost HTTP control API as
`engine-go` (`-port`, `-path`, `-max-peers`) and the companion stream server
(`/torrent/{id}/file/{index}` with HTTP Range).

## Build

Use an on-disk target dir (not `/tmp` tmpfs):

```bash
cd engine-rs
mkdir -p /home/kar/.cache/wt-targets/wt-parity /home/kar/.cache/wt-tmp/wt-parity
CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/wt-parity CARGO_BUILD_JOBS=3 \
  cargo build -p wt-parity --release
```

## Go host baseline

```bash
cd engine-go
go build -tags=nosqlite,noboltdb -o /home/kar/.cache/wt-tmp/engine-go-host .

cd ../engine-rs
WT_PARITY_SCALE=bounded CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/wt-parity CARGO_BUILD_JOBS=3 \
  cargo run -p wt-parity --release -- \
  run --engine /home/kar/.cache/wt-tmp/engine-go-host --label go --scenario all \
  --repeats 5 --seeders 4 \
  --work /home/kar/.cache/wt-tmp/wt-parity \
  --out crates/wt-parity/results/go-all-new.json
```

Replace `results/go-all.json` only after the new run exits successfully and all
scenarios have five samples. Numbers are summarized in [`BASELINE.md`](BASELINE.md).

## Rust host

`engine-host` in `wt-engine` serves the control API and transfers between hosts.
Use the same scale, repeats and seeder count for both engines:

```bash
CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/wt-engine CARGO_BUILD_JOBS=3 \
  cargo build -p wt-engine --release --bin engine-host

WT_PARITY_SCALE=bounded CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/wt-parity CARGO_BUILD_JOBS=3 \
  cargo run -p wt-parity --release -- \
  run --engine /home/kar/.cache/wt-targets/wt-engine/release/engine-host \
  --label rust --scenario all --repeats 5 --seeders 4 \
  --work /home/kar/.cache/wt-tmp/wt-parity \
  --out crates/wt-parity/results/rust-all.json

CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/wt-parity CARGO_BUILD_JOBS=3 \
  cargo run -p wt-parity --release -- \
  compare crates/wt-parity/results/go-all.json crates/wt-parity/results/rust-all.json
```

`compare` prints a table of medians and ratios. Exit code `1` means Rust is
worse than Go beyond the noise tolerance (default 5%).

## Control API contract (`api-diff`)

Diffs every `engine-go` `handleControl` route between Go and Rust hosts and
writes `API_DIFF.md`. Kotlin-parsed fields (`EngineClient.kt`) are marked
CRITICAL when they differ.

```bash
# Build hosts (one at a time)
cd engine-go && go build -tags=nosqlite,noboltdb -o /home/kar/.cache/wt-tmp/engine-go-host .
cd ../engine-rs
CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/wt-parity CARGO_BUILD_JOBS=3 \
  cargo build -p wt-engine --bin engine-host

CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/wt-parity CARGO_BUILD_JOBS=3 \
  cargo run -p wt-parity -- api-diff \
  --go /home/kar/.cache/wt-tmp/engine-go-host \
  --rust /home/kar/.cache/wt-targets/wt-parity/debug/engine-host \
  --work /home/kar/.cache/wt-tmp/api-diff \
  --report crates/wt-parity/API_DIFF.md \
  --repo ..
```

Pass `--build` to rebuild both hosts before the run. Localhost only; children
are shut down and data dirs under `api-diff` are removed afterward.

## Scenarios

| Name | What it measures |
|---|---|
| `download` | Full download throughput (MiB/s) |
| `direct_play` | Stream TTFB (ms) + sequential read rate (MiB/s) |
| `cold_seek` | 20 random seeks to first 64 KiB; p50/p90/max + Go-style score |
| `magnet_metadata` | Magnet → metadata ready (ms) |
| `resources` | Peak RSS (MiB) + CPU seconds for download and Direct Play |
| `control_latency` | `/stats` and `/torrent/<id>` latency under load (ms) |
| `all` | Runs every scenario above |

`WT_PARITY_SCALE=bounded` uses 64 MiB video and 32 MiB seeks, preserving seeks
beyond the 16 MiB cache while keeping fixtures and one leecher below 256 MiB.
Faster iteration: `WT_PARITY_SCALE=small` or `tiny` (smaller fixtures).
Samples preserve repeat order: report repeat 0 separately from repeats 1–4.
Each repeat has a fresh leecher, but the seeders and host filesystem cache are
shared; repeat 0 does not imply a dropped OS page cache.

## Local swarm (no internet)

- Fixtures are built with `wt-meta` (deterministic PRNG payload + SHA-1 pieces).
- Default scale: 256 MiB video-like file, 32 MiB seek file, 256 KiB pieces.
- Torrents are `private=true` with no announce list.
- N host processes seed from pre-written `.boundary` parts + a Go checkpoint so
  configure restores completions; `seedingEnabled` is turned on via `/settings`.
- Each seeder is fronted by a localhost TCP proxy with 20–199 ms one-way delay
  and one of four upload caps (40 / 150 / 400 / 1024 KiB/s), matching
  `seek_search_test.go`.
- Leecher magnets use `x.pe` peer hints only (no public trackers / DHT).

## Compare demo (Go vs Go)

```bash
cp crates/wt-parity/results/go-all.json crates/wt-parity/results/go-all-copy.json
CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/wt-parity CARGO_BUILD_JOBS=3 \
  cargo run -p wt-parity --release -- \
  compare crates/wt-parity/results/go-all.json crates/wt-parity/results/go-all-copy.json
```

See also [`PORT_NOTES.md`](PORT_NOTES.md) for what is implemented vs remaining.
