# Pinned torrent dependency

This directory contains the source distribution of
`github.com/anacrolix/torrent` v1.61.0 (checksum retained in the parent `go.sum`),
with one upstream fix backported:

- [aa71e8d2f5c439257c0313919d671fd087347233](https://github.com/anacrolix/torrent/commit/aa71e8d2f5c439257c0313919d671fd087347233):
  install the peer writer's wakeup channel before filling its buffer, including
  the upstream regression test. A concurrent request update otherwise loses
  its wakeup, leaving urgent playback pieces unrequested until another message.

Additional local hook for experimental anti-DPI:

- `Client.SetOutgoingTCPConnHook` in `client.go` installs a per-client wrapper
  for successfully dialed TCP peer connections (stored on `Client`, threaded
  through `dialPool`/`dialFromSocket`). Webtor installs a write-split stream
  adapter here so outgoing TCP peers are adapted without a process-global race
  across EngineServer instances. UDP/uTP dialers are unchanged.

- `PeerConn.CacheDialAddr` in `peerconn.go` exposes a synchronized dialable peer
  endpoint for the bounded peer cache. Incoming source ports are excluded unless
  an advertised listening port is available.

- `Client.SetUrgentRequestStealing` and `Torrent.NudgeUrgentRequests` in
  `webtor_urgent.go` (with a `urgentStealAfter` field on `Client` and a
  one-condition change to the steal check in `requesting.go`). When enabled, a
  block of a `PiecePriorityNow` piece outstanding longer than the configured
  age may move to a peer that delivered useful data more recently, bypassing
  the upstream "don't steal from the poor" balance rule for urgent blocks only.
  The nudge asks peers with spare request slots that hold an urgent piece to
  recompute requests. Disabled by default; covered by `webtor_urgent_test.go`.

All other upstream source files are unchanged aside from the items above. This
avoids an unrelated engine upgrade. The original LICENSE is included. The parent
module's local replacement ensures both Android builds and ordinary `go test`
use the same fix.

Validate the backport with `go test -race -run TestMsgWriterBroadcastDuringFillWakesWriter`
in this directory. The app's engine suite also exercises real peer transfers,
bounded-memory eviction and seeks beyond the RAM cache.

## Hybrid magnet tracker crash

`AddTorrentOpt` registered only the v1 short infohash in `torrentsByShortHash`, although a hybrid magnet (`btih` + `btmh`) already has both hashes and tracker announcers are keyed by each. `hashInfoBytes` only adds the v2 key when the v2 hash is learned from metadata, so for hybrid magnets it never was. The dispatcher's `addKey` then skipped every v2 tracker key, `initRegularTrackerAnnounceState` still added it to the torrent, and the next `updateTorrentInput` (run from `Client.unlock`, for example after a DHT announce) failed `panicif.False(res.Exists)`. Panicking inside the unlock handler also double-unlocked the client mutex, a fatal error that aborted the Android process within seconds of adding such a magnet.

- `client.go`: `AddTorrentOpt` also registers `*opts.InfoHashV2.Value.ToShort()`. `Torrent.close` already removes both short hashes.
- `torrent.go`: `initRegularTrackerAnnounceState` returns false and adds nothing when the dispatcher did not take the key (torrent no longer registered); `startScrapingTrackerWithInfohash` then starts no announcer for it.
- Tests: `webtor_hybrid_test.go` (library) and `engine-go/hybrid_magnet_test.go` (engine `/add` with trackers enabled) both panic without the patch.
