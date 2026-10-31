//! # Magnet metadata / BEP 9 ut_metadata
use bytes::Bytes;
use rand::seq::SliceRandom;
use thiserror::Error;
use wt_meta::{info_hash_v1, info_hash_v2, parse_info};
use wt_types::{Info, InfoHash, InfoHashV2};
pub use wt_wire::ext::METADATA_PIECE_SIZE;

/// Go `maxMetadataSize` (16 MiB).
pub const MAX_METADATA_SIZE: u32 = 16 * 1024 * 1024;
/// Default per-piece request timeout before clearing and retrying on another
/// (or the same) peer — anacrolix retries on reject/timeout.
pub const METADATA_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

pub use wt_wire::ext::METADATA_PIECE_SIZE as PIECE_LEN;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MetadataError {
    #[error("metadata size {0} exceeds max {MAX_METADATA_SIZE}")]
    SizeTooLarge(u32),
    #[error("metadata size is zero")]
    SizeZero,
    #[error("unexpected piece index {0}")]
    UnexpectedPiece(u32),
    #[error("piece {piece} has wrong length: got {got}, want {want}")]
    BadPieceLen { piece: u32, got: usize, want: usize },
    #[error("incorrect v1 infohash")]
    IncorrectV1,
    #[error("incorrect v2 infohash")]
    IncorrectV2,
    #[error("invalid v2 info")]
    InvalidV2Info,
    #[error("no expected infohashes")]
    NoExpectedHash,
    #[error("bencode/info parse: {0}")]
    Parse(String),
}

/// Result of verifying assembled bytes against expected hashes.
#[derive(Debug, Clone)]
pub struct VerifiedMetadata {
    pub info: Info,
    pub info_bytes: Vec<u8>,
    pub info_hash_v1: InfoHash,
    pub info_hash_v2: InfoHashV2,
}

/// Serve response for an inbound metadata request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Serve {
    Data(Bytes),
    Reject,
}

/// Torrent-level ut_metadata assembly state.
#[derive(Debug, Clone)]
pub struct MetadataExchange {
    expected_v1: Option<InfoHash>,
    expected_v2: Option<InfoHashV2>,
    bytes: Vec<u8>,
    completed: Vec<bool>,
    /// Set once verification succeeds (or constructed from known info bytes).
    verified: Option<VerifiedMetadata>,
}

impl MetadataExchange {
    /// Start from magnet hashes (one or both required).
    pub fn new(expected_v1: Option<InfoHash>, expected_v2: Option<InfoHashV2>) -> Self {
        Self {
            expected_v1,
            expected_v2,
            bytes: Vec::new(),
            completed: Vec::new(),
            verified: None,
        }
    }

    /// Already have the info dict bytes (e.g. from a .torrent or cache).
    pub fn from_info_bytes(
        info_bytes: Vec<u8>,
        expected_v1: Option<InfoHash>,
        expected_v2: Option<InfoHashV2>,
    ) -> Result<Self, MetadataError> {
        let mut me = Self::new(expected_v1, expected_v2);
        me.bytes = info_bytes;
        me.completed.clear();
        let verified = me.verify_bytes()?;
        me.verified = Some(verified);
        Ok(me)
    }

    pub fn has_info(&self) -> bool {
        self.verified.is_some()
    }

    pub fn verified(&self) -> Option<&VerifiedMetadata> {
        self.verified.as_ref()
    }

    pub fn info_bytes(&self) -> Option<&[u8]> {
        if self.has_info() {
            Some(&self.bytes)
        } else {
            None
        }
    }

    pub fn size(&self) -> Option<u32> {
        if self.bytes.is_empty() && self.verified.is_none() {
            None
        } else {
            Some(self.bytes.len() as u32)
        }
    }

    pub fn piece_count(&self) -> usize {
        if self.bytes.is_empty() {
            0
        } else {
            self.bytes.len().div_ceil(METADATA_PIECE_SIZE)
        }
    }

