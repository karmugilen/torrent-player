# webtor-discovery (Rust)

Fast torrents.csv dump index and catalog HTTP for Torrent Atlas (`engine-go/cmd/discovery`).

## What it does

- **mmap + binary search** on the sorted `torrents.csv` dump (~1.16M rows). Load is ~8× faster than the Go `encoding/csv` HashMap path and keeps names in the mapped file instead of duplicating them in a Go map.
- **Dump download** when the local file is missing or older than 24h.
- **Catalog page fetch** against `torrents-csv.com` (JSON), used when built with the `rustdump` tag.

DHT / peer metadata (anacrolix) stays in Go.

## Build

```bash
cargo build --release
```

Produces `target/release/libwebtor_discovery.a` (and `.so`).

`scripts/discovery.sh start` builds this crate, then links discovery with `-tags=nosqlite,noboltdb,rustdump`. Without the staticlib, discovery falls back to pure Go (`!rustdump`).

## Measured (local, 175MB dump)

| Path | Load ~1.16M rows | 100k lookups |
|------|------------------|--------------|
| Go HashMap | ~1400 ms | ~2 ms |
| Rust mmap | ~180 ms | ~74 ms (binary search) |

Startup and daily refresh dominate; resolve correctness is unchanged.
