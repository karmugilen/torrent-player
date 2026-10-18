# Faster discovery and hash-to-name resolution

Research date: 2026-10-18. Baseline: `7f4f789`.
The main intake, lookup, tracker and memory improvements below are implemented.
See [measured results and usage](../engine-go/cmd/discovery/README.md#bulk-lookup-and-fresh-inputs).
A dedicated BEP 9 requester and append journal remain profiling-dependent options;
the existing requester remains in use, and snapshots now stream in small batches.

The aim is more unique names, with explicit verification status, per minute.
Completed failed checks, imported catalog labels and metadata verified against
an infohash must remain separate measurements.

## What the source code shows

| Project and inspected revision | Mechanism | Useful adaptation |
| --- | --- | --- |
| [Bitmagnet](https://github.com/bitmagnet-io/bitmagnet/tree/e31b30d7351c269dc801876f92ebc59d31fee545) | Bounded stages, batched database triage, direct requests to the node supplying a sample, then a dedicated metadata requester | Keep expensive metadata slots for hashes with useful peer evidence; batch duplicate checks |
| [Magnetico maintained fork](https://github.com/tgragnato/magnetico/tree/2eb0cea4c734b8a8a895896ba246bad8d8a4f825) | BEP 51 samples followed by get_peers against the responding node; a separate metadata leecher | Preserve the hash-to-source-node relationship and separate discovery from metadata work |
| [Webtor magnet2torrent](https://github.com/webtor-io/magnet2torrent/tree/232ed8a5be1a292cc2bf254ad67bcfdd7cbf295a) | Magnet trackers plus configurable default trackers; metadata obtained through anacrolix/torrent | Test tracker-assisted peer discovery using our existing library |
| [Webtor torrent-store](https://github.com/webtor-io/torrent-store/tree/a3592277070396bf162362f3af14e5b26ef7188d) | Hash-keyed cached operations, ordered storage providers and backfill into faster tiers | Persist successful metadata and collapse overlapping work for each hash |

Code evidence:

- Bitmagnet [triage](https://github.com/bitmagnet-io/bitmagnet/blob/e31b30d7351c269dc801876f92ebc59d31fee545/internal/dhtcrawler/infohash_triage.go), [get_peers](https://github.com/bitmagnet-io/bitmagnet/blob/e31b30d7351c269dc801876f92ebc59d31fee545/internal/dhtcrawler/get_peers.go), [requester](https://github.com/bitmagnet-io/bitmagnet/blob/e31b30d7351c269dc801876f92ebc59d31fee545/internal/protocol/metainfo/metainforequester/requester.go).
- Magnetico [indexing service](https://github.com/tgragnato/magnetico/blob/2eb0cea4c734b8a8a895896ba246bad8d8a4f825/dht/mainline/indexingService.go), [metadata leecher](https://github.com/tgragnato/magnetico/blob/2eb0cea4c734b8a8a895896ba246bad8d8a4f825/metadata/leech.go). The original boramalper repository is archived; this is a different, maintained fork. Its announcement handler is not itself a direct metadata fast path, so that proposed addition below is our design, not a claim about this fork.
- Webtor [resolver](https://github.com/webtor-io/magnet2torrent/blob/232ed8a5be1a292cc2bf254ad67bcfdd7cbf295a/server/main.go), [store](https://github.com/webtor-io/torrent-store/blob/a3592277070396bf162362f3af14e5b26ef7188d/services/store.go).

These are code patterns, not benchmarks establishing equivalent speed here.
Webtor's tracker success figures in source comments describe its deployment;
our long-lived client and short job deadlines are different.

## Our present constraints

- `collect.go` sets `Passive=true`, `DisableTrackers=true`, `DisablePEX=true`.
  UDP binds an ephemeral port. Merely removing Passive does not establish inbound
  reachability or produce a stream of announcements.
- We already query sample suppliers, cache peer hints, use separate cache/peer
  pools, deduplicate in-flight hashes and perform Rust batch dump lookups.
  Reimplementing those mechanisms offers little new benefit.
- There are 24 peer jobs and a global 24 peer dials/second limit. That permits
  roughly 1,440 sustained dial starts/minute, before failures and multiple
  candidates per hash. Higher completed HTTP check counts do not remove this
  constraint on fresh peer metadata.
- Some HTTP mirrors return challenges/errors. More concurrency can increase
  completed failures while yielding few additional names.
- Full JSON snapshots copy hundreds of MB; this is a separate memory and UI
  latency concern. The service was stopped when checked during this research;
  no new live throughput baseline was measured.

## Implementation order

1. **Attribute outcomes before changing budgets.** In `rates.go`, `cache.go`,
   `collect.go` and `ui.html`, expose each source's attempts, unique new hashes,
   peers found, verified names, failures, bytes and latency percentiles. Treat
   challenge HTML as an unusable response, not a healthy confirmed miss. Use
   source cooldowns and retain a small probing allowance for recovery.

2. **Add tracker-assisted lookup alongside DHT.** Create a bounded tracker
   adapter under `cmd/discovery`, using the patched library's tracker clients.
   Start with tracker URLs carried by public feeds/magnets, reuse clients and
   UDP connection IDs, and race at most two selected tracker endpoints against
   DHT. Reuse public-list cache/validation patterns from `engine-go/trackers.go`.
   Retain the existing rule that generic supplemental trackers require known
   public status; do not silently widen that policy for unknown magnets.
   Respect tracker intervals and join cancelled work. Count tracker endpoints
   as a peer-discovery source; names obtained from their peers remain `peer`
   verified. A scrape supplies activity counts, not names; use it for optional
   batch ranking only where a tracker association exists, never as proof a hash
   is globally dead. See [BEP 15](https://www.bittorrent.org/beps/bep_0015.html).

3. **Give fresh endpoints a direct route to metadata.** Add a stable configurable
   UDP listener with an ordinary responding DHT node, bounded peer store and
   token-validated `OnAnnouncePeer` intake. Enqueue validated public endpoint/hash
   pairs without waiting for an HTTP miss. Keep sampling as the fallback. Show
   observed inbound announcement rate; if the port is unreachable or receives
   few announcements, do not promise a gain. Existing deduplication, rate limits
   and peer-hint expiry must cover this new intake.

4. **Add fresh named inputs independently.** Build conditional RSS/Atom/magnet
   imports, storing hash, title hint, date, size and tracker hints. The research
   probe of [SubsPlease RSS](https://subsplease.org/rss/?r=1080) returned 50 entries
   with magnet links; that demonstrates one working input, not broad coverage.
   Normalize hex and base32 btih. A title hint is searchable/displayable but
   remains provisional (`Resolved=false`) until exact metadata arrives; it
   must not inflate verified-name counts. Apply the eventual verified name
   through `applyNameLocked`. Do not reschedule existing resolved catalog rows.

5. **Reduce metadata overhead where measurement supports it.** Benchmark a small
   BEP 9 requester against the existing torrent client, following Bitmagnet's
   focused request/parse/hash-check design. Pipeline metadata chunks with bounded
   request windows, enforce message limits, reject duplicate/invalid chunks and
   cancel losing candidates. Keep the current client as a fallback for protocol
   compatibility. Enable bounded PEX on that path and measure its additional
   peers: PEX is for the same swarm, and an immediate update is not guaranteed
   within our 12-second job. See [BEP 9](https://www.bittorrent.org/beps/bep_0009.html)
   and [BEP 11](https://www.bittorrent.org/beps/bep_0011.html).

6. **Persist successes incrementally.** Introduce a local metadata cache keyed by
   exact hash and a batched append journal with periodic compaction. Store
   verification/provenance and restore positive entries without network work.
   Reuse the current in-flight deduplication across new sources. Prototype
   incremental observation persistence before replacing the entire JSON store;
   measure restart time, snapshot pause and peak RSS. SQLite WAL is an alternative
   if indexed updates justify another storage dependency. Neither Redis nor S3
   is needed for the single-PC deployment.

7. **Tune concurrency after measuring yield.** Keep asynchronous network work in
   Go and the existing Rust mmap/search/bulk matching path. Profile decode,
   hashing, indexing, allocation and lock time before further ports. A Rust
   decoder is worth adopting only if CPU measurements justify FFI complexity.
   GPU work does not address peer reachability or remote response latency; no
   measured CPU hotspot currently justifies a GPU implementation. Increase
   budgets only when unique verified names/minute rise without runaway memory,
   rate errors or UI latency.

## Validation and acceptance

- Use controlled tracker, DHT and BEP 9 fixtures to prove cancellation, duplicate
  suppression, interval enforcement, exact-hash validation and zero payload I/O.
- Test provisional-title replacement, base32 magnets and source counters without
  changing the existing resolved-record scheduling invariant.
- Run the existing Go race suites with and without `rustdump`; run Rust tests
  when Rust changes. Exercise search, pagination and map filtering.
- Compare alternating 15-minute runs with each feature on/off, recording input
  novelty and source mix. Report fresh verified names/min, named hints/min,
  completed jobs/min, p95 latency, bytes, CPU, RSS and API latency separately.
- First aim for a repeatable improvement in fresh verified naming yield on this
  PC. Neither the inspected projects nor this research establishes 100,000 fresh
  remote names/minute. Bulk local matching is a different benchmark and must be
  labelled accordingly.
