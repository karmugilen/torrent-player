# Playback, seeking, and Rust optimization review

Reviewed 2026-09-30 against commit `0b015c8`. This is a code review and local correctness check, not a measured public-swarm or Android throughput benchmark.

## Implemented follow-up: disk-download seeking

The requested scope is disk downloading with playback waiting for missing verified pieces. A separate RAM-only Watch mode is outside this work.

The follow-up changes background startup hints to High while playback readers are active, so they yield to the reader's urgent seek target and forward readahead. A reader opening another handle retains its own urgent container probes; background startup urgency returns when the last handle closes. Downloading continues to use the existing disk storage and verification path.

Repeated reads inside unchanged piece spans now reuse the demand window, unchanged coordinator demands skip recomputation, and priority application avoids a temporary union map. Reader readahead is updated only when its size changes. A regression covers repeated seeking, a download-state refresh, a second metadata reader, closing it, and closing the final reader using actual piece overrides.

The findings below describe the original baseline. Finding 2 is addressed by this follow-up, and the per-read work in finding 6 is reduced. No device seek-speed improvement has yet been measured.

Follow-up validation: Go and Rust-enabled Go suites passed; playback, startup-hint and local-media transfer tests passed with the race detector; the JNI smoke test passed with Java 21, including verified playback and seeking.

## Recommendation

Fix storage and priority correctness first, measure seek latency next, then consider moving the piece cache and write pipeline to Rust. A full torrent-engine rewrite has a much larger compatibility cost and no demonstrated speed advantage for this app.

The Android build already links Rust (`engine-go/build.sh`): piece-grid aggregation, sparse-file extent hashing during saved-data checking, and verified playback `pread` use Rust. Live peer-piece hashing generally stays in anacrolix/Go, deliberately preserving its smart banning and v2/hybrid hashing behavior (`restore_hash.go`). Moving these already-ported functions again will not address the remaining bottlenecks.

## Findings, in priority order

### 1. RAM-only Watch mode still uses disk completion and eviction logic

**Confirmed with a temporary Rust-enabled Go probe.** `storage.go:276` attaches Watch mode with nil file handles. Incoming `documentPiece.WriteAt` still marks RAM buffers dirty, and `MarkComplete` at line 1135 calls `flushPieceLocked`, which tries to write to those nil handles. The probe attached a single-file Watch torrent, wrote eight bytes, then called `MarkComplete`; it returned `storage for  is not configured`.

The 20 ms disk-flush worker also follows the download path. Existing stream trimming measures occupied disk bytes, which cannot enforce a RAM-only eviction budget. Cache-pressure eviction can similarly attempt to flush dirty pieces to nonexistent files.

Implement separate Watch semantics: verify RAM bytes, mark them available without disk persistence, bound actual RAM allocation, and invalidate torrent completion when verified cached pieces are evicted. Pin active reads and verification buffers; never discard an in-flight buffer. Durable checkpoint completion remains a Download-mode concern. Cover more than 50 MiB, backward seeks after eviction, shared file-boundary pieces, and close/cancellation.

### 2. Startup hints override the reader's tail demotion

**Confirmed with a temporary Rust-enabled Go probe.** `video_priority.go` adds 5 MiB head and 3 MiB tail demands at `Now` for every selected video. `playbackPriorityCoordinator.applyLocked` takes the maximum across hints, focus, and readers. Demoting a reader's tail therefore does not clear the separate hint demand.

On the existing 48 MiB fixture, after seeking to 6 MiB, the reader reported tail piece 46 as `Normal` while the coordinator retained `Now`. Current demotion tests inspect the reader's demand rather than the combined priority. This creates competing urgent work when startup ranges are still missing, particularly with several selected videos.

Give startup hints a lifecycle: retire completed hints, prioritize the actively played file, and release its obsolete startup hints once container probing finishes. Test coordinator demand and actual torrent priority with two readers and multiple files, while preserving remaining readers' legitimate demand.

### 3. Storage I/O holds one torrent-wide mutex

`documentPiece.ReadAt`, `WriteAt`, cache eviction, `MarkComplete`, and the delayed flush worker share `st.mu`. Disk cache loads and dirty-piece writes happen under that mutex. The worker starts asynchronously, but its disk work still excludes peer writes and cache reads. A cache miss can also read a whole piece to serve a small request. These are code-level contention risks; their contribution on a phone has not been measured.

First redesign ownership: pin immutable/versioned buffers, perform I/O outside the metadata lock, and publish completion only for the version actually flushed. Bound the write queue, retain dirty buffers on errors, and drain writes before closing descriptors or changing selection. This fixes the concurrency problem whether implemented in Go or Rust.

### 4. Seek cancellation is constrained by external-player callbacks

Each proxy descriptor has one `HandlerThread`. A missing-data `onRead` can wait up to 45 seconds in `ReadAt`. A subsequent read or release queued on that same handler cannot interrupt it through the callback itself. Separate descriptors have separate threads, but that does not solve cancellation for a blocked descriptor.

