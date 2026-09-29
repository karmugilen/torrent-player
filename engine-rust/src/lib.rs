//! Allocation-free aggregation of compressed torrent piece states.
//! Buffers belong to the caller and are never retained across the C ABI.

mod hash;

use std::slice;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PieceRun {
    pub length: u32,
    pub flags: u32, // 1 = verified, 2 = receiving (verified takes precedence)
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PieceRange {
    pub start: u32,
    pub end: u32, // exclusive
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PieceBucket {
    pub start: u32,
    pub end: u32, // inclusive, matching the existing control API
    pub total: u32,
    pub selected: u32,
    pub verified: u32,
    pub receiving: u32,
    pub selected_verified: u32,
    pub selected_receiving: u32,
}

/// Exclusive end of the bucket beginning at `start`.
/// Checked add so a last bucket that reaches `u32::MAX` does not wrap.
#[inline]
fn exclusive_end(start: u32, size: u32, total: u32) -> u32 {
    start.checked_add(size).map_or(total, |end| end.min(total))
}

fn aggregate(
    runs: &[PieceRun],
    ranges: &mut [PieceRange],
    total: u32,
    max_buckets: u32,
    output: &mut [PieceBucket],
) -> Option<usize> {
    if !(1..=1024).contains(&max_buckets) {
        return None;
    }
    // One pass: reject a zero-length run and require the lengths to cover `total`.
    let mut covered = 0u64;
    for run in runs {
        if run.length == 0 {
            return None;
        }
        covered += u64::from(run.length);
    }
    if covered != u64::from(total) || ranges.iter().any(|r| r.start > r.end || r.end > total) {
        return None;
    }
    if total == 0 {
        return Some(0);
    }
    let size = total.div_ceil(max_buckets);
    let count = total.div_ceil(size) as usize;
    if output.len() < count {
        return None;
    }

    // Sort/merge in the caller's scratch buffer; shared file boundary pieces
    // and duplicate selections must only be counted once.
    ranges.sort_unstable_by_key(|r| (r.start, r.end));
    let mut merged = 0;
    for i in 0..ranges.len() {
        let range = ranges[i];
        if range.start == range.end {
            continue;
        }
        if merged > 0 && range.start <= ranges[merged - 1].end {
            ranges[merged - 1].end = ranges[merged - 1].end.max(range.end);
        } else {
            ranges[merged] = range;
            merged += 1;
        }
    }
    let ranges = &ranges[..merged];

    let mut bucket_start = 0u32;
    for bucket in &mut output[..count] {
        let end = exclusive_end(bucket_start, size, total);
        *bucket = PieceBucket {
            start: bucket_start,
            end: end - 1,
            total: end - bucket_start,
            ..Default::default()
        };
        bucket_start = end;
    }

    let mut cursor = 0u32;
    let mut bucket_index = 0usize;
    let mut bucket_limit = exclusive_end(0, size, total);
    let mut selected_index = 0usize;
    let mut run_index = 0usize;
    while run_index < runs.len() {
        // Runs are read-only; fold adjacent spans with the same heat class.
        // Flags 1 and 3 are both verified (verified wins over receiving).
        let flags = runs[run_index].flags;
        let verified = flags & 1 != 0;
        let receiving = !verified && flags & 2 != 0;
        let mut run_end = cursor;
        while run_index < runs.len() {
            let next = runs[run_index].flags;
            let next_verified = next & 1 != 0;
            let next_receiving = !next_verified && next & 2 != 0;
            if next_verified != verified || next_receiving != receiving {
                break;
            }
            run_end += runs[run_index].length;
            run_index += 1;
        }

        while cursor < run_end {
            if cursor == bucket_limit {
                bucket_index += 1;
                bucket_limit = exclusive_end(cursor, size, total);
            }
            while selected_index < ranges.len() && ranges[selected_index].end <= cursor {
                selected_index += 1;
            }
            let (selected, selection_end) = if selected_index == ranges.len() {
                (false, total)
            } else if cursor < ranges[selected_index].start {
                (false, ranges[selected_index].start)
            } else {
                (true, ranges[selected_index].end)
            };
            let end = run_end.min(bucket_limit).min(selection_end);
            let n = end - cursor;
            if selected || verified || receiving {
                let bucket = &mut output[bucket_index];
                if selected {
                    bucket.selected += n;
                }
                if verified {
                    bucket.verified += n;
                    if selected {
                        bucket.selected_verified += n;
                    }
                } else if receiving {
                    bucket.receiving += n;
                    if selected {
                        bucket.selected_receiving += n;
                    }
                }
            }
            cursor = end;
        }
    }
    Some(count)
}

/// Returns the bucket count, or -1 for invalid inputs. All validation precedes
/// output writes. Ranges are sorted/merged in place, so pass a scratch copy.
///
/// # Safety
/// Nonempty buffers must be aligned, valid for their declared lengths, and
/// nonoverlapping. Runs are read-only; ranges and output require exclusive access.
#[no_mangle]
pub unsafe extern "C" fn webtor_piece_buckets(
    runs: *const PieceRun,
    run_count: usize,
    ranges: *mut PieceRange,
    range_count: usize,
    total: u32,
    max_buckets: u32,
    output: *mut PieceBucket,
    output_count: usize,
) -> i32 {
    if (run_count != 0 && runs.is_null())
        || (range_count != 0 && ranges.is_null())
        || (output_count != 0 && output.is_null())
        || run_count > isize::MAX as usize / size_of::<PieceRun>()
        || range_count > isize::MAX as usize / size_of::<PieceRange>()
        || output_count > 1024
    {
        return -1;
    }
    let runs = if run_count == 0 { &[] } else { slice::from_raw_parts(runs, run_count) };
    let ranges = if range_count == 0 { &mut [] } else { slice::from_raw_parts_mut(ranges, range_count) };
    let output = if output_count == 0 { &mut [] } else { slice::from_raw_parts_mut(output, output_count) };
    aggregate(runs, ranges, total, max_buckets, output).map_or(-1, |n| n as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_files_and_complete_precedence() {
        let runs = [PieceRun { length: 3, flags: 3 }, PieceRun { length: 3, flags: 2 }, PieceRun { length: 4, flags: 0 }];
        let mut ranges = [PieceRange { start: 4, end: 8 }, PieceRange { start: 1, end: 5 }, PieceRange { start: 1, end: 5 }];
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
        let mut out = [PieceBucket { total: 99, ..Default::default() }; 1];
        assert_eq!(aggregate(&[PieceRun { length: 10, flags: 0 }], &mut [], 9, 1, &mut out), None);
        assert_eq!(aggregate(&[PieceRun { length: 10, flags: 0 }], &mut [], 10, 2, &mut out), None);
        assert_eq!(out[0].total, 99);
        assert_eq!(unsafe { webtor_piece_buckets(std::ptr::null(), 1, std::ptr::null_mut(), 0, 1, 1, out.as_mut_ptr(), 1) }, -1);
    }

    #[test]
    fn empty_and_maximum_piece_count() {
        assert_eq!(aggregate(&[], &mut [], 0, 256, &mut []), Some(0));
        let mut out = [PieceBucket::default(); 1024];
        let n = aggregate(&[PieceRun { length: u32::MAX, flags: 1 }], &mut [PieceRange { start: 0, end: u32::MAX }], u32::MAX, 1024, &mut out).unwrap();
        assert_eq!(out[..n].iter().map(|b| u64::from(b.selected_verified)).sum::<u64>(), u64::from(u32::MAX));
    }

    #[test]
    fn bucket_geometry_for_selection_window() {
        let runs = [PieceRun { length: 10, flags: 0 }];
        let mut ranges = [PieceRange { start: 2, end: 7 }];
        let mut out = [PieceBucket::default(); 4];
        assert_eq!(aggregate(&runs, &mut ranges, 10, 4, &mut out), Some(4));
        assert_eq!(
            out,
            [
                PieceBucket { start: 0, end: 2, total: 3, selected: 1, ..Default::default() },
                PieceBucket { start: 3, end: 5, total: 3, selected: 3, ..Default::default() },
                PieceBucket { start: 6, end: 8, total: 3, selected: 1, ..Default::default() },
                PieceBucket { start: 9, end: 9, total: 1, ..Default::default() },
            ]
        );
    }

    #[test]
    fn adjacent_duplicate_and_unsorted_ranges_count_once() {
        let runs = [PieceRun { length: 4, flags: 1 }];
        let mut adjacent = [PieceRange { start: 0, end: 2 }, PieceRange { start: 2, end: 4 }];
        let mut out = [PieceBucket::default(); 4];
        assert_eq!(aggregate(&runs, &mut adjacent, 4, 4, &mut out), Some(4));
        // Index 2 is the shared boundary and belongs to only the second range.
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

        let mut unsorted = [PieceRange { start: 2, end: 4 }, PieceRange { start: 0, end: 2 }];
        let mut sorted = [PieceRange { start: 0, end: 2 }, PieceRange { start: 2, end: 4 }];
        let mut out_unsorted = [PieceBucket::default(); 4];
        let mut out_sorted = [PieceBucket::default(); 4];
        aggregate(&runs, &mut unsorted, 4, 4, &mut out_unsorted).unwrap();
        aggregate(&runs, &mut sorted, 4, 4, &mut out_sorted).unwrap();
        assert_eq!(out_unsorted, out_sorted);
    }

    #[test]
    fn adjacent_equal_flag_runs_match_merged_run() {
        let split = [
            PieceRun { length: 2, flags: 1 },
            PieceRun { length: 2, flags: 1 },
            PieceRun { length: 4, flags: 1 },
        ];
        let merged = [PieceRun { length: 8, flags: 1 }];
        let changed = [PieceRun { length: 3, flags: 1 }, PieceRun { length: 5, flags: 2 }];
        let mut out_split = [PieceBucket::default(); 4];
        let mut out_merged = [PieceBucket::default(); 4];
        let mut out_changed = [PieceBucket::default(); 4];
        assert_eq!(aggregate(&split, &mut [PieceRange { start: 1, end: 7 }], 8, 4, &mut out_split), Some(4));
        assert_eq!(aggregate(&merged, &mut [PieceRange { start: 1, end: 7 }], 8, 4, &mut out_merged), Some(4));
        assert_eq!(out_split, out_merged);

        assert_eq!(aggregate(&changed, &mut [PieceRange { start: 1, end: 7 }], 8, 4, &mut out_changed), Some(4));
        assert_ne!(out_changed, out_merged);
        // size = 2; the flag change at piece 3 splits bucket [2, 4).
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
        let runs = [PieceRun { length: 10, flags: 1 }];
        // total=10, max_buckets=4 → 4 buckets; three slots is short.
        assert_eq!(aggregate(&runs, &mut [], 10, 4, &mut out[..3]), None);
        assert_eq!(aggregate(&runs, &mut [], 10, 0, &mut out), None);
        assert_eq!(aggregate(&runs, &mut [], 10, 1025, &mut out), None);
        assert_eq!(
            aggregate(
                &[PieceRun { length: 5, flags: 1 }, PieceRun { length: 0, flags: 2 }, PieceRun { length: 5, flags: 1 }],
                &mut [],
                10,
                1,
                &mut out,
            ),
            None
        );
        assert_eq!(aggregate(&[PieceRun { length: 0, flags: 0 }], &mut [], 0, 1, &mut out), None);
        assert_eq!(aggregate(&runs, &mut [PieceRange { start: 6, end: 2 }], 10, 1, &mut out), None);
        assert_eq!(out, [sentinel; 4]);
    }

    fn oracle(runs: &[PieceRun], ranges: &[PieceRange], total: u32, max_buckets: u32) -> Vec<PieceBucket> {
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
            out.push(vec![PieceRun { length: total, flags }]);
        }
        out.push((0..total).map(|i| PieceRun { length: 1, flags: i % 4 }).collect());
        out.push(
            (0..total)
                .map(|i| PieceRun { length: 1, flags: if (i / 2) % 2 == 0 { 1 } else { 2 } })
                .collect(),
        );
        if total >= 2 {
            let mid = total / 2;
            out.push(vec![
                PieceRun { length: mid, flags: 3 },
                PieceRun { length: total - mid, flags: 2 },
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
            vec![PieceRange { start: 0, end: total }],
            vec![PieceRange { start: 0, end: total }, PieceRange { start: 0, end: total }],
            vec![PieceRange { start: 0, end: mid }, PieceRange { start: mid, end: total }],
            vec![PieceRange { start: mid, end: total }, PieceRange { start: 0, end: mid.max(1) }],
        ];
        if total >= 4 {
            out.push(vec![
                PieceRange { start: total - 1, end: total },
                PieceRange { start: 1, end: total - 1 },
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
                            let mut out = [PieceBucket { total: 99, ..Default::default() }];
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
                        let n = aggregate(runs, &mut ranges, total, max_buckets, &mut out).unwrap_or_else(|| {
                            panic!("total={total} max_buckets={max_buckets} run_pat={ri} range_pat={rgi}")
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
}
