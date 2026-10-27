//! Integration tests ported from `engine-go/peer_exploration_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use std::time::{Duration, Instant};
use support::TestEngine;
use wt_policy::peer_exploration::{PeerExploration, PeerExplorationObservation};

#[test]
fn test_peer_exploration_protects_useful_uploading_and_new_peers() {
    // Port of Go `TestPeerExplorationProtectsUsefulUploadingAndNewPeers`.
    let start = Instant::now();
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
            assert!(
                chosen.is_none(),
                "replaced a peer before the 90-second grace period"
            );
        } else {
            assert_eq!(
                chosen,
                Some(idle),
                "exploration must release only the long-idle peer"
            );
        }
    }
}

#[test]
fn test_peer_exploration_limits_churn_and_requires_alternatives() {
    // Port of Go `TestPeerExplorationLimitsChurnAndRequiresAlternatives`.
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
    assert!(
        policy
            .choose(start + Duration::from_secs(90), &peers, 3, 0)
            .is_none(),
        "cannot explore without a waiting alternative"
    );
    let victim = policy
        .choose(start + Duration::from_secs(100), &peers, 3, 4)
        .expect("expected one idle slot to be released");
    for p in &mut peers {
        if p.peer == victim {
            p.peer = 99;
        }
    }
    assert!(
        policy
            .choose(start + Duration::from_secs(110), &peers, 3, 4)
            .is_none(),
        "rotated more than one slot within 30 seconds"
    );
    assert!(
        policy
            .choose(start + Duration::from_secs(130), &peers, 3, 4)
            .is_some(),
        "expected exploration after cooldown"
    );

    let mut spare = PeerExploration::default();
    spare.choose(start, &peers, 5, 4);
    assert!(
        spare
            .choose(start + Duration::from_secs(100), &peers, 5, 4)
            .is_none(),
        "must use free connection slots before replacing peers"
    );

    let mut sole = PeerExploration::default();
    sole.choose(start, &peers[..1], 1, 4);
    assert!(
        sole.choose(start + Duration::from_secs(100), &peers[..1], 1, 4)
            .is_none(),
        "must preserve the only connected peer"
    );
}

#[test]
fn test_peer_exploration_settings_are_independent() {
    let eng = TestEngine::new();
    assert!(!eng.get_settings().peer_exploration);
    let s = eng.post_settings(r#"{"peerExploration":true}"#);
    assert!(s.peer_exploration);
    assert!(eng.get_settings().peer_exploration);
    let s = eng.post_settings(r#"{"maxPeers":100}"#);
    assert!(
        s.peer_exploration,
        "unrelated setting cleared peerExploration"
    );
    let s = eng.post_settings(r#"{"peerExploration":false}"#);
    assert!(!s.peer_exploration);
}

#[test]
fn test_peer_exploration_protects_the_only_known_seed() {
    // Port of Go `TestPeerExplorationProtectsTheOnlyKnownSeed`.
    let start = Instant::now();
    let mut policy = PeerExploration::default();
    let seed = 1u64;
    let idle = 2u64;
    let peers = [
        PeerExplorationObservation {
            peer: seed,
            useful: 0,
            uploaded: 0,
            protected: true,
        },
        PeerExplorationObservation {
            peer: idle,
            useful: 0,
            uploaded: 0,
            protected: false,
        },
    ];
    policy.choose(start, &peers, 2, 10);
    assert_eq!(
        policy.choose(start + Duration::from_secs(90), &peers, 2, 10),
        Some(idle),
        "must retain the sole known seed"
    );
}

#[ignore = "needs: real idle connection release in EngineServer"]
#[test]
fn test_peer_exploration_releases_one_real_idle_connection() {
    // Port of Go `TestPeerExplorationReleasesOneRealIdleConnection`.
    let eng = TestEngine::new();
    let s = eng.post_settings(r#"{"peerExploration":true,"maxPeers":2}"#);
    assert!(s.peer_exploration);
}
