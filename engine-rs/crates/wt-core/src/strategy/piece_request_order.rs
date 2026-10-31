//! Persistent piece request order — O(log n) add/update/delete.
//!
//! Mirrors `internal/request-strategy/piece-request-order.go` with a
//! `BTreeSet` standing in for the Go btree.

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};

use wt_types::PiecePriority;

use super::order::piece_order_cmp;

/// Canonical short infohash + piece index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PieceRequestOrderKey {
    pub info_hash: [u8; 20],
    pub index: u32,
}

/// State that participates in (or is stored alongside) the order.
///
/// `availability` and `partial` only order `Normal` pieces; see
/// [`super::order::background_rank`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PieceRequestOrderState {
    pub availability: i32,
    pub priority: PiecePriority,
    pub partial: bool,
}

impl Default for PieceRequestOrderState {
    fn default() -> Self {
        Self {
            availability: 0,
            priority: PiecePriority::None,
            partial: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PieceRequestOrderItem {
    pub key: PieceRequestOrderKey,
    pub state: PieceRequestOrderState,
}

/// Newtype so `BTreeSet` orders by [`piece_order_cmp`], not by key alone.
#[derive(Clone, Copy, Debug)]
struct Ordered(PieceRequestOrderItem);

impl PartialEq for Ordered {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Ordered {}

impl PartialOrd for Ordered {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Ordered {
    fn cmp(&self, other: &Self) -> Ordering {
        // piece_order_cmp ends in index, infohash — a total order over distinct keys.
        piece_order_cmp(&self.0, &other.0)
    }
}

/// Ordered index of requestable pieces for one storage-capacity group.
#[derive(Clone, Debug, Default)]
pub struct PieceRequestOrder {
    tree: BTreeSet<Ordered>,
    keys: HashMap<PieceRequestOrderKey, PieceRequestOrderState>,
}

impl PieceRequestOrder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            tree: BTreeSet::new(),
            keys: HashMap::with_capacity(cap),
        }
    }

    /// Insert or replace. Returns the previous state if the key was present.
    pub fn add(
        &mut self,
        key: PieceRequestOrderKey,
        state: PieceRequestOrderState,
    ) -> Option<PieceRequestOrderState> {
        if let Some(old) = self.keys.get(&key).copied() {
            if old == state {
                return Some(old);
            }
            self.tree
                .remove(&Ordered(PieceRequestOrderItem { key, state: old }));
            self.tree
                .insert(Ordered(PieceRequestOrderItem { key, state }));
            self.keys.insert(key, state);
            return Some(old);
        }
        self.tree
            .insert(Ordered(PieceRequestOrderItem { key, state }));
        self.keys.insert(key, state);
        None
    }

    /// Key must already exist. Returns whether the state changed.
    pub fn update(&mut self, key: PieceRequestOrderKey, state: PieceRequestOrderState) -> bool {
        let old = self.add(key, state);
        debug_assert!(old.is_some(), "PieceRequestOrder::update on missing key");
        old.map(|o| o != state).unwrap_or(true)
    }

    pub fn delete(&mut self, key: PieceRequestOrderKey) -> bool {
        let Some(state) = self.keys.remove(&key) else {
            return false;
        };
        self.tree
            .remove(&Ordered(PieceRequestOrderItem { key, state }));
        true
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn get(&self, key: &PieceRequestOrderKey) -> Option<PieceRequestOrderState> {
        self.keys.get(key).copied()
    }

    pub fn contains(&self, key: &PieceRequestOrderKey) -> bool {
        self.keys.contains_key(key)
    }

    /// Iterate in request order (highest priority first, then piece index).
    pub fn iter(&self) -> impl Iterator<Item = PieceRequestOrderItem> + '_ {
        self.tree.iter().map(|o| o.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wt_types::PiecePriority;

    fn key(index: u32) -> PieceRequestOrderKey {
        PieceRequestOrderKey {
            info_hash: [0u8; 20],
            index,
        }
    }

    #[test]
    fn order_is_priority_then_index() {
        let mut pro = PieceRequestOrder::with_capacity(8);
        for i in 0..8 {
            let pri = match i % 4 {
                0 => PiecePriority::Normal,
                1 => PiecePriority::High,
                2 => PiecePriority::Readahead,
                _ => PiecePriority::Now,
            };
            pro.add(
                key(i),
                PieceRequestOrderState {
                    availability: (8 - i) as i32,
                    priority: pri,
                    partial: false,
                },
            );
        }
        let indexes: Vec<u32> = pro.iter().map(|it| it.key.index).collect();
        // Now pieces (3,7), then Readahead (2,6), High (1,5), Normal (0,4)
        assert_eq!(indexes, vec![3, 7, 2, 6, 1, 5, 0, 4]);
    }

    #[test]
    fn update_moves_in_tree() {
        let mut pro = PieceRequestOrder::new();
        pro.add(
            key(5),
            PieceRequestOrderState {
                availability: 1,
                priority: PiecePriority::Normal,
                partial: false,
            },
        );
        pro.add(
            key(1),
            PieceRequestOrderState {
                availability: 1,
                priority: PiecePriority::Normal,
                partial: false,
            },
        );
        assert!(pro.update(
            key(5),
            PieceRequestOrderState {
                availability: 1,
                priority: PiecePriority::Now,
                partial: false,
            },
        ));
        let indexes: Vec<u32> = pro.iter().map(|it| it.key.index).collect();
        assert_eq!(indexes, vec![5, 1]);
    }

    #[test]
    fn delete_and_len() {
        let mut pro = PieceRequestOrder::new();
        pro.add(key(0), PieceRequestOrderState::default());
        pro.add(key(1), PieceRequestOrderState::default());
        assert!(pro.delete(key(0)));
        assert!(!pro.delete(key(0)));
        assert_eq!(pro.len(), 1);
    }
}
