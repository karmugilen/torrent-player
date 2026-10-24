# Codex Handover: Ghost Pro Mode & Contiguous Path Completion

## Implementation update — 2026-10-22

The pending engine work below is implemented in release **2.1.15 (45)**, with the user's subsequent focused-startup refinement: initial open/seek urgency stays narrow until verified bytes arrive, then steady playback uses the scaled urgent/readahead/lookahead tiers. Both Download and Instant Play use it; Instant Play keeps the existing RAM cap and scales tiers down to fit. Peer recycling intersects missing pieces with each reader's actual forward path. Settings changes immediately resize existing windows and refresh startup hints.

The user also requested live colors and U/R/L labels in the app's Pieces grid. These are implemented through additive `/pieces` playback ranges, separate from completion fills, with grouped-bucket overlap handling and foreground refresh at most once per second (including fully downloaded playback). See `DIRECT_PLAY.md` and `PIECES_HEAT_GRID_UI.md` for the final behavior.

Verification: `cd engine-go && go test -race ./...` passes, including local-peer Ghost Pro seeks beyond a 16 MiB Instant Play cache with and without boost/burst buffering. `:core:test`, `:app:testDebugUnitTest`, and signed `:app:assembleRelease` pass. The production-signed APK is retained as `artifacts/torrent-player-2.1.15-production.apk`. The device's existing app uses this machine's Android local key, so `artifacts/torrent-player-2.1.15.apk` uses `-Pwebtor.localSigning=true` for an in-place update that preserves data.

The original handover is retained below for context; its pending-task list describes the earlier state.

Installation: the local-key release updated device `00121649A004784` successfully with `adb install -r`. Launch returned `Status: ok`; the installed package reports version 2.1.15/code 45. No uninstall or data reset was needed. Public-swarm 4K startup performance and live U/R/L rendering during playback still need a user/device playback check.

