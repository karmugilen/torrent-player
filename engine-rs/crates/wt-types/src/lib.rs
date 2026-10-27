//! Shared data model for the Webtor Rust engine.
//!
//! Owned by the coordinator. Every crate in the workspace agrees on these
//! types; workers ask the coordinator before changing anything here.
//! Mirrors `github.com/anacrolix/torrent` v1.61.0 (`metainfo`, `types`,
//! `storage`) as used by `engine-go`.

use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;

pub type PieceIndex = u32;
/// Index of a 16 KiB block across the whole torrent (`RequestIndex` in Go).
pub type RequestIndex = u32;

pub const BLOCK_SIZE: u32 = 16 * 1024;

// ---------------------------------------------------------------- hashes

/// SHA-1 v1 infohash (also the "short" form of a v2 hash, its first 20 bytes).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct InfoHash(pub [u8; 20]);

/// SHA-256 v2 infohash (BEP 52).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct InfoHashV2(pub [u8; 32]);

impl InfoHash {
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
    pub fn from_hex(s: &str) -> Option<Self> {
        let mut out = [0u8; 20];
        hex::decode_to_slice(s, &mut out).ok()?;
        Some(Self(out))
    }
}

impl InfoHashV2 {
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
    /// First 20 bytes, used wherever a v1-sized key is needed (trackers, DHT, handshakes).
    pub fn to_short(&self) -> InfoHash {
        let mut out = [0u8; 20];
        out.copy_from_slice(&self.0[..20]);
        InfoHash(out)
    }
}

impl fmt::Debug for InfoHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "InfoHash({})", self.to_hex())
    }
}
impl fmt::Display for InfoHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}
impl fmt::Debug for InfoHashV2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "InfoHashV2({})", self.to_hex())
    }
}

pub type PeerId = [u8; 20];

// ---------------------------------------------------------------- metainfo

/// One file of a torrent, in torrent order, including BEP 47 padding files.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FileInfo {
    /// Path components below the torrent name (prefer `path.utf-8` when present).
    pub path: Vec<String>,
    pub length: i64,
    /// Byte offset of the file inside the concatenated torrent payload.
    pub torrent_offset: i64,
    /// BEP 47 attributes, e.g. "p" for padding, "x" executable, "h" hidden.
    pub attr: String,
    /// BEP 52 merkle root of the file, when v2.
    pub pieces_root: Option<[u8; 32]>,
}

impl FileInfo {
    pub fn is_padding(&self) -> bool {
        self.attr.contains('p')
    }
}

/// Decoded `info` dictionary (BEP 3 / 47 / 52). `wt-meta` produces it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Info {
    pub name: String,
    pub piece_length: i64,
    /// Concatenated 20-byte SHA-1 piece hashes. Empty for pure v2 infos.
    pub pieces: Vec<u8>,
    /// Files in torrent order. A single-file torrent has one entry with an empty path.
    pub files: Vec<FileInfo>,
    /// True when the torrent used the single-file (`length`) form.
    pub single_file: bool,
    pub private: Option<bool>,
    pub source: String,
    /// 0 or 1 = v1 only, 2 = v2 (hybrid when `pieces` is also present).
    pub meta_version: i64,
    /// Raw bencoded `file tree`, kept so v2 merkle verification and
    /// re-encoding are lossless.
    pub file_tree_raw: Option<Vec<u8>>,
}

