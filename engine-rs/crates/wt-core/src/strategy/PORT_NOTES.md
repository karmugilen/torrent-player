# strategy + choking port notes

Owned paths: `src/strategy/`, `src/choking/`, plus `tests/strategy_choking.rs`.
`lib.rs` only gained `pub mod choking;` / `pub mod strategy;` (coordinator-approved).

## Ported

### strategy/
- `PieceRequestOrder` with `BTreeSet` (Go btree + keys map): add/update/delete O(log n)
- Webtor `pieceOrderLess`: priority desc → piece index asc → infohash (availability stored, unused)
- `get_requestable_pieces`: storage capacity + max unverified bytes scan
- Desired request collection + `request_less_by_value` (allowed-fast, priority, same-peer, unrequested, outstanding, recency, focus distance, webseed vs file order)
- Webtor helpers: `candidate_budget`, `focus_distance`, `playback_pipeline_limit`

### choking/
- Upload-slot rechoke with optimistic rotation (`compute_choke_decisions`)
- Leech order by download rate; seed order by upload rate
- Go `uploadAllowed` tit-for-tat gate + seed-policy `data_upload_disallowed` hook
- BEP 6 `generate_allowed_fast_set` — matches BEP example vectors (k=7, k=9)

## Differs from Go

- **Choking model**: anacrolix uses per-peer `uploadAllowed` (tit-for-tat + seeding) with no upload-slot table. Rust exposes both: classic slots via `compute_choke_decisions`, and Go-compatible `upload_allowed`. agy-2 can pick either (or combine: slots ∩ upload_allowed).
- **AllowedFast generation**: Go receives AllowedFast but does not generate/send sets. Rust implements the BEP 6 algorithm so the engine can send them; vectors match the BEP reference (not a Go golden file).
- Apply/steal/HOL/hedge/refocus stay in agy-2 peer/torrent code; strategy returns desired candidates only.

## Remains (agy-2)

- Wire `compute_desired_requests` / `sort_requests_for_peer` into peer refill
- Call `compute_choke_decisions` on a 10s timer; send Choke/Unchoke
- On fast handshake, `generate_allowed_fast_set` + send AllowedFast messages
- Pass `data_upload_disallowed` from engine seed policy into `ChokeConfig`

## Tests

- 22 lib unit tests under `strategy::` / `choking::`
- 6 integration tests in `tests/strategy_choking.rs`
- BEP 6 vectors; property: iteration order == `piece_order_cmp`
