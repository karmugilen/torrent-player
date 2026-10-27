# strategy + choking API for agy-2

Wired: `pub mod choking; pub mod strategy;` in lib.rs (coordinator-approved).

## strategy (desired requests)

```rust
// Maintain per storage-group:
PieceRequestOrder::with_capacity(n)
order.add(PieceRequestOrderKey { info_hash: [u8;20], index: u32 },
          PieceRequestOrderState { availability: i32, priority: PiecePriority, partial: bool })
order.update(key, state) -> bool
order.delete(key) -> bool
order.iter() // priority desc, then piece index (Webtor file order)

get_requestable_pieces(&order, &StrategyLimits { capacity: Option<i64>, max_unverified_bytes: i64 },
                       &impl TorrentPieceQuery, |ih, piece, state| -> bool)

compute_desired_requests(&RequestTorrentSnapshot, &DesiredPeerSnapshot, &order, &RequestTuning)
  -> DesiredRequestState { request_indexes, interested, piece_states }
sort_requests_for_peer(&mut indexes, &torrent, &peer, &piece_states)

candidate_budget(nominal_max_requests: u32, bound_candidates: bool) -> usize
focus_distance(piece: u32, focus: u32) -> i32
playback_pipeline_limit(&RequestTuning, rate_bps, rtt_secs, chunk_size) -> Option<u32>
```

## choking

```rust
ChokeState::new(now)
compute_choke_decisions(&[PeerChokeInput], &ChokeConfig, &mut ChokeState, now)
  -> HashMap<PeerId, ChokeDecision>  // Choke | Unchoke
// PeerChokeInput: id, interested, currently_choking, download_rate, upload_rate,
//   bytes_downloaded, bytes_uploaded, is_snubbed
// ChokeConfig: upload_slots (default 4), optimistic_slots (1), intervals, no_upload,
//   data_upload_disallowed (from applySeedPolicy), seeding, tit_for_tat_headroom

upload_allowed(&PeerChokeInput, &ChokeConfig, peer_has_wanted_pieces: bool) -> bool  // Go gate

generate_allowed_fast_set(Ipv4Addr, &infohash, num_pieces, k) -> Vec<u32>  // BEP 6
```

Tests: 22 lib + 6 integration pass; BEP6 vectors match; clippy -D warnings --no-deps clean on wt-core.