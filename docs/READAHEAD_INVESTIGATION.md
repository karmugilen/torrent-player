# Read-ahead and playback stalls

Source investigation and changes, 2026-10-25. No Android device was connected.
The user requested code analysis and explicitly prohibited further tests. These
changes have not been measured against the reported playback session.

## Queue capacity collapsed during nudges

`playbackReader.read` starts a 100 ms urgent nudger in optimized seek mode.
`NudgeUrgentRequests` can update a peer with requests still outstanding.
`applyRequestState` then assigned `peakRequests` the number of **new** requests,
rather than the number outstanding. A nudge that added nothing set it to zero.
`nominalMaxRequests` doubles that value with a floor of one, so this could turn
a useful peer into a one-request downloader while playback waited. Nudges stop
when the read finishes, explaining a possible difference between playback and
ordinary downloading.

Capacity now counts the whole outstanding queue. A bounded playback queue also
refills at half its limit, rather than waiting for its final block to arrive.
The existing bandwidth-delay cap, cancellation accounting, and rejection rules
remain. `FullPipeline`, which also changes cancellation and seek behavior,
remains off. Earlier measurements of refill tuning in `DIRECT_PLAY.md` were
mixed; there is no measured throughput or seek improvement claim for this change.

## Demand sampling discarded sequential progress

Buffer sizing compared only two adjacent callback offsets and reset its clock
on every callback, including those too close together to sample. Rapid 32 KiB
reads were discarded; a later pause was divided into just one callback's bytes.
This could shrink non-Ghost-Pro read-ahead to its 4 MiB minimum.

The sampler now retains its time and offset anchor across callbacks and measures
all sequential progress over at least 200 ms. Seeks reset the anchor. Existing
window limits, smoothing, and controller choices still apply. Ghost Pro already
sizes its window separately from this estimate.

## Download playback waited for a complete verified piece

Ordinary download playback defaulted to verified-piece reads even when the
specific blocks requested by the player had arrived. HTTP partial delivery alone
could not help: the underlying reader still withheld those blocks until the
entire piece arrived and was hashed.

Download playback now enables received-block reads for the video body, for both
HTTP and Standard. Startup head and tail remain verified. Standard still fills
each requested buffer; HTTP can return the available prefix. Piece verification
still controls completion and durable trust, but body bytes can reach the player
before that verification. The optional HTTP A/B mode can still select verified
body reads explicitly.

## Rust received-range core

Every Direct Play block previously appended an interval, sorted all intervals,
and scanned them to merge while holding the storage lock. This work does not
need another full sort because the existing intervals are already normalized.

`engine-rust/src/ranges.rs` inserts an out-of-order interval using binary search,
merges its neighbors and shifts the remaining suffix in caller-owned memory.
It allocates nothing and retains no Go pointers. Ordered appends and last-range
extensions stay on a constant-time Go fast path with no FFI call. Received-byte
reads use binary search instead of scanning the ranges.

The existing `rust` build tag selects the Rust core, including Android builds;
host builds without that tag use an equivalent Go fallback. The bridge checks
the struct layout at compile time. Network scheduling stays in Go, where the
connection state and locking already live. This removes repeated sorting work;
a device-level Rust speedup has not been established.

## Validation boundary

Only formatting, diff checks, and compilation were requested after the no-test
instruction. Compilation does not establish runtime correctness or prove that
the phone's stalls are resolved. No new tests were added or run for this change.

Completed successfully: `git diff --check`, native Rust release compilation,
Go host compilation with and without `-tags=rust`, Rust release compilation for
`aarch64-linux-android`, and the linked Android arm64 Go shared engine using
`-tags=nosqlite,noboltdb,rust`. Compilation outputs were placed under `/tmp`;
Rust libraries use the existing Cargo target directories.
