//! Keyframe byte offsets from a video's own index (MP4 `moov`, Matroska
//! `Cues`). Players seek to keyframes, so bytes fetched there ahead of time
//! are what makes a seek start without waiting on the network.

const READ_STEP: i64 = 4 << 20;
const MAX_INDEX_BYTES: i64 = 32 << 20;
/// Bytes fetched at each chosen keyframe: enough to decode the first frame.
pub const KEYFRAME_FETCH_BYTES: i64 = 512 << 10;

/// Verified bytes at a file offset; `None` while they are not downloaded.
pub type ReadRange<'a> = &'a dyn Fn(i64, i64) -> Option<Vec<u8>>;

/// What a video's own index says about it.
#[derive(Default)]
pub struct VideoIndex {
    pub offsets: Vec<i64>,
    /// Seconds, when the container states it.
    pub duration: Option<f64>,
    /// `(byte offset, seconds)` per keyframe, by offset; empty when the
    /// index has no times.
    pub keyframes: Vec<(i64, f64)>,
}

/// Where a thumbnail should come from: the keyframe nearest `fraction` of
/// the video, so its bytes can be fetched ahead and the frame asked for by
/// that exact time.
pub fn thumb_keyframe(index: &VideoIndex, fraction: f64) -> Option<(i64, f64)> {
    let last = index.keyframes.last()?.1;
    let span = index.duration.filter(|&d| d > 0.0).unwrap_or(last);
    let target = span * fraction.clamp(0.0, 1.0);
    index
        .keyframes
        .iter()
        .copied()
        .min_by(|a, b| (a.1 - target).abs().total_cmp(&(b.1 - target).abs()))
}

/// Pieces at most this big: spread the 3 thumbnails over the whole video
/// (a few MB extra). Bigger: take all 3 from the first one's piece, free.
pub const SPREAD_PIECE_MAX: i64 = 4 << 20;
/// Thumbnails closer than this in time look alike; skip them.
const THUMB_MIN_GAP_SECS: f64 = 4.0;

/// Up to three thumbnail keyframes `(byte offset in file, seconds)`, in time order.
///
/// ```text
///  small pieces:  27% ......... 50% ......... 73%     (one piece each)
///  big pieces:    [ first . middle . last ]           (all in the 27% piece)
/// ```
pub fn thumb_keyframe_set(index: &VideoIndex, file_offset: i64, file_length: i64, piece_length: i64) -> Vec<(i64, f64)> {
    let Some(first) = thumb_keyframe(index, 0.27) else { return Vec::new() };
    let mut picked: Vec<(i64, f64)> = if piece_length > 0 && piece_length <= SPREAD_PIECE_MAX {
        [0.27, 0.5, 0.73].iter().filter_map(|&f| thumb_keyframe(index, f)).collect()
    } else if piece_length > 0 {
        // Keyframes whose fetch window lies inside the first one's pieces.
        let window = |at: i64| {
            let start = file_offset + at;
            let end = (start + KEYFRAME_FETCH_BYTES).min(file_offset + file_length);
            (start / piece_length, (end - 1) / piece_length)
        };
        let (lo, hi) = window(first.0);
        let mut inside: Vec<(i64, f64)> = index.keyframes.iter().copied()
            .filter(|&(at, _)| at >= 0 && at < file_length && { let (a, b) = window(at); a >= lo && b <= hi })
            .collect();
        inside.sort_by(|a, b| a.1.total_cmp(&b.1));
        match (inside.first().copied(), inside.last().copied()) {
            (Some(early), Some(late)) => {
                let mid_t = (early.1 + late.1) / 2.0;
                let mid = inside.iter().copied().min_by(|a, b| (a.1 - mid_t).abs().total_cmp(&(b.1 - mid_t).abs()));
                std::iter::once(early).chain(mid).chain(std::iter::once(late)).collect()
            }
            _ => vec![first],
        }
    } else {
        vec![first]
    };
    picked.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut out: Vec<(i64, f64)> = Vec::with_capacity(3);
    for k in picked {
        if out.last().map_or(true, |last: &(i64, f64)| k.1 - last.1 >= THUMB_MIN_GAP_SECS) {
            out.push(k);
        }
    }
    if out.is_empty() {
        out.push(first);
    }
    out
}

