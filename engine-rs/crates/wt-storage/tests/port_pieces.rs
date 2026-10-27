//! Ports of engine-go/pieces_test.go and pieces_host_test.go, plus engine-rust/src/lib.rs

mod support;

use wt_storage::{
    aggregate, aggregate_piece_buckets, compress_piece_runs, PieceBucket, PieceRange, PieceRun,
    PieceStateRun,
};
use wt_types::PiecePriority;

fn piece_state_run(
    length: usize,
    complete: bool,
    partial: bool,
    checking: bool,
    hashing: bool,
) -> PieceStateRun {
    PieceStateRun {
        length,
        complete,
        partial,
        checking,
        hashing,
        priority: PiecePriority::Normal,
    }
}

#[test]
fn test_compress_piece_runs_counts_only_wanted_partials_while_playing() {
    let mut left = piece_state_run(2, false, true, false, false);
    left.priority = PiecePriority::Normal;
    let mut wanted = piece_state_run(3, false, true, false, false);
    wanted.priority = PiecePriority::Next;

    let got = compress_piece_runs(&[left.clone(), wanted.clone()], 5, true).unwrap();
    assert_eq!(
        got,
        vec![
            PieceRun {
                length: 2,
                flags: 0
            },
            PieceRun {
                length: 3,
                flags: 2
            }
        ]
    );

    let got2 = compress_piece_runs(&[left, wanted], 5, false).unwrap();
    assert_eq!(
        got2,
        vec![PieceRun {
            length: 5,
            flags: 2
        }]
    );
}

#[test]
fn test_compress_piece_runs() {
    let mut high = piece_state_run(4, true, false, false, false);
    high.priority = PiecePriority::Now;
    let mut low = piece_state_run(6, true, false, false, false);
    low.priority = PiecePriority::None;
    // Go sets Ok/QueuedForHash/Marking; Rust compress ignores those fields.

    let states = vec![
        high,
        low,
        piece_state_run(2, true, true, false, false),
        piece_state_run(4, false, true, false, false),
        piece_state_run(1, false, false, true, false),
        piece_state_run(6, false, false, false, true),
        piece_state_run(7, false, false, false, false),
    ];
    let total = 4 + 6 + 2 + 4 + 1 + 6 + 7;
    let got = compress_piece_runs(&states, total, false).unwrap();
    let want = vec![
        PieceRun {
            length: 12,
            flags: 1,
        },
        PieceRun {
            length: 11,
            flags: 2,
        },
        PieceRun {
            length: 7,
            flags: 0,
        },
    ];
    assert_eq!(got, want);

    let only_checking =
        compress_piece_runs(&[piece_state_run(3, false, false, true, false)], 3, false).unwrap();
    assert_eq!(
        only_checking,
        vec![PieceRun {
            length: 3,
            flags: 2
        }]
    );

    let only_hashing =
        compress_piece_runs(&[piece_state_run(3, false, false, false, true)], 3, false).unwrap();
    assert_eq!(
        only_hashing,
        vec![PieceRun {
            length: 3,
            flags: 2
        }]
    );

    let same: Vec<_> = (0..200)
        .map(|_| piece_state_run(1, false, true, false, false))
        .collect();
    let merged = compress_piece_runs(&same, same.len(), false).unwrap();
    assert_eq!(
        merged,
        vec![PieceRun {
            length: 200,
            flags: 2
        }]
    );
    assert!(merged.capacity() <= 64 || merged.len() == 1);

    let empty = compress_piece_runs(&[], 0, false).unwrap();
    assert!(empty.is_empty());

    assert!(
        compress_piece_runs(&[piece_state_run(2, true, false, false, false)], 5, false).is_err()
    );
}

