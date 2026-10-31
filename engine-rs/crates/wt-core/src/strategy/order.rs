//! Piece ordering and `GetRequestablePieces` scan.
//!
//! Priority first, so playback keeps file/playhead order. Background (`Normal`)
//! pieces finish what is started, then go rarest-first.

use std::cmp::Ordering;

use wt_types::PiecePriority;

use super::piece_request_order::{
    PieceRequestOrder, PieceRequestOrderItem, PieceRequestOrderState,
};

/// Pieces this rare come first. Above it, file order: blocks then land next to
/// each other on disk instead of all over a sparse file.
const RARE_AVAILABILITY: i32 = 2;

/// Rank among equal-priority pieces, lower first. Only `Normal` pieces have
/// one: anything higher is near a playhead and stays in file order. Rare pieces
/// are the most likely to vanish and the most useful to trade; started pieces
/// come first so hashing and write-back see whole pieces.
pub fn background_rank(priority: PiecePriority, partial: bool, availability: i32) -> (bool, i32) {
    if priority == PiecePriority::Normal {
        (!partial, availability.min(RARE_AVAILABILITY + 1))
    } else {
        (false, 0)
    }
}

/// Returns `Ordering::Less` when `i` should be requested before `j`: higher
/// priority first, then [`background_rank`], lower piece index, lower infohash.
pub fn piece_order_cmp(i: &PieceRequestOrderItem, j: &PieceRequestOrderItem) -> Ordering {
    // Go: multiless.Int(j.Priority, i.Priority) — higher priority sorts first.
    j.state
        .priority
        .cmp(&i.state.priority)
        .then_with(|| {
            background_rank(i.state.priority, i.state.partial, i.state.availability)
                .cmp(&background_rank(j.state.priority, j.state.partial, j.state.availability))
        })
        .then_with(|| i.key.index.cmp(&j.key.index))
        .then_with(|| i.key.info_hash.cmp(&j.key.info_hash))
}

/// Go `NewDefaultClientConfig` field `MaxUnverifiedBytes` (`64 << 20`).
/// The engine never overrides it. `0` is the scan's uncapped sentinel, so
/// production must not use [`StrategyLimits::default`].
pub const DEFAULT_MAX_UNVERIFIED_BYTES: i64 = 64 << 20;

/// The same cap for a saved download with nothing playing. It must stay under
/// the storage's RAM for saved downloads, so pieces finish in RAM.
pub const SAVED_MAX_UNVERIFIED_BYTES: i64 = 192 << 20;
/// Pieces a saved download keeps open, as a floor when pieces are large.
pub const SAVED_OPEN_MIN_PIECES: i64 = 4;

/// Shared limits for a requestable-piece scan (`requestStrategy.Input`).
#[derive(Clone, Copy, Debug, Default)]
pub struct StrategyLimits {
    /// Remaining storage capacity in bytes. `None` = uncapped.
    pub capacity: Option<i64>,
    /// Cap on cumulative unverified piece length. `0` = uncapped.
    pub max_unverified_bytes: i64,
}

impl StrategyLimits {
    /// Single-torrent Go input: storage capacity uncapped, unverified bytes
    /// capped at 64 MiB. Past that, new pieces wait so the write-back cache
    /// can flush whole pieces instead of thrashing partial ones.
    pub fn go_default() -> Self {
        Self {
            capacity: None,
            max_unverified_bytes: DEFAULT_MAX_UNVERIFIED_BYTES,
        }
    }

    /// Nothing playing: pieces gather in RAM, so RAM sets the limit.
    pub fn saved_download() -> Self {
        Self {
            capacity: None,
            max_unverified_bytes: SAVED_MAX_UNVERIFIED_BYTES,
        }
    }
}

/// Query a torrent piece during the scan (Go `requestStrategy.Torrent`/`Piece`).
pub trait TorrentPieceQuery {
    fn piece_length(&self, info_hash: &[u8; 20]) -> i64;
    /// Ready for immediate download (effective priority > None).
    fn should_request(&self, info_hash: &[u8; 20], piece: u32) -> bool;
    /// Hashing / marking / queued — still counts toward unverified budget.
    fn count_unverified(&self, info_hash: &[u8; 20], piece: u32) -> bool;
}