    pub fn piece_size(total: usize, piece: u32) -> usize {
        let piece = piece as usize;
        let start = piece * METADATA_PIECE_SIZE;
        if start >= total {
            return 0;
        }
        (total - start).min(METADATA_PIECE_SIZE)
    }

    /// Negotiate total size from an extended handshake / peer advertisement.
    pub fn set_size(&mut self, size: u32) -> Result<bool, MetadataError> {
        if self.has_info() {
            return Ok(false);
        }
        if size == 0 {
            return Err(MetadataError::SizeZero);
        }
        if size > MAX_METADATA_SIZE {
            return Err(MetadataError::SizeTooLarge(size));
        }
        if self.bytes.len() == size as usize {
            return Ok(false);
        }
        self.bytes = vec![0u8; size as usize];
        let n = (size as usize).div_ceil(METADATA_PIECE_SIZE);
        self.completed = vec![false; n];
        Ok(true)
    }

    pub fn have_piece(&self, piece: u32) -> bool {
        let piece = piece as usize;
        if self.has_info() {
            piece * METADATA_PIECE_SIZE < self.bytes.len()
        } else {
            piece < self.completed.len() && self.completed[piece]
        }
    }

    pub fn have_all_pieces(&self) -> bool {
        if self.has_info() {
            return true;
        }
        !self.completed.is_empty() && self.completed.iter().all(|&c| c)
    }

    /// Pieces we still need, optionally excluding those already requested on a peer.
    pub fn pieces_to_request(&self, peer: &PeerMetadataState) -> Vec<u32> {
        if self.has_info() || self.completed.is_empty() {
            return Vec::new();
        }
        let mut pending: Vec<u32> = (0..self.completed.len() as u32)
            .filter(|&i| !self.have_piece(i) && !peer.is_requested(i))
            .collect();
        pending.shuffle(&mut rand::thread_rng());
        pending
    }

    /// Save a data piece from a peer. Returns `Some(verified)` when assembly completes.
    pub fn on_data(
        &mut self,
        piece: u32,
        data: &[u8],
    ) -> Result<Option<VerifiedMetadata>, MetadataError> {
        if self.has_info() {
            return Ok(None);
        }
        let piece_usize = piece as usize;
        if piece_usize >= self.completed.len() {
            return Err(MetadataError::UnexpectedPiece(piece));
        }
        let want = Self::piece_size(self.bytes.len(), piece);
        if data.len() != want {
            return Err(MetadataError::BadPieceLen {
                piece,
                got: data.len(),
                want,
            });
        }
        let start = piece_usize * METADATA_PIECE_SIZE;
        self.bytes[start..start + want].copy_from_slice(data);
        self.completed[piece_usize] = true;

        if !self.have_all_pieces() {
            return Ok(None);
        }
        match self.verify_bytes() {
            Ok(v) => {
                self.verified = Some(v.clone());
                self.completed.clear();
                Ok(Some(v))
            }
            Err(e) => {
                self.invalidate();
                Err(e)
            }
        }
    }

    /// Serve a piece to a requesting peer.
    pub fn serve(&self, piece: u32) -> Serve {
        if !self.have_piece(piece) {
            return Serve::Reject;
        }
        let start = piece as usize * METADATA_PIECE_SIZE;
        let len = Self::piece_size(self.bytes.len(), piece);
        Serve::Data(Bytes::copy_from_slice(&self.bytes[start..start + len]))
    }

    fn invalidate(&mut self) {
        self.completed.fill(false);
        self.verified = None;
    }

