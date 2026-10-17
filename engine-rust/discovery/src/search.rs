//! Compact native token postings. Batch input is normalized in Go once; query
//! prefix ranges use a BTreeMap instead of scanning the Go vocabulary.
use std::collections::{BTreeMap, HashMap, HashSet};

const POSTING_CAP: usize = 40_000;

#[derive(Default)]
pub struct SearchIndex {
    hashes: Vec<[u8; 20]>,
    ids: HashMap<[u8; 20], u32>,
    tokens: BTreeMap<String, Vec<u32>>,
}

fn decode_hash(hash: &[u8]) -> Option<[u8; 20]> {
    if hash.len() != 40 {
        return None;
    }
    let mut out = [0; 20];
    for (i, pair) in hash.chunks_exact(2).enumerate() {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        out[i] = ((high << 4) | low) as u8;
    }
    Some(out)
}

impl SearchIndex {
    pub fn add_batch(&mut self, bytes: &[u8]) -> usize {
        let mut added = 0;
        for line in bytes.split(|b| *b == b'\n') {
            if line.len() < 42 || line[40] != b'\t' {
                continue;
            }
            let Some(hash) = decode_hash(&line[..40]) else {
                continue;
            };
            let Ok(name) = std::str::from_utf8(&line[41..]) else {
                continue;
            };
            let id = if let Some(id) = self.ids.get(&hash) {
                *id
            } else {
                let id = self.hashes.len() as u32;
                self.hashes.push(hash);
                self.ids.insert(hash, id);
                added += 1;
                id
            };
            let mut seen = HashSet::new();
            for token in name
                .split_whitespace()
                .filter(|t| t.len() >= 2 && t.len() <= 48)
            {
                if !seen.insert(token) {
                    continue;
                }
                let list = self.tokens.entry(token.to_owned()).or_default();
                if list.len() < POSTING_CAP && list.last() != Some(&id) {
                    list.push(id);
                }
                if seen.len() == 16 {
                    break;
                }
            }
        }
        added
    }

    pub fn candidates(&self, query: &str) -> Option<Vec<[u8; 20]>> {
        if self.hashes.is_empty() {
            return None;
        }
        let mut best: Option<Vec<u32>> = None;
        for token in query.split_whitespace() {
            let mut ids = if let Some(postings) = self.tokens.get(token) {
                postings.clone()
            } else {
                let mut ids = Vec::new();
                for (word, postings) in self.tokens.range(token.to_owned()..) {
                    if !word.starts_with(token) {
                        break;
                    }
                    let count = postings.len().min(POSTING_CAP - ids.len());
                    ids.extend_from_slice(&postings[..count]);
                    if ids.len() >= POSTING_CAP {
                        break;
                    }
                }
                ids
            };
            if ids.is_empty() {
                return Some(Vec::new());
            }
            ids.sort_unstable();
            ids.dedup();
            if best.as_ref().map_or(true, |old| ids.len() < old.len()) {
                best = Some(ids);
            }
        }
        Some(
            best.unwrap_or_default()
                .into_iter()
                .map(|id| self.hashes[id as usize])
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn batched_postings_support_prefixes_duplicates_and_unknowns() {
        let mut index = SearchIndex::default();
        let input = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\tcafe movie 2026\nbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\tcafe music\n";
        assert_eq!(index.add_batch(input), 2);
        assert_eq!(index.add_batch(input), 0);
        assert_eq!(index.candidates("mov").unwrap(), vec![[0xaa; 20]]);
        assert_eq!(index.candidates("2026 cafe").unwrap(), vec![[0xaa; 20]]);
        assert_eq!(index.candidates("missing").unwrap().len(), 0);
        assert_eq!(index.candidates("cafe").unwrap().len(), 2);
    }
    #[test]
    fn rejects_malformed_batches() {
        let mut index = SearchIndex::default();
        assert_eq!(
            index.add_batch(b"bad\nzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz\tno\n"),
            0
        );
        assert!(index.candidates("no").is_none());
    }
}
