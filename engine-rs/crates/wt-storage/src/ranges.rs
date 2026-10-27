use std::{mem::size_of, slice};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteRange {
    pub start: i64,
    pub end: i64,
}

/// Normalizes and merges an incoming range into an already sorted, non-overlapping
/// list of ranges.
pub fn merge_received_range(ranges: &mut Vec<ByteRange>, incoming: ByteRange) {
    if incoming.start < 0 || incoming.end <= incoming.start {
        return;
    }
    ranges.push(incoming);
    let count = ranges.len();
    unsafe {
        let n = webtor_merge_received_range(ranges.as_mut_ptr(), count);
        if n >= 0 {
            ranges.truncate(n as usize);
        } else {
            ranges.pop();
        }
    }
}

/// Reports whether the sorted disjoint `ranges` completely cover `[start, end)`.
pub fn ranges_cover(ranges: &[ByteRange], start: i64, end: i64) -> bool {
    if start >= end {
        return true;
    }
    let mut cur = start;
    for r in ranges {
        if r.start > cur {
            return false;
        }
        if r.end > cur {
            cur = r.end;
            if cur >= end {
                return true;
            }
        }
    }
    cur >= end
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_received_range() {
        let mut ranges = Vec::new();
        merge_received_range(&mut ranges, ByteRange { start: 10, end: 20 });
        assert_eq!(ranges, vec![ByteRange { start: 10, end: 20 }]);

        // Non-overlapping after
        merge_received_range(&mut ranges, ByteRange { start: 30, end: 40 });
        assert_eq!(
            ranges,
            vec![
                ByteRange { start: 10, end: 20 },
                ByteRange { start: 30, end: 40 }
            ]
        );

        // Adjacent merges
        merge_received_range(&mut ranges, ByteRange { start: 20, end: 30 });
        assert_eq!(ranges, vec![ByteRange { start: 10, end: 40 }]);

        // Overlapping prefix
        merge_received_range(&mut ranges, ByteRange { start: 5, end: 15 });
        assert_eq!(ranges, vec![ByteRange { start: 5, end: 40 }]);

        // Internal subset
        merge_received_range(&mut ranges, ByteRange { start: 10, end: 20 });
        assert_eq!(ranges, vec![ByteRange { start: 5, end: 40 }]);
    }

    #[test]
    fn test_ranges_cover() {
        let ranges = vec![ByteRange { start: 10, end: 50 }];
        assert!(ranges_cover(&ranges, 10, 50));
        assert!(ranges_cover(&ranges, 20, 40));
        assert!(!ranges_cover(&ranges, 5, 50));
        assert!(!ranges_cover(&ranges, 10, 55));

        let split = vec![
            ByteRange { start: 0, end: 10 },
            ByteRange { start: 10, end: 20 },
        ];
        assert!(ranges_cover(&split, 0, 20));

        let gap = vec![
            ByteRange { start: 0, end: 10 },
            ByteRange { start: 11, end: 20 },
        ];
        assert!(!ranges_cover(&gap, 0, 20));
    }
}
