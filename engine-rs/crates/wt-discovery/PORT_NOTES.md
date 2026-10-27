# wt-discovery PORT_NOTES

Ports `engine-go/patches/torrent/{tracker,webtorrent,webseed}`, `client-tracker-announcer.go`, DHT usage (`dht.go` / anacrolix/dht v2 behaviour), and `bep40.go`.

## Ported

### `bep40`
- `bep40::priority(a, b) -> u32` matches Go `bep40PriorityIgnoreError` (CRC32C / Castagnoli).
- Go test vectors from `bep40_test.go` pass.

### `tracker`
- Types: `AnnounceRequest` / `AnnounceResponse` / `AnnounceEvent` / `Peer` / scrape items.
- HTTP(S) announce: compact + dict peers, `peers6`, failure reason, warning message, interval / min interval, user-agent, rustls TLS via reqwest.
- HTTP tracker responses decode via `wt_meta::bencode` (`decode_prefix` / `dict_get_str`).
- `DialerHook` wired for HTTP and HTTPS: custom dial → HTTP/1.1 (rustls for HTTPS, Go-like insecure verify for private trackers). Counting-dialer integration test passes.
- UDP announce: BEP 15 connect/announce, Go-matching timeout backoff (`15s * 2^n`, cap n=8), BEP 41 URL path options, scrape.
- UDP trackers speak over `DatagramIo` (standalone `UdpSocket` fallback or shared socket from core).
- Unified `Client` factory (`http`/`https`/`udp`/`udp4`/`udp6`); `ErrBadScheme` for unknown schemes; top-level `ClientOpts.dialer` / `datagram`.
- `AnnounceDispatcher`: per-URL concurrency limit (2), Started/Completed/Stopped/regular scheduling, status callbacks (`TrackerStatusEvent`), peer channel.
- In-process fake HTTP + UDP tracker integration tests.

### `datagram`
- `DatagramIo` trait: `send_to` / `recv_from` / `local_addr` (futures via `IoFut`).
- `StandaloneDatagram` + `impl DatagramIo for Arc<UdpSocket>`.
- Core implements the trait over `wt_wire::UtpSocket::{send_non_utp, recv_non_utp}` (agy-1); discovery does not depend on wt-wire.
- Re-exported as `dht::DhtPacketIo`.

### `webseed`
- BEP 19 URL construction (directory trailing `/` + path escape, S3 `+` → `%2B`).
- File-span location across torrent files; concurrent range fetches with semaphore; 48 KiB max discard for non-Range 200s.
- Integration test against in-process HTTP server.

### `dht`
- Wrapper around pure-Rust `mainline` 8.x (async): get_peers stream, announce_peer with implied_port (`port=None`), bootstrap list aligned with anacrolix defaults, `export_contacts` / `import_contacts` / `add_node`, periodic announce helper.
- `with_datagram_io` / `with_packet_io` retain shared `DatagramIo` for demuxed non-uTP send/recv; iterative KRPC still uses dedicated `mainline` bind (that crate owns the socket). `shared_send_to` / `shared_recv_from` expose the shared path.
- Local `mainline::Testnet` announce/lookup smoke test + shared-IO unit test.

### `webtorrent`
- WebSocket tracker client (tokio-tungstenite + rustls): offers/answers JSON protocol matching anacrolix/webtorrent (binary-as-JSON-string encoding).
- `TrackerClientConfig::dialer` dials TCP for `ws`/`wss` via the same `DialerHook` as HTTP trackers.
- WebRTC data channels via `webrtc` 0.12; ICE/STUN list from `engine-go/server.go`.
- `DataChannelStream`: `AsyncRead + AsyncWrite` for the BitTorrent wire.
- Bounded outstanding offers + TTL cleanup.
- Tests: raw ICE ping, Transport small ping, 4 MiB data-channel transfer, local fake WS tracker offer/answer relay.

### Status events
- `TrackerStatusEvent` mirrors `wt_core::StatusEvent` tracker variants so discovery does not depend on wt-core. Core should map when wiring callbacks.

## Differs from Go

| Area | Difference |
|---|---|
| DHT shared socket | Shared `DatagramIo` is retained and usable for raw send/recv; full iterative KRPC over the shared socket is not yet in-crate (mainline binds its own UDP). Dedicated-socket DHT is what runs for get_peers/announce today. |
| Announcer | Faithful scheduling semantics (events, intervals, per-URL concurrency) without Go’s full `indexed` table machinery. |
| WebTorrent WS headers | Config field present; handshake currently uses default headers (easy to extend). |
| HTTPS dialer TLS | Matches Go `InsecureSkipVerify` for custom-dial path so private/self-signed trackers work. |

## Remains / follow-ups

1. Core/agy-1: implement `DatagramIo` over wt-wire non-uTP demux and wire into UDP trackers + DHT (signature published to coordinator).
2. Optional: in-crate KRPC over shared `DatagramIo` so DHT needs no second UDP bind when uTP shares the socket.
3. DHT IPv6 / full anacrolix feature parity (read-only mode flag is accepted but `mainline` server_mode is used for announce storage).
4. Map `TrackerStatusEvent` → `wt_core::StatusEvent` in wt-core.

## Build

```bash
CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/wt-discovery CARGO_BUILD_JOBS=3 \
  cargo test -p wt-discovery
CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/wt-discovery CARGO_BUILD_JOBS=3 \
  cargo clippy -p wt-discovery --all-targets -- -D warnings
```
