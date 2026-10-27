# Go engine baseline (wt-parity)

Captured with `wt-parity run --engine /tmp/engine-go-host --label go --scenario all
--repeats 3 --seeders 4` on 2026-10-27. Full samples:
`results/go-all.json`.

Host build: `cd engine-go && go build -tags=nosqlite,noboltdb -o /tmp/engine-go-host .`

Local swarm: 4 seeders behind latency/bandwidth proxies (20–199 ms delay;
40 / 150 / 400 / 1024 KiB/s). Private torrents, magnet `x.pe` peers only.
Default scale: 256 MiB video fixture, 32 MiB seek fixture, 256 KiB pieces.

## Medians (n=3 unless noted)

| Scenario | Metric | Median | Unit | Notes |
|---|---|---:|---|---|
| (a) download | throughput | **19.47** | MiB/s | samples 2.91 / 50.08 / 19.47; first run cold |
| (b) direct_play | TTFB | **30.18** | ms | |
| (b) direct_play | sequential read | **51.76** | MiB/s | 8 MiB Range read after play |
| (c) cold_seek | p50 | **3.63** | ms | noisy across repeats (see below) |
| (c) cold_seek | p90 | **24.51** | ms | |
| (c) cold_seek | max | **218.25** | ms | |
| (c) cold_seek | score `(p50+p90)/2` | **14.07** | ms | mirrors `seek_search_test.go` |
| (d) magnet_metadata | time to ready | **308.2** | ms | ut_metadata from local seeders |
| (e) resources / download | peak RSS | **173.6** | MiB | `/proc` during full download |
| (e) resources / download | CPU | **1.97** | s | |
| (e) resources / direct_play | peak RSS | **35.3** | MiB | |
| (e) resources / direct_play | CPU | **0.07** | s | |
| (f) control_latency | `/stats` | **0.364** | ms | n=591 samples under load |
| (f) control_latency | `/torrent/<id>` | **0.292** | ms | n=591 |

## Cold-seek noise

Repeat 0 is a true cold seek against missing data (`score≈885 ms`). Repeats 1–2
are much faster (`score≈14` / `5`), likely because piece caches / peer state from
the shared swarm warm the path. The reported median therefore understates a
fully cold start. When comparing Rust, prefer matching the same repeat pattern
or looking at repeat-0 alone for seek latency.

## Rust host

`wt-engine`'s `engine-host` binary (`src/bin/main_host.rs`) currently parses
`-port`/`-path`/`-max-peers`, prints `engine not wired yet`, and waits for
SIGINT — the control/stream API is not implemented yet (agy-4). No Rust
baseline was collected. Re-run when the host serves the same routes as
`engine-go/server.go`.

```bash
CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/wt-engine CARGO_BUILD_JOBS=3 \
  cargo build -p wt-engine --release --bin engine-host
CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/wt-parity CARGO_BUILD_JOBS=3 \
  cargo run -p wt-parity --release -- \
  run --engine …/engine-host --label rust --scenario all \
  --out crates/wt-parity/results/rust-all.json
CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/wt-parity CARGO_BUILD_JOBS=3 \
  cargo run -p wt-parity --release -- \
  compare crates/wt-parity/results/go-all.json crates/wt-parity/results/rust-all.json
```

## Compare demo (Go vs Go)

```bash
cp crates/wt-parity/results/go-all.json crates/wt-parity/results/go-all-copy.json
cargo run -p wt-parity --release -- \
  compare crates/wt-parity/results/go-all.json crates/wt-parity/results/go-all-copy.json
# expect PASS (ratio ≈ 1.0), exit 0
```
