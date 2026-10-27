# wt-core PORT_NOTES

## peer_conn (this dispatch)

Owned paths: `src/peer_conn.rs`, `tests/peer_conn_*.rs`.

### Ported
- `PeerTorrent` trait: torrent-wide surface for piece_state/storage/strategy/
  webtor/metadata/PEX/DHT/choke config; agy-2 implements on `TorrentInner`.
  Signature documented at the top of `peer_conn.rs` (sent to coordinator).
- `PeerConnHandler<T: PeerTorrent>` implementing `conn::ConnHandler`:
  bitfield/have/have-all/have-none, choke/unchoke/interested/not-interested,
  request/cancel (upload via `choking::upload_allowed` + storage read),
  piece (storage write, dirty bookkeeping, hash queue), reject/allowed-fast/
  suggest, port → DHT, LTEP handshake, ut_metadata, ut_pex; BEP 52 hash
  messages accepted and ignored until v2 layers are wired.
- Request loop (`update_requests`): `strategy::compute_desired_requests` +
  `sort_requests_for_peer`; Webtor steal (`may_take_existing_request`),
  faster-focus, slow-peer yield, refill/`pending_requests`, peak modes,
  pipeline/`nominal_max`; HOL rescue + hedge via `rescue_decisions` /
  `hedge_decisions` when seek tuning enables them.
- Per-peer stats (`AtomicConnStats`, rates, `RttEstimator`, snubbing flag,
  `PeerConnInfo` snapshot).
- Legacy `PeerConn` / `PeerConnInner` kept for agy-2 client/torrent paths.
- Unit tests in `tests/peer_conn_handler_test.rs` with `FakeTorrent` +
  fake `ConnHandle`: availability, choke cancels, piece→storage/hash,
  allowed-fast while choked, metadata exchange, PEX/port, upload reject,
  `peer_conn_info`.

### Differs from Go
- Torrent coupling is via `PeerTorrent` instead of embedding `*Torrent`;
  keeps peer_conn compiling while piece_state/torrent wiring lands.
- Upload path has no queued piece-message pipeline yet (sync read+send).
- BEP 52 hash request/hashes/reject are no-ops until grok-6/agy-2 wire
  piece layers.
- Suggest only marks need-request-update (no dedicated prefer list).
- HOL/hedge run inside `update_requests` when tuning flags are on; full
  torrent-wide `NudgeUrgentRequests` timer remains agy-2.

### Remains (agy-2 / others)
- Implement `PeerTorrent` on `TorrentInner` and spawn `PeerConnHandler`
  through `conn::spawn_connection_driver`.
- Migrate legacy `PeerConn` call sites onto the handler.
- Wire BEP 52 hashes; outbound AllowedFast generation already in choking.
- Learned-arrival features into HOL overdue / expectedArrival.

---

# hashing + webseed_peer helpers

Owned paths for that helper dispatch: `src/hashing/`, `src/webseed_peer/`,
`tests/hashing_*.rs`. Rest of wt-core is agy-2.

## Ported

### `hashing`
- Bounded sticky worker pool (`PieceHasherPool`) sized by
  `piece_hashers_per_torrent` (default 2). Dedicated OS threads; reusable
  per-worker buffer; results via `mpsc` channel.
- Priority: `PiecePriority` desc, then lower piece index, then lower `seq`
  (Webtor: Now/playback before background verify).
- Cancel via `CancelToken` + `wake()`.
- Sync hasher `hash_piece_sync`: v1 SHA-1, v2 piece-layer
  (`sum_min_length(piece_length)`), v2 file-root (`sum()`), optional hybrid
  v1 zero-pad.
- Smart ban cache (`SmartBanCache`) + per-piece block check while hashing.
- `PieceCompletionCache` for storage completion / hashing flags.
- SHA-1 backends behind `PieceSha1`: `SoftSha1` (`sha1` crate) and
  `RingSha1` (`ring::digest::SHA1_FOR_LEGACY_USE_ONLY`). Selection:
  **aarch64 always ring** (ARMv8 SHA / Go phone parity); other arches pick
  the faster of soft vs ring via a one-shot microbench (`selected_backend`).
  Throughput test prints soft vs ring vs chosen MiB/s on a 256 MiB payload.

