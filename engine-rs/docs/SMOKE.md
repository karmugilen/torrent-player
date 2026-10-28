# Android-flow public-swarm smoke tests

Status: **FAIL, awaiting engine fixes**. Six Go host flows pass; the six
initial Rust host flows fail on the same two legal public samples. Host results cannot certify
JNI preview or Android proxy-descriptor playback. No authorized device was
attached at the first `adb devices` check on 2026-10-27.

## Reproduce

Run from the worktree root. Each case starts its own engine with a fresh cache,
uses public trackers and DHT, records replies in JSON, terminates its child and
deletes its payload directory even on failure. No synthetic seeder is involved.

```sh
cd engine-rs
CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/codex-smoke CARGO_BUILD_JOBS=3 \
  CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
  cargo build --release -p wt-engine --bin engine-host
cd ../engine-go
go build -tags=nosqlite,noboltdb -o /home/kar/.cache/wt-tmp/engine-go-host .
cd ..
python3 engine-rs/tools/smoke/smoke.py \
  --engine /home/kar/.cache/wt-tmp/engine-go-host --label go \
  --out engine-rs/tools/smoke/results/go.json
python3 engine-rs/tools/smoke/smoke.py \
  --engine /home/kar/.cache/wt-targets/codex-smoke/release/engine-host --label rust \
  --out engine-rs/tools/smoke/results/rust.json
python3 engine-rs/tools/smoke/phone.py \
  --out engine-rs/tools/smoke/results/phone.json
```