impl Info {
    pub fn total_length(&self) -> i64 {
        self.files.iter().map(|f| f.length).sum()
    }
    pub fn num_pieces(&self) -> u32 {
        let total = self.total_length();
        if self.piece_length <= 0 || total <= 0 {
            return 0;
        }
        ((total + self.piece_length - 1) / self.piece_length) as u32
    }
    pub fn piece(&self, index: PieceIndex) -> Piece {
        let offset = index as i64 * self.piece_length;
        let length = (self.total_length() - offset).clamp(0, self.piece_length);
        Piece { index, offset, length }
    }
    /// v1 SHA-1 of a piece, if the info carries v1 hashes.
    pub fn piece_hash_v1(&self, index: PieceIndex) -> Option<[u8; 20]> {
        let start = index as usize * 20;
        let slice = self.pieces.get(start..start + 20)?;
        let mut out = [0u8; 20];
        out.copy_from_slice(slice);
        Some(out)
    }
    pub fn has_v1(&self) -> bool {
        !self.pieces.is_empty()
    }
    pub fn has_v2(&self) -> bool {
        self.meta_version == 2
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Piece {
    pub index: PieceIndex,
    pub offset: i64,
    pub length: i64,
}

/// A whole .torrent file. `info_bytes` is the exact bencoded info dict, the
/// input to the infohash.
#[derive(Clone, Debug, Default)]
pub struct MetaInfo {
    pub info_bytes: Vec<u8>,
    pub announce: String,
    pub announce_list: Vec<Vec<String>>,
    /// BEP 5 DHT bootstrap nodes as "host:port".
    pub nodes: Vec<String>,
    pub creation_date: i64,
    pub comment: String,
    pub created_by: String,
    pub encoding: String,
    /// BEP 19 web seeds.
    pub url_list: Vec<String>,
    /// BEP 52: file pieces root -> concatenated layer hashes.
    pub piece_layers: Vec<([u8; 32], Vec<u8>)>,
}

/// Parsed magnet link (v1, v2 or hybrid).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Magnet {
    pub info_hash: Option<InfoHash>,
    pub info_hash_v2: Option<InfoHashV2>,
    pub trackers: Vec<String>,
    pub display_name: String,
    /// Web seeds ("ws") and exact sources ("xs"/"as").
    pub web_seeds: Vec<String>,
    /// Every other parameter, in order.
    pub params: Vec<(String, String)>,
}

// ---------------------------------------------------------------- priorities

/// Same ordering and meaning as anacrolix `types.PiecePriority`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(u8)]
pub enum PiecePriority {
    /// Not wanted. Must be the default.
    #[default]
    None = 0,
    Normal = 1,
    High = 2,
    Readahead = 3,
    Next = 4,
    /// A reader is blocked in this piece. Highest urgency.
    Now = 5,
}

// ---------------------------------------------------------------- peers

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PeerSource {
    /// "M": added directly by the engine.
    Direct,
    /// "Tr"
    Tracker,
    /// "I"
    Incoming,
    /// "Hg"
    DhtGetPeers,
    /// "Ha"
    DhtAnnouncePeer,
    /// "X"
    Pex,
    /// "C"
    UtHolepunch,
    /// WebTorrent (WebRTC data channel).
    WebRtc,
    /// BEP 19 HTTP seed.
    WebSeed,
}

impl PeerSource {
    /// The anacrolix code string, used by the UI and engine JSON.
    pub fn code(self) -> &'static str {
        match self {
            PeerSource::Direct => "M",
            PeerSource::Tracker => "Tr",
            PeerSource::Incoming => "I",
            PeerSource::DhtGetPeers => "Hg",
            PeerSource::DhtAnnouncePeer => "Ha",
            PeerSource::Pex => "X",
            PeerSource::UtHolepunch => "C",
            PeerSource::WebRtc => "W",
            PeerSource::WebSeed => "WS",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct PexFlags {
    pub prefers_encryption: bool,
    pub seed_only: bool,
    pub supports_utp: bool,
    pub supports_holepunch: bool,
    pub reachable: bool,
}

/// A peer we may connect to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PeerInfo {
    pub id: Option<PeerId>,
    pub addr: SocketAddr,
    pub source: PeerSource,
    pub supports_encryption: bool,
    pub pex: PexFlags,
    pub trusted: bool,
}

/// Transport a live connection runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Transport {
    Tcp,
    Utp,
    WebRtc,
    WebSeed,
}

// ---------------------------------------------------------------- storage

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("storage closed")]
    Closed,
    /// The bytes are not held (evicted from a bounded cache, never written).
    #[error("data not available")]
    Unavailable,
    #[error("{0}")]
    Other(String),
}

pub type StorageResult<T> = Result<T, StorageError>;

/// Cached completion state of a piece (anacrolix `storage.Completion`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Completion {
    /// The state is known (cached or cheaply checked).
    pub ok: bool,
    /// If `ok`, whether the data is verified complete.
    pub complete: bool,
    pub err: Option<String>,
}

/// Opens per-torrent storage (anacrolix `storage.ClientImpl`).
/// Implemented by `wt-storage`; consumed by `wt-core`.
pub trait ClientStorage: Send + Sync {
    fn open_torrent(&self, info: &Info, info_hash: InfoHash) -> StorageResult<Arc<dyn TorrentStorage>>;
}

/// Per-torrent storage (anacrolix `storage.TorrentImpl`).
pub trait TorrentStorage: Send + Sync {
    /// Storage for one piece. `hash` is the v1 hash when known.
    fn piece(&self, piece: Piece, hash: Option<[u8; 20]>) -> Arc<dyn PieceStorage>;
    fn close(&self) -> StorageResult<()>;
}

/// Per-piece storage (anacrolix `storage.PieceImpl`). Offsets are piece-relative.
pub trait PieceStorage: Send + Sync {
    fn read_at(&self, buf: &mut [u8], off: i64) -> StorageResult<usize>;
    fn write_at(&self, buf: &[u8], off: i64) -> StorageResult<usize>;
    /// The client believes the data passes the hash check.
    fn mark_complete(&self) -> StorageResult<()>;
    fn mark_not_complete(&self) -> StorageResult<()>;
    fn completion(&self) -> Completion;
}
