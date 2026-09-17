# Pieces grid + no-queue integration test report

**Date:** 2026-09-16  
**Scope:** Finish `docs/PIECES_HEAT_GRID_UI.md` execution contract; remove download slot queue; integrate selected-aware heat grid; signed APK.

## Changes (this integration)

### App / UI
- `TorrentFilesScreen.kt` — wired `PieceHeatGridSection`; deleted `PiecesSection` / `PieceCanvas`; Details lifecycle poll uses `SystemClock.elapsedRealtime` ≤ 1 Hz, prompt first fetch, clears telemetry on engine-id change / offline, stops off-screen via `repeatOnLifecycle(STARTED)`.
- `PieceHeatGrid.kt` — rounded `clipPath` for mixed-fill stacks (integration fix).
- Queue removal already landed by worker: no Concurrent downloads setting, no Off/1/2/3 slots, no order/promote controls, legacy prefs cleared on start, `reconcileTransferState` without slot gate.

### Engine / core (from piece_counts worker, verified here)
- Additive `selectedVerified` / `selectedReceiving` on `/pieces` buckets; Kotlin DTO defaults absent fields to 0.

### Docs
- `docs/PIECES_HEAT_GRID_UI.md` — status Implemented; selected-aware formulas replace contradictory `verified/selected` title/legend math; checklists updated; deferred polish listed.
- `docs/APP_IMPROVEMENT_IMPLEMENTATION_PLAN.md` — F3 removal supersedes queue-slot requirements in §10 C/D and checklist.

## Validation

| Check | Result |
|-------|--------|
| `go test ./...` (engine-go) | PASS |
| `go test -race ./...` | PASS |
| `go vet ./...` | PASS |
| `:core:test` | 17 tests, 0 failures |
| `:app:testDebugUnitTest` | 62 tests, 0 failures |
| `:app:lintRelease` | PASS |
| `:app:assembleRelease -Pwebtor.localSigning=true` | BUILD SUCCESSFUL |
| `git diff --check` | clean (exit 0) |

Focused coverage includes mixed selection / inclusive ranges (Go), selected-count parse (core), heat-grid normalize/layout/hit-test (app), queue-removal / transfer policy (app).

## Artifact

| Field | Value |
|-------|--------|
| APK | `artifacts/torrent-player-1.4.5-pieces-grid-no-queue.apk` |
| SHA-256 | `0c1692a17ee03b51ca02a64e8c9fcc0878a5bab3f472e5b09a173d41499fd04f` |
| package | `webtor.app` |
| versionName / versionCode | `1.4.5` / `22` |
| Signing vs `torrent-player-1.4.5-complete-workflow.apk` | Same Android Debug cert SHA-256 `9c33627be30850a9fd315b374896b5fe229bd8c09c51e0d7a77583cd6d3a38f3` |
| Checksums file | appended in `artifacts/SHA256SUMS.txt` |

## Connected device

`adb devices` showed **no attached authorized devices**. Install/launch and log inspection were **not** performed. No uninstall/clear/data loss attempted.

## Not claimed

- Manual download visual QA / screenshots on device
- Live torrent piece-map paint matching against a controlled swarm
- GitHub release

## Limitations

- Receiving pulse and selected-only filter remain deferred polish.
- File-boundary markers assume sequential `SavedFile` lengths / torrent order (no explicit byte offsets on the model).
- Mid-session in-memory queue holds from older builds are moot after process restart; cold restore re-applies saved selection.
