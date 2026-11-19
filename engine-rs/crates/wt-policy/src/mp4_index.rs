//! Where an MP4/MOV file keeps its index (`moov`), read from the first bytes.
//!
//! A player cannot show the length or start playing until it has the whole
//! `moov` box. Files made for streaming put it first; many put it after the
//! media data (`mdat`), at the end of the file. The top-level box headers at
//! the start of the file say which, and exactly where, so the engine can fetch
//! the whole index first instead of guessing a fixed head and tail size.

/// Byte range (file-relative, end exclusive) that holds the `moov` index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mp4Index {
    pub start: i64,
    pub end: i64,
}

/// Top-level box types that can open an MP4/QuickTime file.
const OPENING_BOXES: [&[u8; 4]; 7] = [b"ftyp", b"moov", b"mdat", b"free", b"skip", b"wide", b"pnot"];

/// Find the `moov` index from the first bytes of a file of `file_len` bytes.
///
/// Walks the top-level boxes in `head`:
/// - `moov` found: its exact range (index at the start).
/// - `mdat` that ends before the file does: everything after it (index at the end).
///
/// `None` when the bytes are not an MP4, or the layout cannot be told from them.
pub fn locate_mp4_index(head: &[u8], file_len: i64) -> Option<Mp4Index> {
    if file_len <= 0 {
        return None;
    }
    let mut off: usize = 0;
    let mut first = true;
    while off + 8 <= head.len() {
        let size32 = u32::from_be_bytes(head[off..off + 4].try_into().ok()?);
        let kind: &[u8; 4] = head[off + 4..off + 8].try_into().ok()?;
        if first && !OPENING_BOXES.contains(&kind) {
            return None;
        }
        first = false;
        let size: i64 = match size32 {
            0 => file_len - off as i64,
            1 => {
                if off + 16 > head.len() {
                    return None;
                }
                i64::try_from(u64::from_be_bytes(head[off + 8..off + 16].try_into().ok()?)).ok()?
            }
            n => i64::from(n),
        };
        let start = off as i64;
        let end = start.checked_add(size)?;
        if size < 8 || end > file_len {
            return None;
        }
        match kind {
            b"moov" => return Some(Mp4Index { start, end }),
            b"mdat" if end < file_len => {
                // Whatever follows the media data holds the index.
                return Some(Mp4Index { start: end, end: file_len });
            }
            b"mdat" => return None, // media runs to the end and no moov came before it
            _ => {}
        }
        off = usize::try_from(end).ok()?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxed(kind: &[u8; 4], size: u32) -> Vec<u8> {
        let mut b = size.to_be_bytes().to_vec();
        b.extend_from_slice(kind);
        b
    }

    #[test]
    fn index_after_media_is_everything_after_mdat() {
        let mut head = boxed(b"ftyp", 32);
        head.resize(32, 0);
        head.extend(boxed(b"mdat", 1_000_000));
        let idx = locate_mp4_index(&head, 1_000_032 + 5_000_000).unwrap();
        assert_eq!(idx, Mp4Index { start: 1_000_032, end: 6_000_032 });
    }

    #[test]
    fn index_first_is_the_moov_range() {
        let mut head = boxed(b"ftyp", 24);
        head.resize(24, 0);
        head.extend(boxed(b"moov", 9_000_000));
        assert_eq!(locate_mp4_index(&head, 500_000_000), Some(Mp4Index { start: 24, end: 9_000_024 }));
    }

    #[test]
    fn large_mdat_uses_64_bit_size() {
        let mut head = boxed(b"ftyp", 16);
        head.resize(16, 0);
        head.extend(1u32.to_be_bytes());
        head.extend_from_slice(b"mdat");
        head.extend(5_000_000_000u64.to_be_bytes());
        let idx = locate_mp4_index(&head, 5_000_000_016 + 20_000_000).unwrap();
        assert_eq!(idx, Mp4Index { start: 5_000_000_016, end: 5_020_000_016 });
    }

    #[test]
    fn skips_free_boxes_before_the_index() {
        let mut head = boxed(b"ftyp", 16);
        head.resize(16, 0);
        head.extend(boxed(b"free", 8));
        head.extend(boxed(b"moov", 4_000));
        assert_eq!(locate_mp4_index(&head, 10_000), Some(Mp4Index { start: 24, end: 4_024 }));
    }

    #[test]
    fn not_mp4_or_unknown_layout_gives_none() {
        // Matroska starts with the EBML magic.
        assert_eq!(locate_mp4_index(&[0x1A, 0x45, 0xDF, 0xA3, 0, 0, 0, 0, 0, 0], 1 << 30), None);
        // mdat to the end of the file: no index after it.
        let mut head = boxed(b"ftyp", 16);
        head.resize(16, 0);
        head.extend(boxed(b"mdat", 0));
        assert_eq!(locate_mp4_index(&head, 1 << 20), None);
        // Box claims to run past the file.
        assert_eq!(locate_mp4_index(&boxed(b"ftyp", 4096), 100), None);
        // Header cut short before the layout is known.
        let mut short = boxed(b"ftyp", 16);
        short.resize(16, 0);
        assert_eq!(locate_mp4_index(&short, 1 << 20), None);
        assert_eq!(locate_mp4_index(&[], 1 << 20), None);
    }
}