/// `None`: the index is not downloaded yet, try again. `Some(empty)`: this
/// file has no index we can use.
pub fn video_index(read: ReadRange<'_>, length: i64) -> Option<VideoIndex> {
    let head = read(0, 12.min(length))?;
    if head.get(4..8) == Some(b"ftyp") {
        mp4_offsets(read, length)
    } else if head.get(..4) == Some(&[0x1A, 0x45, 0xDF, 0xA3]) {
        mkv_offsets(read, length)
    } else {
        Some(VideoIndex::default())
    }
}

/// Indexes `0..n` so that any prefix is spread evenly: middle, quarters, eighths.
fn coarse_to_fine(n: usize) -> impl Iterator<Item = usize> {
    let bits = n.next_power_of_two().trailing_zeros();
    (0..1usize << bits)
        .map(move |i| if bits == 0 { 0 } else { i.reverse_bits() >> (usize::BITS - bits) })
        .filter(move |&i| i < n)
}

/// Pieces covering the start of evenly chosen keyframes, within `max_bytes`,
/// in the order to fetch them: a seek lands near one early, wherever it goes.
pub fn pieces_for_keyframes(
    offsets: &[i64],
    torrent_offset: i64,
    piece_length: i64,
    begin_piece: u32,
    end_piece: u32,
    max_bytes: i64,
) -> Vec<u32> {
    if offsets.is_empty() || piece_length <= 0 {
        return Vec::new();
    }
    let per_point = KEYFRAME_FETCH_BYTES.max(piece_length);
    let points = ((max_bytes / per_point) as usize).min(offsets.len());
    if points == 0 {
        return Vec::new();
    }
    let max_pieces = (max_bytes / piece_length) as usize;
    let mut pieces = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for point in coarse_to_fine(points) {
        let abs = torrent_offset + offsets[point * offsets.len() / points];
        let first = abs / piece_length;
        let last = (abs + KEYFRAME_FETCH_BYTES - 1) / piece_length;
        for piece in first..=last {
            let piece = piece as u32;
            if piece >= begin_piece && piece < end_piece && pieces.len() < max_pieces && seen.insert(piece) {
                pieces.push(piece);
            }
        }
    }
    pieces
}

fn read_long(read: ReadRange<'_>, abs: i64, len: i64) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(len as usize);
    let mut done = 0;
    while done < len {
        let step = READ_STEP.min(len - done);
        out.extend(read(abs + done, step)?);
        done += step;
    }
    Some(out)
}

fn be32(buf: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(buf.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

fn be64(buf: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(buf.get(at..at.checked_add(8)?)?.try_into().ok()?))
}

// ------------------------------------------------------------------- MP4

/// `(box size, header size)`.
fn mp4_box_header(head: &[u8], available: i64) -> Option<(i64, i64)> {
    match be32(head, 0)? {
        0 => Some((available, 8)),
        1 => Some((i64::try_from(be64(head, 8)?).ok()?, 16)),
        size => Some((i64::from(size), 8)),
    }
}

fn mp4_offsets(read: ReadRange<'_>, length: i64) -> Option<VideoIndex> {
    let mut off = 0;
    while off + 8 <= length {
        let head = read(off, 16.min(length - off))?;
        let Some((size, header)) = mp4_box_header(&head, length - off) else {
            return Some(VideoIndex::default());
        };
        if size < header || off.checked_add(size).is_none_or(|end| end > length) {
            return Some(VideoIndex::default());
        }
        if head.get(4..8) == Some(b"moov") {
            if size - header > MAX_INDEX_BYTES {
                return Some(VideoIndex::default());
            }
            let moov = read_long(read, off + header, size - header)?;
            let keyframes = moov_keyframes(&moov).unwrap_or_default();
            return Some(VideoIndex {
                offsets: keyframes.iter().map(|k| k.0).collect(),
                duration: mp4_duration(&moov),
                keyframes: keyframes.into_iter().filter_map(|(o, t)| Some((o, t?))).collect(),
            });
        }
        off += size;
    }
    Some(VideoIndex::default())
}

/// Child boxes of `buf` as `(type, body)`.
fn mp4_children(buf: &[u8]) -> Vec<(&[u8], &[u8])> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while let Some(head) = buf.get(at..) {
        let Some((size, header)) = mp4_box_header(head, head.len() as i64) else {
            break;
        };
        let (Ok(size), Ok(header)) = (usize::try_from(size), usize::try_from(header)) else {
            break;
        };
        if size < header {
            break;
        }
        let (Some(kind), Some(body)) = (head.get(4..8), head.get(header..size)) else {
            break;
        };
        out.push((kind, body));
        at += size;
    }
    out
}