#[test]
fn test_compress_piece_runs_rejects_bad_lengths() {
    let valid = piece_state_run(3, true, false, false, false);
    let cases: Vec<(Vec<PieceStateRun>, usize)> = vec![
        (vec![piece_state_run(0, true, false, false, false)], 3),
        // Rust length is usize; negative Go case maps to zero-length rejection above.
        (
            vec![
                valid.clone(),
                piece_state_run(0, false, false, false, false),
            ],
            3,
        ),
        (
            vec![
                piece_state_run(3, false, false, false, false),
                piece_state_run(3, false, true, false, false),
            ],
            5,
        ),
        (vec![], usize::MAX), // total mismatch / invalid
    ];
    for (i, (states, total)) in cases.into_iter().enumerate() {
        let total = if total == usize::MAX {
            // Go case totals[i]=-1 → treat as invalid by using a sentinel path
            // via empty states with nonzero conceptually invalid; use total 0 with empty is ok,
            // so use total 1 with empty states instead.
            if states.is_empty() {
                1
            } else {
                total
            }
        } else {
            total
        };
        assert!(
            compress_piece_runs(&states, total, false).is_err(),
            "case {i} should reject"
        );
    }
}

#[test]
fn test_compress_piece_runs_uint32_limit() {
    let limit = u32::MAX as u64;
    let overflow = (limit + 1) as usize;
    assert!(compress_piece_runs(
        &[PieceStateRun {
            length: overflow,
            complete: false,
            partial: false,
            checking: false,
            hashing: false,
            priority: PiecePriority::None,
        }],
        overflow.saturating_add(10),
        false
    )
    .is_err());

    assert!(compress_piece_runs(
        &[
            PieceStateRun {
                length: limit as usize,
                complete: false,
                partial: false,
                checking: false,
                hashing: false,
                priority: PiecePriority::None,
            },
            PieceStateRun {
                length: 1,
                complete: false,
                partial: false,
                checking: false,
                hashing: false,
                priority: PiecePriority::None,
            }
        ],
        (limit as usize).saturating_add(10),
        false
    )
    .is_err());

    let exact = PieceStateRun {
        length: limit as usize,
        complete: true,
        partial: false,
        checking: false,
        hashing: false,
        priority: PiecePriority::None,
    };
    let got = compress_piece_runs(&[exact], limit as usize, false).unwrap();
    assert_eq!(
        got,
        vec![PieceRun {
            length: u32::MAX,
            flags: 1
        }]
    );
}

