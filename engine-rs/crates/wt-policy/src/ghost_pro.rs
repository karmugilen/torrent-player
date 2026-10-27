//! Ghost Pro bitfield-aware slot recycling (`engine-go/ghost_pro.go`).
//!
//! Dial-rate probing reuses [`crate::GhostDialController`]. Peer recycling
//! uses [`GhostProSwarm::choose_victim`] with overlap snapshots the engine
//! computes from bitfields + wanted pieces.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use wt_types::PieceIndex;

use crate::peer_exploration::PeerKey;
use crate::piece_span::{span_from_file, FileLayout, PieceSpan, PlaybackWindow};

#[derive(Clone, Debug)]
struct Progress {
    useful: i64,
    uploaded: i64,
    idle_since: Instant,
}

/// Peer sample for Ghost Pro recycling.
#[derive(Clone, Debug)]
pub struct GhostProPeerObservation {
    pub peer: PeerKey,
    pub useful: i64,
    pub uploaded: i64,
    pub overlap_pieces: i32,
    pub contiguous_overlap: i32,
    pub is_seeder: bool,
}

/// Bitfield-aware peer recycling state (per torrent).
#[derive(Clone, Debug, Default)]
pub struct GhostProSwarm {
    previous: HashMap<PeerKey, Progress>,
    next_sample: Option<Instant>,
    next_rotation: Option<Instant>,
}

impl GhostProSwarm {
    /// Prefer zero contiguous-path overlap after 15s idle, then zero selection
    /// overlap, then lowest-overlap peer idle ≥ 60s. Seeders are protected.
    pub fn choose_victim(
        &mut self,
        now: Instant,
        peers: &[GhostProPeerObservation],
        capacity: usize,
        pending: usize,
        playback_active: bool,
    ) -> Option<PeerKey> {
        if self.next_sample.map(|t| now < t).unwrap_or(false) {
            return None;
        }
        self.next_sample = Some(now + Duration::from_secs(10));
        let mut next = HashMap::with_capacity(peers.len());

        let mut zero_overlap_victim: Option<PeerKey> = None;
        let mut zero_oldest: Option<Instant> = None;
        let mut path_victim: Option<PeerKey> = None;
        let mut path_oldest: Option<Instant> = None;
        let mut idle_victim: Option<PeerKey> = None;
        let mut idle_oldest: Option<Instant> = None;
        let mut min_overlap = i32::MAX;

        for obs in peers {
            let known = self.previous.get(&obs.peer);
            let idle_since = match known {
                Some(prev) if obs.useful == prev.useful && obs.uploaded == prev.uploaded => {
                    prev.idle_since
                }
                _ => now,
            };
            next.insert(
                obs.peer,
                Progress {
                    useful: obs.useful,
                    uploaded: obs.uploaded,
                    idle_since,
                },
            );

            if obs.is_seeder {
                continue;
            }
            let idle_duration = now.duration_since(idle_since);
            if playback_active
                && obs.contiguous_overlap == 0
                && idle_duration >= Duration::from_secs(15)
                && (path_victim.is_none() || path_oldest.map(|o| idle_since < o).unwrap_or(true))
            {
                path_victim = Some(obs.peer);
                path_oldest = Some(idle_since);
            }
            if obs.overlap_pieces == 0
                && idle_duration >= Duration::from_secs(15)
                && (zero_overlap_victim.is_none()
                    || zero_oldest.map(|o| idle_since < o).unwrap_or(true))
            {
                zero_overlap_victim = Some(obs.peer);
                zero_oldest = Some(idle_since);
            }
            if idle_duration >= Duration::from_secs(60) {
                let overlap = if playback_active {
                    obs.contiguous_overlap
                } else {
                    obs.overlap_pieces
                };
                if idle_victim.is_none()
                    || overlap < min_overlap
                    || (overlap == min_overlap
                        && idle_oldest.map(|o| idle_since < o).unwrap_or(true))
                {
                    idle_victim = Some(obs.peer);
                    idle_oldest = Some(idle_since);
                    min_overlap = overlap;
                }
            }
        }
        self.previous = next;

        if capacity < 2
            || peers.len() < 2
            || peers.len() < capacity
            || pending == 0
            || self.next_rotation.map(|t| now < t).unwrap_or(false)
        {
            return None;
        }

        let victim = path_victim.or(zero_overlap_victim).or(idle_victim);
        if let Some(v) = victim {
            self.next_rotation = Some(now + Duration::from_secs(20));
            self.previous.remove(&v);
        }
        victim
    }
}

