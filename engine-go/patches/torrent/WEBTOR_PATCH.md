# Pinned torrent dependency

This directory contains the source distribution of
`github.com/anacrolix/torrent` v1.61.0 (checksum retained in the parent `go.sum`),
with one upstream fix backported:

- [aa71e8d2f5c439257c0313919d671fd087347233](https://github.com/anacrolix/torrent/commit/aa71e8d2f5c439257c0313919d671fd087347233):
  install the peer writer's wakeup channel before filling its buffer, including
  the upstream regression test. A concurrent request update otherwise loses
  its wakeup, leaving urgent playback pieces unrequested until another message.

All other upstream source files are unchanged. This avoids an unrelated engine
upgrade. The original LICENSE is included. The parent module's local replacement
ensures both Android builds and ordinary `go test` use the same fix.

Validate the backport with `go test -race -run TestMsgWriterBroadcastDuringFillWakesWriter`
in this directory. The app's engine suite also exercises real peer transfers,
bounded-memory eviction and seeks beyond the RAM cache.