fn mp4_child<'a>(buf: &'a [u8], kind: &[u8]) -> Option<&'a [u8]> {
    mp4_children(buf).into_iter().find(|(k, _)| *k == kind).map(|(_, body)| body)
}

/// Seconds from `mvhd`.
fn mp4_duration(moov: &[u8]) -> Option<f64> {
    let mvhd = mp4_child(moov, b"mvhd")?;
    let (timescale, duration) = if *mvhd.first()? == 1 {
        (f64::from(be32(mvhd, 20)?), be64(mvhd, 24)? as f64)
    } else {
        (f64::from(be32(mvhd, 12)?), f64::from(be32(mvhd, 16)?))
    };
    (timescale > 0.0 && duration > 0.0).then(|| duration / timescale)
}

/// `(offset, seconds)` per video keyframe; seconds are `None` without a
/// usable `stts`/`mdhd`.
fn moov_keyframes(moov: &[u8]) -> Option<Vec<(i64, Option<f64>)>> {
    for (kind, trak) in mp4_children(moov) {
        if kind != b"trak" {
            continue;
        }
        let Some(mdia) = mp4_child(trak, b"mdia") else {
            continue;
        };
        if mp4_child(mdia, b"hdlr").and_then(|h| h.get(8..12)) != Some(b"vide") {
            continue;
        }
        let stbl = mp4_child(mp4_child(mdia, b"minf")?, b"stbl")?;
        let timescale = mp4_child(mdia, b"mdhd").and_then(|m| {
            let at = if *m.first()? == 1 { 20 } else { 12 };
            be32(m, at).filter(|&t| t > 0)
        });
        let keyframes = stbl_keyframes(stbl)?;
        let times = timescale.and_then(|ts| {
            let samples: Vec<u64> = keyframes.iter().map(|k| k.1).collect();
            Some(stts_times(mp4_child(stbl, b"stts")?, &samples)?
                .into_iter()
                .map(|t| t as f64 / f64::from(ts))
                .collect::<Vec<_>>())
        });
        return Some(keyframes
            .iter()
            .enumerate()
            .map(|(i, k)| (k.0, times.as_ref().map(|t| t[i])))
            .collect());
    }
    None
}

/// Decode time (in track timescale units) of each 1-based `samples` entry,
/// which must be ascending.
fn stts_times(stts: &[u8], samples: &[u64]) -> Option<Vec<u64>> {
    let entries = be32(stts, 4)? as usize;
    let mut out = Vec::with_capacity(samples.len());
    let (mut first, mut time, mut want) = (1u64, 0u64, samples.iter().peekable());
    for e in 0..entries {
        let count = u64::from(be32(stts, 8 + e * 8)?);
        let delta = u64::from(be32(stts, 12 + e * 8)?);
        while let Some(&&s) = want.peek() {
            if s >= first + count {
                break;
            }
            out.push(time + (s - first) * delta);
            want.next();
        }
        first += count;
        time += count * delta;
    }
    (out.len() == samples.len()).then_some(out)
}

