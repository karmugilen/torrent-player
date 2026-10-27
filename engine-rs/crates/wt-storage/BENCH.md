# Storage benchmark (2026-10-27)

Two sequential runs per implementation on the same Linux host, Go first then Rust; the second pair ran under markedly heavier shared filesystem load. Numbers are exploratory, not a stable performance verdict. The release Rust binary and Go `-tags=nosqlite,noboltdb` test use 256 MiB payloads, 1 MiB pieces, 16 KiB chunks, identical deterministic piece/seek order, a 64 MiB Direct Play limit, and successful reads. Read cases prepopulate the file outside the timer. Direct Play throughput counts logical payload once (write plus read); these are hot-cache tests, not physical disk throughput. Rust's first sample used `cargo test --release --bench` and the second used `cargo bench`; both are optimized. Allocation totals include setup outside the timed interval and process peak RSS is cumulative, not per-case.

| Case | Go run 1 / 2 | Rust run 1 / 2 | Rust / Go run 1 / 2 | 0.95 target |
|---|---:|---:|---:|---|
| Download sequential write MiB/s | 512 / 593 | 1377 / 77.9 | 2.69 / 0.13 | unstable, investigate |
| Download sequential read MiB/s | 861 / 11394 | 5894 / 10867 | 6.85 / 0.95 | unstable, investigate |
| Download random write MiB/s | 1722 / 616 | 507 / 82.9 | 0.29 / 0.13 | investigate |
| Download random read MiB/s | 12915 / 12356 | 9567 / 7636 | 0.74 / 0.62 | investigate |
| Direct Play sequential MiB/s | 4210 / 4310 | 3552 / 3888 | 0.84 / 0.90 | investigate |
| Direct Play seeks MiB/s | 4834 / 5905 | 5228 / 6851 | 1.08 / 1.16 | pass in both |
| Checkpoint saves/s | 1466 / 121 | 177 / 43 | 0.12 / 0.36 | investigate |
| Checkpoint restores/s | 296736 / 299850 | 588235 / 645161 | 1.98 / 2.15 | pass in both |
| 10,000-piece aggregations/s | 166556 / 185322 | 371747 / 373972 | 2.23 / 2.02 | pass in both |

The checkpoint and aggregation operations are not data transfers; ops/s are meaningful here, unlike a fabricated MiB/s denominator. Counts for the second run: Go/Rust allocations respectively: sequential write 1,213/1,097; sequential read 81/298; random write 1,206/1,101; random read 83/299; Direct Play sequential 22,239/1,095, seeks 1,263/918; checkpoint save 36,003/8,005, restore 18,023/8,009; aggregation 20,002/10,002. Peak process RSS was 157.8 MiB Go and 67.5 MiB Rust, including all earlier cases.

**Investigation and fix plan:** The 38-fold change in Rust sequential write and 12-fold change in Go checkpoint time show that run order and concurrent disk activity dominate these results. Both `writePieceCheckpoint` and `write_piece_checkpoint` call fsync on each save; the Rust checkpoint samples take 11.3–46.1 s for 2,000 saves versus Go 1.36–16.57 s, so do not remove durability to chase this result. Repeat on an idle host with alternated implementations and syscall tracing of fsync latency; inspect coalescing at the storage layer if the gap persists. For random reads compare `documentPiece.ReadAt` with `DocumentPiece::read_at` using CPU profiles, and for Direct Play profile cache write/read and lock costs; retain a measured change only if it improves repeated paired runs without changing semantics. `perf` is not installed on this host; no CPU profile is claimed. The read/write comparison has further limitations: Go includes `Close` in its read timing while Rust does not, and the cache state and completion/write-back scheduling are uncontrolled. Consequently the listed failures are investigation items, not proof of a regression.

Reproduce: copy `engine-go` to `/home/kar/.cache/wt-tmp/go-storage-bench` with its patched torrent dependencies, copy `benches/go_storage_bench_test.go` to that directory as `storage_bench_test.go`, and run `GOTMPDIR=/home/kar/.cache/wt-tmp/perf go test -tags=nosqlite,noboltdb -run '^TestRunBenchmarks$' -count=1 -v`. Run Rust from `engine-rs` with `CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/codex-3 CARGO_BUILD_JOBS=3 cargo bench -p wt-storage --bench storage_bench`. Run only one benchmark at a time; both delete their data fixtures under `/home/kar/.cache/wt-tmp/perf` on successful completion.

## Five-run follow-up and code-path I/O audit

Three additional Go/Rust pairs were run in that order, bringing each implementation to five samples. Medians and minima below are MiB/s for transfers; checkpoint and aggregation values are total milliseconds for 2,000 or 5,000 operations, so lower is better there. The first two pairs are the samples above. These are still shared-disk results: Go sequential write ranged down to 7.89 MiB/s, so they do not establish an isolated implementation cost.

| Case | Go median / min | Rust median / min | Advantage ratio, medians |
|---|---:|---:|---:|
| sequential write | 512.13 / 7.89 | 120.58 / 57.57 | 0.24 |
| sequential read | 11394.34 / 212.21 | 10866.70 / 5894.13 | 0.95 |
| random write | 615.62 / 220.12 | 506.64 / 82.85 | 0.82 |
| random read | 9623.47 / 5037.05 | 9566.83 / 7635.82 | 0.99 |
| Direct Play sequential | 4210.28 / 3379.92 | 3887.68 / 3226.82 | 0.92 |
| Direct Play seeks | 4834.13 / 4429.69 | 5348.38 / 4303.67 | 1.11 |
| checkpoint save, ms | 16567.79 / 1363.92 | 20826.15 / 9724.60 | 0.80 |
| checkpoint restore, ms | 6.74 / 6.57 | 3.30 / 3.10 | 2.04 |
| aggregation, ms | 30.02 / 26.98 | 13.45 / 13.03 | 2.23 |

Static audit of normal successful paths, not measured syscall counts:

- Checkpoint save: both low-level functions always rewrite the file, even when unchanged. Each creates the parent if needed, opens/truncates a temp file, writes the complete checkpoint, issues one file fsync, closes and renames; neither fsyncs the directory. Rust batches serialization in one write whereas Go issues separate writes for header, fields, path and bitfield. Rust uses a separate bitfield allocation; neither performs a destination read or metadata check. Higher-level dirty-state scheduling can skip a save. Shared fsync latency is a plausible contributor to the timing gap, but has not been independently measured.
- Download random read of an uncached 16 KiB block wholly inside one file: both take the storage lock, locate the overlapping file and issue one positional read in the normal full-read case, without open, write, sync or rename. Partial reads and interruptions may require retries. The benchmark's prefilled file is hot in the OS cache.
- Direct Play cached 16 KiB read or write: both take a storage lock and copy in memory; neither needs a file read/write for the block itself. Startup-piece completion is different: Go clones and queues the piece to a background drain, which hashes and performs open, two writes (digest and payload), fsync and rename outside the lock; Rust's `save_startup_piece_locked` performs hash, open, two writes, fsync and rename synchronously while holding the storage lock. This is a concrete latency risk; port Go's bounded background queue and close/eviction coordination before comparing under idle load. Startup plans determine how many such saves occur; the benchmark does not currently count them.

These call counts are from source inspection, not a counting wrapper; runtime confirmation and quiet-host repeated measurements remain outstanding. No speculative storage implementation change was made.
