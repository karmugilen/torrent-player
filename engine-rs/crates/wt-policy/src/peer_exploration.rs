//! Slow peer-slot exploration (`engine-go/peer_exploration.go`).
//!
//! The engine samples connected peers each tick and passes snapshots into
//! [`PeerExploration::choose`]. A returned [`PeerKey`] is a candidate to close
//! so discovery can fill the slot; the engine rechecks live stats before close.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Engine-assigned opaque peer identity (maps to a live connection).
pub type PeerKey = u64;

#[derive(Clone, Debug)]
struct Progress {
    useful: i64,
    uploaded: i64,
    idle_since: Instant,
}

/// Point-in-time peer sample for exploration.
#[derive(Clone, Debug)]
pub struct PeerExplorationObservation {
    pub peer: PeerKey,
    pub useful: i64,
    pub uploaded: i64,
    /// Sole known seeder (or similarly protected).
    pub protected: bool,
}

/// Keep productive connections; release at most one long-idle slot.
#[derive(Clone, Debug, Default)]
pub struct PeerExploration {
    previous: HashMap<PeerKey, Progress>,
    next_sample: Option<Instant>,
    next_rotation: Option<Instant>,
}

impl PeerExploration {
    pub fn choose(
        &mut self,
        now: Instant,
        peers: &[PeerExplorationObservation],
        capacity: usize,
        pending: usize,
    ) -> Option<PeerKey> {
        if self.next_sample.map(|t| now < t).unwrap_or(false) {
            return None;
        }
        self.next_sample = Some(now + Duration::from_secs(10));
        let mut next = HashMap::with_capacity(peers.len());
        let mut victim: Option<PeerKey> = None;
        let mut oldest: Option<Instant> = None;
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
            if !obs.protected
                && now.duration_since(idle_since) >= Duration::from_secs(90)
                && (victim.is_none() || oldest.map(|o| idle_since < o).unwrap_or(true))
            {
                victim = Some(obs.peer);
                oldest = Some(idle_since);
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
        if let Some(v) = victim {
            self.next_rotation = Some(now + Duration::from_secs(30));
            self.previous.remove(&v);
        }
        victim
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(now: Instant) -> Instant {
        // Instant has no unix epoch; use a stable reference by measuring from now.
        let _ = now;
        Instant::now()
    }

    #[test]
    fn test_peer_exploration_protects_useful_uploading_and_new_peers() {
        let start = base(Instant::now());
        let mut policy = PeerExploration::default();
        let idle = 1u64;
        let downloading = 2u64;
        let uploading = 3u64;
        for seconds in (0..=90).step_by(10) {
            let peers = [
                PeerExplorationObservation {
                    peer: idle,
                    useful: 0,
                    uploaded: 0,
                    protected: false,
                },
                PeerExplorationObservation {
                    peer: downloading,
                    useful: seconds as i64 * 1024,
                    uploaded: 0,
                    protected: false,
                },
                PeerExplorationObservation {
                    peer: uploading,
                    useful: 0,
                    uploaded: seconds as i64 * 1024,
                    protected: false,
                },
            ];
            let chosen = policy.choose(start + Duration::from_secs(seconds), &peers, 3, 10);
            if seconds < 90 {
                assert!(chosen.is_none(), "replaced before 90s grace");
            } else {
                assert_eq!(chosen, Some(idle));
            }
        }
    }

    #[test]
    fn test_peer_exploration_limits_churn_and_requires_alternatives() {
        let start = Instant::now();
        let a = 1u64;
        let b = 2u64;
        let c = 3u64;
        let mut peers = vec![
            PeerExplorationObservation {
                peer: a,
                useful: 0,
                uploaded: 0,
                protected: false,
            },
            PeerExplorationObservation {
                peer: b,
                useful: 0,
                uploaded: 0,
                protected: false,
            },
            PeerExplorationObservation {
                peer: c,
                useful: 0,
                uploaded: 0,
                protected: false,
            },
        ];
        let mut policy = PeerExploration::default();
        policy.choose(start, &peers, 3, 4);
        assert!(policy
            .choose(start + Duration::from_secs(90), &peers, 3, 0)
            .is_none());
        let victim = policy
            .choose(start + Duration::from_secs(100), &peers, 3, 4)
            .expect("one idle slot");
        for p in &mut peers {
            if p.peer == victim {
                p.peer = 99;
            }
        }
        assert!(policy
            .choose(start + Duration::from_secs(110), &peers, 3, 4)
            .is_none());
        assert!(policy
            .choose(start + Duration::from_secs(130), &peers, 3, 4)
            .is_some());

        let mut spare = PeerExploration::default();
        spare.choose(start, &peers, 5, 4);
        assert!(spare
            .choose(start + Duration::from_secs(100), &peers, 5, 4)
            .is_none());

        let mut sole = PeerExploration::default();
        sole.choose(start, &peers[..1], 1, 4);
        assert!(sole
            .choose(start + Duration::from_secs(100), &peers[..1], 1, 4)
            .is_none());
    }

    #[test]
    fn test_peer_exploration_protects_the_only_known_seed() {
        let start = Instant::now();
        let mut policy = PeerExploration::default();
        let peers = [
            PeerExplorationObservation {
                peer: 1,
                useful: 0,
                uploaded: 0,
                protected: true,
            },
            PeerExplorationObservation {
                peer: 2,
                useful: 0,
                uploaded: 0,
                protected: false,
            },
        ];
        policy.choose(start, &peers, 2, 10);
        assert_eq!(
            policy.choose(start + Duration::from_secs(90), &peers, 2, 10),
            Some(2)
        );
    }
}
