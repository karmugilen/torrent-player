//! Priority raise / pure combination (Go `PiecePriority.Raise`, `Piece.purePriority`).

use wt_types::PiecePriority;

/// Go `(*PiecePriority).Raise` — keep the greater of the two.
#[inline]
pub fn raise_priority(cur: PiecePriority, maybe: PiecePriority) -> PiecePriority {
    if maybe > cur {
        maybe
    } else {
        cur
    }
}

/// Max of piece priority, file priority, and reader Now / Readahead interest.
///
/// Mirrors `Piece.purePriority`: raise file prios, then Now if in reader-now
/// set, then Readahead if in reader-readahead set, then the piece's own prio.
#[inline]
pub fn combine_pure_priority(
    piece: PiecePriority,
    file: PiecePriority,
    reader_now: bool,
    reader_readahead: bool,
) -> PiecePriority {
    let mut ret = PiecePriority::None;
    ret = raise_priority(ret, file);
    if reader_now {
        ret = raise_priority(ret, PiecePriority::Now);
    }
    if reader_readahead {
        ret = raise_priority(ret, PiecePriority::Readahead);
    }
    raise_priority(ret, piece)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raise_keeps_max() {
        assert_eq!(
            raise_priority(PiecePriority::Normal, PiecePriority::High),
            PiecePriority::High
        );
        assert_eq!(
            raise_priority(PiecePriority::Now, PiecePriority::High),
            PiecePriority::Now
        );
    }

    #[test]
    fn combine_reader_now_wins() {
        let p = combine_pure_priority(PiecePriority::Normal, PiecePriority::High, true, true);
        assert_eq!(p, PiecePriority::Now);
    }

    #[test]
    fn combine_readahead_over_file() {
        let p = combine_pure_priority(PiecePriority::None, PiecePriority::Normal, false, true);
        assert_eq!(p, PiecePriority::Readahead);
    }
}
