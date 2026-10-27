# Task: wt-discovery follow-up — finish the leftovers
Target: engine-rs/crates/wt-discovery/ only (your crate; committed as fb0e093).
Change (from your PORT_NOTES "Left" list):
1. Replace src/tracker/bencode_lite.rs with wt_meta::bencode (done and committed) and delete bencode_lite.rs.
2. Wire DialerHook through the HTTP client so the engine can route and track tracker sockets (engine-go network.go / trackerSockets.DialContext semantics: the engine supplies a dial function; HTTP, HTTPS and WebSocket trackers use it). Add a test with a counting dialer.
3. Shared UDP socket: DHT and UDP trackers must be able to share the uTP socket like anacrolix does. Read engine-rs/crates/wt-wire/src/utp.rs and transport.rs (agy-1). Define in wt-discovery a small trait for "send datagram / receive non-uTP datagrams" that the core can implement over wt-wire's socket, make DHT and UDP trackers work over it, and keep a standalone socket fallback. If wt-wire does not expose a way to receive non-uTP packets, ask the coordinator to route the request to agy-1 with the exact signature you need.
4. Local fake WebSocket tracker signaling test (offer/answer relay between two in-process peers through the fake tracker).
5. clippy --all-targets -D warnings + fmt clean.
Observable acceptance: all wt-discovery tests pass including the new ones; no bencode_lite.rs; PORT_NOTES updated with what remains (should be nothing or clearly justified).
