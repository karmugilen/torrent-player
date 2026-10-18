# Discovery CONTEXT

Agent map for the local **Torrent Atlas** collector. Point here when changing discovery naming, dump/resolve, search UI, or `:18181` ops. Human-facing product copy lives in `README.md`; measured dumps and Rust crate notes in `engine-rust/discovery/README.md`.

## What it is

Local-only metadata atlas: bounded DHT discovery, public feeds, hash→name resolve and world map UI. **No movie payload download/upload.** Listen `http://127.0.0.1:18181`. Store is streaming JSON (no SQL).

```sh
./scripts/discovery.sh start|stop|status   # from repo root
```

Binary + PID + log + `discovery.json` + `torrents.csv` under `~/.cache/torrent-player/` (or `$XDG_CACHE_HOME/torrent-player/`).

## Glossary

| Term | Meaning |
|------|---------|
| **Observation** | One infohash row: name, peers, countries, source, timestamps. |
| **Name apply** | Single merge (`applyNameLocked` / `namePatch`) for dump/catalog/cache/peer → Observation. |
| **Pending set** | Unresolved hashes for O(pending) job picks; not a full-store scan. |
| **Dump index** | Local `torrents.csv` hash→name. Lookup only; store mutation stays in Go apply. |
| **Windowed view** | Ranked API/browser window (`limit` 150–600); full store stays on disk. |
| **Match count** | Total rows matching current filters; drives “Load more”. |
| **rustdump** | Go build tag linking `engine-rust/discovery` mmap dump + catalog HTTP. |

## Name sources (priority in practice)

1. **Local dump** (`torrents.csv`, ~175MB, ~1.16M rows, sorted by infohash) — main volume; refresh ~24h.
2. **Catalog** — `https://torrents-csv.com/service/search` (paced workers).
3. **HTTPS .torrent caches** — race mirrors; exact infohash; public torrents only.
4. **Peer BEP 9** — after cache miss; bounded dial limits.
5. **Public feeds** — SubsPlease, Nyaa and EZTV, checked every five minutes with
   conditional requests. Titles remain `NameHint` / `Resolved=false` until exact
   metadata or a catalog matches. Feed tracker hints, publisher `.torrent` links
   and fresh token-validated DHT announcements can bypass generic mirror misses.
6. **Additional local CSV / CSV.GZ** — streamed from `cache/catalogs`, checked
   every five minutes. Columns: `infohash` / `hash` / `info_hash`, `name` / `title`,
   optional `size_bytes` / `size`. Imported catalog labels are not peer verified.

Already-**Resolved** hashes are never re-scheduled for cache/peer. New DHT samples call dump lookup first (`tryResolveHashFromDump`).

## Layout

| Path | Role |
|------|------|
| `main.go` | Process, HTTP, JSON load/prune, dashboard fields |
| `apply.go` | Name apply seam + search index update |
| `dump.go` | Dump ingest/seed/pending match (Go orchestration) |
| `dump_rust.go` | cgo → Rust dump/catalog (`//go:build rustdump`) |
| `dump_go.go` | Pure-Go stubs (`//go:build !rustdump`) |
| `catalog.go` | Remote catalog workers |
| `cache.go` / `collect.go` / `sampling.go` / `schedule.go` | DHT sample, cache/peer workers, job priority |
| `api_view.go` | Windowed `/api/state`, sort/filter, search candidates |
| `rates.go` | Rolling lookup/live/import/verification rates |
| `bulk.go` | Bounded read-only hash/magnet batch resolution; independent indexed-capacity measurement |
| `feeds.go` / `trackers.go` / `announcements.go` | Provisional feed intake, associated tracker/publisher lookup, fresh endpoint fast path |
| `local_catalogs.go` / `restore.go` / `source_meter.go` | Additional CSVs, streaming restore, per-mirror outcome and latency metrics |
| `search_index.go` / `search_rust.go` | Native compact postings with Go fallback for fast `q=` |
| `ui.html` | Map, search, trending, paging, shareable filters, magnet copy |
| `../../../engine-rust/discovery/` | Rust mmap index + dump download + catalog fetch |

## Build

`scripts/discovery.sh start` runs `cargo build --release` in `engine-rust/discovery`, then:

```text
go build -tags=nosqlite,noboltdb,rustdump -o ~/.cache/torrent-player/discovery ./cmd/discovery
```

If the Rust `.a` is missing, tags drop to `nosqlite,noboltdb` (Go dump HashMap path).

Tests:

```sh
cd engine-go/cmd/discovery
go test -tags=nosqlite,noboltdb ./...
go test -tags=nosqlite,noboltdb,rustdump ./...   # needs libwebtor_discovery.a
```

## Store and API

