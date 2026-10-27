# HANDOFF — grok-1 (types.rs JSON Go-compat)

Paused by coordinator 2026-10-27 (~before 6 pm resume). **Do not edit `server.rs` until resume; agy-4 applies call-sites below.**

## Done

- Prior session: `wt-parity api-diff` CLI + `API_DIFF.md` / `API_DIFF_CRITICAL.md` (11 MATCH / 19 DIFF / 14 CRITICAL).
- This session: mapped Go `encoding/json` vs serde gaps from `engine-go/types.go`, `bypass.go`, `connections.go`, `claude_seek.go`, `cache_store.go`, `live_preview.go`, and ad-hoc `server.go` maps.
- Confirmed Go float64 whole numbers encode as JSON integers (`"progress":0` not `0.0`); nil slices → `null`; pointer-without-omitempty → `null`; omitempty skips.

## Not started / half done

- **`types.rs` rewrite not applied** (pause before edits). Crate still compiles as-is.
- No Go golden generator / unit tests yet.
- api-diff not re-run for before/after on this task.

## Exact next steps (resume order)

1. Port `types.rs` with:
   - `serialize_go_float` / `deserialize_go_float` for every `float64` field (`progress`, `ratio`, `seed_ratio`, `network_err_ms`, file `progress`).
   - Pointer fields **without** omitempty → `Option<T>` **without** `skip_serializing_if` (emit `null`): `info_hash`, `error`, `time_remaining`, BypassStatus.`error`, AddResponse.`info_hash`, etc.
   - omitempty → `skip_serializing_if`: `focused_file`, `warning`, `next_preload`, TrackerView.`message`, PieceTelemetry playback fields, OkResponse.`id`, PlayRequest bools/`offset`/`file_index`, etc.
   - Nil-able slices → `Option<Vec<T>>` or `NilVec<T>` newtype with `From<Vec<T>>`: BypassStatus.`coverage`, and optionally TorrentView `files`/`trackers`/`selected` if server ever leaves them unset (Go nil → null).
   - Add missing response/request structs used by routes: `FileOnlyResponse { file_only: bool }` (`{"fileOnly":…}`), `ForgetPeersRequest { info_hash }`, `ErrorResponse { error }`, `ClearCacheRequest { categories }`, ensure `SeekStats` has no nested `arrival` on the Go type (arrival is only on `SettingsResponse`; Kotlin merges from settings.arrival — current Rust `SeekStats.arrival` is extra vs Go).
   - `PeerConnectionView.country`: Go is `string` + omitempty (omit empty string). Prefer `String` + `skip_serializing_if = "String::is_empty"` (or keep Option and document).
   - `SettingsRequest.max_peers`: Go is bare `int` (0 = unset). Align deserialize; server today uses `Option` — see call-site note.
   - `SettingsResponse.idle_drop_minutes`: Go `int` — keep JSON number; type `i64`/`i32` both fine.
   - `NetworkChangeRequest.connected`: confirm Go shape in network handler.

2. Apply **agy-4 server.rs call-sites** (section below).

3. Golden tests: tiny Go program under `/home/kar/.cache/wt-tmp/go-json` that copies struct defs and prints JSON; Rust `#[cfg(test)]` in `types.rs` (or `tests/types_json.rs`) asserts byte/semantic equality.

4. Re-run `wt-parity api-diff`; classify remaining diffs as types-fixed vs **server behaviour / not wired** for agy-4.

5. clippy `-D warnings`, fmt, update `PORT_NOTES.md` in wt-engine (types section) + wt-parity.

## server.rs call-site changes for agy-4 (after types land)

File: `engine-rs/crates/wt-engine/src/server.rs`

### BypassStatus.coverage / error (null vs omit / wrong fill)

Around `handle_get_bypass` (~1020):

