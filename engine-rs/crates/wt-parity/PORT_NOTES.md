# wt-parity port notes

Harness crate (not a line-by-line Go port). Drives `engine-go` / `wt-engine`
host binaries over the same HTTP control + stream API the Android app uses.

## Ported / implemented

- CLI: `wt-parity run` and `wt-parity compare`
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

## Differs from Go unit tests

- Out-of-process host binaries (not in-process `EngineServer` + FD descriptors)
- Peer injection via magnet `x.pe` through proxies (no `SetOutgoingTCPConnHook`)
- Must enable `seedingEnabled` on seeders (host default is off)
- Cold-seek medians across repeats understate a fully cold start (repeat 0 is
  slow; later repeats warm caches)

## Remains

- Rust `engine-host` is still a stub (`engine not wired yet`); baseline is
  Go-only until agy-4 lands a working host
- Optional: multi-file download scenario using `parity-multi` fixture
- Optional: force fully cold seeks between repeats (fresh leecher + swarm)
- Optional: tighter integration with wt-engine feature flags once the API lands
