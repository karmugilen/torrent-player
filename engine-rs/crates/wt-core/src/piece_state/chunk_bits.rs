//! Lock-free set of received blocks, one bit per torrent-wide RequestIndex.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

/// Sized once when the torrent's info arrives. Before that it is empty and
/// inserts are dropped: no block can be received without info.
#[derive(Default)]
pub struct ChunkBits {
    words: OnceLock<Box<[AtomicU64]>>,
}

impl ChunkBits {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn init(&self, bits: u32) {
        let _ = self.words.get_or_init(|| {
            (0..(bits as usize).div_ceil(64)).map(|_| AtomicU64::new(0)).collect()
        });
    }

    fn word(&self, index: usize) -> Option<&AtomicU64> {
        self.words.get()?.get(index)
    }

    /// `Acquire` pairs with the `Release` in [`Self::insert`]: seeing the bit
    /// means seeing the write that came before it.
    #[inline]
    pub fn contains(&self, req: u32) -> bool {
        self.word(req as usize / 64)
            .is_some_and(|w| w.load(Ordering::Acquire) & (1 << (req % 64)) != 0)
    }

    #[inline]
    pub fn insert(&self, req: u32) {
        if let Some(w) = self.word(req as usize / 64) {
            w.fetch_or(1 << (req % 64), Ordering::Release);
        }
    }

    /// Sets the bit. False if it was already set: exactly one caller wins.
    #[inline]
    pub fn insert_new(&self, req: u32) -> bool {
        let bit = 1 << (req % 64);
        self.word(req as usize / 64)
            .is_none_or(|w| w.fetch_or(bit, Ordering::AcqRel) & bit == 0)
    }

    #[inline]
    pub fn remove(&self, req: u32) {
        if let Some(w) = self.word(req as usize / 64) {
            w.fetch_and(!(1 << (req % 64)), Ordering::Release);
        }
    }

    /// `mask` for the bits of word `index` inside `[begin, end)`.
    fn range_mask(index: usize, begin: u32, end: u32) -> u64 {
        let first = (index as u64) * 64;
        let low = u64::from(begin).saturating_sub(first);
        let high = (u64::from(end) - first).min(64);
        let upper = if high == 64 { u64::MAX } else { (1u64 << high) - 1 };
        upper & !((1u64 << low) - 1)
    }

    fn word_range(begin: u32, end: u32) -> std::ops::Range<usize> {
        begin as usize / 64..(end as usize).div_ceil(64)
    }

    /// Every bit of `[begin, end)` is set. False for an empty range.
    pub fn all_in_range(&self, begin: u32, end: u32) -> bool {
        begin < end
            && Self::word_range(begin, end).all(|i| {
                let mask = Self::range_mask(i, begin, end);
                self.word(i).is_some_and(|w| w.load(Ordering::Acquire) & mask == mask)
            })
    }

    pub fn any_in_range(&self, begin: u32, end: u32) -> bool {
        begin < end
            && Self::word_range(begin, end).any(|i| {
                let mask = Self::range_mask(i, begin, end);
                self.word(i).is_some_and(|w| w.load(Ordering::Acquire) & mask != 0)
            })
    }

    pub fn remove_range(&self, begin: u32, end: u32) {
        if begin >= end {
            return;
        }
        for i in Self::word_range(begin, end) {
            let mask = Self::range_mask(i, begin, end);
            if let Some(w) = self.word(i) {
                w.fetch_and(!mask, Ordering::Release);
            }
        }
    }
}

impl std::fmt::Debug for ChunkBits {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let set: u32 = self
            .words
            .get()
            .map_or(0, |w| w.iter().map(|w| w.load(Ordering::Relaxed).count_ones()).sum());
        write!(f, "ChunkBits({set} set)")
    }
}
