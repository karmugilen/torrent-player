# Privacy policy: Torrent Player

Last updated: 2026-10-26

Torrent Player (`torrentplayer.app` on Google Play, `webtor.app` on F-Droid
and GitHub) is a free, open-source BitTorrent client for Android. This policy
explains what the app does with information. The source code is at
<https://github.com/karmugilen/torrent-player>.

## What the developer collects

Nothing. The app has no accounts, advertising, analytics, crash reporting or
telemetry, and it sends nothing to the developer. There is no developer server.

## What stays on your phone

Your torrent list, settings, playback positions, downloaded files, caches
(peer addresses, DHT contacts, video previews) and a small learned model of
how fast peers deliver data are stored only on your device. Android's backup
covers only the app's settings file. Uninstalling the app deletes its private
data; files you saved to a shared folder stay where you put them.

## Network connections the app makes

BitTorrent is peer-to-peer. When you add or play a torrent, or seed one:

- **Peers and trackers.** Your IP address and the torrent's info hash are shared
  with the trackers, DHT nodes and peers the app connects to, and the data you
  download and upload is exchanged with those peers. This is how BitTorrent
  works and other people in the swarm can see it.
- **Public tracker list.** Once a day the app downloads a public list of
  trackers from
  `https://raw.githubusercontent.com/ngosang/trackerslist/master/trackers_best.txt`
  (GitHub). The request does not include your torrents. Up to 24 of these
  trackers are added to public torrents (never to private ones).
- **WebRTC.** To connect to browser-based peers, the app asks STUN servers run
  by Google, Twilio and Cloudflare for your public address. They see your IP
  address.
- **Seeding.** Seeding is on by default: after a download completes, its pieces
  are uploaded to other peers until a share ratio of 1.0 or 60 minutes, at up to
  256 KiB/s and only on Wi-Fi. You can turn it off in Settings, Seeding.

Peer country flags and the peer map use an offline database bundled with the
app (DB-IP Lite); no lookups are sent.

## Permissions

- **Internet and network state:** to download and upload torrent data and pause
  transfers on networks you exclude.
- **Notifications and foreground service:** to show transfer progress and keep
  downloads running while the app is in the background.
- **Files:** only folders you pick in Android's file picker.

## Children

The app is not directed at children.

## Your responsibility

Only download and share content you have the right to. The app has no built-in
torrent search.

## Changes and contact

Changes to this policy are published in this file with a new date. Questions:
<https://github.com/karmugilen/torrent-player/issues>.
