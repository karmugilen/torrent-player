# Cached public tracker discovery and release plan

Date: 2026-09-15

## Objective

Improve the chance of finding reachable peers when a public torrent's supplied
trackers are unavailable. Keep the working playback, storage, pause/resume, JNI,
WebRTC, TCP, uTP, DHT and PEX behavior and existing connection budgets.
More trackers cannot create seeds or guarantee a speed or peer-count increase.

## Findings

- The current engine appends 20 hardcoded trackers on every add path, even for
  private torrents. Some fallback endpoints are old.
- The pinned anacrolix/torrent library has a tracker replacement API whose Stop methods do
  not remove the running announcers. Use bounded additions instead. It already schedules tracker announces and retries.
- ngosang/trackerslist publishes a maintained, ranked `trackers_best.txt` list.
  Fetching that small list is preferable to probing hundreds of trackers on a phone.
- The working Go migration and playback changes are still uncommitted on
  `go-engine`; the release must include them.
- F-Droid MR !48893 exists. Its old 1.4.3 Node build and APK scan passed, but
  `fdroid rewritemeta` failed. The new Go build needs its own validation.
- GitHub CLI authentication is currently invalid; publishing requires the user
  to restore their configured login. GitLab authentication works.

## Implementation

1. Add a Go tracker pool with an app-private, versioned JSON cache. Load it
   immediately, use built-in fallbacks if absent/invalid, and refresh asynchronously.
2. Fetch only the fixed HTTPS `trackers_best.txt` source, with a 10-second timeout,
   a 64 KiB response limit, conditional ETag/Last-Modified requests, daily refresh,
   and bounded exponential retry after failures. Cancel work at engine shutdown.
3. Normalize and deduplicate public UDP/HTTP(S)/WS(S) URLs, reject malformed and
   local/private literal addresses, and cap downloaded entries at 20. Retain the
   four bundled WebSocket endpoints for WebRTC discovery. Never cache torrent
   hashes, peer addresses, user tracker credentials, or torrent-specific trackers.
4. Preserve every torrent's original tracker tiers. Supplement confirmed public
   torrents with at most 24 entries; add refreshed entries only while a torrent has room under that
   lifetime cap. New/restored torrents use the current pool. Existing announces
   and peer connections stay intact; no forced resets. Private torrents get no additions.
5. Magnet privacy is unknown until metadata arrives. Use its supplied trackers,
   peer hints and existing DHT for metadata first, then attach public supplements.
   This intentionally avoids announcing an unknown private hash to extra trackers.
6. Keep tracker updates off the add/play paths. No new polling in Android, no
   connectivity-limit changes, and no aggressive reannounce or health-probe loop.

## Verification and delivery

- [x] Regression tests: parsing, bounds, cache corruption/restart, conditional
  refresh, failure preservation/backoff, concurrency and shutdown cancellation.
- [x] Integration tests: supplied tracker preservation, public/private torrent
  handling, deferred magnet attachment, bounded additions and no peer drops.
- [x] Go test/race checks, existing playback/JNI tests, Android unit tests and
  optimized release APK; inspect APK identity/signature/native library.
- [x] Bump to the next available release version; record checksums and test results.
- [ ] Validate F-Droid metadata and source-build recipe with verified Go tooling and
  dependency preparation. Update the existing MR with current evidence.
- [ ] Commit, push and merge the tested release, publish the APK on GitHub, and
  report F-Droid's actual pipeline/review status without claiming acceptance.

## Sources

- https://github.com/ngosang/trackerslist
- https://webtorrent.io/docs
- https://f-droid.org/docs/Submitting_to_F-Droid_Quick_Start_Guide/
- https://gitlab.com/fdroid/fdroiddata/-/merge_requests/48893
