//! Upload-slot choking with optimistic unchoke rotation.
//!
//! Classic BitTorrent rechoke: prefer peers by download rate while leeching,
//! by upload rate while seeding; keep `upload_slots` unchoked plus one
//! rotating optimistic unchoke. Respects seed-policy / no-upload flags.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Default number of regular upload slots (BEP / reference clients use 4).
pub const DEFAULT_UPLOAD_SLOTS: usize = 4;
/// Optimistic unchoke rotation interval.
pub const OPTIMISTIC_UNCHOKE_INTERVAL: Duration = Duration::from_secs(30);
/// Regular rechoke interval.
pub const REGULAR_UNCHOKE_INTERVAL: Duration = Duration::from_secs(10);
/// anacrolix tit-for-tat headroom (100 KiB).
pub const TIT_FOR_TAT_HEADROOM: i64 = 8 << 20;

pub type PeerId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChokeDecision {
    Choke,
    Unchoke,
}

/// Per-peer inputs for a rechoke round.
#[derive(Clone, Debug)]
pub struct PeerChokeInput {
    pub id: PeerId,
    /// Peer is interested in our pieces.
    pub interested: bool,
    /// We are currently choking them.
    pub currently_choking: bool,
    /// Download rate from this peer (bytes/s) — used while leeching.
    pub download_rate: f64,
    /// Upload rate to this peer (bytes/s) — used while seeding.
    pub upload_rate: f64,
    pub bytes_downloaded: i64,
    pub bytes_uploaded: i64,
    /// Snubbed peers are never optimistically unchoked.
    pub is_snubbed: bool,
}

#[derive(Clone, Debug)]
pub struct ChokeConfig {
    pub upload_slots: usize,
    /// Extra optimistic slots (normally 1). Included in total unchokes.
    pub optimistic_slots: usize,
    pub optimistic_interval: Duration,
    pub regular_interval: Duration,
    pub no_upload: bool,
    /// From seed policy (`DisallowDataUpload`).
    pub data_upload_disallowed: bool,
    pub seeding: bool,
    pub tit_for_tat_headroom: i64,
}

impl Default for ChokeConfig {
    fn default() -> Self {
        Self {
            upload_slots: DEFAULT_UPLOAD_SLOTS,
            optimistic_slots: 1,
            optimistic_interval: OPTIMISTIC_UNCHOKE_INTERVAL,
            regular_interval: REGULAR_UNCHOKE_INTERVAL,
            no_upload: false,
            data_upload_disallowed: false,
            seeding: false,
            tit_for_tat_headroom: TIT_FOR_TAT_HEADROOM,
        }
    }
}

/// Mutable rechoke state (inject time via `now` arguments).
#[derive(Clone, Debug)]
pub struct ChokeState {
    pub last_regular: Instant,
    pub last_optimistic: Instant,
    pub optimistic_peers: Vec<PeerId>,
    /// Round-robin cursor into interested choked candidates.
    optimistic_cursor: usize,
}

impl ChokeState {
    pub fn new(now: Instant) -> Self {
        Self {
            last_regular: now,
            last_optimistic: now,
            optimistic_peers: Vec::new(),
            optimistic_cursor: 0,
        }
    }
}