    fn verify_bytes(&mut self) -> Result<VerifiedMetadata, MetadataError> {
        let info = parse_info(&self.bytes).map_err(|e| MetadataError::Parse(e.to_string()))?;
        let v1 = info_hash_v1(&self.bytes);
        let v2 = info_hash_v2(&self.bytes);

        match (&self.expected_v1, &self.expected_v2) {
            (Some(exp1), None) => {
                if v1 == *exp1 {
                    // ok; may discover v2
                } else if v2.to_short() == *exp1 {
                    // Magnet carried v2 short hash as v1 slot
                    if !info_has_v2(&info) {
                        return Err(MetadataError::InvalidV2Info);
                    }
                } else {
                    return Err(MetadataError::IncorrectV1);
                }
            }
            (Some(exp1), Some(exp2)) => {
                if v1 != *exp1 {
                    return Err(MetadataError::IncorrectV1);
                }
                if v2 != *exp2 {
                    return Err(MetadataError::IncorrectV2);
                }
            }
            (None, Some(exp2)) => {
                if v2 != *exp2 {
                    return Err(MetadataError::IncorrectV2);
                }
            }
            (None, None) => return Err(MetadataError::NoExpectedHash),
        }

        // Update expected hashes if we learned the other.
        if self.expected_v1.is_none() {
            self.expected_v1 = Some(v1);
        }
        if self.expected_v2.is_none() {
            self.expected_v2 = Some(v2);
        }

        Ok(VerifiedMetadata {
            info,
            info_bytes: self.bytes.clone(),
            info_hash_v1: v1,
            info_hash_v2: v2,
        })
    }
}

fn info_has_v2(info: &Info) -> bool {
    info.has_v2()
}

/// Per-peer outstanding metadata piece requests.
#[derive(Debug, Clone, Default)]
pub struct PeerMetadataState {
    requested: Vec<bool>,
    /// When each piece was last requested (for timeout retry).
    requested_at: Vec<Option<std::time::Instant>>,
}

