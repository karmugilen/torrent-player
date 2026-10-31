//! Integration smoke tests for the choking public API.

use std::net::Ipv4Addr;
use std::time::Instant;

use wt_core::choking::{
    compute_choke_decisions, generate_allowed_fast_set, upload_allowed, ChokeConfig, ChokeDecision,
    ChokeState, PeerChokeInput,
};

#[test]
fn bep6_vectors() {
    let ip = Ipv4Addr::new(80, 4, 4, 200);
    let ih = [0xaa_u8; 20];
    assert_eq!(
        generate_allowed_fast_set(ip, &ih, 1313, 7),
        vec![1059, 431, 808, 1217, 287, 376, 1188]
    );
    assert_eq!(
        generate_allowed_fast_set(ip, &ih, 1313, 9),
        vec![1059, 431, 808, 1217, 287, 376, 1188, 353, 508]
    );
}

#[test]
fn choke_and_seed_policy() {
    let now = Instant::now();
    let mut state = ChokeState::new(now);
    let cfg = ChokeConfig {
        upload_slots: 2,
        optimistic_slots: 0,
        ..ChokeConfig::default()
    };
    let peers = vec![
        PeerChokeInput {
            id: 1,
            interested: true,
            currently_choking: true,
            download_rate: 10.0,
            upload_rate: 0.0,
            bytes_downloaded: 0,
            bytes_uploaded: 0,
            is_snubbed: false,
        },
        PeerChokeInput {
            id: 2,
            interested: true,
            currently_choking: true,
            download_rate: 100.0,
            upload_rate: 0.0,
            bytes_downloaded: 0,
            bytes_uploaded: 0,
            is_snubbed: false,
        },
    ];
    let d = compute_choke_decisions(&peers, &cfg, &mut state, now);
    assert_eq!(d.get(&2), Some(&ChokeDecision::Unchoke));
    assert_eq!(d.get(&1), Some(&ChokeDecision::Unchoke));

    let mut blocked = cfg.clone();
    blocked.data_upload_disallowed = true;
    assert!(!upload_allowed(&peers[0], &blocked, true));
}