#[test]
fn test_aggregate_piece_buckets() {
    // overlap precedence and geometry
    {
        let runs = [
            PieceRun {
                length: 3,
                flags: 3,
            },
            PieceRun {
                length: 3,
                flags: 2,
            },
            PieceRun {
                length: 4,
                flags: 0,
            },
        ];
        let ranges = [
            PieceRange { start: 4, end: 8 },
            PieceRange { start: 1, end: 5 },
            PieceRange { start: 1, end: 5 },
        ];
        let got = aggregate_piece_buckets(&runs, &ranges, 10, 4).unwrap();
        let want = vec![
            PieceBucket {
                start: 0,
                end: 2,
                total: 3,
                selected: 2,
                verified: 3,
                selected_verified: 2,
                ..Default::default()
            },
            PieceBucket {
                start: 3,
                end: 5,
                total: 3,
                selected: 3,
                receiving: 3,
                selected_receiving: 3,
                ..Default::default()
            },
            PieceBucket {
                start: 6,
                end: 8,
                total: 3,
                selected: 2,
                ..Default::default()
            },
            PieceBucket {
                start: 9,
                end: 9,
                total: 1,
                ..Default::default()
            },
        ];
        assert_eq!(got, want);
    }

    // shared boundary counted once
    {
        let runs = [
            PieceRun {
                length: 4,
                flags: 1,
            },
            PieceRun {
                length: 2,
                flags: 2,
            },
        ];
        let ranges = [
            PieceRange { start: 2, end: 5 },
            PieceRange { start: 0, end: 3 },
            PieceRange { start: 3, end: 3 },
        ];
        let got = aggregate_piece_buckets(&runs, &ranges, 6, 1).unwrap();
        let want = vec![PieceBucket {
            start: 0,
            end: 5,
            total: 6,
            selected: 5,
            verified: 4,
            receiving: 2,
            selected_verified: 4,
            selected_receiving: 1,
        }];
        assert_eq!(got, want);
    }

    // verified wins across a bucket boundary
    {
        let got = aggregate_piece_buckets(
            &[PieceRun {
                length: 5,
                flags: 3,
            }],
            &[],
            5,
            2,
        )
        .unwrap();
        let want = vec![
            PieceBucket {
                start: 0,
                end: 2,
                total: 3,
                verified: 3,
                ..Default::default()
            },
            PieceBucket {
                start: 3,
                end: 4,
                total: 2,
                verified: 2,
                ..Default::default()
            },
        ];
        assert_eq!(got, want);
    }

    // merged runs match split runs
    {
        let ranges = [PieceRange { start: 1, end: 6 }];
        let split = aggregate_piece_buckets(
            &[
                PieceRun {
                    length: 2,
                    flags: 1,
                },
                PieceRun {
                    length: 2,
                    flags: 1,
                },
                PieceRun {
                    length: 3,
                    flags: 2,
                },
            ],
            &ranges,
            7,
            3,
        )
        .unwrap();
        let merged = aggregate_piece_buckets(
            &[
                PieceRun {
                    length: 4,
                    flags: 1,
                },
                PieceRun {
                    length: 3,
                    flags: 2,
                },
            ],
            &ranges,
            7,
            3,
        )
        .unwrap();
        assert_eq!(split, merged);
    }

    // empty
    {
        let got = aggregate_piece_buckets(&[], &[], 0, 256).unwrap();
        assert!(got.is_empty());
    }

    // bounds
    assert!(aggregate_piece_buckets(&[], &[], 0, 0).is_err());
    assert!(aggregate_piece_buckets(&[], &[], 1, 1025).is_err());

    // large total few runs
    {
        const TOTAL: u32 = 200_000;
        const MAX_BUCKETS: u32 = 256;
        let runs = [
            PieceRun {
                length: 1000,
                flags: 1,
            },
            PieceRun {
                length: 500,
                flags: 2,
            },
            PieceRun {
                length: TOTAL - 1500,
                flags: 0,
            },
        ];
        let ranges = [
            PieceRange {
                start: 1100,
                end: 1800,
            },
            PieceRange {
                start: 100,
                end: 1200,
            },
        ];
        let got = aggregate_piece_buckets(&runs, &ranges, TOTAL, MAX_BUCKETS).unwrap();
        let size = TOTAL.div_ceil(MAX_BUCKETS);
        assert_eq!(got.len() as u32, TOTAL.div_ceil(size));
        let mut sum = PieceBucket::default();
        for (i, b) in got.iter().enumerate() {
            let start = i as u32 * size;
            let end = (start + size).min(TOTAL) - 1;
            assert_eq!(b.start, start);
            assert_eq!(b.end, end);
            assert_eq!(b.total, end - start + 1);
            sum.total += b.total;
            sum.selected += b.selected;
            sum.verified += b.verified;
            sum.receiving += b.receiving;
            sum.selected_verified += b.selected_verified;
            sum.selected_receiving += b.selected_receiving;
        }
        assert_eq!(sum.total, TOTAL);
        assert_eq!(sum.verified, 1000);
        assert_eq!(sum.receiving, 500);
        assert_eq!(sum.selected, 1700);
        assert_eq!(sum.selected_verified, 900);
        assert_eq!(sum.selected_receiving, 500);
        assert_eq!(
            got[0],
            PieceBucket {
                start: 0,
                end: 781,
                total: 782,
                selected: 682,
                verified: 782,
                selected_verified: 682,
                ..Default::default()
            }
        );
        assert_eq!(
            got[1],
            PieceBucket {
                start: 782,
                end: 1563,
                total: 782,
                selected: 782,
                verified: 218,
                receiving: 500,
                selected_verified: 218,
                selected_receiving: 500,
            }
        );
        assert_eq!(
            got[2],
            PieceBucket {
                start: 1564,
                end: 2345,
                total: 782,
                selected: 236,
                ..Default::default()
            }
        );
        let last = got.last().unwrap();
        assert_eq!(last.start, 199_410);
        assert_eq!(last.end, 199_999);
        assert_eq!(last.total, 590);
        assert_eq!(last.selected, 0);
        assert_eq!(last.verified, 0);
        assert_eq!(last.receiving, 0);
    }
}