impl PeerMetadataState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_requested(&self, piece: u32) -> bool {
        self.requested.get(piece as usize).copied().unwrap_or(false)
    }

    pub fn outstanding(&self) -> usize {
        self.requested.iter().filter(|&&r| r).count()
    }

    pub fn mark_requested(&mut self, piece: u32) {
        self.mark_requested_at(piece, std::time::Instant::now());
    }

    pub fn mark_requested_at(&mut self, piece: u32, now: std::time::Instant) {
        let i = piece as usize;
        if i >= self.requested.len() {
            self.requested.resize(i + 1, false);
            self.requested_at.resize(i + 1, None);
        }
        self.requested[i] = true;
        self.requested_at[i] = Some(now);
    }

    pub fn clear_requested(&mut self, piece: u32) {
        if let Some(slot) = self.requested.get_mut(piece as usize) {
            *slot = false;
        }
        if let Some(slot) = self.requested_at.get_mut(piece as usize) {
            *slot = None;
        }
    }

    /// Clear pieces whose request is older than `timeout` so they can be
    /// re-requested (Go ut_metadata reject/timeout retry).
    pub fn expire_timed_out(
        &mut self,
        now: std::time::Instant,
        timeout: std::time::Duration,
    ) -> Vec<u32> {
        let mut expired = Vec::new();
        for i in 0..self.requested.len() {
            if !self.requested[i] {
                continue;
            }
            let Some(at) = self.requested_at.get(i).copied().flatten() else {
                continue;
            };
            if now.saturating_duration_since(at) >= timeout {
                self.requested[i] = false;
                if let Some(slot) = self.requested_at.get_mut(i) {
                    *slot = None;
                }
                expired.push(i as u32);
            }
        }
        expired
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use wt_meta::{info_hash_v1, parse_metainfo};

    fn testdata(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../wt-meta/testdata")
            .join(name)
    }

    #[test]
    fn set_size_and_piece_math() {
        let mut m = MetadataExchange::new(Some(InfoHash([0; 20])), None);
        assert!(m.set_size(20_000).unwrap());
        assert_eq!(m.piece_count(), 2);
        assert_eq!(MetadataExchange::piece_size(20_000, 0), METADATA_PIECE_SIZE);
        assert_eq!(
            MetadataExchange::piece_size(20_000, 1),
            20_000 - METADATA_PIECE_SIZE
        );
        assert!(matches!(
            m.set_size(MAX_METADATA_SIZE + 1),
            Err(MetadataError::SizeTooLarge(_))
        ));
    }

    #[test]
    fn assemble_from_three_peers_one_corrupt() {
        let raw = std::fs::read(testdata("sintel.torrent")).expect("sintel fixture");
        let mi = parse_metainfo(&raw).expect("parse");
        let info_bytes = mi.info_bytes.clone();
        let ih = info_hash_v1(&info_bytes);
        assert!(
            info_bytes.len() > METADATA_PIECE_SIZE,
            "need multi-piece info"
        );

        let n = info_bytes.len().div_ceil(METADATA_PIECE_SIZE);
        let pieces: Vec<Vec<u8>> = (0..n)
            .map(|i| {
                let start = i * METADATA_PIECE_SIZE;
                let len = MetadataExchange::piece_size(info_bytes.len(), i as u32);
                info_bytes[start..start + len].to_vec()
            })
            .collect();
        let mut corrupt0 = pieces[0].clone();
        corrupt0[0] ^= 0xff;

        // Round 1: peer 0 sends corrupt piece 0; peers 1.. serve the rest → verify fails.
        let mut meta = MetadataExchange::new(Some(ih), None);
        meta.set_size(info_bytes.len() as u32).unwrap();
        let mut peers = [
            PeerMetadataState::new(),
            PeerMetadataState::new(),
            PeerMetadataState::new(),
        ];
        peers[0].mark_requested(0);
        assert!(meta.on_data(0, &corrupt0).unwrap().is_none());
        peers[0].clear_requested(0);
        for p in 1..n as u32 {
            let peer = &mut peers[(p as usize % 2) + 1];
            peer.mark_requested(p);
            let r = meta.on_data(p, &pieces[p as usize]);
            peer.clear_requested(p);
            if p + 1 == n as u32 {
                assert!(
                    matches!(
                        r,
                        Err(MetadataError::IncorrectV1) | Err(MetadataError::Parse(_))
                    ),
                    "expected verify failure, got {r:?}"
                );
            } else {
                assert!(r.unwrap().is_none());
            }
        }
        assert!(!meta.has_info());

        // Round 2: three peers serve good pieces → complete.
        let mut meta = MetadataExchange::new(Some(ih), None);
        meta.set_size(info_bytes.len() as u32).unwrap();
        let mut done = None;
        for (i, piece) in pieces.iter().enumerate() {
            let peer_i = i % 3;
            peers[peer_i].mark_requested(i as u32);
            if let Some(v) = meta.on_data(i as u32, piece).unwrap() {
                done = Some(v);
            }
            peers[peer_i].clear_requested(i as u32);
        }
        let v = done.expect("completed");
        assert_eq!(v.info_hash_v1, ih);
        assert_eq!(v.info_bytes, info_bytes);
        match meta.serve(0) {
            Serve::Data(d) => assert_eq!(&d[..], &pieces[0][..]),
            Serve::Reject => panic!("should serve"),
        }
    }

    #[test]
    fn wrong_hash_invalidates() {
        let raw = std::fs::read(testdata("trackerless.torrent")).unwrap();
        let mi = parse_metainfo(&raw).unwrap();
        let mut wrong = InfoHash([0; 20]);
        wrong.0[0] = 1;
        let mut meta = MetadataExchange::new(Some(wrong), None);
        meta.set_size(mi.info_bytes.len() as u32).unwrap();
        let err = meta.on_data(0, &mi.info_bytes).unwrap_err();
        assert_eq!(err, MetadataError::IncorrectV1);
        assert!(!meta.have_all_pieces());
    }

    #[test]
    fn from_info_bytes_ok() {
        let raw = std::fs::read(testdata("trackerless.torrent")).unwrap();
        let mi = parse_metainfo(&raw).unwrap();
        let ih = info_hash_v1(&mi.info_bytes);
        let meta =
            MetadataExchange::from_info_bytes(mi.info_bytes.clone(), Some(ih), None).unwrap();
        assert!(meta.has_info());
        assert!(matches!(meta.serve(0), Serve::Data(_)));
    }
}
