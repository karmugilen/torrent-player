hashing + webseed_peer compile and tests pass (lib filters + tests/hashing_*.rs). agy-2 call surface:

hashing::PieceHasherPool::new(workers, result_tx) / with_workers(n) -> (pool, rx)
  queue(HashJob { piece_index, priority, storage, expected: ExpectedHash, piece_length, data_length, v1_zero_pad, smartban, seq })
  set_cancel_token(CancelToken) / wake() / shutdown() / set_smart_ban_cache(Arc<SmartBanCache>)
hash_piece_sync(storage, &ExpectedHash, data_len, piece_len, v1_pad, smartban, &mut buf) -> Sync outcome
ExpectedHash::{V1([u8;20]), V2PieceLayer([u8;32]), V2FileRoot([u8;32])}
SmartBanCache::{record_block, check_block, forget_block_seq, has_peer_for_blocks}
PieceCompletionCache::{apply_hash_ok, apply_hash_bad, next_novel_hash_count, ignore_for_requests}
SHA-1: SoftSha1 | RingSha1 behind PieceSha1; selected_backend() — aarch64=ring, else faster of soft/ring (this host Soft~140 MiB/s vs ring~38)

webseed_peer::WebseedPeer::new(WebseedPeerConfig) / set_info(Info)
  fetch_request_range(begin, end, request_span, want_chunk, cancel).await -> Vec<WebseedChunk>
  provided_bad_data() / convict(err, term) / wake_for_urgent_piece(piece, prio, complete) -> Option<WakeReason>
wake_for_urgent_piece(has_webseeds, closed, data_dl_disallowed, complete, pure_priority) -> Option<WakeReason>