# PeerTorrent trait (peer_conn.rs) — for agy-2 to implement on TorrentInner

```rust
/// Opaque peer key used in assignment maps (typically SocketAddr hash or u64).
pub type PeerKey = u64;

pub trait PeerTorrent: Send + Sync {
    fn peer_key(&self, addr: SocketAddr) -> PeerKey;
    fn have_info(&self) -> bool;
    fn num_pieces(&self) -> u32;
    fn piece_length(&self) -> i64;
    fn total_length(&self) -> i64;
    fn chunk_size(&self) -> u32;
    fn info_hash(&self) -> InfoHash;
    fn data_download_disallowed(&self) -> bool;
    fn data_upload_disallowed(&self) -> bool;
    fn is_seeding(&self) -> bool;
    fn closed(&self) -> bool;

    // Piece / chunk state (piece_state)
    fn piece_complete(&self, piece: PieceIndex) -> bool;
    fn piece_priority(&self, piece: PieceIndex) -> PiecePriority;
    fn chunk_dirty(&self, req: RequestIndex) -> bool;
    fn mark_chunk_dirty(&self, req: RequestIndex);
    fn clear_chunk_requested(&self, req: RequestIndex);
    fn mark_chunk_requested(&self, req: RequestIndex);
    fn piece_all_dirty(&self, piece: PieceIndex) -> bool;
    fn note_availability(&self, piece: PieceIndex, delta: i32); // peer gained/lost piece

    // Storage
    fn write_chunk(&self, piece: PieceIndex, begin: u32, data: &[u8]) -> Result<()>;
    fn read_chunk(&self, piece: PieceIndex, begin: u32, length: u32) -> Result<Bytes>;

    // Hashing handoff
    fn queue_piece_hash(&self, piece: PieceIndex);

    // Request assignment bookkeeping
    fn requesting_peer(&self, req: RequestIndex) -> Option<PeerKey>;
    fn request_when(&self, req: RequestIndex) -> Option<Instant>;
    fn assign_request(&self, req: RequestIndex, peer: PeerKey, when: Instant);
    fn clear_assignment(&self, req: RequestIndex);
    fn request_torrent_snapshot(&self) -> RequestTorrentSnapshot;
    fn piece_request_order(&self) -> PieceRequestOrder; // or build snapshot only

    // Webtor / focus
    fn request_focus_index(&self) -> Option<RequestIndex>;
    fn urgent_steal_after(&self) -> Duration;
    fn seek_tuning(&self) -> SeekTuning;
    fn hedge_peer(&self, req: RequestIndex) -> Option<PeerKey>;
    fn set_hedge_peer(&self, req: RequestIndex, peer: PeerKey);
    fn clear_hedge(&self, req: RequestIndex);
    fn record_rescue(&self);
    fn record_hedge(&self);
    fn peer_views_for_webtor(&self) -> Vec<WebtorPeerSnap>; // for fasterFocus/HOL

    // Strategy helpers already pure — peer builds DesiredPeerSnapshot locally

    // Metadata / PEX / DHT / sources
    fn on_metadata_extended(&self, peer: PeerKey, msg: &MetadataMsg) -> Result<()>;
    fn metadata_serve(&self, piece: u32) -> Option<Bytes>;
    fn metadata_size(&self) -> Option<u32>;
    fn has_metadata(&self) -> bool;
    fn ingest_pex(&self, from: SocketAddr, msg: &PexMsg) -> Result<()>;
    fn add_dht_node(&self, addr: SocketAddr);
    fn on_peer_listen_port(&self, peer_addr: SocketAddr, port: u16);

    // Upload policy inputs
    fn choke_config(&self) -> ChokeConfig;

    // Callbacks / events
    fn bump_change(&self);
    fn on_chunk_received(&self, peer: PeerKey, req: RequestIndex, len: usize);
}
```

PeerConnHandler implements ConnHandler; agy-2 spawns conn::driver with it and implements PeerTorrent on TorrentInner.