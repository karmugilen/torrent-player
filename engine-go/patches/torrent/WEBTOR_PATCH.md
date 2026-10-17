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

All other upstream source files are unchanged aside from the items above. This
avoids an unrelated engine upgrade. The original LICENSE is included. The parent
module's local replacement ensures both Android builds and ordinary `go test`
use the same fix.

Validate the backport with `go test -race -run TestMsgWriterBroadcastDuringFillWakesWriter`
in this directory. The app's engine suite also exercises real peer transfers,
bounded-memory eviction and seeks beyond the RAM cache.
