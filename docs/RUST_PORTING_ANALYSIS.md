# Rust porting assessment

Source review, 2026-10-25. No tests, benchmarks, or device profiling were run.
Priorities below are estimates from ownership, allocation, iteration and locking
in the current code, not measured performance gains.

## Recommendation

The strongest substantial Rust candidate is a coherent piece-cache and storage
module. The best smaller candidates are bulk bitmap operations and playback
priority planning. Network scheduling, Android lifecycle and rendering should
remain with the code that currently owns them.

Changing language alone will not fix read-ahead policy, a slow peer, disk latency
or a mutex held across I/O. Ports should change the data representation or
ownership responsible for the cost and expose a small, batched interface.

## What already uses Rust

- `engine-rust/src/lib.rs`: aggregation of compressed piece runs into UI buckets.
- `engine-rust/src/hash.rs`, through `extent_rust.go`: sparse file extent hashing
  and pread of file spans. Saved-data checking uses the native hash path when
  appropriate; ordinary peer-piece verification still uses the Go library.
- `engine-rust/src/ranges.rs`: out-of-order received-byte interval merging.
  Ordered appends use the Go fast path without an FFI call.
- `engine-rust/discovery`: mmap dump indexing, search postings and catalog fetch.
  This serves the separate desktop discovery command, not the Android playback
  engine. Its documented startup improvements are not Android app measurements.

## 1. Piece-cache ownership and storage — greatest potential, largest project

Evidence:

- `storage.go:175` stores payloads and many related flags in separate Go maps.
- `storage.go:1302` allocates a piece buffer on a cache miss and merges each peer
  block into it.
- `direct_storage.go:415` scans the LRU for eviction candidates in up to two
  passes, checking several maps for each candidate.
- `direct_storage.go:248` clones verified startup pieces for asynchronous saving.
- `direct_storage.go:875` allocates and copies a verified resume snapshot.
- `storage.go:1113`, `:1133`, `:1164` perform some disk loading and flushing while
  the shared storage mutex is held. The asynchronous flush goroutine still takes
  that mutex while doing the writes.

A native cache could own reusable piece buffers, compact piece metadata,
received ranges, eviction classes, and read/hash/save leases. A lease keeps one
buffer generation alive while asynchronous work uses it, avoiding some full-piece
clones. Completed jobs must report the generation they actually processed so a
seek, eviction or late write cannot publish stale completion.

The interface should accept block writes, read requests and batches of demand
changes and return readiness/completion events. Go retains torrent decisions;
Rust owns cache bytes and their lifetime. File I/O must run outside the shared
metadata lock, with explicit protection against close and descriptor reuse.

Likely value: less allocation churn and copying, predictable buffer reuse, and
shorter storage lock holds during playback and downloading. Go byte payloads
are pointer-free already: Rust does not eliminate a full GC scan of every video
byte, because that scan does not currently occur. Native buffers still count
toward Android process memory. `storage.go:48–69` budgets the Go runtime from the
cache allowances; moving payloads outside Go requires a combined native/Go memory
budget, not simply preserving that allowance in both places.

Do not port the LRU alone: its touches/removals are already constant time.

## 2. Peer bitfield overlap — best smaller computational candidate

`ghost_pro.go:132` constructs a map of missing selected pieces by querying
individual piece states. `calculatePeerOverlap` at line 188 walks that map for
each peer. The controller computes selection and playback overlap separately.
`PeerPieces()` clones the peer bitmap under the torrent lock, so those two
overlap calls can also clone it twice per peer. The controller runs on the
one-second clock, with overlap work gated by its sample interval.

With P missing pieces and C peers, the counting work is roughly proportional
to C×P, plus bitmap copies. This is most relevant for large episode packs,
small pieces and high peer limits.

Represent selected, completed and forward-demand pieces as compact bitmaps;
compute both intersections and their cardinalities in bulk. A Rust adapter
could process all peer snapshots in one call and return numeric counts. Avoid
serializing a whole bitmap for every Contains check or keeping mirrored peer
state current across two languages just for occasional sampling.

First choice: use the existing Go Roaring bitmap representation and a combined
snapshot/count method inside the torrent library. That can remove the same
algorithmic and lock costs without FFI. A Rust port becomes more attractive if
native cache metadata already supplies the missing/demand bitmaps.

## 3. Playback demand planner — useful after batching priority application

`playback.go:145`, `:287` and `:1567` expand windows into per-piece maps, copy
reader demand, combine hints and resume demand, and build a storage-demand map.
Changed priorities are then applied individually. Each `Piece.SetPriority`
acquires the torrent client lock (`patches/torrent/piece.go:270`).

