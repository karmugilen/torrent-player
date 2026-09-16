# Torrent Player 1.4.5 (22)

Resuming large downloads after app restart or force-stop could fail with
`Engine 500: verify saved data: context deadline exceeded`. The app rehashed
saved files inside a control request limited to 60 seconds.

Saved-data verification now runs in the background without that request deadline.
The library and notification show checking progress. Pause stops checking between
pieces, resume continues, and remove/shutdown cancel the verification worker.
When checking finishes, only missing or corrupt pieces need downloading.

Install this update over the existing APK and tap Retry or Resume on affected
downloads. Existing files are reused. A large download can take several minutes
to check; Play becomes available when checking finishes. A force-stop interrupts
the app, so checking restarts on the next resume.

The tracker cache, valid magnet trackers and peer connection settings are unchanged.

## Package and validation

- `torrent-player-1.4.5.apk`: arm64-v8a, 8,800,309 bytes, version code 22.
- Optimized release build signed with the same local certificate as 1.4.4.
- 26 Go tests with the race detector, Go vet, JNI smoke test, 14 core tests,
  30 app tests and Android release lint/build passed.
- The new regression fails against the original 1.4.4 configure handler with
  the exact reported timeout, and passes with this fix.
- Local TCP recovery verifies saved data, rejects a corrupted piece and transfers
  exactly the missing bytes. Full device force-stop validation remains for testing.

F-Droid 1.4.4 passed all nine pipeline jobs; the submission still needs maintainer
review and publication. The 1.4.5 recipe must pass its own pipeline.