```rust
// BEFORE
let status = BypassStatus {
    enabled,
    active: enabled,
    mode: if enabled { "active".to_string() } else { "off".to_string() },
    coverage: vec!["all".to_string()],
    error: None,
};

// AFTER (once coverage is Option<Vec<String>> or NilVec)
let status = BypassStatus {
    enabled,
    active: enabled,
    // Go: mode off + coverage nil when disabled; tcp-write-split + real list when on
    mode: if enabled {
        "tcp-write-split".to_string() // or whatever activate sets; Go default active mode
    } else {
        "off".to_string()
    },
    coverage: if enabled {
        Some(vec![
            "outgoing-tcp-peers".into(),
            "http-trackers".into(),
            "websocket-trackers".into(),
        ])
    } else {
        None // JSON null — matches Go nil slice
    },
    error: None, // must serialize as null (types: no skip_serializing_if)
};
```

If using `NilVec`: `coverage: NilVec::from(None)` / `NilVec::from(vec![…])` with `From` impls so literals stay ergonomic.

### File-only response shape (behaviour + types)

Around `handle_file_only` (~550): Go returns `{"fileOnly": bool}`, not `OkResponse`.

```rust
// BEFORE
serde_json::to_vec(&OkResponse { ok: true, id: Some(id.to_string()) })

// AFTER
serde_json::to_vec(&FileOnlyResponse { file_only: r.read().file_only })
// or after toggling: FileOnlyResponse { file_only: enabled }
```

### Configure / focus / pause / resume / forget-peers / remove OkResponse

Go configure/forget-peers: `OkResponse{Ok: true}` **without** id.  
Go focus/pause/resume/remove: `OkResponse{Ok: true, ID: …}`.

Audit each `OkResponse { ok: true, id: Some(...) }` vs `id: None` against Go. Types already support omitempty on `id`.

### Cache categories (behaviour)

`handle_get_cache` (~1053): Go returns 4 categories (`startup`, `resume`, `peers`, `dht`). Rust currently returns 2. Fill all four (values can be 0 until wired).

### Clear cache status codes (behaviour)

Go `/cache/clear` returns 400 on bad categories; Rust always 200. agy-4.

### Preview-state (behaviour)

Go requires `?fileIndex=` and returns 400 if missing/invalid; Rust returns 200 stub. agy-4 / grok-3 live_preview.

### Settings defaults (behaviour)

Rust defaults differ (e.g. `idle_drop_minutes` 15 vs Go 0, `ghost_pro_mode` true vs false, `dial_rate_per_sec` 50 vs 30, `seeding_enabled` true vs false). Not types.rs — server init / getters.

### /play bad id (behaviour)

Go 404 vs Rust 400 — agy-4 status mapping.

### Magnet add infoHash (behaviour)

Rust returning magnet URI string as infoHash — server add path, not types.

### Pieces / torrent name/length (behaviour / core wiring)

Stub vs real metainfo — agy-4 + core.

## Open questions

- Prefer `Option<Vec<T>>` vs `NilVec<T>` newtype for nil slices? NilVec + `From<Vec<_>>` keeps call sites as `.into()`.
- Should `SeekStats.arrival` be removed from Rust to match Go (Kotlin reads arrival from settings root)?

## Verify commands

```bash
cd engine-rs
CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/wt-engine CARGO_BUILD_JOBS=3 \
  cargo test -p wt-engine types_json -- --nocapture
CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/wt-engine CARGO_BUILD_JOBS=3 \
  cargo clippy -p wt-engine -- -D warnings

CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/wt-parity CARGO_BUILD_JOBS=3 \
  cargo run -p wt-parity -- api-diff \
  --go /home/kar/.cache/wt-tmp/engine-go-host \
  --rust /home/kar/.cache/wt-targets/wt-parity/debug/engine-host \
  --work /home/kar/.cache/wt-tmp/api-diff \
  --report crates/wt-parity/API_DIFF.md \
  --repo ..
```

Baseline before types fix: **11 MATCH / 19 DIFF / 14 CRITICAL** (`API_DIFF_CRITICAL.md`).