/// `(offset, 1-based sample number)` per sync sample.
fn stbl_keyframes(stbl: &[u8]) -> Option<Vec<(i64, u64)>> {
    // No sync table means every sample is a keyframe: nothing to choose from.
    let stss = mp4_child(stbl, b"stss")?;
    let stsc = mp4_child(stbl, b"stsc")?;
    let stsz = mp4_child(stbl, b"stsz")?;
    let chunk_offset = |i: usize| -> Option<i64> {
        if let Some(stco) = mp4_child(stbl, b"stco") {
            Some(i64::from(be32(stco, 8 + i * 4)?))
        } else {
            i64::try_from(be64(mp4_child(stbl, b"co64")?, 8 + i * 8)?).ok()
        }
    };
    let chunks = if let Some(stco) = mp4_child(stbl, b"stco") {
        be32(stco, 4)? as usize
    } else {
        be32(mp4_child(stbl, b"co64")?, 4)? as usize
    };
    let sync_count = be32(stss, 4)? as usize;
    let fixed_size = be32(stsz, 4)?;
    let run_count = be32(stsc, 4)? as usize;

    let mut out = Vec::with_capacity(sync_count.min(1 << 20));
    let mut next_sync = 0usize;
    let mut sample = 0u64;
    let mut run = 0usize;
    for chunk in 0..chunks {
        while run + 1 < run_count
            && u64::from(be32(stsc, 8 + (run + 1) * 12)?) <= chunk as u64 + 1
        {
            run += 1;
        }
        let in_chunk = be32(stsc, 8 + run * 12 + 4)?;
        let mut at = chunk_offset(chunk)?;
        for _ in 0..in_chunk {
            sample += 1;
            let Some(sync) = (next_sync < sync_count).then(|| be32(stss, 8 + next_sync * 4)).flatten() else {
                return Some(out);
            };
            if u64::from(sync) == sample {
                out.push((at, sample));
                next_sync += 1;
            }
            let size = if fixed_size != 0 {
                fixed_size
            } else {
                be32(stsz, 12 + (sample as usize - 1) * 4)?
            };
            at += i64::from(size);
        }
    }
    Some(out)
}

// ------------------------------------------------------------------ Matroska

const MKV_SEGMENT: u64 = 0x1853_8067;
const MKV_SEEK_HEAD: u64 = 0x114D_9B74;
const MKV_INFO: u64 = 0x1549_A966;
const MKV_TIMECODE_SCALE: u64 = 0x2A_D7B1;
const MKV_DURATION: u64 = 0x4489;
const MKV_SEEK: u64 = 0x4DBB;
const MKV_SEEK_ID: u64 = 0x53AB;
const MKV_SEEK_POSITION: u64 = 0x53AC;
const MKV_CUES: u64 = 0x1C53_BB6B;
const MKV_CLUSTER: u64 = 0x1F43_B675;
const MKV_CUE_POINT: u64 = 0xBB;
const MKV_CUE_TIME: u64 = 0xB3;
const MKV_CUE_TRACK_POSITIONS: u64 = 0xB7;
const MKV_CUE_CLUSTER_POSITION: u64 = 0xF1;
const MKV_CUE_RELATIVE_POSITION: u64 = 0xF0;
const MKV_HEAD_BYTES: i64 = 1 << 20;

/// An element ID, marker bits kept: `(id, bytes used)`.
fn ebml_id(buf: &[u8], at: usize) -> Option<(u64, usize)> {
    let first = *buf.get(at)?;
    let len = first.leading_zeros() as usize + 1;
    if len > 4 {
        return None;
    }
    let bytes = buf.get(at..at + len)?;
    Some((bytes.iter().fold(0u64, |v, b| v << 8 | u64::from(*b)), len))
}

/// A size, marker bit removed: `(size, bytes used)`; `None` size if unknown.
fn ebml_size(buf: &[u8], at: usize) -> Option<(Option<u64>, usize)> {
    let first = *buf.get(at)?;
    let len = first.leading_zeros() as usize + 1;
    if len > 8 {
        return None;
    }
    let bytes = buf.get(at..at + len)?;
    let mask = if len == 8 { 0 } else { 0xFFu8 >> len };
    let value = bytes[1..].iter().fold(u64::from(first & mask), |v, b| v << 8 | u64::from(*b));
    let unknown = value == (1u64 << (7 * len)) - 1;
    Some(((!unknown).then_some(value), len))
}

/// `(id, body start, body end)`; an unknown size runs to the end of `buf`.
fn ebml_element(buf: &[u8], at: usize) -> Option<(u64, usize, usize)> {
    let (id, id_len) = ebml_id(buf, at)?;
    let (size, size_len) = ebml_size(buf, at + id_len)?;
    let start = at + id_len + size_len;
    let end = match size {
        Some(size) => start.checked_add(usize::try_from(size).ok()?)?,
        None => buf.len(),
    };
    Some((id, start, end))
}

