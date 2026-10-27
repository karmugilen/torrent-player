//! Head-of-line detection, rescue, and hedge decisions.

use std::time::Duration;

use wt_types::RequestIndex;

use crate::tuning::SeekTuning;

use super::clock::{age, Clock};
use super::decisions::{expected_arrival_with_learned, pending_requests};
use super::traits::{ArrivalPredictor, PeerView, TorrentView};

/// 2 MiB of 16 KiB blocks (`holWindowBlocks`).
pub const HOL_WINDOW_BLOCKS: u32 = 128;
/// Bounds rescues per nudge (`holRescuesPerNudge`).
pub const HOL_RESCUES_PER_NUDGE: usize = 16;
/// Bounds hedges per nudge (`hedgesPerNudge`).
pub const HEDGES_PER_NUDGE: usize = 16;

/// First missing block at or after the playhead within the HOL window.
pub fn head_of_line<T: TorrentView>(t: &T) -> Option<RequestIndex> {
    let focus = t.request_focus_index()?;
    let end = focus
        .saturating_add(HOL_WINDOW_BLOCKS)
        .min(t.max_end_request());
    (focus..end).find(|&r| !t.block_arrived(r))
}

/// Overdue if age ≥ max(HOLAfterMs, 2× holder RTT). Learned path optional.
pub fn block_is_overdue(
    clock: &dyn Clock,
    tuning: &SeekTuning,
    request_when: std::time::Instant,
    holder_rtt: Duration,
    learned_overdue: Option<bool>,
) -> bool {
    if let Some(overdue) = learned_overdue {
        return overdue;
    }
    let after = Duration::from_secs_f64(tuning.hol_after_ms / 1000.0);
    age(clock, request_when) >= after.max(holder_rtt.saturating_mul(2))
}

/// One HOL rescue move: cancel holder, request on best sooner peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RescueMove<Id> {
    pub req: RequestIndex,
    pub from: Id,
    pub to: Id,
}

/// Decide rescues for overdue HOL blocks (Go `webtorRescueHeadOfLine` body).
pub fn rescue_decisions<'a, T, P>(
    clock: &dyn Clock,
    tuning: &SeekTuning,
    torrent: &T,
    peers: &'a [P],
    learned: Option<&dyn ArrivalPredictor>,
    features_for: &dyn Fn(&P) -> Option<[f64; crate::arrival::ARRIVAL_FEATURES]>,
) -> Vec<RescueMove<P::Id>>
where
    T: TorrentView<PeerId = P::Id>,
    P: PeerView,
    P::Id: PartialEq + Copy,
{
    let mut out = Vec::new();
    if !tuning.hol_rescue {
        return out;
    }
    let Some(head) = head_of_line(torrent) else {
        return out;
    };
    let Some(focus) = torrent.request_focus_index() else {
        return out;
    };
    let end = focus
        .saturating_add(HOL_WINDOW_BLOCKS)
        .min(torrent.max_end_request());
    let chunk = torrent.chunk_size();

    for r in head..end {
        if out.len() >= HOL_RESCUES_PER_NUDGE {
            break;
        }
        if torrent.block_arrived(r) {
            continue;
        }
        let Some(holder_id) = torrent.requesting_peer(r) else {
            continue;
        };
        let Some(when) = torrent.request_when(r) else {
            continue;
        };
        let Some(holder) = peers.iter().find(|p| p.id() == holder_id) else {
            continue;
        };
        if !block_is_overdue(clock, tuning, when, holder.rtt(), None) {
            continue;
        }
        let piece = torrent.piece_of_request(r);
        if torrent.hashing_or_marking(piece) {
            continue;
        }
        let holder_at = expected_arrival_with_learned(
            tuning,
            learned,
            features_for(holder),
            holder.download_rate_bps(),
            holder.rtt(),
            holder.queued_requests(),
            chunk,
        );
        let mut best: Option<&P> = None;
        let mut best_at = f64::MAX;
        for c in peers {
            // Go also skips peers whose Cancelled set contains r.
            if c.id() == holder_id || c.closed() || c.peer_choking() || !c.has_piece(piece) {
                continue;
            }
            if pending_requests(tuning, c.queued_requests(), c.cancelled_requests())
                >= c.nominal_max_requests()
            {
                continue;
            }
            let at = expected_arrival_with_learned(
                tuning,
                learned,
                features_for(c),
                c.download_rate_bps(),
                c.rtt(),
                c.queued_requests(),
                chunk,
            );
            if best.is_none() || at < best_at {
                best = Some(c);
                best_at = at;
            }
        }
        if let Some(best) = best {
            if best_at < holder_at {
                out.push(RescueMove {
                    req: r,
                    from: holder_id,
                    to: best.id(),
                });
            }
        }
    }
    out
}

