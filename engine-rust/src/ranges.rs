//! Incremental received-byte tracking. The caller owns the buffer; Rust never
//! allocates or retains it. All but the final range are sorted and disjoint.

use std::{mem::size_of, slice};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ByteRange {
    pub start: i64,
    pub end: i64,
}

/// Insert the final range into the normalized prefix, merging overlap and
/// adjacency. Returns the new count, or -1 for an invalid buffer/new range.
///
/// # Safety
/// `ranges` must be aligned and writable for `count` initialized ByteRanges.
/// The prefix must contain nonnegative, nonempty ranges in increasing order,
/// with a gap between each pair. Access must be exclusive during the call.
#[no_mangle]
pub unsafe extern "C" fn webtor_merge_received_range(
    ranges: *mut ByteRange,
    count: usize,
) -> isize {
    if ranges.is_null() || count == 0 || count > isize::MAX as usize / size_of::<ByteRange>() {
        return -1;
    }
    let ranges = slice::from_raw_parts_mut(ranges, count);
    let existing = count - 1;
    let mut incoming = ranges[existing];
    if incoming.start < 0 || incoming.end <= incoming.start {
        return -1;
    }
    let begin = ranges[..existing].partition_point(|r| r.end < incoming.start);
    let mut end = begin;
    while end < existing && ranges[end].start <= incoming.end {
        incoming.start = incoming.start.min(ranges[end].start);
        incoming.end = incoming.end.max(ranges[end].end);
        end += 1;
    }
    ranges.copy_within(end..existing, begin + 1);
    ranges[begin] = incoming;
    (existing - (end - begin) + 1) as isize
}
