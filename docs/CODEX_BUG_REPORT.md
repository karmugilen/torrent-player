# Bug report

Direct Play seeking now keeps a rewind cushion in RAM and moves the whole window when the jump leaves that cushion. Short steps forward, back, and inside the window use bytes already buffered. A larger jump retargets urgent, readahead, and lookahead onto the new spot, and every open viewer is resized together so they still fit in the cache.

Each bug below was reproduced by a regression test on the unfixed code, then the same test passed after the fix. `LibrarySession` itself is not constructed in unit tests (this tree has no Robolectric), so the two Android call sites are covered by the pure functions and the engine endpoint they call.

## 1. A saved header is served as zeros

**Symptom.** The external player dies in the container header, or reports that Direct Play bytes were evicted, on a piece that was just saved. Later peer writes are ignored and a failed hash does not clear the piece.

**Cause.** `drainStartupPieces` in `engine-go/direct_storage.go` wrote a verified clone, then set `startupSaved` and `completed` without looking at RAM. Pass 1 of `ensureStreamSlotLocked` can evict that piece while the clone is still queued, and a later `WriteAt` fills a new zero-padded buffer. Reads prefer RAM. If that partial buffer was evicted again, `streamLost` stayed set and the read failed even though the file on disk was the verified piece.

**Fix.** On a successful commit (`direct_storage.go:339`), drop the RAM buffer when it is missing or differs from the clone, and clear `streamLost`, `streamRanges`, and `streamWhole`. The player then reads the saved file.

**Test.** `TestSavedMetadataCommitDropsPartialRAM` failed with zeros at offset 8192. `TestSavedMetadataCommitClearsEvictedFlag` failed with `direct play bytes were evicted`. Both pass after the fix.

## 2. Seeking stalls, and a second viewer overflows the cache

**Symptom.** A short rewind re-downloads. A jump into the middle waits while the old window still occupies RAM. A second open viewer makes verified pieces disappear. On large pieces (about 16 MiB) two viewers pin more than the cache even after both have been positioned.

**Cause.** `repositionWindowLocked` in `engine-go/playback.go` sized each Direct Play reader from `len(owners)` before that reader was published, and nothing resized the reader that was already playing. Lookbehind was 512 KiB and was left out of `streamDemand`, so those pieces were evicted first. `max(pieceLen, share-3*pieceLen)` also snapped a tight share back up to a whole piece, and a byte window spilled into the neighboring piece when the playhead was not aligned.

**Fix.** Direct Play windows are whole pieces (`directPlaySpansLocked`, `playback.go:1208`). Each reader counts itself. About a tenth of its share stays behind the playhead, at least two pieces when the share can hold them, taken from the same allowance so the total still fits. Opening, reading, parking, or closing a reader resizes the others after it releases its own lock (`flushShareRefresh`). Lookbehind pieces are marked in demand (`playback.go:327`). Download mode still uses a 512 KiB lookbehind. The policy paragraph in `docs/DIRECT_PLAY.md` matches this.

**Test.** `TestDirectPlaySeekIsSmoothForwardBackwardAndMiddle` failed with lookbehind `{127, 128}` (one piece) and would not bring two viewers inside 64 cache pieces. `TestDirectPlayLargePiecesStayInsideTheCache` failed with 6 pieces pinned in a 4-piece cache. `TestGhostProInstantPlayKeepsAllTiersWithinRAMAllowance` still requires one viewer to fill at least 90% and four viewers to stay inside the cache with every tier non-empty.

## 3. An old viewer keeps its window forever

**Symptom.** After the playhead moves to a newer viewer, the previous descriptor keeps its full piece window. RAM stays split, and peers stay up because that window still looks unfilled.

**Cause.** `idleExpired` used a one-shot timer. If the reader was still the playhead when the timer fired, it returned without parking and without arming another timer (`playback.go` idle path, previously around the `isPlayingLocked` check).

**Fix.** A reader that is still playing arms the next 15 second timer (`playback.go:928`). When it is no longer the playhead, the next fire parks it and the remaining viewers grow into the freed share.

**Test.** `TestFormerPlayheadParksOnceItIsNoLongerPlaying` failed with "playing reader dropped its idle timer". `TestPlayingReaderKeepsWindowAndPeersWhileIdle` still keeps the current playhead's window while a probe parks.

## 4. Idle Direct Play never rests, or rests and then loses every peer

**Symptom.** Ghost Pro Direct Play stays connected after the player's buffer is full, because lookahead was evicted and the rest check treated that as "still filling". Separately, a session that rests and is woken in the same moment can stick at zero peer connections. Pause and resume do not bring peers back, because `resting` is already false.

**Cause.** `playbackBufferFilling` required lookahead pieces, which are only `PiecePriorityHigh` and are the first evicted. `restDirectPlay` and `wakeDirectPlay` both called `SetMaxEstablishedConns` outside the peer-list lock, so rest's `SetMax(0)` could land after wake had restored the cap.

**Fix.** Rest waits on urgent, readahead, and the rewind cushion (`playbackWindowFilling` in `engine-go/direct_play_idle.go:192`). Both connection-cap updates take `peersMu`, and rest does nothing if wake already cleared `resting` (`direct_play_idle.go:117`).

