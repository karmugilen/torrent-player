# Resume recovery after process death

Date: 2026-09-16

## Confirmed cause

`/configure` attaches existing Android file descriptors, then synchronously
hashes every selected piece under the record lock. JNI control requests have a
60-second deadline. Large saved downloads exceed it and return
`Engine 500: verify saved data: context deadline exceeded`. Retrying repeats the
same work. The record lock also blocks status and pause during verification.

## Fix

1. Attach descriptors and acknowledge configuration immediately. Check saved
   pieces in a cancellable background task independent of the request deadline.
2. Keep payload transfer disabled until verification finishes. Preserve valid
   saved pieces; download missing/corrupt pieces afterward. Never trust file
   length or previously displayed progress as proof of valid data.
3. Report checking progress to Android, preserve displayed saved progress during
   the check, and keep pause/resume/remove responsive. Pause also pauses checking
   between pieces. Completion respects the latest pause and file selection.
4. Cancel and join verification before removing a torrent or closing the engine.
   A subsequent launch reopens the same files and starts a safe fresh check.
5. Regress expired request deadlines, restart with partial/corrupt data, pause and
   resume during verification, removal/shutdown and actual peer transfer after
   checking. Run Go race tests, JNI and Android checks; build an update APK.

## F-Droid

The 1.4.4 submission pipeline 2851213344 passed all checks. MR !48893 is still
open; maintainer review, signing and publication remain pending. Update the
existing submission to the tested fix once its source/release is published.

## Validation completed

- [x] Deadline regression reproduces the exact failure with the old handler.
- [x] Background check, pause/resume, cancellation and restart/peer recovery pass.
- [x] Go race/vet, JNI, Android unit tests, release lint and signed APK pass.
- [x] F-Droid clean source build and APK scan pass for source `b772ad8`.
- [x] The existing F-Droid MR now targets 1.4.5; pipeline 2853130363 succeeded.
- [x] Publish/merge 1.4.5 and report the final external review status.

## Delivery

- [GitHub PR #2](https://github.com/karmugilen/torrent-player/pull/2) merged.
- [1.4.5 release and APK](https://github.com/karmugilen/torrent-player/releases/tag/v1.4.5) published. Remote asset SHA-256 matches the local tested APK.
- F-Droid [pipeline 2853130363](https://gitlab.com/karmugilrc/fdroiddata/-/pipelines/2853130363) passed all nine jobs; MR checklist and maintainer notification posted; maintainer review/publication remains pending.
- Device force-stop/reboot testing remains for the user. No user download files were changed during this fix.
