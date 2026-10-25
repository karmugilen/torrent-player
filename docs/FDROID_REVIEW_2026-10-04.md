# F-Droid review follow-up (2026-10-04)

Review by @mezinster, posted 2026-10-03T20:30:09.784Z on [MR !48893](https://gitlab.com/fdroid/fdroiddata/-/merge_requests/48893).

## Reviewer comment

Static review of Torrent Player 2.0.0 (`5cfbdc1e`, pipeline 2858040818). cc @linsui VirusTotal: **0/67** — https://www.virustotal.com/gui/file/36b314c122709b59a58912a8aa84368462cc629ebb99cbc1538326bcaf47cb68

@karmugilrc thanks, the recipe looks good: the commit is the latest tag `v2.0.0`, MIT matches upstream, `libengine.so` is built from source by `engine-go/build.sh` with `go mod verify`, the anet patch avoids its prebuilt binaries, there is no tracking or GMS, and there is no built-in search or indexer (only magnet/.torrent intents). Permissions are INTERNET, ACCESS_NETWORK_STATE, FOREGROUND_SERVICE(_DATA_SYNC) and POST_NOTIFICATIONS, with storage through SAF. Engine control goes over JNI, and the only socket is the loopback stream server. Backup covers only `settings.xml`.

A few points:

1. **Network connections that the description doesn't mention.** The engine starts in `Application.onCreate` (`WebtorApp.kt:39`) and refreshes a tracker list from `https://raw.githubusercontent.com/ngosang/trackerslist/master/trackers_best.txt` once a day (`engine-go/trackers.go:25`, started at `server.go:214-215`). Up to 24 public trackers are then added to every non-private torrent (`trackers.go:326-366`, fallback list `server.go:34-57`). Private torrents are correctly skipped at `trackers.go:333`. WebRTC also uses STUN servers at Google, Twilio and Cloudflare (`server.go:173-182`). `full_description.txt` mentions DHT, UDP trackers and WebSockets but not these. Could you describe them in the description? A setting to turn off the tracker-list refresh, the extra trackers or WebRTC would be even better.
2. **Changelog**: `fastlane/metadata/android/en-US/changelogs/` has 19-22 but no `23.txt` for 2.0.0.
3. **Reproducible builds for future releases**: as far as I can tell, 1.4.5 and 2.0.0 verify because the GitHub assets were replaced with the buildserver output and then signed. Your own build differed in `classes.dex`, `baseline.prof` and `libengine.so`. The bot will compare every future tag against the APK on GitHub, so each release has to be built in the same environment as F-Droid (or made deterministic) before you publish it. Otherwise the update MRs will fail verification.
4. Nits: the HTTP user agent and client version still say 1.4.5 (`server.go:170-171`), only arm64-v8a is built, and the MR description still describes 1.4.5.

On-device testing is still to be done.

## Scope of this update

- Disclose automatic tracker-list requests, supplemental public trackers, STUN providers, IP exposure, lack of opt-out controls and arm64 support in the store description. Clarify that the no-tracking claim concerns advertising and analytics.
- Add the missing versionCode 23 changelog.
- Keep the released 2.0.0 application code and APK unchanged. The stale 1.4.5 protocol identification and optional network controls are deferred to a future release.
- Refresh the MR description to 2.0.0 and verify the metadata-only source revision against the existing signed reference APK.
- On-device testing remains pending; no Android device is connected here.

## Future release verification

Build each candidate using the current F-Droid buildserver image and the same recipe, Go version, Java/Gradle toolchain and Android NDK used by F-Droid. Run source and APK scans. Sign the buildserver-produced APK with the existing release key outside the source repository; do not use the debug signer. Before announcing or distributing a release, verify that F-Droid can rebuild that source and reproduce the signed candidate using `Binaries` and `AllowedAPKSigningKeys`. A successful local Android build alone is insufficient.

If reproducible comparison differs in `classes.dex`, `baseline.prof` or `libengine.so`, resolve the environment or deterministic-build issue before publishing the candidate. Do not overwrite an already distributed release to repair verification. Advance to a new version for application changes. The current release has passed the pipeline comparison; this update must pass again before being described as verified.

## Verification of the metadata-only source

The Android and Go git tree hashes at `92b1a9914c17d0fb03cd6577ce889faf66cc2cd9` match `v2.0.0` exactly. [Pipeline 2910503829](https://gitlab.com/karmugilrc/fdroiddata/-/pipelines/2910503829) rebuilt 2.0.0 and successfully compared it against the existing signed reference APK. All nine jobs passed, including the final APK scan. On-device testing remains pending.