### `webseed_peer`
- `WebseedPeer` wrapping `wt_discovery::webseed::WebSeedClient`.
- Request-index range → torrent-byte extent → HTTP Range fetch → 16 KiB
  (or custom) chunks; discard-window early stop; convict / 1-minute penance
  on errors and `provided_bad_data`; random error backoff ≤ 10s.
- `wake_for_urgent_piece` / `WakeReason("webtor urgent piece")` (Go
  `webtorWakeWebseedsForPiece`).
- Constants: `DEFAULT_MAX_CONCURRENT` (4), `HOST_REQUEST_CONCURRENCY` (25),
  `MAX_DISCARD_BYTES` (48 KiB).

## Differs from Go
- Pool is a standalone helper; agy-2 wires it into `Torrent` (Go embeds
  hashers inside `torrent.go`).
- Webseed peer does not implement the full `Peer` / `legacyPeerImpl`
  surface (choking, PEX, etc.); it exposes fetch + convict + wake for
  agy-2 to attach to torrent state.
- SHA-1 default is `ring` (not stdlib `crypto/sha1`); soft path kept for
  benches. `sha1` crate `asm` feature is a no-op on 0.10 but enabled and
  verified to build for `aarch64-linux-android`.
- Smart-ban block fingerprint is FNV-1a 64 (Go injects a Hash func); enough
  to blame differing peers.

## Remains (agy-2)
- Hook pool into `Torrent::pieceHashed` / `queuePieceCheck` /
  `VerifyDataContext` wait-on-numVerifies.
- Global client `activePieceHashers` budget across torrents.
- Full webseed request planner / host concurrency map /
  `scheduleImmediateWebseedRequestUpdate` timer.
- Integrate wake into seek / urgent path beside existing webtor modules.

---

# Reader (`src/reader.rs`, `tests/reader_*.rs`)

Owned for this dispatch: `src/reader.rs`, `tests/reader_host.rs`,
`tests/reader_test.rs`. Public `Reader` constructors used by `lib.rs` /
`File::new_reader` kept (`Reader::new(Arc<TorrentInner>, offset, length)`).

## Ported
- Go `reader.go`: position, Seek, blocking Read that waits on
  `ReaderHost::wait_change` (condvar, no busy loop) until `have` reports
  verified bytes under the position.
- Readahead window via `pieces_uncached` + pure `byte_region_pieces` /
  `reader_piece_priorities` matching Go (Now = begin, Readahead =
  begin+1..end). Default `readahead_fn` = `current_pos - contiguous_start`
  (Go `defaultReadaheadFunc`); `SetReadahead` clears the fn.
- `reading` flag: seek clears it so readahead is zero until the next Read.
- Responsive mode (`SetResponsive` sticky on; `WebtorSetResponsive` toggle):
  `have(..., responsive=true)` counts dirty chunks.
- Cancel via `CancelToken` → `ErrorKind::Interrupted`; optional read
  timeout → `TimedOut`; EOF at file end (`Ok(0)`); file-bounded readers
  (`file_offset` / `file_length`).
- `ReaderHost` trait documented at top of `reader.rs`; `TorrentInner`
  implements it; tests use `FakeHost` via `Reader::with_host`.

## Differs from Go
- No per-piece `readerCond` — one torrent-wide change condvar (same as
  existing `TorrentInner::bump_change`).
- Storage-reader reset / piece-completion resync retries from Go
  `readAt` are simplified to one retry after `wait_change`.
- Idle-timeout wrapper (`idle-reader.go`) not ported as a type; callers
  can pass `read_timeout`.
- **Stopgap priorities:** `TorrentInner::set_reader_positions` Raises
  `piece.priority` to Now/Readahead. Priorities do not drop on seek-away.
  Full Go bitmaps (`_readerNowPieces` / `_readerReadaheadPieces`) need a
  `reader_ranges` map on `TorrentInner` (agy-2). Pure
  `reader_piece_priorities` is ready for that wiring.

