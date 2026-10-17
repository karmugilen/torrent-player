//! Memory-mapped torrents.csv index.
//!
//! The upstream dump is sorted by infohash. We mmap the file, index every data
//! line start, and binary-search on the leading 40 hex chars. Row fields are
//! parsed only on hit so load stays cheap and lookups avoid a giant HashMap.

use memmap2::Mmap;
use std::fs::File;
use std::path::Path;

#[derive(Clone, Debug, Default)]
pub struct DumpRow {
    pub name: String,
    pub size: i64,
    pub seeders: i32,
    pub leechers: i32,
}

pub struct DumpIndex {
    mmap: Mmap,
    /// Byte offsets of each data line (after the header).
    lines: Vec<u32>,
}

impl DumpIndex {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| e.to_string())?;
        let mmap = unsafe { Mmap::map(&file) }.map_err(|e| e.to_string())?;
        if mmap.len() < 16 {
            return Err("dump too small".into());
        }
        let lines = index_lines(&mmap)?;
        Ok(Self { mmap, lines })
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Exact infohash lookup. `hash` must be 40 lowercase hex bytes.
    pub fn lookup(&self, hash: &[u8]) -> Option<DumpRow> {
        if hash.len() != 40 || self.lines.is_empty() {
            return None;
        }
        let data = &self.mmap[..];
        let mut lo = 0usize;
        let mut hi = self.lines.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let off = self.lines[mid] as usize;
            let line_hash = line_infohash(data, off)?;
            match line_hash.cmp(hash) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => return parse_row(data, off),
            }
        }
        None
    }

    /// Walk every row. Callback returns false to stop.
    pub fn for_each<F>(&self, mut f: F)
    where
        F: FnMut(&[u8], DumpRow) -> bool,
    {
        let data = &self.mmap[..];
        for &off in &self.lines {
            let off = off as usize;
            let Some(hash) = line_infohash(data, off) else {
                continue;
            };
            let Some(row) = parse_row(data, off) else {
                continue;
            };
            if !f(hash, row) {
                break;
            }
        }
    }
}

fn index_lines(data: &[u8]) -> Result<Vec<u32>, String> {
    // Skip header line.
    let mut i = 0usize;
    while i < data.len() && data[i] != b'\n' {
        i += 1;
    }
    if i >= data.len() {
        return Err("missing header newline".into());
    }
    i += 1;
    let mut lines = Vec::with_capacity(1_200_000);
    while i < data.len() {
        let start = i;
        // Fast-forward to next newline.
        while i < data.len() && data[i] != b'\n' {
            i += 1;
        }
        if i > start {
            // Require a plausible infohash prefix.
            if i - start >= 40 && is_hex40(&data[start..start + 40]) {
                lines.push(start as u32);
            }
        }
        if i < data.len() {
            i += 1;
        }
    }
    Ok(lines)
}

#[inline]
fn is_hex40(b: &[u8]) -> bool {
    if b.len() < 40 {
        return false;
    }
    for &c in &b[..40] {
        if !c.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

#[inline]
fn line_infohash(data: &[u8], off: usize) -> Option<&[u8]> {
    if off + 40 > data.len() {
        return None;
    }
    let h = &data[off..off + 40];
    if !is_hex40(h) {
        return None;
    }
    // Following char should be comma (or end).
    if off + 40 < data.len() && data[off + 40] != b',' && data[off + 40] != b'\n' {
        return None;
    }
    Some(h)
}

fn parse_row(data: &[u8], off: usize) -> Option<DumpRow> {
    let end = match data[off..].iter().position(|&c| c == b'\n') {
        Some(n) => off + n,
        None => data.len(),
    };
    let line = &data[off..end];
    // Strip CR.
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let mut fields = split_csv_limited(line, 6);
    if fields.len() < 2 {
        return None;
    }
    let name = clean_name(fields[1]);
    if name.is_empty() {
        return None;
    }
    let size = fields
        .get(2)
        .and_then(|s| std::str::from_utf8(s).ok())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    let seeders = fields
        .get(4)
        .and_then(|s| std::str::from_utf8(s).ok())
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0);
    let leechers = fields
        .get(5)
        .and_then(|s| std::str::from_utf8(s).ok())
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0);
    // Silence unused mut warning path for fields reuse.
    let _ = fields.pop();
    Some(DumpRow {
        name,
        size,
        seeders,
        leechers,
    })
}

/// Split on commas without full CSV quote handling for the known dump shape.
/// Names can contain commas inside quotes — handle double-quoted fields.
fn split_csv_limited<'a>(line: &'a [u8], max_fields: usize) -> Vec<&'a [u8]> {
    let mut out = Vec::with_capacity(max_fields);
    let mut i = 0usize;
    while i <= line.len() && out.len() < max_fields {
        if i >= line.len() {
            out.push(&line[i..i]);
            break;
        }
        if line[i] == b'"' {
            i += 1;
            let start = i;
            while i < line.len() {
                if line[i] == b'"' {
                    if i + 1 < line.len() && line[i + 1] == b'"' {
                        i += 2;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
            out.push(&line[start..i]);
            if i < line.len() && line[i] == b'"' {
                i += 1;
            }
            if i < line.len() && line[i] == b',' {
                i += 1;
            }
        } else {
            let start = i;
            while i < line.len() && line[i] != b',' {
                i += 1;
            }
            out.push(&line[start..i]);
            if i < line.len() {
                i += 1;
            } else {
                break;
            }
        }
    }
    out
}

fn clean_name(raw: &[u8]) -> String {
    let s = std::str::from_utf8(raw).unwrap_or("").trim();
    if s.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(s.len().min(512));
    for ch in s.chars() {
        if out.chars().count() >= 512 {
            break;
        }
        if ch == '\0' || ch == '\n' || ch == '\r' {
            continue;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn binary_search_finds_rows() {
        let dir = tempfile_dir();
        let path = dir.join("t.csv");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            "infohash,name,size_bytes,created_unix,seeders,leechers,completed,scraped_date,published"
        )
        .unwrap();
        writeln!(
            f,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa,Alpha,100,0,5,1,0,0,"
        )
        .unwrap();
        writeln!(
            f,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb,\"Beta, Part 2\",200,0,0,0,0,0,"
        )
        .unwrap();
        writeln!(
            f,
            "cccccccccccccccccccccccccccccccccccccccc,Gamma,300,0,9,2,0,0,"
        )
        .unwrap();
        drop(f);
        let idx = DumpIndex::open(&path).unwrap();
        assert_eq!(idx.len(), 3);
        let a = idx
            .lookup(b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .unwrap();
        assert_eq!(a.name, "Alpha");
        assert_eq!(a.seeders, 5);
        let b = idx
            .lookup(b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
            .unwrap();
        assert_eq!(b.name, "Beta, Part 2");
        assert!(idx
            .lookup(b"dddddddddddddddddddddddddddddddddddddddd")
            .is_none());
    }

    fn tempfile_dir() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("webtor-dump-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