Android supplies seek offsets through subsequent reads and requires the full requested length except at EOF: [ProxyFileDescriptorCallback](https://developer.android.com/reference/android/os/ProxyFileDescriptorCallback). Callback dispatch uses the supplied handler: [StorageManager](https://developer.android.com/reference/android/os/storage/StorageManager#openProxyFileDescriptor(int,android.os.ProxyFileDescriptorCallback,android.os.Handler)).

Measure this with VLC and another player on a device: seek again while an earlier read lacks peers. An HTTP disconnect has a separate request cancellation path, but player behavior must be verified. An integrated player could send explicit seek/cancel commands independently of reads. Simply shortening the deadline can produce fatal EIO in external players; it is not a complete fix.

### 5. Readahead and startup policy need better inputs

`playback.go` uses a fixed 2 MiB urgent window, 24 MiB forward window, and 512 KiB lookbehind. Its open phase holds forward readahead until head/tail probing or a body read. A fast-start MP4 that never reads the tail can stay in that phase while consuming its first 5 MiB.

Use bounded startup probing with an exit condition for sequential playback and, where practical, container-aware metadata ranges. Tune forward buffering from consumption rate and verified contiguous bytes; download speed alone is not media bitrate. External players do not currently provide presentation time or reliable bitrate, so use conservative byte-consumption estimates unless an integrated player supplies them. Align budgets to piece size and total process memory.

### 6. Every read rebuilds piece-demand maps

`updateWindowLocked` builds a demand map, the coordinator copies it, then rebuilds aggregate and changed-piece maps. Watch mode additionally refreshes completion for every demanded piece. File focus can contribute a whole-file map to that aggregation. Reader readahead also contributes upstream priority independently of these explicit overrides.

Skip updates when effective piece spans and policy are unchanged; apply span differences on boundary crossings and update completion only for invalidated/re-entered pieces. Keep reader-derived priority in the tests. This is a good Go optimization before adding FFI calls.

### 7. JNI allocates and copies for each playback read

`main.go:219` allocates a Go byte slice and copies the result into the Android callback byte array. Pool bounded scratch buffers first and measure allocation/GC cost. A Rust bridge does not automatically eliminate the required copy. Never pin a Java array across a blocking torrent/network wait; a direct-buffer design would require changing the caller interface and still accommodating the proxy callback's byte array.

## What to port to Rust

| Component | Priority | Reason and boundary |
| --- | --- | --- |
| Piece cache and buffer ownership | First substantial candidate, after correctness fixes | Explicit buffer lifetimes, pooling, bounded memory, eviction and pinning. Keep torrent scheduling in Go. |
| Batched positional writes | Alongside cache ownership | Pass batches of spans/buffers over FFI, use a bounded worker queue, return precise I/O errors. Gain comes from batching and concurrency, not the language alone. |
| Span priority planner | Later, only if profiles justify it | Return compact changed ranges; Go still applies anacrolix priorities. Avoid a call per piece. |
| Live hashing | Conditional | Compare against Go on ARM64 first; preserve smart banning and BitTorrent v2/hybrid semantics. Restore extent hashing already uses Rust. |
| Peer protocols, DHT, trackers, WebRTC | Keep current engine initially | A rewrite requires transport, recovery, storage and resume compatibility work before performance comparisons are meaningful. |
| Android UI and media decoding | Keep Android facilities | Decoder acceleration and external-player controls are separate from torrent-cache ownership. |

For a full-Rust experimental backend, [librqbit](https://github.com/ikatson/rqbit) is worth evaluating: its upstream documentation describes prioritization and seekable torrent streaming. That establishes feasibility, not Android performance or parity with this app's WebRTC, storage-descriptor, checkpoint and network-recovery paths. Evaluate those explicitly behind an engine interface before replacing the current runtime.

## Measurement and delivery order

1. Fix Watch completion/eviction and startup-hint ownership with regressions.
2. Record read wait time, time to first verified bytes after a seek, storage lock wait/hold time, flush duration, allocations, cache hits, verified contiguous buffer, hash throughput, and redundant downloaded bytes.
3. Apply Go span-diff updates, scratch-buffer reuse and storage concurrency changes. Compare before/after using the same local seed, bandwidth/latency conditions and media files.
4. Prototype Rust cache/write ownership only if profiling still identifies that path as material. Retain a Go baseline for comparison.
5. Validate on ARM64 Android with head/tail-moov MP4, MKV, large pieces, several selected videos, two player handles, cache overflow, repeated seeks, pause/remove and Wi-Fi/mobile transitions. Track p50/p95 seek-to-frame latency, rebuffer duration, useful verified throughput, peak RAM and battery cost separately.

Current default connection settings already limit dialing to 30/s and half-open connections to 24 per torrent/48 globally. Increasing peers or removing verification is not an evidence-based optimization. Public-swarm results also depend on peer availability and uplink contention.

## Checks performed

- `go test ./...`: passed.
- `cargo test`: 10 tests passed.
- `cargo build --release`: passed.
- `go test -tags=rust ./...`: passed.
- Two temporary probes reproduced Watch completion failure and retained coordinator tail urgency; their files were removed afterward.

No device seek timing, battery measurements or public-swarm speedups were established. The recommended order follows confirmed correctness failures and observable code-path costs.