/// File selection input for wanted-piece calculation.
#[derive(Clone, Debug)]
pub struct WantedFile {
    pub index: usize,
    pub layout: FileLayout,
    pub selected: bool,
}

#[derive(Clone, Debug, Default)]
pub struct WantedContext {
    pub direct_play: bool,
    pub file_only: bool,
    pub focused_file: Option<usize>,
    pub piece_length: i64,
    pub num_pieces: PieceIndex,
}

/// Uncompleted pieces within the active selection (or focused Direct Play file).
pub fn compute_wanted_pieces(
    files: &[WantedFile],
    ctx: &WantedContext,
    complete: impl Fn(PieceIndex) -> bool,
) -> HashSet<PieceIndex> {
    let mut wanted = HashSet::new();
    if ctx.piece_length <= 0 {
        return wanted;
    }
    for f in files {
        if !f.selected {
            continue;
        }
        if (ctx.direct_play || ctx.file_only)
            && ctx.focused_file.map(|i| i != f.index).unwrap_or(true)
        {
            continue;
        }
        let span = span_from_file(&f.layout, ctx.piece_length, 0, f.layout.length);
        for p in span.begin..span.end.min(ctx.num_pieces) {
            if !complete(p) {
                wanted.insert(p);
            }
        }
    }
    wanted
}

/// Intersect each reader's forward path with missing selected pieces.
pub fn contiguous_wanted_pieces(
    windows: &[PlaybackWindow],
    wanted: &HashSet<PieceIndex>,
) -> HashSet<PieceIndex> {
    let mut pieces = HashSet::new();
    for window in windows {
        for span in [window.urgent, window.readahead, window.lookahead] {
            for i in span.begin..span.end {
                if wanted.contains(&i) {
                    pieces.insert(i);
                }
            }
        }
    }
    pieces
}

/// Count how many wanted pieces appear in the peer's bitfield.
pub fn peer_overlap(wanted: &HashSet<PieceIndex>, peer_has: impl Fn(PieceIndex) -> bool) -> i32 {
    let mut n = 0;
    for &p in wanted {
        if peer_has(p) {
            n += 1;
        }
    }
    n
}

