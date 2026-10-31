//! Compact roaring bitset wrapper for RequestIndex / PieceIndex sets.

use roaring::RoaringBitmap;

/// Allocation-light bitset over `u32` indices (Go `roaring.Bitmap` /
/// `typedRoaring.Bitmap`).
#[derive(Clone, Debug, Default)]
pub struct Bitset {
    inner: RoaringBitmap,
}

impl Bitset {
    pub fn new() -> Self {
        Self::default()
    }


    #[inline]
    pub fn insert(&mut self, v: u32) -> bool {
        self.inner.insert(v)
    }

    #[inline]
    pub fn remove(&mut self, v: u32) -> bool {
        self.inner.remove(v)
    }

    #[inline]
    pub fn contains(&self, v: u32) -> bool {
        self.inner.contains(v)
    }

    #[inline]
    pub fn clear(&mut self) {
        self.inner.clear();
    }

    /// Remove `[begin, end)`.
    pub fn remove_range(&mut self, begin: u32, end: u32) {
        if begin >= end {
            return;
        }
        self.inner.remove_range(begin..end);
    }

    /// Insert `[begin, end)`.
    pub fn insert_range(&mut self, begin: u32, end: u32) {
        if begin >= end {
            return;
        }
        self.inner.insert_range(begin..end);
    }


    pub fn len(&self) -> u64 {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Count of set bits in `[begin, end)`.
    pub fn range_cardinality(&self, begin: u32, end: u32) -> u64 {
        if begin >= end {
            return 0;
        }
        self.inner.range_cardinality(begin..end)
    }

    pub fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        self.inner.iter()
    }

    /// Indices in `[begin, end)` that are **not** set (Go clean-chunk walk).
    pub fn unset_in_range(&self, begin: u32, end: u32) -> Vec<u32> {
        if begin >= end {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut set = self.inner.range(begin..end);
        let mut next_set = set.next();
        let mut cur = begin;
        while cur < end {
            match next_set {
                Some(s) if s == cur => {
                    next_set = set.next();
                    cur = cur.saturating_add(1);
                }
                _ => {
                    out.push(cur);
                    cur = cur.saturating_add(1);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_ops_and_unset() {
        let mut b = Bitset::new();
        b.insert_range(10, 15);
        assert_eq!(b.range_cardinality(10, 15), 5);
        assert!(b.contains(12));
        b.remove_range(11, 14);
        assert!(b.contains(10));
        assert!(!b.contains(11));
        assert!(b.contains(14));
        assert_eq!(b.unset_in_range(10, 15), vec![11, 12, 13]);
    }
}