fn ebml_uint(body: &[u8]) -> Option<u64> {
    (body.len() <= 8).then(|| body.iter().fold(0u64, |v, b| v << 8 | u64::from(*b)))
}

fn mkv_offsets(read: ReadRange<'_>, length: i64) -> Option<VideoIndex> {
    let head = read(0, MKV_HEAD_BYTES.min(length))?;
    let Some((_, _, header_end)) = ebml_element(&head, 0) else {
        return Some(VideoIndex::default());
    };
    let Some((MKV_SEGMENT, segment, _)) = ebml_element(&head, header_end) else {
        return Some(VideoIndex::default());
    };
    let Some(cues_at) = mkv_cues_position(&head, segment) else {
        return Some(VideoIndex::default());
    };
    let cues_abs = segment as i64 + cues_at;
    if cues_abs >= length {
        return Some(VideoIndex::default());
    }
    let cues_head = read(cues_abs, 12.min(length - cues_abs))?;
    let Some((MKV_CUES, start, end)) = ebml_element(&cues_head, 0) else {
        return Some(VideoIndex::default());
    };
    let size = (end - start) as i64;
    if size > MAX_INDEX_BYTES || cues_abs + start as i64 + size > length {
        return Some(VideoIndex::default());
    }
    let cues = read_long(read, cues_abs + start as i64, size)?;
    let points = mkv_cue_offsets(&cues, segment as i64);
    let scale = mkv_timecode_scale(&head, segment) as f64 / 1e9;
    Some(VideoIndex {
        offsets: points.iter().map(|p| p.0).collect(),
        duration: mkv_duration(&head, segment),
        keyframes: points.into_iter().filter_map(|(o, t)| Some((o, t? as f64 * scale))).collect(),
    })
}

/// Nanoseconds per Matroska tick (`TimecodeScale`, default 1 ms).
fn mkv_timecode_scale(head: &[u8], segment: usize) -> u64 {
    let mut at = segment;
    while let Some((id, start, end)) = ebml_element(head, at) {
        if id == MKV_INFO {
            let info = &head[start..end];
            let mut inner = 0;
            while let Some((id, start, end)) = ebml_element(info, inner) {
                if id == MKV_TIMECODE_SCALE {
                    return ebml_uint(&info[start..end]).filter(|&s| s > 0).unwrap_or(1_000_000);
                }
                inner = end;
            }
            break;
        }
        at = end;
    }
    1_000_000
}

/// Seconds from the Segment's `Info`, which sits near its start.
fn mkv_duration(head: &[u8], segment: usize) -> Option<f64> {
    let mut at = segment;
    while let Some((id, start, end)) = ebml_element(head, at) {
        if id == MKV_INFO {
            let info = &head[start..end];
            let (mut scale, mut duration, mut inner) = (1_000_000u64, None, 0);
            while let Some((id, start, end)) = ebml_element(info, inner) {
                let body = &info[start..end];
                if id == MKV_TIMECODE_SCALE {
                    scale = ebml_uint(body)?;
                } else if id == MKV_DURATION {
                    duration = match body.len() {
                        4 => Some(f64::from(f32::from_be_bytes(body.try_into().ok()?))),
                        8 => Some(f64::from_be_bytes(body.try_into().ok()?)),
                        _ => None,
                    };
                }
                inner = end;
            }
            return duration.map(|d| d * scale as f64 / 1e9).filter(|&d| d > 0.0);
        }
        at = end;
    }
    None
}

/// Cues position relative to the segment, from the elements at its start.
fn mkv_cues_position(head: &[u8], segment: usize) -> Option<i64> {
    let mut at = segment;
    while let Some((id, start, end)) = ebml_element(head, at) {
        match id {
            MKV_CUES => return Some((at - segment) as i64),
            MKV_CLUSTER => return None,
            MKV_SEEK_HEAD => {
                if let Some(position) = mkv_seek_head_cues(head.get(start..end)?) {
                    return Some(position);
                }
            }
            _ => {}
        }
        at = end;
    }
    None
}