#[test]
fn test_aggregate_piece_buckets_rejects_invalid_input() {
    let valid = [PieceRun {
        length: 4,
        flags: 0,
    }];
    let cases: Vec<(&str, &[PieceRun], &[PieceRange], u32, u32)> = vec![
        (
            "inverted range",
            &valid,
            &[PieceRange { start: 3, end: 1 }],
            4,
            4,
        ),
        (
            "end past total",
            &valid,
            &[PieceRange { start: 0, end: 5 }],
            4,
            4,
        ),
        (
            "total zero with nonzero run",
            &[PieceRun {
                length: 1,
                flags: 1,
            }],
            &[],
            0,
            256,
        ),
        (
            "zero-length run",
            &[PieceRun {
                length: 0,
                flags: 1,
            }],
            &[],
            0,
            256,
        ),
        (
            "run coverage short",
            &[PieceRun {
                length: 3,
                flags: 1,
            }],
            &[],
            4,
            4,
        ),
        (
            "run coverage long",
            &[PieceRun {
                length: 5,
                flags: 1,
            }],
            &[],
            4,
            4,
        ),
        ("maxBuckets low", &valid, &[], 4, 0),
        ("maxBuckets high", &valid, &[], 4, 1025),
    ];
    for (name, runs, ranges, total, max) in cases {
        assert!(
            aggregate_piece_buckets(runs, ranges, total, max).is_err(),
            "accepted invalid input: {name}"
        );
    }

    let buckets = aggregate_piece_buckets(&[], &[], 0, 256).unwrap();
    assert!(buckets.is_empty());
    assert!(aggregate_piece_buckets(&[], &[PieceRange { start: 0, end: 0 }], 0, 1).is_ok());
}

#[test]
fn test_aggregate_piece_buckets_sweep_matches_compressed_runs() {
    let runs = [
        PieceRun {
            length: 1,
            flags: 3,
        },
        PieceRun {
            length: 2,
            flags: 3,
        },
        PieceRun {
            length: 1,
            flags: 2,
        },
        PieceRun {
            length: 2,
            flags: 2,
        },
        PieceRun {
            length: 4,
            flags: 0,
        },
    ];
    let ranges = [
        PieceRange { start: 4, end: 8 },
        PieceRange { start: 1, end: 5 },
        PieceRange { start: 1, end: 5 },
    ];
    let buckets = aggregate_piece_buckets(&runs, &ranges, 10, 4).unwrap();
    assert_eq!(buckets.len(), 4);
    let mut selected = 0;
    let mut selected_verified = 0;
    let mut selected_receiving = 0;
    let mut covered = 0;
    for b in &buckets {
        assert!(b.end >= b.start);
        assert_eq!(b.total, b.end - b.start + 1);
        covered += b.total;
        selected += b.selected;
        selected_verified += b.selected_verified;
        selected_receiving += b.selected_receiving;
    }
    assert_eq!(covered, 10);
    assert_eq!(buckets[0].start, 0);
    assert_eq!(buckets[3].start, 9);
    assert_eq!(buckets[3].end, 9);
    assert_eq!(buckets[3].total, 1);
    assert_eq!(selected, 7);
    assert_eq!(selected_verified, 2);
    assert_eq!(selected_receiving, 3);
    assert_eq!(buckets[0].verified, 3);
    assert_eq!(buckets[0].receiving, 0);
}

// --- engine-rust/src/lib.rs ports ---

#[test]
fn overlapping_files_and_complete_precedence() {
    let runs = [
        PieceRun {
            length: 3,
            flags: 3,
        },
        PieceRun {
            length: 3,
            flags: 2,
        },
        PieceRun {
            length: 4,
            flags: 0,
        },
    ];
    let mut ranges = [
        PieceRange { start: 4, end: 8 },
        PieceRange { start: 1, end: 5 },
        PieceRange { start: 1, end: 5 },
    ];
    let mut out = [PieceBucket::default(); 4];
    assert_eq!(aggregate(&runs, &mut ranges, 10, 4, &mut out), Some(4));
    assert_eq!(out.iter().map(|b| b.selected).sum::<u32>(), 7);
    assert_eq!(out.iter().map(|b| b.selected_verified).sum::<u32>(), 2);
    assert_eq!(out.iter().map(|b| b.selected_receiving).sum::<u32>(), 3);
    assert_eq!((out[3].start, out[3].end, out[3].total), (9, 9, 1));
    assert_eq!(out[0].receiving, 0);
}

