# metadata + pex + ratelimit port notes

Owned paths: `src/metadata/`, `src/pex/`, `src/ratelimit/`, `tests/meta_pex_rate*.rs`.
`lib.rs` gained exactly `pub mod metadata;`, `pub mod pex;`, `pub mod ratelimit;`.

## Ported

### metadata/ (BEP 9 ut_metadata)
- Size negotiation (`set_size`, max 16 MiB)
- 16 KiB piece assembly, random request order helper
- Verify via `wt_meta::parse_info` + `info_hash_v1`/`info_hash_v2` against magnet expectations (v1-only, v2-only, hybrid)
- Invalidate on bad hash; serve Data/Reject once complete
- Per-peer `PeerMetadataState` for outstanding piece requests

### pex/ (BEP 11)
- Event feed with add/drop, hold-back when `nc < 25`
- `genmsg(None)` uncapped initial snapshot; incremental capped at `PEX_MAX_DELTA` (50)
- Add/drop cancel-out in message factory
- Ingest → `PeerInfo { source: Pex }` with 45s per-conn spacing + 60s torrent cooldown
- `PexConnView` for cursor / remote-live bookkeeping

### ratelimit/
- Token bucket matching `golang.org/x/time/rate`: Inf, burst, `allow_n`, `wait_n`, `reserve_delay`, `set_limit`/`set_burst`
- Default 1 MiB burst helper for download/upload

## Differs
- PEX timers/IO stay in peer_conn; modules are pure + injected `Instant`
- Metadata does not open storage or touch the peer map (agy-2)
- Limiter is a small local impl (not a crate wrapping x/time/rate)

## Remains (agy-2)
- Wire extended-handshake `metadata_size` → `set_size` + request loop
- Send/receive ut_metadata / ut_pex Extended messages
- Tick dial/download/upload limiters on the hot paths
