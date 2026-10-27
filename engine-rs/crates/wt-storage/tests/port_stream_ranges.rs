//! Ports of engine-go/stream_ranges_test.go

mod support;

use rand::{Rng, SeedableRng};
use wt_storage::{merge_received_range, ByteRange};

fn reference_merge(ranges: &[ByteRange]) -> Vec<ByteRange> {
    let mut ranges = ranges.to_vec();
    ranges.sort_by_key(|r| r.start);
    let mut merged: Vec<ByteRange> = Vec::new();
    for r in ranges {
        if let Some(last) = merged.last_mut() {
            if r.start <= last.end {
                last.end = last.end.max(r.end);
                continue;
            }
        }
        merged.push(r);
    }
    merged
}

/// The incremental merge must match sorting every range and merging them.
#[test]
fn test_received_ranges_merge_like_a_sorted_merge() {
    let mut rng = rand::rngs::StdRng::seed_from_u64(1);
    for _round in 0..2000 {
        let mut got: Vec<ByteRange> = Vec::new();
        let mut all: Vec<ByteRange> = Vec::new();
        let k_max = 1 + rng.gen_range(0..40);
        for _k in 0..k_max {
            let mut start = (rng.gen_range(0..64) as i64) * 1024;
            if rng.gen_range(0..3) == 0 && !all.is_empty() {
                start = got.last().unwrap().end;
            }
            let r = ByteRange {
                start,
                end: start + (1 + rng.gen_range(0..4) as i64) * 1024,
            };
            all.push(r);
            merge_received_range(&mut got, r);
            let want = reference_merge(&all);
            assert_eq!(got, want, "after {all:?}: got {got:?}, want {want:?}");
        }
    }
}