The public samples are Sintel and Big Buck Bunny, obtained directly from the
[WebTorrent legal test torrents page](https://webtorrent.io/free-torrents).
The script resolves the current magnet links from that page and preserves all
tracker, webseed and exact-source parameters. The `magnet` case sends no metadata
to the host. `torrent` sends the public .torrent as base64 `torrentData`, matching
Android's content-URI import. `immediate` sends the magnet and immediately calls
configure; its descriptor-array size is read from the public fixture before
starting the timer, without injecting its metadata into the engine.

Limits: 512 KiB/s download setting, 32 MiB RAM cache, 55 peers, five 64 KiB range
reads, a 60-second observation window, 360-second process deadline and a
conservative 128 MiB allocation/peer-byte watchdog below the 200 MiB per-download
ceiling. Work is confined to `/home/kar/.cache/wt-tmp/codex-smoke`; the target
directory is `/home/kar/.cache/wt-targets/codex-smoke`. A safety stop is a failure.
Metadata has Android's 90-second prepare timeout, configuration has a 65-second
client timeout around the engine's 60-second wait, and each stream has a
20-second socket timeout. `--seconds 1` is a quick diagnostic, not the measured
acceptance run. Results preserve binary SHA-256, UTC timestamp, every control
request/reply, stream hashes, CPU seconds and peak RSS sampled from `/proc`.

## Exact Android call sequences

Source locations refer to this worktree. `EngineClient` actually lives in
`android/core/src/main/kotlin/webtor/core/EngineClient.kt`, rather than under
`android/app/src/main/java`. `EngineHost.request` calls JNI synchronously; it
does not itself issue localhost HTTP. The host driver exercises the same
control methods over the desktop control listener. There is no arbitrary delay
between adjacent control calls: each follows the previous response and local
storage work. UI actions introduce user-dependent time.

| Action | Control/native sequence and timing |
| --- | --- |
| Engine start/settings | `WebtorApp.onCreate` starts native engine; `EngineHost.awaitReady` bounds waiting at 20 s. `LibrarySession` initialization posts separate `/settings` bodies in order: `maxPeers`, `downloadRateBytesSec`, `dialRatePerSec` (30 or 80), `peerExploration`, `ghostMode`, `ghostProMode`, `claudeSeekMode`, `hedgeRequests`, `slowPeerYield`, `dropChokers`, `idleDropMinutes`, auto/manual `ramCacheBytes`, seeding settings, `payloadTransfersAllowed`; applies anti-DPI via `/bypass`; then `GET /cache`, restores the library. Settings are partial updates, not one replacement object. |
| Connectivity/settings changes | Network changes are serialized/coalesced: wait for ready (retry after 1 s), `POST /settings {payloadTransfersAllowed}`, `POST /network {generation,connected}`, seeding update; retry failures after 2 s. Individual settings controls post only their own field. `/settings` supplies max peers/rates/seek statistics; `/cache`, `/cache/clear`, `/bypass` are independent controls. |
| Add magnet | `AddSheet` debounces draft text 250 ms; prefetch can start before Add is pressed. Replace any prior draft with `/remove {destroyStore:true}`. `POST /add {torrentId:magnet,prepare:true}`; retain returned ID immediately. `waitReady`: `GET /torrent/<id>` then native `awaitChange` until `ready`, at most 90 s; no repeated configure polling. Once ready, default video selection is sent to `/select` inside `runCatching`. Go currently returns 409 before storage is configured; Kotlin deliberately tolerates that reply. |
| Add .torrent | `MainActivity` accepts VIEW/SEND content URI and `application/x-bittorrent`; `LibrarySession.openTorrent` copies/imports it and uses the same `add(...prepare:true)`/waitReady/selection sequence. Saved imports can supply base64 `torrentData`. The exported activity refuses arbitrary filesystem paths. |
| Choose files before commit | Each checkbox/All/None/Videos changes draft selection and asynchronously posts `/select {id,selected}` only once metadata is ready; failures are tolerated before configuration. |
| Commit download/Direct Play | Ready metadata and a nonempty selection are required for Direct Play. `GET /metadata/<id>`; create/open Android destination files for download, or use one null descriptor per file for Direct Play; refresh RAM setting; `POST /configure {id,descriptors,selected,storageMode:"download"|"stream",streamCacheBytes}`; close Android descriptor copies in finally. Direct Play creates no destination descriptors. A pending normal download can be committed before metadata: retain native draft, later metadata-ready status schedules materialization, fetches `/metadata`, opens destinations, configures, then pauses/resumes to match saved intent. |
| Change committed selection | Download: open/create destinations, `/select {id,selected,descriptors}`, close copies. Direct Play: `/select {id,selected}` with **no descriptors field**. Empty selection is a valid stopped configuration. |
| Play | Clear prior file-only playback with `/file-only {id,enabled:false}`. A verified complete destination can play locally with no torrent stream; file-only priority may still call `/play`. Otherwise restore missing native record, `GET /torrent/<id>`, defer while `checking`, `/resume` if paused, `/play {id,fileIndex,offset,...enabled flags,nextFileIndex?}`. Offset is explicit zero for From Start, otherwise a saved byte hint. Optional preload polls `/torrent` up to its deadline. HTTP method launches `streamUrl`; proxy method opens JNI playback instead. |
| HTTP stream and seek | `GET <streamUrl>` / Range requests use `/torrent/<id>/file/<index>` on the **stream server**, not the control port. Player seeks supply byte offsets through Range; no `/seek` control or repeated `/play` is necessary. Smoke verifies 206, matching Content-Range, exactly 64 KiB and SHA-256; five offsets are 5%, 25%, 50%, 75%, 95% of usable file length. |
| Native proxy playback/seek | `PlaybackProvider.openFile`: `openPlayback(id,index)` → Android proxy FD. `onRead(offset,size,data)` calls `readPlayback(handle,offset,size,data)` on its own HandlerThread; seeks are read offsets. `onRelease` → `closePlayback(handle)` and lease release. Negative count is EIO. This is not an HTTP endpoint. |
| Preview | `ThumbnailRepository` first queries `/preview-state/<id>?fileIndex=<index>`; local complete/partial file decoding is attempted before native fallback. Fallback is JNI `openPreview` → `MediaMetadataRetriever` with seekable `MediaDataSource`; reads use `readPreview` at decoder-selected offsets, bounded to 1 MiB per read; close in finally and via a 10 s deadline. Return 0 handle means unavailable; nonpositive reads mean EOF. Up to three distinct frames; incomplete data stops after the first frame. Active visible incomplete previews retry every 4 s; paused/complete entries stop retrying. Host `/preview-state` success does **not** establish preview decoding success. |
| Focus and telemetry | Episode/detail focus posts `/focus {id,fileIndex}`; clear uses JSON null. Details poll `/pieces/<id>` and `/preview-state`, wait for native events up to 30 s (1 s in Ghost Pro), throttle to at least 1 s. Connection map polls `/connections/<id>` each 1 s. Library event observer waits up to 5 s and refreshes changed or metadata-pending native records; failures retry after 500 ms. |
| Pause/resume | Existing native ID: `/pause` or `/resume`; retain the user's manual pause separately from `payloadTransfersAllowed`. Missing ID: restore first. Pausing incomplete data must prevent further unfinished playback. |
| Remove/drop | Delete: `/remove {id,destroyStore:eraseFiles}`; remove local library/destination state as requested. Drop/stop native session preserves storage with `destroyStore:false`. Forgetting discovery addresses uses `/forget-peers {infoHash}`. `EngineClient.remove` tolerates only 404. App shutdown removes sessions with `destroyStore:false` and explicitly does **not** call `/shutdown`. |
| Restart/restore | Android owns persisted library JSON; the host does not auto-recreate records. Eligible saved rows call `/add {torrentId:source,prepare:true,torrentData:savedMetadata?}`. Metadata-pending/no-files rows immediately `/pause` or `/resume`, read `/torrent`, and materialize after metadata arrives. Known metadata rows `waitReady` at most 30 s, fetch `/metadata` only if not saved, open destinations (or nulls for Direct Play), refresh RAM, `/configure`, close descriptors, `/pause` or `/resume`, then `/torrent`. Restore tolerates `already configured`, honors generation/removal checks, and removes stale native records preserving storage. |

Primary code: `LibrarySession.kt` initialization 190–228, network 475–497,
selection 790–860, commit 900–1040, play 1358–1445, prepare 1996–2145,
restore 2279–2420, observer 2490–2580; `EngineClient.kt` 190–455;
`EngineHost.kt` 20–92; `PlaybackProvider.kt` 60–99;
`ThumbnailRepository.kt` 110–160 and 240–350; `VideoPreviewState.kt` 20–43;
`MainActivity.kt` 315–340.

The reported installed-APK failure, `/add` immediately followed by
`/configure`, is retained as an additional regression. The current Kotlin
prepare path inspected above waits for ready; attributing that earlier phone
trace to the current source without the APK's exact revision would be incorrect.

## Measurements and findings

Initial neutral-settings results (before fixes) are below. The 60-second
throughput column here reflects the older observation-only window, where Direct
Play may become idle after fetching startup ranges; `go-active.json` adds a
continuous player reader and supersedes those throughput figures. Native
preview remains unverified. Failed Rust CPU/RSS numbers describe a stalled engine
and **cannot** be compared as a performance win.

| Sample / mode / storage | Engine | Metadata s | First byte s | 60 s MiB/s | Seek s (5/25/50/75/95%) | Peak RSS MiB | CPU s | Host flow |
| --- | --- | ---: | ---: | ---: | --- | ---: | ---: | --- |
| big-buck-bunny / immediate / stream | go | 3.126 | 0.885 | 0.092 | 0.060, 0.782, 1.102, 0.684, 1.066 | 75.215 | 3.250 | PASS |
| big-buck-bunny / immediate / stream | rust | unavailable | unavailable | unavailable | unavailable | 8.902 | 0.010 | FAIL |
| big-buck-bunny / magnet / stream | go | 1.027 | 4.109 | 0.078 | 1.311, 0.759, 1.626, 1.021, 2.343 | 101.562 | 2.850 | PASS |
| big-buck-bunny / magnet / stream | rust | unavailable | unavailable | unavailable | unavailable | 9.023 | 0.150 | FAIL |
| big-buck-bunny / torrent / stream | go | 0.041 | 2.185 | 0.062 | 1.445, 1.402, 1.758, 2.345, 1.157 | 80.387 | 2.630 | PASS |
| big-buck-bunny / torrent / stream | rust | 0.001 | unavailable | unavailable | unavailable | 9.621 | 0.030 | FAIL |
| sintel / immediate / stream | go | 1.061 | 6.066 | 0.057 | 1.065, 1.110, 2.059, 0.604, 1.634 | 83.164 | 2.930 | PASS |
| sintel / immediate / stream | rust | unavailable | unavailable | unavailable | unavailable | 9.262 | 0.010 | FAIL |
| sintel / magnet / stream | go | 1.018 | 2.891 | 0.060 | 1.876, 1.452, 0.761, 1.547, 1.253 | 87.207 | 2.930 | PASS |
| sintel / magnet / stream | rust | unavailable | unavailable | unavailable | unavailable | 9.117 | 0.160 | FAIL |
| sintel / torrent / stream | go | 0.041 | 2.719 | 0.011 | 8.513, 0.931, 0.733, 1.476, 1.227 | 87.293 | 2.750 | PASS |
| sintel / torrent / stream | rust | 0.001 | unavailable | unavailable | unavailable | 9.844 | 0.020 | FAIL |

`go.json` retains two early immediate-case harness failures: a duplicate configure
was sent after the immediate configure had already succeeded. That harness error
was corrected; both full reruns in `go-immediate.json` pass and supersede those
two entries. Prepare `/select` returning 409 is tolerated by Kotlin and by the
corrected driver, with `tolerated_by_kotlin` recorded in the trace.

Filed in `docs/requests.md` (new owners after coordinator's roster change):

- **codex-engine:** magnet `xs`/`as` metadata sources are discarded; Go resolves
  metadata around 1 s, Rust normal prepare times out at 90 s and immediate
  configure at 60 s. Both Sintel and Big Buck Bunny reproduce. Reverification
  pending owner fix.
- **codex-play:** official .torrent metadata is ready and `/play` returns 200,
  but the stream has no first byte within 20 s on both samples. Standalone
  `WebseedPeer` fetching is not called by the production client/torrent runtime.
  Reverification pending owner fix.
- **codex-engine:** prepare `/select` returns Rust 200 versus Go 409 before
  configuration. Reverification pending owner fix.
- **codex-engine/codex-play:** live download-speed telemetry is hardcoded zero
  on the inspected Rust path; request included with the payload regression.

The driver now offers `--profile app` (default), enabling UiState's Ghost Pro,
Claude seek, hedging, slow-peer yield, choker dropping, play boost and adaptive
buffer; download playback also supplies `fileOnly:true`. `--profile neutral`
disables experiments for isolation. Both profiles deliberately cap download to
512 KiB/s and cache to 32 MiB for test limits. `--storage download` passes live
inherited writable descriptors into both hosts, matching native descriptor
configuration without writing Android source. The 1-second download diagnostic
is only a harness check, not a 60-second throughput result.


## Phone

`results/phone.json` records `adb devices` with an empty list. The phone driver
selects the newest APK by modification time from the required artifacts folder,
runs `adb install -r`, starts an explicit magnet VIEW intent in `webtor.app`,
and captures bounded engine/error/crash logcat without clearing it. It never
uninstalls or clears app data. Installation, VIEW delivery and logs still need
a connected device; Play, seek and preview require actual UI/native verification.

## Peer parity (grok-wire + grok-swarm)

Measured with `tools/smoke/peers_compare.py` (60 s, max-peers 55), same machine.

### Fixes landed (wire / peer_conn — grok-wire)

- Full Go `sendInitialMessages`: LTEP (`m`/`v`/`p`/`yourip`/`reqq`/`upload_only`/`encryption`/`metadata_size`), Fast HaveAll/HaveNone/bitfield, Port, Unchoke
- `PeerMaxRequests` 250 + advertise `reqq` 1024; apply peer `reqq` clamped to 250
- Unbounded outbound write queue (no silent Piece `try_send` drops)
- Conn reader `tokio::select!` **biased** so Piece reads beat outstanding-aware wake timers (was starving reads → ~45 s stalls at high pipeline)
- `expire_stale_requests` (RTT×8, min 4 s) so stuck outstanding can refill
- Outstanding/Normal-priority `wake_interval` + choke reconcile when `data_upload_disallowed` flips; `allow`/`disallow_data_upload` wake conns
- Local `transfer_test` 64 MiB: **5/5 pass** ~2.95 MB/s debug with full handshake + max=250

### Fixes landed (swarm / dial / metadata / PEX — grok-swarm)

- Continuous DHT announcer; tracker `numwant` 200; dialPool TCP+uTP; MSE fallback; PopMax dial order; high-water trim
- Failed dials re-queue with exponential backoff (5s…120s, max 8 attempts) so PopMax does not burn DHT/tracker contacts until rediscovery
- Tracker peers marked trusted; announce slots nudged when `want_peers` and next_at >2 min
- `peer_closed` re-queues outgoing peers (5 s); ut_metadata timeout retry; PEX add/drop/genmsg
- **Retention:** ignore malformed ut_metadata (`missing msg_type`) and soft-discard PEX Cooldown/TooFrequent instead of closing the peerconn — was dropping ~40 dial_ok down to ~4 live peers
- **Dial race:** prefer **uTP** when both TCP+uTP raw-connect within 200ms grace (TCP kept as handshake spare); fast-fail preferred MSE (2s) only when uTP won; other-transport plaintext-then-MSE fallback after same-transport Go fallback
- **Dial order:** among eligible peers, never-failed before retries, then trusted, then source rank — stops burning half-open on firewalled tracker contacts while DHT waits
- **Mid-file responsive:** playback enables dirty-chunk reads outside startup head/tail (Go applyResponsiveLocked); was always false
- **drop_chokers loop:** policy sampler now calls `Torrent::drop_chokers(5s)` when setting on (Go dropChokersLoop) — was AtomicBool-only
- **MSE fallback:** other-transport retries always try both obfuscation modes (preferred=false no longer skips MSE)
- **SeekTuning wire:** EngineServer now `set_seek_tuning` on new()+settings (Go `baseSeekTuning`); hedge/slowPeerYield/learnedArrival were AtomicBool-only and never reached wt-core
- **Sticky Interested:** do not send NotInterested while incomplete and peer still has pieces we lack (qBittorrent punishment)
- **Peak floor:** preserve `peak_requests` while choked; floor at 64 so WholeQueue wakes cannot collapse nominal_max to 2; ceiling 128 so stall cannot walk peak to LTEP reqq 250
- **MSE preferred fast-fail:** 2s on preferred obfuscation then plaintext/other-transport fallback (was burning 5s × MSE fails)
- **Half-open:** dial path raises torrent/global half-open to at least 64/128 (engine config may still say 24/48) so timeout-heavy Ubuntu can keep more dials in flight
- **uTP BEP 29 (wt-wire):** fixed swapped send/recv connection_ids; SYN-ACK `ack_nr = seq-1` (libtorrent ignores DATA with `ack_nr > seq-1`); wall-clock timestamps; ACK every DATA; retransmit while blocked on read. Public qBittorrent HS ok; `WT_FORCE_UTP` now holds 15–16 uTP peers (was 0 dial_ok)
- **drop_chokers gate:** only drop when dial-pool waiting > 2× free established slots (Go WebtorDropChokers); was killing 107.175 while maxPeers still free
- **TCP prefer:** 750ms grace when uTP wins dial race so TCP can still take the conn (uTP piece path often stays at downloaded=0)
- **Client workers:** scale wt-client tokio workers to 8–16 from available_parallelism (fixed 4 starved dials under stream; dial_ok ~19 vs ~60 idle)
- **Request refill cap:** `refill_peer_requests` uses peak×2/pipeline, never bare max_requests=250 (was flooding pending=100–228 with downloaded=0)
- **/connections diagnostics:** optional peerChoking / amInterested / pendingRequests / peerCompletion for swarm probes

### Numbers (latest same-machine runs after uTP BEP29 fix)

| Label | meta s | peers min→max | speed peak MiB/s | notes |
|-------|--------|---------------|------------------|-------|
| go-ubuntu-r17 | 11.0 | 10→37 | **7.15** | TCP 30 + uTP 5; **34/35** downloaded>0; 297 MiB |
| go-ubuntu-r16 | 4.0 | 11→41 | 5.59 | TCP 30 + uTP 11; 38/41 active; 228 MiB |
| go-ubuntu-r18 | 14.0 | 16→32 | **10.55** | TCP 23 + uTP 9; 29/32 active; **452 MiB** |
| go-ubuntu-r19 | 10.0 | 14→33 | **12.02** | TCP 23 + uTP 9; **31/32** active; **496 MiB** |
| rust-ubuntu-r59 | 5.0 | 8→20 | 1.23 | TCP 17 + uTP 3; **13/20** active; 12 MiB; drop_chokers gate+req cap; 107.175 2.9 MiB pending~65; mid-run decay |
| rust-ubuntu-r56 | 9.5 | 13→21 | 1.30 | TCP 16 + uTP 3; **13/19** active; 34 MiB; workers+TCP prefer |
| rust-ubuntu-r53 | 3.5 | 8→19 | 1.40 | TCP 15 + uTP 4; 9/19 active; 23 MiB; drop_chokers gate keeps 107.175 |
| rust-ubuntu-r51 | 3.0 | 9→19 | 1.59 | TCP 18; 10/18 active; 40 MiB; mid-file responsive; 107.175 4.1 MiB |
| rust-ubuntu-r50 | 3.0 | 7→15 | 1.38 | TCP 12 + uTP 3; 6/15 active; 30 MiB; nodelay+floor64 |
| rust-ubuntu-r49 | 4.5 | 5→18 | 1.46 | TCP 15 + uTP 3; 8/18 active; 34 MiB |
| rust-ubuntu-r48 | 4.0 | 12→16 | **2.05** | TCP 10 + uTP 2; **11/12** active; **48 MiB**; drop_chokers wired |
| rust-ubuntu-r47 | 4.0 | 5→18 | 1.88 | plaintext-first trial (bt_hs spike); 8/18 active; 40 MiB |
| rust-sintel-r11 | 1.5 | 14→34 | **22.14** | TCP 27 + uTP 5; 22/32 active; 392 MiB |
| rust-sintel-r10 | 1.5 | 10→32 | 20.70 | TCP 24 + uTP 7; 22/31 active; 366 MiB |
| rust-sintel-r8 | 1.5 | 18→44 | **37.16** | peak above r7; end peers drained after burst |
| rust-ubuntu-r46 | 3.5 | 8→15 | **1.78** | TCP 12 + uTP 3; **13/15** active; **45 MiB**; SeekTuning wired |
| rust-ubuntu-r45 | 4.5 | 9→19 | 0.96 | TCP 17 + uTP 2; 12/19 active; 24 MiB; MSE fast-fail |
| rust-ubuntu-r44 | 4.0 | 4→13 | 1.28 | TCP 13; 10/13 active; 30 MiB; sticky interest (107.175 3MB) |
| rust-ubuntu-r43 | 4.5 | 13→14 | 1.46 | TCP 12 + uTP 2; 9/14 active; 27 MiB; pipeline floor |
| rust-ubuntu-r41 | 4.0 | 8→18 | 1.58 | TCP 15 + uTP 3; 7/18 active; 22 MiB; SlowPeerYield fix |
| rust-ubuntu-r38 | 5.5 | **10→33** | 0.75 | TCP 26 + **uTP 7**; 13/33 active; 15 MiB — peer count peak |
| rust-ubuntu-forceutp-r2 | 5.0 | 6→16 | 0.08 | **uTP 16**; was 0 dial_ok before BEP29 |
| rust-sintel-r7 | 1.5 | **21→39** | **34.8** | TCP 32 + **uTP 5**; **30/37** active; **855 MiB** |
| rust-sintel-r6 | 2.5 | 13→24 | 10.53 | TCP 20 + uTP 3; 17/23 active; 164 MiB |

**Status:** Best Ubuntu still **r48: 2.05 MiB/s / 48 MiB / 11/12 active**. After drop_chokers gate + request-cap: peers hold ~19–22 (was ~11 when chokers killed 107.175) and 13/20 active, but peak ~1.3 and mid-run decay leaves ~12–34 MiB/60s. Sintel-r8/r11 healthy (22–37 MiB/s). Gap vs Go r19 (**12 MiB/s / 496 MiB / 31/32**) remains — next: mid-file stall after startup head + per-peer fill. Not Ubuntu speed parity yet.