- **Retention**: count unlimited (`retainedTorrentLimit = 0`). Age: named ~30d, unresolved ~24h on load.
- **Persist**: full map → `discovery.json` (can be hundreds of MB). Startup load can take tens of seconds; script waits up to ~90s on `/api/state?limit=1`.
- Restore decodes one record at a time. Save streams 512-record batches to an
  fsynced temporary file and atomic rename; it does not clone/serialize the
  entire store in memory. Writers can run between snapshot batches.
- **`POST /api/resolve`**: `{ "hashes": [...] }`, 1–50,000 inputs, maximum 4 MiB,
  at most two concurrent requests. Accepts hex/base32 hashes and v1 magnets,
  deduplicates aliases, returns named matches, verification/source and misses.
  Uses saved names then the batched Rust CSV index. No network jobs, new-name
  counters or observations are created. The website accepts 100,000 hashes and
  splits them into 10,000-hash requests with a bounded preview and CSV export.
- **`GET /api/state`**: query `q`, `sort`, `kind`, `trend`, `pending`, `located`, `country`, `top`, `limit`, `offset`. Returns stats for the **full** store + a **window** of `records` + `matchCount`.
- Default window 150, max 600. UI **Next/Previous** requests a fixed 150-row window using `offset`, with shareable URL filters.
- Search uses token index when possible; ultra-common tokens keep a **capped** posting list (no full-scan fallback for those). Exact 40-hex `q` is a direct hash lookup.

## Rust dump (when `rustdump`)

- Mmap CSV; binary search on leading infohash; parse row fields on hit.
- Load ~180ms vs ~1400ms Go map (same ~1.16M rows); names stay in the mapped file.
- Seed walk uses a C callback; dashboard pointer stays in an atomic Go slot (no Go pointer through cgo ctx).
- Dump mutex is **not** held across Go apply callbacks (Arc clone before iterate).

## Invariants (keep these)

- Metadata-only: storage rejects payload; client `NoUpload` / no data download.
- One name-apply path for every source.
- Resolved ⇒ never cache/peer re-resolve.
- Browser never receives the full million-row store in one payload.
- Dump index does not mutate Observations by itself.
- Bounded DHT/peer rates; dump + catalog carry indexed naming volume.
- Provisional feed labels never count as verified names. Indexed batch matches
  never inflate live lookup/new-name counters.

## Touch map (common tasks)

| Task | Start here |
|------|------------|
| Faster / richer dump resolve | `engine-rust/discovery`, `dump.go`, `dump_rust.go` |
| Catalog pacing / queries | `catalog.go` (+ Rust fetch when tagged) |
| Search speed / ranking window | `search_index.go`, `api_view.go`, `ui.html` |
| Job priority / skip resolved | `schedule.go`, `collect.go` |
| Retention / persist | `main.go` (`newDashboard`, `persistAll`) |
| Ops / restart | `scripts/discovery.sh`, log `~/.cache/torrent-player/discovery.log` |

## Out of scope here

- Android app / `engine-go` playback / piece FFI (`engine-rust` pieces crate).
- Closed Rust engine A/B tree (`torrent-player-rust` / librqbit).
- Publishing the atlas beyond localhost.

## Current performance and UI paths

- Rust owns compact document IDs and token/prefix postings. Go submits normalized
  names in ~128KB batches; native queries return only candidate hashes. Pure-Go
  builds retain the Go token index.
- Pending dump matching crosses FFI once per batch, with up to four Rust mmap
  lookup workers. Go owns the common name-apply merge. Catalog TLS connections
  are reused in the native adapter.
- API ranking deep-copies only the selected window. Stable hash tie-breaks keep
  pagination consistent; phrase punctuation, reordered terms, accents, and
  magnets work through the server search path.
- Per-minute rates use a rolling 60-second window: completed network lookups,
  fresh/live names, index imports, and peer/cache verified names are separate.
  Disk restores and repeated imports do not inflate live naming volume.
- A missed cache lookup hands off to the peer pool; cache workers cannot reclaim
  that hash until the peer attempt finishes. Peer jobs reuse the miss rather than
  repeating the same HTTP work. Resolved rows are never re-scheduled.
- Full JSON persistence is serialized and runs every two minutes plus explicit
  pause/shutdown/seed completion; it previously ran every ten seconds.
- Resolver dispatch uses bounded 128-candidate batches per fresh/retry pool,
  refreshed every 250ms; every dispatch rechecks the live record. Cache misses
  hand off directly to the peer batch. Both stages count as one retry attempt.
  Recently reachable swarms outrank overdue empty retries, with retry fairness
  maintained independently in each worker pool.
- Cache workers skip admission while all mirrors are cooling down. Peer workers
  bypass offline mirrors so fresh DHT samples do not wait for mirror recovery.
  Unhealthy mirrors back off for up to 60s, then admit one recovery probe;
  healthy mirrors keep full concurrency. API source stats expose cooldown/probe.
  A cooldown race or timeout waiting for source slots is not a completed
  network lookup or a healthy miss; it retries cache work without consuming a hash retry attempt.