/// One hedge: duplicate request on soonest other peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HedgeMove<Id> {
    pub req: RequestIndex,
    pub holder: Id,
    pub hedger: Id,
}

/// Decide hedges (Go `webtorHedgeHeadOfLine` body).
pub fn hedge_decisions<'a, T, P>(
    clock: &dyn Clock,
    tuning: &SeekTuning,
    torrent: &T,
    peers: &'a [P],
    learned: Option<&dyn ArrivalPredictor>,
    features_for: &dyn Fn(&P) -> Option<[f64; crate::arrival::ARRIVAL_FEATURES]>,
    learned_overdue_for: &dyn Fn(&P, RequestIndex) -> Option<bool>,
) -> Vec<HedgeMove<P::Id>>
where
    T: TorrentView<PeerId = P::Id>,
    P: PeerView,
    P::Id: PartialEq + Copy,
{
    let mut out = Vec::new();
    if !tuning.hedge {
        return out;
    }
    let Some(head) = head_of_line(torrent) else {
        return out;
    };
    let Some(focus) = torrent.request_focus_index() else {
        return out;
    };
    let end = focus
        .saturating_add(HOL_WINDOW_BLOCKS)
        .min(torrent.max_end_request());
    let chunk = torrent.chunk_size();

    for r in head..end {
        if out.len() >= HEDGES_PER_NUDGE {
            break;
        }
        if torrent.block_arrived(r) {
            continue;
        }
        let Some(holder_id) = torrent.requesting_peer(r) else {
            continue;
        };
        if torrent.hedge_peer(r).is_some() {
            continue;
        }
        let Some(when) = torrent.request_when(r) else {
            continue;
        };
        let Some(holder) = peers.iter().find(|p| p.id() == holder_id) else {
            continue;
        };
        let learned_od = learned_overdue_for(holder, r);
        if !block_is_overdue(clock, tuning, when, holder.rtt(), learned_od) {
            continue;
        }
        let piece = torrent.piece_of_request(r);
        if torrent.hashing_or_marking(piece) {
            continue;
        }
        let mut best: Option<&P> = None;
        let mut best_at = f64::MAX;
        for c in peers {
            if c.id() == holder_id
                || c.closed()
                || c.peer_choking()
                || !c.has_piece(piece)
                || c.valid_receive_count(r) > 0
            {
                continue;
            }
            if pending_requests(tuning, c.queued_requests(), c.cancelled_requests())
                >= c.nominal_max_requests()
            {
                continue;
            }
            let at = expected_arrival_with_learned(
                tuning,
                learned,
                features_for(c),
                c.download_rate_bps(),
                c.rtt(),
                c.queued_requests(),
                chunk,
            );
            if at == f64::MAX {
                continue;
            }
            if best.is_none() || at < best_at {
                best = Some(c);
                best_at = at;
            }
        }
        if let Some(best) = best {
            out.push(HedgeMove {
                req: r,
                holder: holder_id,
                hedger: best.id(),
            });
        }
    }
    out
}

/// Whether hedge answered bookkeeping should clear (`webtorHedgeAnswered`).
pub fn hedge_answered<Id: PartialEq>(
    hedges: &std::collections::HashMap<RequestIndex, Id>,
    req: RequestIndex,
    peer: Id,
) -> bool {
    hedges.get(&req).is_some_and(|h| *h == peer)
}
