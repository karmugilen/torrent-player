# Task (grok-swarm): find and connect to as many peers as Go, and get metadata as fast
You are grok-swarm. You OWN engine-rs/crates/wt-core/src/client.rs, src/sources/, src/metadata/, src/pex/, src/webseed_peer.rs, src/webseed_download.rs, and engine-rs/crates/wt-discovery/.
Question to answer: why does Rust hold 2-4 peers where Go holds 38, and take 18-30 s for metadata where Go takes 8 s? Check against anacrolix (client.go, torrent.go openNewConns/wantPeers, tracker_scraper.go, dht.go, pex*.go, ut-metadata, webtor patches) and the config in engine-go/server.go:
1. Tracker announces: are HTTP/UDP/WS announces actually sent to every tracker (original and supplemented)? Check numwant, event=started, the left/uploaded values, re-announce intervals, tracker status ("waiting" never changing is a bug), and whether returned peers reach the torrent's peer pool.
2. DHT: bootstrap succeeds? The get_peers traversal returns how many peers for Ubuntu? Are they added? Go announces continuously while it wants conns (dhtAnnouncer).
3. Dialing: how many candidates are dialled per second (Go dial rate limiter), half-open limits (24 per torrent, 48 total), dial timeouts (8 s nominal, 2 s minimum), TCP+uTP both tried, backoff after failures (not too long), and wants_conns/peers low and high water (Go: max(40, 2x max-peers) and max(120, 4x max-peers)).
4. PEX: we receive PEX peers and dial them; we send PEX too.
5. Metadata: request ut_metadata pieces from several peers in parallel and retry on reject or timeout, as anacrolix does.
Find each cause with traces (the zz_ diagnostic test), fix it, and measure after each fix. grok-wire works on the peer wire protocol and data flow (peer_conn.rs, conn/, torrent_impls.rs, strategy/, choking/, wt-wire). Coordinate through requests.md.