/// Raise pieces in a span into a set (helper for tests / engine).
pub fn collect_span(dst: &mut HashSet<PieceIndex>, span: PieceSpan) {
    for i in span.begin..span.end {
        dst.insert(i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contiguous_wanted_pieces_excludes_reader_gaps_and_completed_pieces() {
        let windows = [
            PlaybackWindow {
                urgent: PieceSpan::new(10, 12),
                readahead: PieceSpan::new(12, 14),
                lookahead: PieceSpan::new(14, 16),
                ..Default::default()
            },
            PlaybackWindow {
                urgent: PieceSpan::new(100, 102),
                ..Default::default()
            },
        ];
        let wanted: HashSet<_> = [8, 10, 11, 13, 15, 50, 100, 101].into_iter().collect();
        let got = contiguous_wanted_pieces(&windows, &wanted);
        assert_eq!(got.len(), 6);
        assert!(!got.contains(&8));
        assert!(!got.contains(&12));
        assert!(!got.contains(&50));
        assert!(got.contains(&100));
        assert!(got.contains(&15));
    }

    #[test]
    fn test_ghost_pro_prioritizes_idle_peers_without_playback_path_pieces() {
        let now = Instant::now();
        let outside = 1u64;
        let on_path = 2u64;
        let active = 3u64;
        let seeder = 4u64;
        let peers = [
            GhostProPeerObservation {
                peer: on_path,
                useful: 0,
                uploaded: 0,
                overlap_pieces: 1,
                contiguous_overlap: 1,
                is_seeder: false,
            },
            GhostProPeerObservation {
                peer: active,
                useful: 0,
                uploaded: 0,
                overlap_pieces: 50,
                contiguous_overlap: 0,
                is_seeder: false,
            },
            GhostProPeerObservation {
                peer: seeder,
                useful: 0,
                uploaded: 0,
                overlap_pieces: 100,
                contiguous_overlap: 0,
                is_seeder: true,
            },
            GhostProPeerObservation {
                peer: outside,
                useful: 0,
                uploaded: 0,
                overlap_pieces: 80,
                contiguous_overlap: 0,
                is_seeder: false,
            },
        ];
        for (name, playback, capacity, pending, want) in [
            ("playback path", true, 4, 1, Some(outside)),
            ("background", false, 4, 1, None),
            ("spare slots", true, 5, 1, None),
            ("no candidates", true, 4, 0, None),
        ] {
            let mut state = GhostProSwarm::default();
            let mut obs = peers.to_vec();
            state.choose_victim(now, &obs, capacity, pending, playback);
            obs[1].useful += 1;
            let got = state.choose_victim(
                now + Duration::from_secs(20),
                &obs,
                capacity,
                pending,
                playback,
            );
            assert_eq!(got, want, "{name}");
        }
    }

    #[test]
    fn test_ghost_pro_chooses_zero_overlap_idle_victim_and_protects_seeders() {
        let mut state = GhostProSwarm::default();
        let now = Instant::now();
        let p1 = 1u64;
        let p2 = 2u64;
        let p3 = 3u64;
        let obs1 = [
            GhostProPeerObservation {
                peer: p1,
                useful: 1000,
                uploaded: 500,
                overlap_pieces: 0,
                contiguous_overlap: 0,
                is_seeder: false,
            },
            GhostProPeerObservation {
                peer: p2,
                useful: 2000,
                uploaded: 1000,
                overlap_pieces: 5,
                contiguous_overlap: 0,
                is_seeder: false,
            },
            GhostProPeerObservation {
                peer: p3,
                useful: 5000,
                uploaded: 2000,
                overlap_pieces: 10,
                contiguous_overlap: 0,
                is_seeder: true,
            },
        ];
        assert!(state.choose_victim(now, &obs1, 3, 5, false).is_none());

        let t10 = now + Duration::from_secs(10);
        let obs2 = [
            GhostProPeerObservation {
                peer: p1,
                useful: 1000,
                uploaded: 500,
                overlap_pieces: 0,
                contiguous_overlap: 0,
                is_seeder: false,
            },
            GhostProPeerObservation {
                peer: p2,
                useful: 2500,
                uploaded: 1200,
                overlap_pieces: 5,
                contiguous_overlap: 0,
                is_seeder: false,
            },
            GhostProPeerObservation {
                peer: p3,
                useful: 5000,
                uploaded: 2000,
                overlap_pieces: 10,
                contiguous_overlap: 0,
                is_seeder: true,
            },
        ];
        assert!(state.choose_victim(t10, &obs2, 3, 5, false).is_none());

        let t20 = now + Duration::from_secs(20);
        let obs3 = [
            GhostProPeerObservation {
                peer: p1,
                useful: 1000,
                uploaded: 500,
                overlap_pieces: 0,
                contiguous_overlap: 0,
                is_seeder: false,
            },
            GhostProPeerObservation {
                peer: p2,
                useful: 3000,
                uploaded: 1500,
                overlap_pieces: 5,
                contiguous_overlap: 0,
                is_seeder: false,
            },
            GhostProPeerObservation {
                peer: p3,
                useful: 5000,
                uploaded: 2000,
                overlap_pieces: 10,
                contiguous_overlap: 0,
                is_seeder: true,
            },
        ];
        assert_eq!(state.choose_victim(t20, &obs3, 3, 5, false), Some(p1));

        let t25 = now + Duration::from_secs(25);
        let p4 = 4u64;
        let obs4 = [
            GhostProPeerObservation {
                peer: p4,
                useful: 500,
                uploaded: 200,
                overlap_pieces: 0,
                contiguous_overlap: 0,
                is_seeder: false,
            },
            GhostProPeerObservation {
                peer: p2,
                useful: 3500,
                uploaded: 1800,
                overlap_pieces: 5,
                contiguous_overlap: 0,
                is_seeder: false,
            },
            GhostProPeerObservation {
                peer: p3,
                useful: 5000,
                uploaded: 2000,
                overlap_pieces: 10,
                contiguous_overlap: 0,
                is_seeder: true,
            },
        ];
        assert!(state.choose_victim(t25, &obs4, 3, 5, false).is_none());
    }
}
