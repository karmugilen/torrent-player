# Torrent Player 1.4.4

Public torrents can now discover peers through a maintained tracker list that is
fetched in the background and cached across app restarts. Downloads and playback
continue using the existing engine while the list refreshes.

## Changes

- Validate trackers supplied by magnets and torrent files before adding them.
  Ignore discovery labels such as `tr=DHT`, malformed URLs and unsupported
  endpoints instead of triggering the library's `unknown scheme` panic. Preserve
  supported tracker URLs, tiers and path/query passkeys; DHT remains enabled.
- Daily HTTPS refresh of ngosang/trackerslist's `trackers_best.txt`, conditional
  requests, a 10-second timeout and a 64 KiB response limit.
- Atomic app-private cache, URL validation, duplicate removal and retries starting
  at 15 minutes and backing off to at most 24 hours. Offline use retains the last
  good list or bundled fallback.
- Up to 20 downloaded tracker URLs plus four bundled WebSocket signaling URLs.
  Each torrent receives at most 24 supplemental trackers during its lifetime.
  Supplied tracker tiers remain intact; refreshing does not reset peer connections.
  Exported metadata/magnets exclude managed supplements so restarts cannot
  promote them into permanent trackers.
- Supplements are added only after metadata confirms the torrent is public.
  Magnets use supplied trackers, peer hints and existing DHT for initial metadata.
  This change does not implement a separate private-swarm networking mode.
- Wire the existing six-second tracker dial timeout to the library's dedicated
  tracker dialer as well as its HTTP dialer.
- Exclude the library's unused SQLite/Bolt storage backends from the native
  build; the app continues using its existing document storage implementation.
  Include license notices for linked Go dependencies.
- Includes the tested native Go migration, direct JNI commands/events, smaller APK,
  reliable storage and pause/resume, and verified partial/completed media playback.

Peer limits, TCP, uTP, WebRTC, DHT and PEX settings are unchanged. Actual peer counts
and speed depend on the swarm and network; no percentage improvement is claimed.

## Build and testing

Android package `webtor.app`, version 1.4.4 (21), arm64-v8a, Android 8.0 or newer.
The upstream APK retains the certificate used by previous GitHub releases so it
can be installed as an update. That existing certificate is an Android debug
certificate; the APK itself uses release optimization and resource shrinking.
F-Droid builds from source and signs independently.

Verification results and the final APK checksum are recorded in
[`artifacts/RELEASE_1.4.4_TEST_REPORT.md`](../artifacts/RELEASE_1.4.4_TEST_REPORT.md).

## Sources

- Tracker list: https://github.com/ngosang/trackerslist
- Existing F-Droid submission: https://gitlab.com/fdroid/fdroiddata/-/merge_requests/48893
