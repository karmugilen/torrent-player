# Google Play release: Torrent Player (`torrentplayer.app`)

The same code goes to Google Play as `torrentplayer.app` and to F-Droid and
GitHub as `webtor.app` ("webtor" is also the name of an unrelated website).
Both are signed with the release key
(SHA-256 `b26e9a22c1a40c5f8bd2bb5ba9e8639f7835c1ca986e9607e240beeddbd9f71f`).
They are separate apps on the phone: a Play install and an F-Droid install can
sit side by side, and moving between them means reinstalling. The package name
comes from the Gradle property `webtor.applicationId` (default `webtor.app`),
which `scripts/play-bundle.sh` sets; the content provider authority follows it.

## What the code already meets (2.3.0, versionCode 99)

- Target API 36 (Android 16), required for new apps and updates since 2026-08-31.
- 64-bit native code with 16 KB page alignment (`engine-go/build.sh` links with
  `-z max-page-size=16384`). Check: `llvm-readelf -lW libengine.so` shows `0x4000`.
- Android 15+ dataSync time limit: `PlayService.onTimeout` stops the service in
  time and posts a notice; opening the app resets the 6-hour budget.
- App Bundle: `scripts/play-bundle.sh` builds `artifacts/torrent-player-<version>.aab`
  signed with the release key.
- Privacy policy: [PRIVACY.md](../PRIVACY.md), URL
  `https://github.com/karmugilen/torrent-player/blob/go-engine/PRIVACY.md`.
- Listing text and graphics: `fastlane/metadata/android/en-US/` (title, short
  description, icon 512 px, feature graphic 1024×500, phone screenshots). Play
  shows HTML lists poorly; paste the plain text below as the full description.

## Steps only the account owner can do

1. **Developer account.** Register at <https://play.google.com/console/signup>
   (one-time US$25, identity verification). A personal account created after
   2023-11-13 must run a **closed test with at least 12 testers for 14 days in a
   row** before it can publish to production. Organization accounts (D-U-N-S
   number) skip this.
2. **Create the app.** Name "Torrent Player", default language English (US), App,
   Free, package name `torrentplayer.app` (created 2026-10-26).
3. **App signing: use the existing key.** On the first upload choose
   *Use a different key* → *Export and upload a key from Java keystore*. Play shows
   a PEPK command and an encryption key file. Run it with:
   `--keystore=$HOME/.config/webtor/webtor-release.jks --alias=webtor`
   and upload the zip. Using the existing key keeps one key for every store and
   lets the same key prove ownership for Android developer verification. The
   same key is also used as the upload key.
4. **Closed testing track.** Upload the `.aab`, add the testers' Google accounts
   (or a Google Group), roll out, and keep 12+ testers opted in for 14 days.
5. **Apply for production** in the Console, then promote the release.

## App content answers

- **Privacy policy:** the URL above.
- **Ads:** No ads.
- **App access:** all functionality available without login.
- **Content rating (IARC):** category *Utility, Productivity, Communication or
  Other*. No violence, sexuality, language, drugs or gambling. Users exchange files
  with other users (peer-to-peer): answer Yes to sharing content between users.
  No location sharing, no purchases.
- **Target audience:** 18 and over (not designed for children).
- **News app:** No. **Government app:** No. **Financial features:** None.
  **Health:** None.
- **Data safety:** No data collected, no data shared. The app has no developer
  server, analytics or ads; peer-to-peer transfers happen at the user's request,
  and files and settings stay on the device. Users can delete all data by
  uninstalling.
- **Foreground service permissions:** type *Data sync*. Description: "Keeps
  user-started torrent downloads and seeding running while the app is in the
  background. The ongoing notification shows progress and has a Stop button.
  Without it, downloads stop when the user leaves the app." Play asks for a short
  video: record adding a torrent, leaving the app and the notification
  continuing (upload unlisted to YouTube).
- **Category:** Tools. **Contact email:** a public support address of your choice.

## Policy notes

Torrent clients are allowed on Google Play (several are listed), but the app and
listing must not promote infringement. The app has no built-in torrent search;
keep it that way, and use only legally shareable content (Big Buck Bunny, Sintel)
in screenshots and videos.

## Each later release

1. Bump `versionCode`/`versionName`, add the changelog, commit and push.
2. F-Droid and GitHub: `scripts/fdroid-release.sh`, then `gh release create`
   (see [FDROID_SUBMISSION.md](FDROID_SUBMISSION.md)).
3. Play: `scripts/play-bundle.sh`, upload the `.aab` to the Play track, paste the
   changelog as release notes.

## Full description (plain text for Play)

```
Torrent Player is a lightweight, high-performance torrent downloader and streaming engine designed specifically for Android.

Download torrents directly to your local storage or stream growing video files on-the-fly using your favorite external media players.

Core Features:

• Direct Local Storage: Single-contract download directly to local public storage (Downloads/Webtor) for straightforward file management.
• In-Progress Video Streaming: Stream growing video files directly to external players such as VLC, Nova Video Player, MPV, or Just Player without waiting for downloads to finish.
• Selective File Downloading: Choose specific files to prioritize or download in multi-file swarms.
• Bitfield Piece Visualizer: Interactive real-time visualizer showing piece distribution and swarm availability.
• Optimized Resource & Battery Usage: Adaptive polling back-off and a native Go engine with bounded preparation storage.
• Modern Material 3 Interface: Intuitive UI with dynamic colors, dark theme support, and animated video preview cards.
• Direct Play: Stream a video from RAM without saving the whole file. Only each video's start and end are kept (up to 1 GB) so episodes reopen fast.
• Continue Watching and Play Next: Resume where you left off and move on to the next episode, with its start fetched in advance.
• Fast Seeking: A small model learned on your phone predicts when each peer will deliver a block, so the data at the play position is requested from the peers likely to send it first. It runs and is stored only on the device.
• Peer Map: Shows peer countries using an offline database bundled with the app (DB-IP Lite, CC BY 4.0). No lookup requests are made.

Privacy & Freedom:

• 100% Free and Open Source Software (MIT License).
• No advertising trackers, analytics, or telemetry.
• Zero advertisements.
• Direct peer-to-peer swarm connectivity (DHT, UDP trackers, TCP, uTP, WebSockets, and WebRTC).

Network connections:

• When the app starts, the engine automatically refreshes a cached public tracker list from https://raw.githubusercontent.com/ngosang/trackerslist/master/trackers_best.txt. Successful refreshes are scheduled once per day; failed requests may be retried. This request downloads a shared list and does not send torrent hashes or user-supplied tracker URLs.
• Up to 24 additional public BitTorrent trackers are automatically added to each non-private torrent to help discover peers. Cached and built-in fallback trackers are used when the list is unavailable. Private torrents are excluded. Torrent announces share your IP address and torrent info hash with the trackers contacted.
• WebRTC peer connections use STUN servers operated by Google, Twilio, and Cloudflare to discover a public network address for connectivity. These providers can see the source IP address of STUN requests.
• Seeding is on by default: after a download completes, its verified pieces are uploaded to other peers until a share ratio of 1.0 or 60 minutes of seeding, at up to 256 KiB/s and only on Wi-Fi. Seeding pauses during playback. Turn it off or change the limits in Settings, Seeding.
• The app does not provide settings to disable public tracker-list refresh, extra public trackers, or WebRTC. This build supports arm64-v8a devices.
```
