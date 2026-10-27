# wt-discovery PORT_NOTES

Ports `engine-go/patches/torrent/{tracker,webtorrent,webseed}`, `client-tracker-announcer.go`, DHT usage (`dht.go` / anacrolix/dht v2 behaviour), and `bep40.go`.

## Ported

### `bep40`
- `bep40::priority(a, b) -> u32` matches Go `bep40PriorityIgnoreError` (CRC32C / Castagnoli).
- Go test vectors from `bep40_test.go` pass.

### `tracker`
- Types: `AnnounceRequest` / `AnnounceResponse` / `AnnounceEvent` / `Peer` / scrape items.
- HTTP(S) announce: compact + dict peers, `peers6`, failure reason, warning message, interval / min interval, user-agent, rustls TLS via reqwest.
- UDP announce: BEP 15 connect/announce, Go-matching timeout backoff (`15s * 2^n`, cap n=8), BEP 41 URL path options, scrape.
- Unified `Client` factory (`http`/`https`/`udp`/`udp4`/`udp6`); `ErrBadScheme` for unknown schemes.
- `AnnounceDispatcher`: per-URL concurrency limit (2), Started/Completed/Stopped/regular scheduling, status callbacks (`TrackerStatusEvent`), peer channel.
- Custom `DialerHook` / `ListenPacketHook` in the public API (HTTP dialer accepted; full reqwest connector wiring still open — see below).
- In-process fake HTTP + UDP tracker integration tests.

### `webseed`
- BEP 19 URL construction (directory trailing `/` + path escape, S3 `+` → `%2B`).
- File-span location across torrent files; concurrent range fetches with semaphore; 48 KiB max discard for non-Range 200s.
- Integration test against in-process HTTP server.

### `dht`
- Wrapper around pure-Rust `mainline` 8.x (async): get_peers stream, announce_peer with implied_port (`port=None`), bootstrap list aligned with anacrolix defaults, `export_contacts` / `import_contacts` / `add_node`, periodic announce helper.
- `DhtPacketIo` trait + `looks_like_krpc` for future shared-socket demux with wt-wire uTP.
- Local `mainline::Testnet` announce/lookup smoke test.

### `webtorrent`
- WebSocket tracker client (tokio-tungstenite + rustls): offers/answers JSON protocol matching anacrolix/webtorrent (binary-as-JSON-string encoding).
- WebRTC data channels via `webrtc` 0.12; ICE/STUN list from `engine-go/server.go`.
- `DataChannelStream`: `AsyncRead + AsyncWrite` for the BitTorrent wire.
- Bounded outstanding offers + TTL cleanup.
- Tests: raw ICE ping, Transport small ping, 4 MiB data-channel transfer.

### Status events
- `TrackerStatusEvent` mirrors `wt_core::StatusEvent` tracker variants so discovery does not depend on wt-core. Core should map when wiring callbacks.

## Differs from Go

| Area | Difference |
|---|---|
| DHT | Uses `mainline` (IPv4-focused) instead of anacrolix/dht v2; shared UDP socket with uTP is trait-ready but dedicated-socket mode is what runs today. |
| HTTP dialer | `DialerHook` is stored for API parity; requests currently use reqwest’s default rustls dial. Engine socket tracking needs a custom hyper connector. |
| Announcer | Faithful scheduling semantics (events, intervals, per-URL concurrency) without Go’s full `indexed` table machinery. |
| Bencode | Private `bencode_lite` for HTTP tracker responses until `wt-meta` publishes bencode. |
| WebTorrent WS headers | Config field present; handshake currently uses default headers (easy to extend). |

## Remains / follow-ups

1. Wire `DialerHook` through a custom reqwest/hyper connector so the engine can route/track TCP sockets like Go `DialContext`.
2. Finish shared UDP demux with wt-wire: implement `DhtPacketIo` on a multiplexed socket (DHT packets start with `d`). Coordinator ask on ownership timed out; current choice is **wire-or-shared demux feeding `DhtPacketIo`** (discovery does not own the only UDP socket when uTP is enabled).
3. Replace `bencode_lite` with `wt-meta` bencode once published.
4. DHT IPv6 / full anacrolix feature parity (read-only mode flag is accepted but `mainline` server_mode is used for announce storage).
5. WebSocket tracker end-to-end test with a local fake tracker (signaling + two peers) — data-channel 4 MiB is covered; fake WS tracker is still thin.
6. Map `TrackerStatusEvent` → `wt_core::StatusEvent` in wt-core.

## Build

```bash
CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/wt-discovery CARGO_BUILD_JOBS=3 \
  cargo test -p wt-discovery
CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/wt-discovery CARGO_BUILD_JOBS=3 \
  cargo clippy -p wt-discovery -- -D warnings
```