/// Compute choke/unchoke for every peer in `peers`.
///
/// Always returns a decision for each input peer. Rotates optimistic unchokes
/// when `optimistic_interval` has elapsed; recomputes regular slots every call
/// (caller should invoke on the regular timer, typically 10s).
pub fn compute_choke_decisions(
    peers: &[PeerChokeInput],
    config: &ChokeConfig,
    state: &mut ChokeState,
    now: Instant,
) -> HashMap<PeerId, ChokeDecision> {
    let mut out: HashMap<PeerId, ChokeDecision> =
        peers.iter().map(|p| (p.id, ChokeDecision::Choke)).collect();

    if config.no_upload || config.data_upload_disallowed || config.upload_slots == 0 {
        state.optimistic_peers.clear();
        return out;
    }

    let regular_slots = config.upload_slots.saturating_sub(config.optimistic_slots);
    // Interested peers ranked for regular slots.
    let mut candidates: Vec<&PeerChokeInput> = peers.iter().filter(|p| p.interested).collect();

    if config.seeding {
        // Seeds: prefer peers we upload to fastest (they drain us most usefully).
        candidates.sort_by(|a, b| {
            b.upload_rate
                .partial_cmp(&a.upload_rate)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        });
    } else {
        // Leeches: prefer peers that upload to us fastest (tit-for-tat).
        candidates.sort_by(|a, b| {
            b.download_rate
                .partial_cmp(&a.download_rate)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        });
    }

    let mut unchoked: Vec<PeerId> = Vec::with_capacity(config.upload_slots);
    for p in candidates.iter().take(regular_slots) {
        out.insert(p.id, ChokeDecision::Unchoke);
        unchoked.push(p.id);
    }

    // Optimistic rotation.
    if config.optimistic_slots > 0 {
        let rotate = now.duration_since(state.last_optimistic) >= config.optimistic_interval
            || state.optimistic_peers.is_empty();
        if rotate {
            state.last_optimistic = now;
            state.optimistic_peers.clear();
            let optimistic_candidates: Vec<PeerId> = peers
                .iter()
                .filter(|p| {
                    p.interested
                        && !p.is_snubbed
                        && !unchoked.contains(&p.id)
                        && out.get(&p.id) == Some(&ChokeDecision::Choke)
                })
                .map(|p| p.id)
                .collect();
            if !optimistic_candidates.is_empty() {
                let mut attempts = 0;
                while state.optimistic_peers.len() < config.optimistic_slots
                    && attempts < optimistic_candidates.len()
                {
                    let idx = state.optimistic_cursor % optimistic_candidates.len();
                    state.optimistic_cursor = state.optimistic_cursor.wrapping_add(1);
                    let id = optimistic_candidates[idx];
                    if !state.optimistic_peers.contains(&id) {
                        state.optimistic_peers.push(id);
                    }
                    attempts += 1;
                }
            }
        }
    } else {
        state.optimistic_peers.clear();
    }

    for &id in &state.optimistic_peers {
        // Only keep optimistic if still interested and not already regular.
        if let Some(p) = peers.iter().find(|p| p.id == id) {
            if p.interested {
                out.insert(id, ChokeDecision::Unchoke);
            }
        }
    }

    state.last_regular = now;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(id: u64, interested: bool, down: f64, up: f64) -> PeerChokeInput {
        PeerChokeInput {
            id,
            interested,
            currently_choking: true,
            download_rate: down,
            upload_rate: up,
            bytes_downloaded: 0,
            bytes_uploaded: 0,
            is_snubbed: false,
        }
    }

    #[test]
    fn leech_unchokes_fastest_downloaders() {
        let now = Instant::now();
        let mut state = ChokeState::new(now);
        let cfg = ChokeConfig {
            upload_slots: 3,
            optimistic_slots: 1,
            optimistic_interval: Duration::from_secs(30),
            ..ChokeConfig::default()
        };
        let peers = vec![
            peer(1, true, 10.0, 0.0),
            peer(2, true, 50.0, 0.0),
            peer(3, true, 30.0, 0.0),
            peer(4, true, 5.0, 0.0),
            peer(5, false, 100.0, 0.0), // not interested
        ];
        let d = compute_choke_decisions(&peers, &cfg, &mut state, now);
        // regular_slots = 2: peers 2 and 3; plus 1 optimistic
        assert_eq!(d.get(&2), Some(&ChokeDecision::Unchoke));
        assert_eq!(d.get(&3), Some(&ChokeDecision::Unchoke));
        assert_eq!(d.get(&5), Some(&ChokeDecision::Choke));
        let unchoked: Vec<_> = d
            .iter()
            .filter(|(_, v)| **v == ChokeDecision::Unchoke)
            .map(|(k, _)| *k)
            .collect();
        assert_eq!(unchoked.len(), 3);
    }

    #[test]
    fn seeding_orders_by_upload_rate() {
        let now = Instant::now();
        let mut state = ChokeState::new(now);
        let cfg = ChokeConfig {
            upload_slots: 2,
            optimistic_slots: 0,
            seeding: true,
            ..ChokeConfig::default()
        };
        let peers = vec![
            peer(1, true, 0.0, 10.0),
            peer(2, true, 0.0, 40.0),
            peer(3, true, 0.0, 20.0),
        ];
        let d = compute_choke_decisions(&peers, &cfg, &mut state, now);
        assert_eq!(d.get(&2), Some(&ChokeDecision::Unchoke));
        assert_eq!(d.get(&3), Some(&ChokeDecision::Unchoke));
        assert_eq!(d.get(&1), Some(&ChokeDecision::Choke));
    }

    #[test]
    fn seed_policy_chokes_everyone() {
        let now = Instant::now();
        let mut state = ChokeState::new(now);
        let cfg = ChokeConfig {
            data_upload_disallowed: true,
            ..ChokeConfig::default()
        };
        let peers = vec![peer(1, true, 100.0, 0.0)];
        let d = compute_choke_decisions(&peers, &cfg, &mut state, now);
        assert_eq!(d.get(&1), Some(&ChokeDecision::Choke));
    }

    #[test]
    fn optimistic_rotates_after_interval() {
        let now = Instant::now();
        let mut state = ChokeState::new(now);
        let cfg = ChokeConfig {
            upload_slots: 2,
            optimistic_slots: 1,
            optimistic_interval: Duration::from_secs(30),
            ..ChokeConfig::default()
        };
        // One regular slot (2-1=1): always peer 1 (fastest). Optimistic among 2,3.
        let peers = vec![
            peer(1, true, 100.0, 0.0),
            peer(2, true, 1.0, 0.0),
            peer(3, true, 1.0, 0.0),
        ];
        let d1 = compute_choke_decisions(&peers, &cfg, &mut state, now);
        assert_eq!(d1.get(&1), Some(&ChokeDecision::Unchoke));
        let opt1 = state.optimistic_peers.clone();
        assert_eq!(opt1.len(), 1);

        // Within interval: same optimistic.
        let d2 = compute_choke_decisions(&peers, &cfg, &mut state, now + Duration::from_secs(10));
        assert_eq!(state.optimistic_peers, opt1);
        assert_eq!(d2.get(&opt1[0]), Some(&ChokeDecision::Unchoke));

        // After interval: cursor advances.
        let _d3 = compute_choke_decisions(&peers, &cfg, &mut state, now + Duration::from_secs(31));
        assert_eq!(state.optimistic_peers.len(), 1);
        // With two candidates, rotation should eventually differ; after one step
        // cursor moved so next pick differs when starting from same list order.
        assert_ne!(state.optimistic_peers, opt1);
    }
}