#[test]
fn invalid_input_does_not_write_output() {
    let mut out = [PieceBucket {
        total: 99,
        ..Default::default()
    }; 1];
    assert_eq!(
        aggregate(
            &[PieceRun {
                length: 10,
                flags: 0
            }],
            &mut [],
            9,
            1,
            &mut out
        ),
        None
    );
    assert_eq!(
        aggregate(
            &[PieceRun {
                length: 10,
                flags: 0
            }],
            &mut [],
            10,
            2,
            &mut out
        ),
        None
    );
    assert_eq!(out[0].total, 99);
}

#[test]
fn empty_and_maximum_piece_count() {
    assert_eq!(aggregate(&[], &mut [], 0, 256, &mut []), Some(0));
    let mut out = [PieceBucket::default(); 1024];
    let n = aggregate(
        &[PieceRun {
            length: u32::MAX,
            flags: 1,
        }],
        &mut [PieceRange {
            start: 0,
            end: u32::MAX,
        }],
        u32::MAX,
        1024,
        &mut out,
    )
    .unwrap();
    assert_eq!(
        out[..n]
            .iter()
            .map(|b| u64::from(b.selected_verified))
            .sum::<u64>(),
        u64::from(u32::MAX)
    );
}

#[test]
fn bucket_geometry_for_selection_window() {
    let runs = [PieceRun {
        length: 10,
        flags: 0,
    }];
    let mut ranges = [PieceRange { start: 2, end: 7 }];
    let mut out = [PieceBucket::default(); 4];
    assert_eq!(aggregate(&runs, &mut ranges, 10, 4, &mut out), Some(4));
    assert_eq!(
        out,
        [
            PieceBucket {
                start: 0,
                end: 2,
                total: 3,
                selected: 1,
                ..Default::default()
            },
            PieceBucket {
                start: 3,
                end: 5,
                total: 3,
                selected: 3,
                ..Default::default()
            },
            PieceBucket {
                start: 6,
                end: 8,
                total: 3,
                selected: 1,
                ..Default::default()
            },
            PieceBucket {
                start: 9,
                end: 9,
                total: 1,
                ..Default::default()
            },
        ]
    );
}

#[test]
fn adjacent_duplicate_and_unsorted_ranges_count_once() {
    let runs = [PieceRun {
        length: 4,
        flags: 1,
    }];
    let mut adjacent = [
        PieceRange { start: 0, end: 2 },
        PieceRange { start: 2, end: 4 },
    ];
    let mut out = [PieceBucket::default(); 4];
    assert_eq!(aggregate(&runs, &mut adjacent, 4, 4, &mut out), Some(4));
    assert_eq!(out.map(|b| b.selected), [1, 1, 1, 1]);
    assert_eq!(out[2].selected_verified, 1);

    let mut duplicated = [
        PieceRange { start: 0, end: 4 },
        PieceRange { start: 0, end: 4 },
        PieceRange { start: 1, end: 3 },
    ];
    let mut out_dup = [PieceBucket::default(); 4];
    aggregate(&runs, &mut duplicated, 4, 4, &mut out_dup).unwrap();
    assert_eq!(out_dup, out);

    let mut unsorted = [
        PieceRange { start: 2, end: 4 },
        PieceRange { start: 0, end: 2 },
    ];
    let mut sorted = [
        PieceRange { start: 0, end: 2 },
        PieceRange { start: 2, end: 4 },
    ];
    let mut out_unsorted = [PieceBucket::default(); 4];
    let mut out_sorted = [PieceBucket::default(); 4];
    aggregate(&runs, &mut unsorted, 4, 4, &mut out_unsorted).unwrap();
    aggregate(&runs, &mut sorted, 4, 4, &mut out_sorted).unwrap();
    assert_eq!(out_unsorted, out_sorted);
}