A pure Rust planner could accept compact spans, startup/resume hints and the
cache allowance, and return only changed priority runs and demand spans. It
should own no connections, timers or reader locks. This keeps policy coherent
and reduces per-piece map work, especially with large windows and small pieces.

The essential prerequisite is a Go method that applies a batch under one client
lock. Porting the map merge while retaining one lock acquisition per piece
leaves a major cost in place. Replacing maps with interval operations in Go is
also a valid first implementation of this interface. Keep the tiny adaptive
rate calculation in Go; moving a few arithmetic operations adds little value.

## 4. Normal piece verification — conditional, preserve torrent semantics

Sparse restore hashing already uses Rust, but normal incoming-piece checks
flow through `torrent.go:1263` and `:1339`. These stream storage bytes into the
piece hasher, sometimes together with per-block checks used to identify peers
that supplied corrupt data. `storage/wrappers.go:62` already offers an
`io.WriterTo` seam for more efficient storage reads.

If cache bytes become Rust-owned, hashing an immutable leased piece in place
could remove intermediate reads/copies. Keep CPU acceleration enabled for the
actual Android target and return enough information for corrupt-peer attribution.
Preserve hybrid/v2 padding and Merkle verification; the current restore path
deliberately leaves those cases to the library.

Do not turn on `SelfHashing` globally as a shortcut: the library explicitly
skips its normal corrupt-peer attribution when that interface is used. Merely
replacing the SHA implementation has no established advantage; the ownership
and copy reduction is the main reason to consider this port.

## 5. Piece telemetry and status transport — secondary

Bucket aggregation is already Rust. Go still constructs/sorts resident-piece
runs in `pieces.go:67`. That snapshot is cached until its revision changes.
If the cache moves to Rust, let it maintain the resident-state runs there too,
avoiding another metadata walk and sort under the storage mutex.

The Android control path uses direct JNI dispatch, not an HTTP socket, but
still serializes Go responses to JSON and parses them into Kotlin objects.
`LibrarySession.kt:2500` refreshes individual torrent statuses after events;
`EngineClient.kt:499` reconstructs each file list. Large libraries/packs may
benefit from compact batched status snapshots, static metadata caching and
changed-record updates. That is principally a transport/data-shape improvement;
rewriting JSON parsing in Rust while recreating all Kotlin objects preserves
much of the work. The one-second status fingerprint already avoids full piece
telemetry and JSON encoding, so it is not an obvious port target.

## 6. Standard playback JNI buffers — improve copies before porting

`main.go:220` allocates a Go byte buffer for every Standard playback read, then
copies its result into Java using `SetByteArrayRegion`. Preview reads have the
same pattern. A per-reader reusable buffer or suitable direct-buffer interface
could reduce temporary allocations. Pooling and close/concurrent-read rules
matter more than the language used.

Moving only this wrapper to Rust leaves the Go read and Java copy in the path.
A larger native storage port could make native-to-Java delivery useful, but
do not pin a Java critical array while waiting for torrent data. HTTP playback
does not use this JNI payload-copy path; optimizing it will not improve the
default HTTP stream by itself.

## Keep in their current language

- **Torrent connections, DHT, trackers, TCP/uTP/WebRTC and request ownership:**
  state is deeply integrated with the Go torrent client. A Rust rewrite would
  replace a large protocol engine. Faster local computation does not make a
  stalled peer respond sooner.
- **Request heap/comparator alone:** it consults live request owners, peer
  availability and cancellation state. Per-comparison FFI or rebuilding large
  snapshots per refill can add cost. Consider only a coherent batched scheduler
  after evidence identifies candidate enumeration/ordering as a CPU bottleneck.
- **Android Compose, MediaStore/SAF, permissions, service lifecycle and player
  launching:** these are Android integration work, not native compute kernels.
- **Thumbnail decoding:** the code delegates decoding to Android media/bitmap
  facilities. Porting the Kotlin coordinator does not replace the decoder or
  remove network waits.
- **Small policies, formatting, checkpoints and cache directory housekeeping:**
  no source evidence justifies adding another language interface for them.

## Practical order

1. Remove duplicate bitmap snapshots and use bulk bitmap counts; batch piece
   priority updates. These improvements can be made in Go first.
2. Design and port the native piece cache as one ownership module, including
   buffer reuse, generations, leases and asynchronous storage jobs.
3. Move resident verification and telemetry into that module where it removes
   copies or duplicate metadata.
4. Port the pure demand planner if its remaining map work is material.

This assessment does not establish which cost dominates on the phone. None of
the candidates has a supported percentage speed, RAM or battery improvement
from source inspection alone.