- HTTP admission bounds active hash races to 48, with 96 cache workers. Queue
  waiting no longer consumes the 2.5s network deadline.
  There are 64 peer metadata jobs with an independent 24-traversal DHT budget;
  DHT admission waits do not consume the 12s metadata-fetch deadline (45s total
  queue residence bound). The 24 traversals share a 256-query/s wire budget;
  traversal and dial budgets are raised together so jobs do not just wait longer.
  Known endpoints dial immediately, with a 350ms hedge before DHT fallback.
  Traversals release admission after 12 unique endpoints or 750ms after the
  first peer reply; empty traversals end after 3s. Metadata fetching continues
  independently for up to 12s. Tracker results stream individually; one slow
  tracker never blocks another's peers. Metadata completion immediately releases
  the job instead of waiting another 1.2s for geography. Stalled connected peers
  yield after 4s without metadata progress once source discovery finishes.
  `peerActive`, `peerCapacity`, `dhtActive`, `dhtCapacity`, and
  `peerMetadataChunks` expose pipeline activity in the API and website.
  Cold jobs without cached endpoints/publisher hints have a separate 24-job
  admission limit, released as soon as peers arrive. Cached endpoints get a
  priority boost across fresh/retry queues; ordinary backlog retains a fair share.
  Fresh sampled hashes with a reporting node outrank old blind backlog. When
  reporting nodes or associated trackers supply enough endpoints, the redundant
  full traversal is canceled, including its admission wait.
  `dhtQueued`, `dhtLookups`, `dhtPeerLookups`, `averageDhtMs`,
  `peerMetadataActive`, `peerConnected`, and `peerDialing` distinguish actual
  endpoint fetching and socket activity from jobs waiting for DHT.
  Refreshed feed associations can wake old blind failures after at least a minute.
  DHT sends are capped at 256/s, peer dials at 96/s. Metadata jobs can race six
  peer addresses per swarm within 192 global half-open and three
  established-per-swarm limits. The API exposes `dhtQueryLimit`, `peerDialLimit`,
  and `peerHalfOpenLimit` alongside actual socket activity. Empty unresponsive swarms
  release after traversal admission plus 3s; progressing peers keep the 12s
  metadata deadline.
- Per-mirror HTTP 404/410 miss caches expire after 2 minutes and cap at 8,192
  hashes each. Cached handoffs never inflate network lookup rates. API fields
  `cacheRequests`, `cacheMissReuses`, `cacheActive`, `cacheCapacity` expose actual
  HTTP activity and admission occupancy.
- CSV misses are remembered per loaded dump. Pending matching skips unchanged
  pending revisions; loading a new dump automatically invalidates old misses.
- Rust catalog parsing uses serde_json, preserves Unicode/escaped punctuation,
  bounds response size, and rejects HTML/malformed responses for Go fallback.
  Exact cache/peer metadata already in flight can correct an imported name
  without scheduling a new lookup or counting the hash as a second new name.
- Search/filter requests abort older requests and ignore stale responses. Source
  filters, paging, and URL state are handled on the server; geography refreshes
  on country selection. Adult-name hiding redacts titles without dropping rows.
- DHT responds on UDP `:18182` (`--dht-listen`; falls back to an ephemeral port
  if busy) with bounded expiring peer storage. Only token-validated announcements
  enter the endpoint fast path; the announcement count reports actual inbound
  traffic, not claimed reachability. PEX is enabled; payload I/O remains blocked.
- Tracker URLs come from associated public feed magnets, with at most two per
  job, 16 active requests, 64 reusable endpoints, and per-hash tracker intervals.
  Unknown sampled hashes are not broadcast to a generic tracker list. Endpoints
  resolve to public IPv4 addresses; publisher metadata requires the exact hash.
- Mirrors expose requests/hits/misses/failures/cancellations and approximate p95
  latency. HTTP 403, challenges, malformed metadata and transport failures back
  off; only actual 404/410 responses populate the per-hash miss cache.
- `TORRENT_ATLAS_FEEDS` overrides the space-separated HTTPS feed list (empty
  disables it). The launch script forwards this setting to systemd. Local
  catalogs can be relocated with `--catalog-dir`.
- Reproduce real indexed HTTP capacity with `python scripts/benchmark-discovery.py`.
  It samples 100,000 distinct hashes across the CSV and validates nonempty named
  responses. This measures reusable labels, not new peer metadata per minute.

### Live metadata measurement, 2026-10-18

After the admission/slot fixes with the old network budgets, a 64.1s window
resolved six new peer names (5.6/min). With aligned 256 DHT queries/s and
96 peer dials/s, a subsequent 64.2s window resolved 25 new peer names
(23.3/min), with zero payload bytes. Both windows excluded their initial 30s
warmup and had no new cache names during measurement. These are live network
observations, not guaranteed throughput; peer availability changes between runs.