#[test]
fn adjacent_equal_flag_runs_match_merged_run() {
    let split = [
        PieceRun {
            length: 2,
            flags: 1,
        },
        PieceRun {
            length: 2,
            flags: 1,
        },
        PieceRun {
            length: 4,
            flags: 1,
        },
    ];
    let merged = [PieceRun {
        length: 8,
        flags: 1,
    }];
    let changed = [
        PieceRun {
            length: 3,
            flags: 1,
        },
        PieceRun {
            length: 5,
            flags: 2,
        },
    ];
    let mut out_split = [PieceBucket::default(); 4];
    let mut out_merged = [PieceBucket::default(); 4];
    let mut out_changed = [PieceBucket::default(); 4];
    assert_eq!(
        aggregate(
            &split,
            &mut [PieceRange { start: 1, end: 7 }],
            8,
            4,
            &mut out_split
        ),
        Some(4)
    );
    assert_eq!(
        aggregate(
            &merged,
            &mut [PieceRange { start: 1, end: 7 }],
            8,
            4,
            &mut out_merged
        ),
        Some(4)
    );
    assert_eq!(out_split, out_merged);

    assert_eq!(
        aggregate(
            &changed,
            &mut [PieceRange { start: 1, end: 7 }],
            8,
            4,
            &mut out_changed
        ),
        Some(4)
    );
    assert_ne!(out_changed, out_merged);
    assert_eq!(out_changed[1].verified, 1);
    assert_eq!(out_changed[1].receiving, 1);
    assert_eq!(out_changed[1].selected_verified, 1);
    assert_eq!(out_changed[1].selected_receiving, 1);
    assert_eq!(out_changed.iter().map(|b| b.verified).sum::<u32>(), 3);
    assert_eq!(out_changed.iter().map(|b| b.receiving).sum::<u32>(), 5);
}

#[test]
fn bad_inputs_do_not_write_output() {
    let sentinel = PieceBucket {
        start: 11,
        end: 22,
        total: 33,
        selected: 44,
        verified: 55,
        receiving: 66,
        selected_verified: 77,
        selected_receiving: 88,
    };
    let mut out = [sentinel; 4];
    let runs = [PieceRun {
        length: 10,
        flags: 1,
    }];
    assert_eq!(aggregate(&runs, &mut [], 10, 4, &mut out[..3]), None);
    assert_eq!(aggregate(&runs, &mut [], 10, 0, &mut out), None);
    assert_eq!(aggregate(&runs, &mut [], 10, 1025, &mut out), None);
    assert_eq!(
        aggregate(
            &[
                PieceRun {
                    length: 5,
                    flags: 1
                },
                PieceRun {
                    length: 0,
                    flags: 2
                },
                PieceRun {
                    length: 5,
                    flags: 1
                }
            ],
            &mut [],
            10,
            1,
            &mut out,
        ),
        None
    );
    assert_eq!(
        aggregate(
            &[PieceRun {
                length: 0,
                flags: 0
            }],
            &mut [],
            0,
            1,
            &mut out
        ),
        None
    );
    assert_eq!(
        aggregate(
            &runs,
            &mut [PieceRange { start: 6, end: 2 }],
            10,
            1,
            &mut out
        ),
        None
    );
    assert_eq!(out, [sentinel; 4]);
}

fn oracle(
    runs: &[PieceRun],
    ranges: &[PieceRange],
    total: u32,
    max_buckets: u32,
) -> Vec<PieceBucket> {
    assert!(total <= 64);
    if total == 0 {
        return Vec::new();
    }
    let mut flags = vec![0u32; total as usize];
    let mut at = 0usize;
    for run in runs {
        for _ in 0..run.length {
            flags[at] = run.flags;
            at += 1;
        }
    }
    assert_eq!(at, total as usize);
    let mut selected = vec![false; total as usize];
    for range in ranges {
        for piece in range.start..range.end {
            selected[piece as usize] = true;
        }
    }
    let size = total.div_ceil(max_buckets);
    let count = total.div_ceil(size) as usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let start = (i as u32) * size;
        let end = start.saturating_add(size).min(total);
        let mut bucket = PieceBucket {
            start,
            end: end - 1,
            total: end - start,
            ..Default::default()
        };
        for piece in start..end {
            let is_selected = selected[piece as usize];
            let flag = flags[piece as usize];
            if is_selected {
                bucket.selected += 1;
            }
            if flag & 1 != 0 {
                bucket.verified += 1;
                if is_selected {
                    bucket.selected_verified += 1;
                }
            } else if flag & 2 != 0 {
                bucket.receiving += 1;
                if is_selected {
                    bucket.selected_receiving += 1;
                }
            }
        }
        out.push(bucket);
    }
    out
}

