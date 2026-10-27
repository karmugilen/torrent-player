use std::mem::size_of;
use std::slice;
use wt_types::{PiecePriority, StorageError, StorageResult};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PieceRun {
    pub length: u32,
    pub flags: u32, // 1 = verified, 2 = receiving (verified takes precedence)
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PieceRange {
    pub start: u32,
    pub end: u32, // exclusive
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PieceStateRun {
    pub length: usize,
    pub complete: bool,
    pub checking: bool,
    pub hashing: bool,
    pub partial: bool,
    pub priority: PiecePriority,
}

/// Exclusive end of the bucket beginning at `start`.
#[inline]
fn exclusive_end(start: u32, size: u32, total: u32) -> u32 {
    start.checked_add(size).map_or(total, |end| end.min(total))
}

pub fn compress_piece_runs(
    states: &[PieceStateRun],
    total: usize,
    playing: bool,
) -> StorageResult<Vec<PieceRun>> {
    let mut runs: Vec<PieceRun> = Vec::with_capacity(64);
    let mut sum = 0u64;
    let limit = u32::MAX as u64;
    let total64 = total as u64;

    for state in states {
        if state.length == 0 {
            return Err(StorageError::Other("invalid piece run".into()));
        }
        let length = state.length as u64;
        let next = sum + length;
        if next > limit || next > total64 {
            return Err(StorageError::Other("invalid piece run".into()));
        }
        sum = next;

        let flags = if state.complete {
            1
        } else if state.checking
            || state.hashing
            || (state.partial && (!playing || state.priority >= PiecePriority::Next))
        {
            2
        } else {
            0
        };

        let n = length as u32;
        if let Some(last) = runs.last_mut() {
            if last.flags == flags {
                last.length += n;
                continue;
            }
        }
        runs.push(PieceRun { length: n, flags });
    }

    if sum != total64 {
        return Err(StorageError::Other("invalid piece run".into()));
    }
    Ok(runs)
}

pub fn aggregate_piece_buckets(
    runs: &[PieceRun],
    ranges: &[PieceRange],
    total: u32,
    max_buckets: u32,
) -> StorageResult<Vec<PieceBucket>> {
    if !(1..=1024).contains(&max_buckets) {
        return Err(StorageError::Other("invalid max buckets".into()));
    }
    let mut scratch_ranges = ranges.to_vec();
    if total == 0 {
        let mut out = [PieceBucket::default(); 1];
        let n = aggregate(runs, &mut scratch_ranges, 0, max_buckets, &mut out)
            .ok_or_else(|| StorageError::Other("piece aggregation failed".into()))?;
        return Ok(out[..n].to_vec());
    }
    let size = total.div_ceil(max_buckets);
    let count = total.div_ceil(size) as usize;
    let mut out = vec![PieceBucket::default(); count];

    let n = aggregate(runs, &mut scratch_ranges, total, max_buckets, &mut out)
        .ok_or_else(|| StorageError::Other("piece aggregation failed".into()))?;
    out.truncate(n);
    Ok(out)
}

pub fn aggregate(
    runs: &[PieceRun],
    ranges: &mut [PieceRange],
    total: u32,
    max_buckets: u32,
    output: &mut [PieceBucket],
) -> Option<usize> {
    if !(1..=1024).contains(&max_buckets) {
        return None;
    }
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
    let runs = if run_count == 0 {
        &[]
    } else {
        slice::from_raw_parts(runs, run_count)
    };
    let ranges = if range_count == 0 {
        &mut []
    } else {
        slice::from_raw_parts_mut(ranges, range_count)
    };
    let output = if output_count == 0 {
        &mut []
    } else {
        slice::from_raw_parts_mut(output, output_count)
    };
    aggregate(runs, ranges, total, max_buckets, output).map_or(-1, |n| n as i32)
}
