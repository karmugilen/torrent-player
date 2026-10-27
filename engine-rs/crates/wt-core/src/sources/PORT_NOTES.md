# wt-core `sources/` PORT_NOTES

Peer-discovery glue owned under `src/sources/` (trackers, DHT, WebTorrent → torrents).
agy-2 wires this into `Client` / `Torrent`; agy-1 supplies shared `DatagramIo`.

## Ported

- `Sources` per client: `AnnounceDispatcher`, optional `DhtNode` (shared `DatagramIo` or dedicated), WebTorrent `TrackerClient`s, periodic DHT announce flag.
- `SourcesConfig::from_client_config` reads `no_dht`, `disable_webtorrent`, `periodically_announce_torrents_to_dht`, HTTP UA, ICE, peer water marks, status callbacks; dialer + datagram set by agy-2/agy-1.
- `TorrentKeys` / hybrid magnet: register **both** v1 and `InfoHashV2::to_short()`; `add_trackers` for an unregistered short hash is a no-op (WEBTOR_PATCH).
- `TorrentAnnounceInfo` + `PeerSink` traits for agy-2 (stats, want_peers, peer delivery, WebRTC streams, web-seed URLs).
- Tracker peers → `PeerSource::Tracker`; DHT get_peers → `PeerSource::DhtGetPeers`.
- `map_tracker_status`: `wt_discovery::TrackerStatusEvent` → `wt_core::StatusEvent` for `callbacks.status_updated`.
- Tests: fake HTTP+UDP trackers, hybrid registration, status/Started path, DHT testnet smoke.

## Differs from Go

| Area | Difference |
|---|---|
| DHT shared socket | Shared IO retained via `with_datagram_io`; iterative KRPC still uses mainline’s own bind until in-crate KRPC lands. |
| WebTorrent | WS trackers started per URL; full multi-offer swarm pacing is thinner than Go. |
| Tiering | Tracker URLs flattened into the dispatcher (Go keeps tier structure mainly for scrape ordering). |
| `DhtAnnouncePeer` | Passive announce-peer callbacks need DHT server hooks; get_peers path uses `DhtGetPeers`. |

## Remains

1. agy-2: construct `Sources` from `Client`, implement traits on `TorrentInner`, call register/refresh/unregister/pause.
2. agy-1: pass `SharedDatagramAdapter` into `SourcesConfig.datagram`.
3. Optional: DHT `DhtAnnouncePeer` push path; richer WS offer budget tied to want_peers.

## Build

```bash
CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/wt-core-sources CARGO_BUILD_JOBS=3 \
  cargo test -p wt-core --test sources_glue
```