fn run_patterns(total: u32) -> Vec<Vec<PieceRun>> {
    if total == 0 {
        return vec![Vec::new()];
    }
    let mut out = Vec::new();
    for flags in [0, 1, 2, 3] {
        out.push(vec![PieceRun {
            length: total,
            flags,
        }]);
    }
    out.push(
        (0..total)
            .map(|i| PieceRun {
                length: 1,
                flags: i % 4,
            })
            .collect(),
    );
    out.push(
        (0..total)
            .map(|i| PieceRun {
                length: 1,
                flags: if (i / 2) % 2 == 0 { 1 } else { 2 },
            })
            .collect(),
    );
    if total >= 2 {
        let mid = total / 2;
        out.push(vec![
            PieceRun {
                length: mid,
                flags: 3,
            },
            PieceRun {
                length: total - mid,
                flags: 2,
            },
        ]);
    }
    out
}

fn range_patterns(total: u32) -> Vec<Vec<PieceRange>> {
    if total == 0 {
        return vec![Vec::new(), vec![PieceRange { start: 0, end: 0 }]];
    }
    let mid = total / 2;
    let mut out = vec![
        Vec::new(),
        vec![PieceRange { start: 0, end: 0 }],
        vec![PieceRange {
            start: 0,
            end: total,
        }],
        vec![
            PieceRange {
                start: 0,
                end: total,
            },
            PieceRange {
                start: 0,
                end: total,
            },
        ],
        vec![
            PieceRange { start: 0, end: mid },
            PieceRange {
                start: mid,
                end: total,
            },
        ],
        vec![
            PieceRange {
                start: mid,
                end: total,
            },
            PieceRange {
                start: 0,
                end: mid.max(1),
            },
        ],
    ];
    if total >= 4 {
        out.push(vec![
            PieceRange {
                start: total - 1,
                end: total,
            },
            PieceRange {
                start: 1,
                end: total - 1,
            },
            PieceRange { start: 2, end: 2 },
            PieceRange { start: 0, end: 3 },
        ]);
    }
    if total >= 7 {
        out.push(vec![PieceRange { start: 2, end: 7 }]);
    }
    out
}

#[test]
fn brute_force_oracle_matches_expanded_counts() {
    for total in 0..=64u32 {
        let mut buckets = vec![1u32, 2, 3, 4, 7, 8, 15, 16, 64, 1024];
        if (1..=1024).contains(&total) {
            buckets.push(total);
        }
        buckets.sort_unstable();
        buckets.dedup();
        for max_buckets in buckets {
            for (ri, runs) in run_patterns(total).iter().enumerate() {
                for (rgi, ranges_in) in range_patterns(total).iter().enumerate() {
                    if total == 0 {
                        let mut ranges = ranges_in.clone();
                        let mut out = [PieceBucket {
                            total: 99,
                            ..Default::default()
                        }];
                        assert_eq!(
                            aggregate(runs, &mut ranges, 0, max_buckets, &mut out),
                            Some(0),
                            "total=0 max_buckets={max_buckets} run_pat={ri} range_pat={rgi}"
                        );
                        assert_eq!(out[0].total, 99);
                        continue;
                    }
                    let expected = oracle(runs, ranges_in, total, max_buckets);
                    let mut ranges = ranges_in.clone();
                    let mut out = vec![PieceBucket::default(); expected.len()];
                    let n = aggregate(runs, &mut ranges, total, max_buckets, &mut out)
                        .unwrap_or_else(|| {
                            panic!(
                                "total={total} max_buckets={max_buckets} run_pat={ri} range_pat={rgi}"
                            )
                        });
                    assert_eq!(n, expected.len(), "total={total} max_buckets={max_buckets}");
                    assert_eq!(
                        out, expected,
                        "total={total} max_buckets={max_buckets} run_pat={ri} range_pat={rgi}"
                    );
                }
            }
        }
    }
}
