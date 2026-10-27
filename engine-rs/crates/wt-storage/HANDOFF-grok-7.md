# HANDOFF grok-7 (wt-storage accessors + ignored tests)

## Done
- Public accessors on `DocumentTorrentStorage` + `files_overlapping` (see STORAGE_TESTS.md / PORT_NOTES).
- Un-ignored all 16 waiting tests; `cargo test -p wt-storage` → **109 pass / 0 ignored / 0 fail**.
- `cargo clippy -p wt-storage --all-targets -- -D warnings` and `cargo fmt` clean.
- Direct Play close skips flush in stream mode; `on_stream_evict` fires async.

## Half-done / deferred
- No async Go `scheduleDiskFlushLocked` (sync flush_retry only, per coordinator).

## Next steps
- Forward new public method list in STORAGE_TESTS.md to agy-4.
- Use `CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/grok-7` only.

## Verify
```
CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/grok-7 CARGO_BUILD_JOBS=3 \
  cargo test -p wt-storage -- --test-threads=1
```
