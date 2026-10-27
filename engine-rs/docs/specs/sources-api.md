wt-core::sources is ready (pub mod sources;). agy-2 should call:

```rust
Sources::start(SourcesConfig) -> Result<Arc<Sources>>
SourcesConfig::from_client_config(&ClientConfig) // then set peer_id, dialer, datagram

TorrentKeys { v1: Option<InfoHash>, v2_short: Option<InfoHash> }
TorrentKeys::from_spec(&TorrentSpec) // BOTH hashes for hybrid
TorrentKeys::all_short() -> Vec<InfoHash>

trait TorrentAnnounceInfo: peer_id/downloaded/uploaded/left/port/announce_key/need_data/want_peers/completed/newly_completed
trait PeerSink: add_peers(ih, Vec<PeerInfo>); add_webrtc_peer(...); add_web_seeds(ih, urls)

Sources::register_torrent(RegisterTorrent { keys, trackers, web_seeds, info, sink })
Sources::refresh_torrent(&keys)
Sources::add_trackers(&keys, tiers) // no-op if short hash unregistered (WEBTOR_PATCH)
Sources::set_paused(&keys, bool)
Sources::unregister_torrent(&keys)
Sources::add_dht_nodes(&[String])
Sources::dht_contacts().await -> Vec<SocketAddr>
Sources::is_registered(&InfoHash) -> bool
Sources::close().await

map_tracker_status(&TrackerStatusEvent) -> StatusEvent  // wire status_callbacks
```

Shared UDP: SourcesConfig.datagram = Some(Arc::new(SharedDatagramAdapter::new(utp))) from agy-1.
Tests: cargo test -p wt-core --test sources_glue (also tests/sources_standalone via #[path]).