# wt-parity port notes

Harness crate (not a line-by-line Go port). Drives `engine-go` / `wt-engine`
host binaries over the same HTTP control + stream API the Android app uses.

## Ported / implemented

- CLI: `wt-parity run`, `wt-parity compare`, `wt-parity api-diff`
- Control client for `/stats`, `/add`, `/configure`, `/play`, `/settings`,
  `/torrent/<id>`, stream `Range` GETs
- Deterministic fixtures via `wt-meta` (piece SHA-1, private torrents)
- Local swarm: N host seeders + latency/bandwidth TCP proxies (seek_search style)
- Seeder bootstrap: `.boundary/<hash>/<i>.part` + Go `.checkpoints` bitfield
- Scenarios: download, direct_play, cold_seek, magnet_metadata, resources,
  control_latency (`all`)
- `/proc` RSS + CPU sampling
- JSON results under `results/`; human summary in `BASELINE.md`
- Default work dir: `/home/kar/.cache/wt-tmp/wt-parity` (avoid `/tmp` tmpfs)

### api-diff (control API contract)

- `wt-parity api-diff --go <engine-go-host> --rust <engine-host>`
- Builds hosts when missing / with `--build` (Go: `nosqlite,noboltdb`; Rust:
  `CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/wt-parity`)
- Spawns both on localhost, seeds a tiny fixture via Go store prep, drives every
  `handleControl` route (stats/add magnet+torrentData/torrent/select/configure/
  play/file-only/pause/resume/forget-peers/pieces/metadata/settings GET+POST/
  focus/remove/shutdown + preview-state/connections/bypass/cache/network + 4xx)
- Diffs HTTP status, JSON field names/types, null vs missing, array shapes,
  exact values for names/lengths/file lists (volatile: speeds, ports, ids)
- Writes `API_DIFF.md` (MATCH/DIFF per route); Kotlin-read fields from
  `EngineClient.kt` marked **CRITICAL**
- Fixtures under `/home/kar/.cache/wt-tmp/api-diff`; child processes cleaned up

## Differs from Go unit tests

- Out-of-process host binaries (not in-process `EngineServer` + FD descriptors)
- Peer injection via magnet `x.pe` through proxies (no `SetOutgoingTCPConnHook`)
- Must enable `seedingEnabled` on seeders (host default is off)
- Cold-seek medians across repeats understate a fully cold start (repeat 0 is
  slow; later repeats warm caches)
- api-diff compares two live hosts with separate data dirs; `/stats.path` and
  per-engine `id` are type-checked only

## Remains

- Rust `engine-host` control surface is partially wired; many api-diff CRITICAL
  rows are expected until agy-4 aligns JSON with Go (re-run api-diff to refresh
  counts)
- Optional: multi-file download scenario using `parity-multi` fixture
- Optional: force fully cold seeks between repeats (fresh leecher + swarm)
- Optional: tighter integration with wt-engine feature flags once the API lands