## 1. Project & Architecture Context
- **Repository**: `torrent-player` (Branch: `experiment/http-player-handoff`)
- **Components**:
  - **Engine (Go)**: Embedded BitTorrent engine located in [`engine-go/`](file:///home/kar/funProjects/torrent-player/engine-go) using `github.com/anacrolix/torrent`. Communicates with Android via localhost HTTP control API (`/settings`, `/torrents`, `/playback`).
  - **Android Client (Kotlin + Jetpack Compose)**:
    - Core client: [`android/core/src/main/kotlin/webtor/core/EngineClient.kt`](file:///home/kar/funProjects/torrent-player/android/core/src/main/kotlin/webtor/core/EngineClient.kt)
    - App UI & state: [`android/app/src/main/java/webtor/app/`](file:///home/kar/funProjects/torrent-player/android/app)
- **Environment Rules**:
  - Python: Always run `actenv` before python scripts (`zsh -ic "actenv && python ..."`).
  - Pip packages: Use `uv pip install ...`.

---

## 2. Feature Specifications

### A. Ghost Pro Mode (Completed & Verified)
Ghost Pro Mode is an intelligent BitTorrent swarm management mode designed for high-bitrate streaming and optimal throughput:
1. **Dynamic Closed-Loop Dial Rate Tuning**: Dynamically adjusts peer dial rate between 30/s (base) and 80/s based on swarm saturation, active leechers, and useful transfer rates.
2. **Bitfield-Aware Peer Recycling**: Evaluates connected peers every 10s. If slots are saturated (`ActivePeers >= capacity`) and pending peers exist (`PendingPeers > 0`), it evicts idle leechers holding **zero overlap** with wanted pieces (`idleDuration >= 15s`) to free connection slots for fresh peers.
3. **Seeder Protection**: Seeders (peers holding 100% of pieces) are strictly protected from eviction.
4. **Mutual Exclusivity & Conflict Hierarchy**:
   - `Ghost Pro Mode` active: Auto turns off and grays out (38% alpha) `Ghost Mode`, `Peer Exploration`, and `Faster Peer Search`.
   - `Ghost Mode` active: Auto turns off and grays out `Faster Peer Search`.
   - `Burst-aware buffer` active: Auto turns off and grays out `Adaptive buffer`.

### B. Forcing Contiguous Path Completion (In Progress)
- **User Objective**: When streaming 4K video during download mode (where all selected files are normally marked `PiecePriorityNormal`), prevent peers from downloading random pieces across the file leaving gaps.
- **Engine Priority Order in `anacrolix/torrent`**:
  `PiecePriorityNow` (4) > `PiecePriorityReadahead` (3) > `PiecePriorityHigh` (2) > `PiecePriorityNormal` (1).
- **Required Behavior under Ghost Pro Mode**:
  1. **Immediate Playhead Window**: Scale urgent window (`PiecePriorityNow`) to `max(playbackUrgentBytes, 16MB, 2*pieceLen)`.
  2. **Forward Readahead Window**: Scale readahead window (`PiecePriorityReadahead`) to `max(forward, 64MB, 6*pieceLen)` (up to 96MB if `p.boost`).
  3. **Forward Lookahead Window**: Elevate forward lookahead (`PiecePriorityHigh`) to `max(32MB, 4*pieceLen)` right after the readahead window.
  4. **Startup Hints Boost**: When starting playback in download mode under Ghost Pro, elevate head/tail metadata spans to `max(head, 16MB, 2*pieceLen)`.
  5. **Playback-Aware Swarm Recycling**: In `ghostProSwarmState.chooseVictim()`, prioritize evicting idle peers that hold **zero pieces on the active contiguous path** (`contiguousOverlap == 0`).

---

## 3. Modified & Created Files Summary

| File | Status | Description of Changes |
| :--- | :--- | :--- |
| [`engine-go/ghost_pro.go`](file:///home/kar/funProjects/torrent-player/engine-go/ghost_pro.go) | **Created** | Implements `ghostProSwarmState`, `chooseVictim`, dial controller observation, and bitfield overlap calculation. |
| [`engine-go/ghost_pro_test.go`](file:///home/kar/funProjects/torrent-player/engine-go/ghost_pro_test.go) | **Created** | Unit tests for seeder protection, zero-overlap eviction, 20s rotation cooldown, and mutual exclusivity. |
| [`engine-go/server.go`](file:///home/kar/funProjects/torrent-player/engine-go/server.go) | **Modified** | Added `ghostProMode`, `ghostProDial`, `isGhostProEnabled()`, `/settings` JSON handler, and mutual exclusivity resets. |
| [`engine-go/types.go`](file:///home/kar/funProjects/torrent-player/engine-go/types.go) | **Modified** | Added `GhostProMode` to `SettingsRequest` and `SettingsResponse`. |
| [`engine-go/video_priority.go`](file:///home/kar/funProjects/torrent-player/engine-go/video_priority.go) | **Modified** | `applyVideoStartupHints` scales head/tail spans when Ghost Pro is enabled (`max(head, 16MB, 2*pieceLen)`). |
| [`engine-go/playback.go`](file:///home/kar/funProjects/torrent-player/engine-go/playback.go) | **In Progress** | `playbackWindowState` and `playbackReader` struct updated with `lookahead pieceSpan`. Needs `repositionWindowLocked` updates. |
| [`android/core/src/main/kotlin/webtor/core/EngineClient.kt`](file:///home/kar/funProjects/torrent-player/android/core/src/main/kotlin/webtor/core/EngineClient.kt) | **Modified** | Added `client.setGhostProMode(enabled: Boolean)`. |
| [`android/core/src/test/kotlin/webtor/core/EngineClientTest.kt`](file:///home/kar/funProjects/torrent-player/android/core/src/test/kotlin/webtor/core/EngineClientTest.kt) | **Modified** | Added unit test `ghostProModeCanBeToggledWithoutChangingOtherSettings`. |
| [`android/app/src/main/java/webtor/app/UiModels.kt`](file:///home/kar/funProjects/torrent-player/android/app/src/main/java/webtor/app/UiModels.kt) | **Modified** | Added `ghostProMode: Boolean = false` to `UiState`. |
| [`android/app/src/main/java/webtor/app/LibrarySession.kt`](file:///home/kar/funProjects/torrent-player/android/app/src/main/java/webtor/app/LibrarySession.kt) | **Modified** | Added `setGhostProMode()`, sanitized cold-start preferences, handled mutual exclusions for toggles. |
| [`android/app/src/main/java/webtor/app/MainActivity.kt`](file:///home/kar/funProjects/torrent-player/android/app/src/main/java/webtor/app/MainActivity.kt) | **Modified** | Connected `onGhostProMode` callback from UI to `session.setGhostProMode`. |
| [`android/app/src/main/java/webtor/app/SettingsScreen.kt`](file:///home/kar/funProjects/torrent-player/android/app/src/main/java/webtor/app/SettingsScreen.kt) | **Modified** | Added Ghost Pro toggle row, added `enabled` parameter with 38% alpha gray-out and dynamic supporting text. |
| [`docs/DIRECT_PLAY.md`](file:///home/kar/funProjects/torrent-player/docs/DIRECT_PLAY.md) | **Modified** | Documented Ghost Pro Mode design and mutual exclusions. |

---

## 4. Pending Tasks for Codex

### Step 1: Finish `engine-go/playback.go`
In `repositionWindowLocked` (around lines 910-990):
1. Detect whether Ghost Pro is active:
   ```go
   isGhostPro := p.priorityOwner != nil && p.priorityOwner.server != nil && p.priorityOwner.server.isGhostProEnabled()
   ```
2. Adjust forward and urgent bounds if `isGhostPro`:
   - `urgentBytes`:
     ```go
     urgentBytes := int64(playbackUrgentBytes)
     if isGhostPro {
         pieceLen := int64(defaultPieceLength)
         if p.storage != nil {
             p.storage.mu.Lock()
             pieceLen = p.storage.chunkLength
             p.storage.mu.Unlock()
         }
         urgentBytes = max(urgentBytes, 16<<20, 2*pieceLen)
     }
     urgentEndOff := min(p.length, offset+urgentBytes)
     ```
   - `forward`:
     ```go
     if isGhostPro {
         forward = max(forward, 64<<20, 6*pieceLen)
         if p.boost {
             forward = max(forward, 96<<20)
         }
     }
     ```
   - Calculate `newLookahead`:
     ```go
     lookaheadBytes := int64(0)
     if isGhostPro {
         lookaheadBytes = max(32<<20, 4*pieceLen)
     }
     lookaheadEnd := min(p.length, forwardEnd + lookaheadBytes)
     newLookahead := spanFromFile(p.file, forwardEnd, lookaheadEnd)
     ```
3. Update `playhead := unionSpan(newUrgent, newReadahead, newLookbehind, newLookahead)` and `window`:
   ```go
   window := playbackWindowState{
       urgent: newUrgent, readahead: newReadahead, lookbehind: newLookbehind, lookahead: newLookahead,
       openPhase: p.openPhase, headBoost: p.headBoostActive, tailBoost: p.tailBoostActive,
   }
   ```
4. Update `currentDemandLocked` signature:
   ```go
   func (p *playbackReader) currentDemandLocked(urgent, readahead, lookbehind, lookahead pieceSpan) map[int]torrent.PiecePriority {
       demand := make(map[int]torrent.PiecePriority)
       raiseSpan(demand, urgent, torrent.PiecePriorityNow)
       raiseSpan(demand, readahead, torrent.PiecePriorityReadahead)
       raiseSpan(demand, lookahead, torrent.PiecePriorityHigh)
       raiseSpan(demand, lookbehind, torrent.PiecePriorityHigh)
       // ... existing boost logic ...
       return demand
   }
   ```
5. Store `p.urgent, p.readahead, p.lookbehind, p.lookahead = newUrgent, newReadahead, newLookbehind, newLookahead`.

### Step 2: Update `engine-go/ghost_pro.go` for Playback Contiguous Overlap
1. Allow `ghostProSwarmState` to recognize active playback demand:
   - When a reader is active on a `TorrentRecord`, get its contiguous piece range (`urgent + readahead + lookahead`).
   - For peers evaluated in `chooseVictim`, compute `contiguousOverlap`.
   - Prefer evicting peers with `contiguousOverlap == 0` when playback is active to immediately fill slots with peers who have the pieces right in front of the playhead.

### Step 3: Verification & Test Execution
Run Go tests:
```bash
cd engine-go && go test -race ./...
```
Run Android unit tests:
```bash
cd android && ./gradlew :core:test :app:testDebugUnitTest
```

### Step 4: Build & Install Release APK
Build signed release APK:
```bash
cd android && ./gradlew assembleRelease -Pwebtor.localSigning=true
```
Install to connected device (Target: `00121649A004784`):
```bash
adb install -r android/app/build/outputs/apk/release/app-release.apk
adb shell am start -n webtor.app/.MainActivity
```