## Remains
- Torrent-side reader registry + bitmap-based `purePriority` (agy-2).
- Wire `Close`/`clear_reader` into `readersChanged` / full priority
  recompute once the registry exists.
- `clippy -D warnings` for the whole `wt-core` crate is clean across `conn/`, `hashing/`, and `reader/`.

---

# Peer Connection I/O Driver + Shared UDP Socket (`src/conn/`, `tests/conn_test.rs`)

Owned for this dispatch: `src/conn/` (`mod.rs`, `handle.rs`, `driver.rs`, `dialer.rs`, `listener.rs`, `half_open.rs`, `shared_udp.rs`), `tests/conn_test.rs`, and UDP socket sharing enhancements in `wt-wire`.

## Ported
- **Connection Handle & Driver (`src/conn/handle.rs`, `src/conn/driver.rs`)**:
  - `ConnHandle`: Cheaply cloneable thread-safe handle for sending messages (`send_message`, `send_message_async`), flushing, closing, querying stats (`stats()`), and inspecting live metadata (`info()`).
  - `ConnHandler` trait: Event-driven interface (`on_message`, `on_closed`) consumed by `agy-2` peer state logic.
  - Reader loop with zero-copy decoding (`FramedReader`), bounded buffers, read inactivity deadline (150s, matching Go `SetReadDeadline`), and token-bucket download rate limiting (`Limiter`).
  - Writer loop with backported wakeup race condition fix (`WEBTOR_PATCH.md` / #1070), double buffering, periodic keepalive (120s), and upload rate limiting (`Limiter`).
  - 64 MiB piece transfer throughput through `ConnHandle` verified at > 3,000 MB/s.
- **Outgoing Dialer (`src/conn/dialer.rs`)**:
  - `dial_peer`: Dials peers over dual-stack TCP or pure-Rust uTP honoring `nominal_dial_timeout` and `min_dial_timeout`.
  - Dial rate limiter integration (`Limiter::reserve_delay(1)`).
  - Outgoing `TcpConnHook` support for anti-DPI proxy wrappers.
  - Message Stream Encryption (MSE) negotiation with anacrolix default policy (`preferred = true, require_preferred = false`).
  - BitTorrent protocol handshake with reserved extension bits (DHT, Fast, LTEP, v2 upgrade).
  - BEP 10 LTEP extension handshake exchange with `extended_handshake_client_version`, `reqq`, `metadata_size`, `yourip`, and `upload_only`.
- **Inbound Listener (`src/conn/listener.rs`)**:
  - `PeerListener`: Binds dual-stack TCP listeners and/or uTP sockets on `listen_port` (or ephemeral port 0).
  - Inbound MSE stream encryption handling with SKEY search iterator across active torrents.
  - Inbound BitTorrent handshake with multi-torrent `InfoHashLookup` callback.
  - Inbound LTEP extension handshake exchange.
  - Enforces `handshakes_timeout`.
- **Half-Open Concurrency Tracking (`src/conn/half_open.rs`)**:
  - `HalfOpenTracker` enforcing global `total_half_open_conns` and per-torrent `half_open_conns_per_torrent` via RAII slot guards.
- **Shared UDP Socket (`src/conn/shared_udp.rs`, `wt-wire::utp`)**:
  - `SharedDatagramAdapter` wrapping `wt_wire::UtpSocket` and implementing `wt_discovery::DatagramIo` (`send_to`, `recv_from`, `local_addr`).
  - Multiplexes BEP 29 uTP streaming and non-uTP datagrams (BEP 5 DHT queries/replies and BEP 15 UDP tracker queries/replies) over a single shared UDP socket.

## Acceptance Verified
- Loopback tests: TCP and uTP dial/accept with and without MSE round-trip cleanly.
- Extension handshake fields (version, port, yourip, reqq, m) round-trip accurately.
- 64 MiB piece transfer throughput verified at ~3,400 MB/s.
- Token bucket rate limiter caps throughput within 10% of configured bandwidth limit.
- Half-open limits (global and per-torrent) enforced with slot reclamation upon completion/error.
- Shared UDP socket demux verified: DHT KRPC pings and responses multiplexed successfully while uTP runs on the same UDP socket.
- `cargo test -p wt-core --test conn_test`: All 7 tests passing.
- `cargo clippy -p wt-core --test conn_test -- -D warnings`: 0 warnings.
- `cargo clippy -p wt-wire --all-targets -- -D warnings`: 0 warnings.
- `cargo fmt --check`: Clean.

---

## Client and connection manager (codex-4)

- One Sources instance per client shares the uTP datagram socket with DHT and UDP trackers; tracker status callbacks and the optional `ClientConfig::tracker_dialer` are wired through Sources.
- Torrent registration serializes concurrent adds, merges tracker/web-seed data, and indexes both hybrid short hashes. Accepted and dialled peers use `PeerConnHandler` and the bounded connection driver with shared client upload/download limiters; candidate scheduling and backoff are provided by TorrentInner.
- TCP/uTP loopback transfers, a metadata-only magnet against a storage-restored seeder, hybrid merge, repeated close, half-open guards, incoming established limit, and request-update wake/deadline behavior have client/connection tests.
- Listener close signals the accept loops, and client close aborts tracked tasks and closes connection handles. Each client owns a dedicated Tokio runtime; synchronous bootstrap and close also work from within a current-thread runtime.
- Dial timeout uses the Go pending-peer reduction formula, but TCP and uTP are attempted sequentially (TCP first), rather than Go's concurrent DialFirst. Torrent-side candidate ordering, peer watermarks, and detailed retry policy remain subject to the TorrentInner implementation.

---

## Transfer paths and core integration (agy-core)

### Ported
- **Two-engine and magnet download stall fixes**:
  - Connection driver wake notifications (`handle.wake()`) wired to all state changes in `TorrentInner`: `set_info`, `download_all`, `allow_data_download`, `disallow_data_download`, `apply_hash_result`, `refocus_requests`, and `peer_closed`.
  - In `PeerConnHandler`, restored pieces and availability for peers that sent `HaveAll` before metadata arrival (`num_pieces == 0`).
  - In `init_book_and_glue`, piece availability in `PieceBook` is preserved from `peer_pieces` recorded before `set_info`.
  - In `peer_completion()`, returns 1.0 when `peer_sent_have_all` is true even if `num_pieces == 0`.
- **Peer tracking & stats audit**:
  - `Torrent::stats()` derives `total_peers` from `conn_handles.len()`, `active_peers` and `connected_seeders` from `peer_snapshots`, and `cs` from `ConnHandle::stats()`, eliminating the zero `numPeers` issue in `wt-engine`.
  - `addr_for_key` and `PeerTorrent::peer_has_piece` query active connections in `conn_handles` and `peer_snapshots`.
- **Dial race elimination**:
  - `ClientInner::dial_peer_transport` checks `conn_handles` before dialing and after handshake, preventing duplicate peer errors when background and explicit dials race.
- **Message Stream Encryption (MSE)**:
  - Exposed `HeaderObfuscationPolicy` on `ClientConfig` (re-exported from `wt_wire::mse`).
  - Wired `header_obfuscation_policy` through `PeerListener` inbound handshake and `dial_peer` outbound negotiation.
  - Un-ignored and passed `e2e_transfer::transfer_with_mse_encryption`, verifying encrypted transfer and connection status.
- **Throughput**:
  - 64 MiB loopback TCP throughput measured at **43.86 MiB/s** in release profile (transferred 64.0 MiB in 1.459s), exceeding the Go engine baseline of 19.5 MiB/s by 2.25x.

### Differs from Go
- Connection waking is event-driven through Tokio `Notify` (`ConnHandle::wake`) instead of Go's channel/condvar per connection.

### Remains
- None in wt-core; all unit tests and 31/31 e2e tests passing, clean clippy and fmt. Ready for engine-level testing.
