//! Integration smoke tests for the strategy + choking public API.

use std::net::Ipv4Addr;
use std::time::Instant;

use wt_core::choking::{
    compute_choke_decisions, generate_allowed_fast_set, upload_allowed, ChokeConfig, ChokeDecision,
    ChokeState, PeerChokeInput,
};
use wt_core::strategy::{
    candidate_budget, focus_distance, get_requestable_pieces, piece_order_cmp, PieceRequestOrder,
    PieceRequestOrderItem, PieceRequestOrderKey, PieceRequestOrderState, StrategyLimits,
    TorrentPieceQuery,
};
use wt_types::PiecePriority;

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
fn piece_order_priority_then_index_not_rarest() {
    let mut items = [
        PieceRequestOrderItem {
            key: PieceRequestOrderKey {
                info_hash: [0; 20],
                index: 5,
            },
            state: PieceRequestOrderState {
                availability: 9,
                priority: PiecePriority::Normal,
                partial: false,
            },
        },
        PieceRequestOrderItem {
            key: PieceRequestOrderKey {
                info_hash: [0; 20],
                index: 1,
            },
            state: PieceRequestOrderState {
                availability: 1,
                priority: PiecePriority::Now,
                partial: false,
            },
        },
        PieceRequestOrderItem {
            key: PieceRequestOrderKey {
                info_hash: [0; 20],
                index: 2,
            },
            state: PieceRequestOrderState {
                availability: 100,
                priority: PiecePriority::Now,
                partial: true,
            },
        },
    ];
    items.sort_by(piece_order_cmp);
    // Now before Normal; within Now, lower index first (file order), availability ignored.
    assert_eq!(items[0].key.index, 1);
    assert_eq!(items[1].key.index, 2);
    assert_eq!(items[2].key.index, 5);
}

#[test]
fn order_tree_iteration() {
    let mut pro = PieceRequestOrder::with_capacity(10);
    for i in (0..10).rev() {
        pro.add(
            PieceRequestOrderKey {
                info_hash: [0; 20],
                index: i,
            },
            PieceRequestOrderState {
                availability: i as i32,
                priority: if i < 3 {
                    PiecePriority::Now
                } else {
                    PiecePriority::Normal
                },
                partial: false,
            },
        );
    }
    let idxs: Vec<_> = pro.iter().map(|i| i.key.index).collect();
    assert_eq!(&idxs[..3], &[0, 1, 2]);
    assert_eq!(&idxs[3..], &[3, 4, 5, 6, 7, 8, 9]);
}

struct Q;
impl TorrentPieceQuery for Q {
    fn piece_length(&self, _: &[u8; 20]) -> i64 {
        1024
    }
    fn should_request(&self, _: &[u8; 20], _: u32) -> bool {
        true
    }
    fn count_unverified(&self, _: &[u8; 20], _: u32) -> bool {
        false
    }
}

#[test]
fn get_requestable_respects_capacity() {
    let mut pro = PieceRequestOrder::new();
    for i in 0..5u32 {
        pro.add(
            PieceRequestOrderKey {
                info_hash: [0; 20],
                index: i,
            },
            PieceRequestOrderState {
                availability: 1,
                priority: PiecePriority::Normal,
                partial: false,
            },
        );
    }
    let limits = StrategyLimits {
        capacity: Some(2500),
        max_unverified_bytes: 0,
    };
    let mut n = 0;
    get_requestable_pieces(&pro, &limits, &Q, |_, _, _| {
        n += 1;
        true
    });
    assert_eq!(n, 2);
}

#[test]
fn webtor_helpers() {
    assert_eq!(candidate_budget(64, true), 256);
    assert!(focus_distance(5, 10) > focus_distance(11, 10));
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