/// Walk `order` in priority order, applying storage + unverified budgets.
///
/// `request_piece` is called for each piece where `should_request` is true.
/// Return `false` from the callback to stop early (propagates as `false`).
/// Returns `true` if the scan completed without the callback stopping it.
pub fn get_requestable_pieces<Q, F>(
    order: &PieceRequestOrder,
    limits: &StrategyLimits,
    query: &Q,
    mut request_piece: F,
) -> bool
where
    Q: TorrentPieceQuery + ?Sized,
    F: FnMut([u8; 20], u32, PieceRequestOrderState) -> bool,
{
    let mut storage_left = limits.capacity;
    let mut unverified: i64 = 0;
    let max_unverified = limits.max_unverified_bytes;

    for item in order.iter() {
        let ih = item.key.info_hash;
        let piece_len = query.piece_length(&ih);

        if let Some(ref mut left) = storage_left {
            if *left < piece_len {
                break;
            }
            *left -= piece_len;
        }

        let requesting = query.should_request(&ih, item.key.index);
        if requesting {
            if !request_piece(ih, item.key.index, item.state) {
                return false;
            }
        } else if !query.count_unverified(&ih, item.key.index) {
            continue;
        }

        unverified += piece_len;
        if max_unverified != 0 && unverified >= max_unverified {
            break;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::super::piece_request_order::{
        PieceRequestOrder, PieceRequestOrderKey, PieceRequestOrderState,
    };
    use super::*;
    use wt_types::PiecePriority;

    struct SimpleQuery {
        piece_len: i64,
        request: Vec<bool>,
    }

    impl TorrentPieceQuery for SimpleQuery {
        fn piece_length(&self, _: &[u8; 20]) -> i64 {
            self.piece_len
        }
        fn should_request(&self, _: &[u8; 20], piece: u32) -> bool {
            self.request.get(piece as usize).copied().unwrap_or(false)
        }
        fn count_unverified(&self, _: &[u8; 20], _: u32) -> bool {
            false
        }
    }

    #[test]
    fn go_default_matches_client_config() {
        let limits = StrategyLimits::go_default();
        assert_eq!(limits.max_unverified_bytes, 64 << 20);
        assert_eq!(limits.capacity, None);
        // Default stays the uncapped sentinel. Production uses go_default.
        assert_eq!(StrategyLimits::default().max_unverified_bytes, 0);
    }

    #[test]
    fn scan_stops_at_unverified_budget() {
        let mut order = PieceRequestOrder::with_capacity(4);
        let ih = [1u8; 20];
        for i in 0..4u32 {
            order.add(
                PieceRequestOrderKey {
                    info_hash: ih,
                    index: i,
                },
                PieceRequestOrderState {
                    availability: 1,
                    priority: PiecePriority::Normal,
                    partial: false,
                },
            );
        }
        let query = SimpleQuery {
            piece_len: 100,
            request: vec![true, true, true, true],
        };
        let limits = StrategyLimits {
            capacity: None,
            max_unverified_bytes: 250, // two pieces (200), third trips the limit after add
        };
        let mut seen = Vec::new();
        get_requestable_pieces(&order, &limits, &query, |_, p, _| {
            seen.push(p);
            true
        });
        // After pieces 0 and 1: unverified=200; piece 2 is still submitted then unverified=300 >= 250 break.
        // Go adds piece length after the callback, then checks — so 0,1,2 are seen.
        assert_eq!(seen, vec![0, 1, 2]);
    }

    #[test]
    fn property_iteration_matches_piece_order_less() {
        // Random-ish priorities: iteration must be sorted by piece_order_cmp.
        let mut order = PieceRequestOrder::with_capacity(16);
        let prios = [
            PiecePriority::None,
            PiecePriority::Normal,
            PiecePriority::High,
            PiecePriority::Readahead,
            PiecePriority::Next,
            PiecePriority::Now,
        ];
        for i in 0..16u32 {
            order.add(
                PieceRequestOrderKey {
                    info_hash: [0u8; 20],
                    index: i,
                },
                PieceRequestOrderState {
                    availability: (i % 5) as i32,
                    priority: prios[(i as usize) % prios.len()],
                    partial: i % 2 == 0,
                },
            );
        }
        let items: Vec<_> = order.iter().collect();
        for w in items.windows(2) {
            assert!(
                piece_order_cmp(&w[0], &w[1]) != Ordering::Greater,
                "out of order: {:?} then {:?}",
                w[0],
                w[1]
            );
        }
    }
}