**Test.** `TestPlaybackRestWaitsForRewindCushionNotLookahead` failed while lookahead was incomplete. `TestRestDoesNotZeroTheConnectionCapAfterWake` failed because the cap was 0 after wake. The hook that opens that window is nil outside the test.

## 5. Play on a blocked network starts the torrent anyway

**Symptom.** With Pause on low battery, or with the network policy blocking data, Play on a stopped or idle-dropped incomplete torrent shows the battery or network message, but the engine session is already added and resumed. The row stays unpaused. When the phone charges or the network returns, that torrent transfers on its own. A fully downloaded local file is unaffected.

**Cause.** `LibrarySession.play` called `restoreEntry` before the battery and network checks (`LibrarySession.kt`, previously just above the `engineId == null` restore).

**Fix.** `playbackDataBlock` (`Formatters.kt:254`) is consulted after the completed-file return and before `restoreEntry` (`LibrarySession.kt:1590`), and again before `/play` in case the block starts during restore.

**Test.** `PlaybackPolicyTest.playIsBlockedBeforeASessionStarts` covers the messages. It does not construct `LibrarySession`, so the call order was checked in `play()` rather than executed.

## 6. An update turns off a switch the user had saved

**Symptom.** After an update, Ghost mode, Peer exploration, or Fast peer search that the user turned on is off, and Ghost Pro (never opened) is on. Adaptive buffer that the user turned on is off, and Burst-aware buffer (never opened) is on. The saved values are still in preferences. A new install is unchanged.

**Cause.** A missing `ghostProMode` or `burstAwareBuffer` key took the new default `true`, and that default was treated as an explicit choice that forces the other switches off (`LibrarySession.kt` settings load).

**Fix.** `resolvePlaybackExperiments` (`Formatters.kt:274`) keeps a saved conflicting switch when the new key was never written. An explicit Ghost Pro or Burst-aware choice still turns the overlapping switches off. The loader uses `settings.contains` before applying those defaults (`LibrarySession.kt:152`).

**Test.** `PlaybackPolicyTest.aSavedSwitchBeatsAnUntouchedNewDefault` and `aNewInstallUsesTheRecommendedExperiments`.

## 7. Delete after Drop keeps dialing old peers

**Symptom.** Drop, and the 15 minute idle drop, correctly keep peer addresses for the next Play. Delete after that does not forget them, because the engine id is already gone. Keep-files on a live session also kept them. Adding the same torrent again dials those addresses until the hint TTL.

**Cause.** `handleRemove` forgets hints only when `destroyStore` is true, and Android delete called `remove` only when `engineId != null`. `deleteKeep` passed `destroyStore: false` even for a live session.

**Fix.** `POST /forget-peers` (`engine-go/server.go:1582`) forgets hints by info hash with no live torrent. `EngineClient.forgetPeerHints` calls it. Both delete paths call it (`LibrarySession.kt:1742` and `:1808`), including after a drop. Drop itself is unchanged.

**Test.** `TestForgetPeersForADroppedTorrent` failed with 404 before the route existed. `EngineClientTest.forgetPeerHintsPostsTheInfoHash` checks the request. `TestDropKeepsPeerHintsAndDeleteForgetsThem` still checks that Drop keeps hints.

## Not changed

**Episode switch above the 1 GiB startup cap.** `startupPlanLocked` stops at `directStartupTotalBudget`, and `setStartupFile` deletes saved pieces that fall out of the rotated plan. A huge pack can re-download an episode's head and tail after the user opens another episode and comes back. Keeping every piece would exceed the stated 1 GiB cap, so this was left as the cap's behavior.

**These were checked and are not bugs.** `warmDirectPlay` passes the pre-patch entry into `restoreEntry`, which re-reads the generation. Explicit Pause keeps `engineId`, so opening the file list does not restart it. Drop (`destroyStore: false`) keeps peer hints; only delete forgets them. Read paths do not wait on `rec.mu`. Lock order on the playing-reader path remains reader lock, then coordinator lock.

## Tests

Confirmed red before the fix, then green:

- `TestSavedMetadataCommitDropsPartialRAM`
- `TestSavedMetadataCommitClearsEvictedFlag`
- `TestDirectPlaySeekIsSmoothForwardBackwardAndMiddle`
- `TestDirectPlayLargePiecesStayInsideTheCache`
- `TestFormerPlayheadParksOnceItIsNoLongerPlaying`
- `TestPlaybackRestWaitsForRewindCushionNotLookahead`
- `TestRestDoesNotZeroTheConnectionCapAfterWake`
- `TestForgetPeersForADroppedTorrent`

Final runs:

- `cd engine-go && go test -race -count=1 .` — pass (`ok webtor/engine-go`, 67.9s)
- `cd android && ./gradlew :app:testDebugUnitTest :core:test` — `BUILD SUCCESSFUL`
- `engine-go/patches/torrent` was not modified, so its tests were not run

The patched torrent library, app version, and `artifacts/` were left unchanged. Nothing was installed on a device.
