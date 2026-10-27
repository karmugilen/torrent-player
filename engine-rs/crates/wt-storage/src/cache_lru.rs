use std::collections::HashMap;

#[derive(Clone, Copy, Debug)]
struct Node {
    key: i32,
    prev: Option<usize>,
    next: Option<usize>,
}

/// Piece LRU ordering from least to most recently used.
/// Touch and remove are O(1).
#[derive(Default, Debug)]
pub struct PieceLRU {
    map: HashMap<i32, usize>,
    nodes: Vec<Option<Node>>,
    free_head: Option<usize>,
    head: Option<usize>, // least recently used
    tail: Option<usize>, // most recently used
}

impl PieceLRU {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn touch(&mut self, key: i32) {
        if let Some(&node_idx) = self.map.get(&key) {
            // Already present, move to tail (most recently used)
            if self.tail == Some(node_idx) {
                return;
            }
            self.detach(node_idx);
            self.attach_tail(node_idx);
        } else {
            // New node
            let node_idx = self.alloc_node(key);
            self.attach_tail(node_idx);
            self.map.insert(key, node_idx);
        }
    }

    pub fn remove(&mut self, key: i32) {
        if let Some(node_idx) = self.map.remove(&key) {
            self.detach(node_idx);
            self.free_node(node_idx);
        }
    }

    pub fn oldest(&self, skip: i32) -> Option<i32> {
        let mut cur = self.head;
        while let Some(idx) = cur {
            let node = self.nodes[idx].as_ref().unwrap();
            if node.key != skip {
                return Some(node.key);
            }
            cur = node.next;
        }
        None
    }

    pub fn each<F>(&self, mut visit: F)
    where
        F: FnMut(i32) -> bool,
    {
        let mut cur = self.head;
        while let Some(idx) = cur {
            let node = self.nodes[idx].as_ref().unwrap();
            if !visit(node.key) {
                break;
            }
            cur = node.next;
        }
    }

    pub fn reset(&mut self) {
        self.map.clear();
        self.nodes.clear();
        self.free_head = None;
        self.head = None;
        self.tail = None;
    }

    fn detach(&mut self, idx: usize) {
        let node = self.nodes[idx].as_ref().unwrap();
        let prev = node.prev;
        let next = node.next;

        if let Some(p) = prev {
            self.nodes[p].as_mut().unwrap().next = next;
        } else {
            self.head = next;
        }

        if let Some(n) = next {
            self.nodes[n].as_mut().unwrap().prev = prev;
        } else {
            self.tail = prev;
        }

        let cur = self.nodes[idx].as_mut().unwrap();
        cur.prev = None;
        cur.next = None;
    }

    fn attach_tail(&mut self, idx: usize) {
        let cur = self.nodes[idx].as_mut().unwrap();
        cur.prev = self.tail;
        cur.next = None;

        if let Some(t) = self.tail {
            self.nodes[t].as_mut().unwrap().next = Some(idx);
        } else {
            self.head = Some(idx);
        }
        self.tail = Some(idx);
    }

    fn alloc_node(&mut self, key: i32) -> usize {
        if let Some(idx) = self.free_head {
            let next_free = match self.nodes[idx] {
                Some(Node { next, .. }) => next,
                None => None,
            };
            self.free_head = next_free;
            self.nodes[idx] = Some(Node {
                key,
                prev: None,
                next: None,
            });
            idx
        } else {
            let idx = self.nodes.len();
            self.nodes.push(Some(Node {
                key,
                prev: None,
                next: None,
            }));
            idx
        }
    }

    fn free_node(&mut self, idx: usize) {
        self.nodes[idx] = Some(Node {
            key: 0,
            prev: None,
            next: self.free_head,
        });
        self.free_head = Some(idx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_piece_lru() {
        let mut lru = PieceLRU::new();
        assert_eq!(lru.len(), 0);
        assert_eq!(lru.oldest(-1), None);

        lru.touch(1);
        lru.touch(2);
        lru.touch(3);
        assert_eq!(lru.len(), 3);
        assert_eq!(lru.oldest(-1), Some(1));
        assert_eq!(lru.oldest(1), Some(2));

        // Touch 1 again, now 2 is oldest
        lru.touch(1);
        assert_eq!(lru.oldest(-1), Some(2));

        // Remove 2, now 3 is oldest
        lru.remove(2);
        assert_eq!(lru.len(), 2);
        assert_eq!(lru.oldest(-1), Some(3));

        // Visit order
        let mut visited = Vec::new();
        lru.each(|k| {
            visited.push(k);
            true
        });
        assert_eq!(visited, vec![3, 1]);

        lru.reset();
        assert_eq!(lru.len(), 0);
        assert_eq!(lru.oldest(-1), None);
    }
}