fn mkv_seek_head_cues(seek_head: &[u8]) -> Option<i64> {
    let mut at = 0;
    while let Some((id, start, end)) = ebml_element(seek_head, at) {
        if id == MKV_SEEK {
            let seek = seek_head.get(start..end)?;
            let (mut target, mut position, mut inner) = (None, None, 0);
            while let Some((id, start, end)) = ebml_element(seek, inner) {
                let body = seek.get(start..end)?;
                match id {
                    MKV_SEEK_ID => target = ebml_uint(body),
                    MKV_SEEK_POSITION => position = ebml_uint(body),
                    _ => {}
                }
                inner = end;
            }
            if target == Some(MKV_CUES) {
                return i64::try_from(position?).ok();
            }
        }
        at = end;
    }
    None
}

/// `(absolute offset, CueTime ticks)` per cue point, by offset.
fn mkv_cue_offsets(cues: &[u8], segment: i64) -> Vec<(i64, Option<u64>)> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some((id, start, end)) = ebml_element(cues, at) {
        if id == MKV_CUE_POINT {
            if let Some((offset, time)) = cues.get(start..end).and_then(mkv_cue_point) {
                out.push((segment + offset, time));
            }
        }
        at = end;
    }
    out.sort_unstable_by_key(|p| p.0);
    out.dedup_by_key(|p| p.0);
    out
}

fn mkv_cue_point(point: &[u8]) -> Option<(i64, Option<u64>)> {
    let time = mkv_cue_time(point);
    mkv_cue_position(point).map(|o| (o, time))
}

fn mkv_cue_time(point: &[u8]) -> Option<u64> {
    let mut at = 0;
    while let Some((id, start, end)) = ebml_element(point, at) {
        if id == MKV_CUE_TIME {
            return ebml_uint(point.get(start..end)?);
        }
        at = end;
    }
    None
}

fn mkv_cue_position(point: &[u8]) -> Option<i64> {
    let mut at = 0;
    while let Some((id, start, end)) = ebml_element(point, at) {
        if id == MKV_CUE_TRACK_POSITIONS {
            let track = point.get(start..end)?;
            let (mut cluster, mut relative, mut inner) = (None, 0, 0);
            while let Some((id, start, end)) = ebml_element(track, inner) {
                let body = track.get(start..end)?;
                match id {
                    MKV_CUE_CLUSTER_POSITION => cluster = ebml_uint(body),
                    MKV_CUE_RELATIVE_POSITION => relative = ebml_uint(body).unwrap_or(0),
                    _ => {}
                }
                inner = end;
            }
            return i64::try_from(cluster?.checked_add(relative)?).ok();
        }
        at = end;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 100 min video, 1 MB/s, a keyframe every 2 s.
    fn movie() -> VideoIndex {
        let keyframes: Vec<(i64, f64)> = (0..3000).map(|i| (i as i64 * 2_000_000, i as f64 * 2.0)).collect();
        VideoIndex { offsets: keyframes.iter().map(|k| k.0).collect(), duration: Some(6000.0), keyframes }
    }

    #[test]
    fn small_pieces_spread_three_thumbs_over_the_video() {
        let set = thumb_keyframe_set(&movie(), 0, 6_000_000_000, 1 << 20);
        let secs: Vec<f64> = set.iter().map(|k| k.1).collect();
        assert_eq!(secs, vec![1620.0, 3000.0, 4380.0]);
    }

    #[test]
    fn big_pieces_take_three_thumbs_from_one_piece() {
        let piece = 16i64 << 20;
        let set = thumb_keyframe_set(&movie(), 0, 6_000_000_000, piece);
        assert_eq!(set.len(), 3);
        let first_piece = set[0].0 / piece;
        for (at, _) in &set {
            assert_eq!(at / piece, first_piece);
            assert_eq!((at + KEYFRAME_FETCH_BYTES - 1) / piece, first_piece);
        }
        assert!(set[2].1 - set[0].1 >= 8.0);
    }

    #[test]
    fn close_keyframes_collapse_to_one() {
        let index = VideoIndex { offsets: vec![0], duration: Some(10.0), keyframes: vec![(0, 0.0), (100, 1.0)] };
        assert_eq!(thumb_keyframe_set(&index, 0, 1000, 16 << 20).len(), 1);
    }

    fn read_from(data: &[u8]) -> impl Fn(i64, i64) -> Option<Vec<u8>> + '_ {
        move |at, len| {
            let (at, len) = (at as usize, len as usize);
            data.get(at..at + len).map(<[u8]>::to_vec)
        }
    }

    fn ebml(id: u64, body: &[u8]) -> Vec<u8> {
        let mut out: Vec<u8> = id.to_be_bytes().into_iter().skip_while(|&b| b == 0).collect();
        out.push(0x01);
        out.extend_from_slice(&(body.len() as u64).to_be_bytes()[1..]);
        out.extend_from_slice(body);
        out
    }

    fn uint(id: u64, v: u64) -> Vec<u8> {
        ebml(id, &v.to_be_bytes())
    }

    fn mkv(cues: &[(u64, u64)]) -> Vec<u8> {
        let info = [uint(MKV_TIMECODE_SCALE, 1_000_000), ebml(MKV_DURATION, &100_000f64.to_be_bytes())].concat();
        let points: Vec<u8> = cues
            .iter()
            .flat_map(|&(time, pos)| {
                ebml(MKV_CUE_POINT, &[uint(MKV_CUE_TIME, time), ebml(MKV_CUE_TRACK_POSITIONS, &[uint(0xF7, 1), uint(MKV_CUE_CLUSTER_POSITION, pos)].concat())].concat())
            })
            .collect();
        let segment = [ebml(MKV_INFO, &info), ebml(MKV_CUES, &points), vec![0; 4096]].concat();
        [ebml(0x1A45_DFA3, &[]), ebml(MKV_SEGMENT, &segment)].concat()
    }

    #[test]
    fn mkv_cues_give_keyframe_times() {
        let file = mkv(&[(0, 1000), (25_000, 2000), (50_000, 3000)]);
        let read = read_from(&file);
        let index = video_index(&read, file.len() as i64).unwrap();
        let segment = (4 + 8) + (4 + 8); // EBML header element, then Segment id + size
        assert_eq!(index.duration, Some(100.0));
        assert_eq!(index.keyframes, vec![(segment + 1000, 0.0), (segment + 2000, 25.0), (segment + 3000, 50.0)]);
        assert_eq!(thumb_keyframe(&index, 0.25), Some((segment + 2000, 25.0)));
        assert_eq!(thumb_keyframe(&index, 0.45), Some((segment + 3000, 50.0)));
    }

    fn mp4_box(kind: &[u8], body: &[u8]) -> Vec<u8> {
        [&((body.len() + 8) as u32).to_be_bytes()[..], kind, body].concat()
    }

    fn words(w: &[u32]) -> Vec<u8> {
        w.iter().flat_map(|v| v.to_be_bytes()).collect()
    }

    #[test]
    fn mp4_stts_gives_keyframe_times() {
        let stbl = [
            mp4_box(b"stss", &words(&[0, 2, 1, 6])),
            mp4_box(b"stsc", &words(&[0, 1, 1, 10, 1])),
            mp4_box(b"stsz", &words(&[0, 1000, 10])),
            mp4_box(b"stco", &words(&[0, 1, 5000])),
            mp4_box(b"stts", &words(&[0, 1, 10, 9000])),
        ]
        .concat();
        let mdia = [
            mp4_box(b"mdhd", &words(&[0, 0, 0, 90_000, 900_000, 0])),
            mp4_box(b"hdlr", &[words(&[0, 0]), b"vide".to_vec(), vec![0; 13]].concat()),
            mp4_box(b"minf", &mp4_box(b"stbl", &stbl)),
        ]
        .concat();
        let moov = [mp4_box(b"mvhd", &words(&[0, 0, 0, 1000, 1000, 0])), mp4_box(b"trak", &mp4_box(b"mdia", &mdia))].concat();
        let file = [mp4_box(b"ftyp", b"isom\0\0\0\0"), mp4_box(b"moov", &moov), vec![0; 20_000]].concat();
        let read = read_from(&file);
        let index = video_index(&read, file.len() as i64).unwrap();
        assert_eq!(index.offsets, vec![5000, 10_000]);
        assert_eq!(index.keyframes, vec![(5000, 0.0), (10_000, 0.5)]);
        assert_eq!(thumb_keyframe(&index, 0.4), Some((10_000, 0.5)));
    }

    #[test]
    fn no_times_means_no_thumb_keyframe() {
        assert_eq!(thumb_keyframe(&VideoIndex::default(), 0.25), None);
    }
}
