//! Error types for wt-meta.

use std::fmt;

/// Errors from bencode, metainfo, magnet, or merkle parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Malformed bencode at the given byte offset.
    Bencode { offset: usize, message: String },
    /// Input had unused trailing bytes after a complete value.
    TrailingBytes { unused: usize },
    /// A required field was missing or invalid.
    MetaInfo(String),
    /// Magnet URI parse failure.
    Magnet(String),
    /// Merkle / piece-layer validation failure.
    Merkle(String),
    /// Integer did not fit in i64.
    IntegerOverflow,
    /// Unexpected EOF while decoding.
    UnexpectedEof,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Bencode { offset, message } => {
                write!(f, "bencode syntax error (offset {offset}): {message}")
            }
            Error::TrailingBytes { unused } => {
                write!(f, "{unused} unused trailing bytes")
            }
            Error::MetaInfo(m) => write!(f, "metainfo: {m}"),
            Error::Magnet(m) => write!(f, "magnet: {m}"),
            Error::Merkle(m) => write!(f, "merkle: {m}"),
            Error::IntegerOverflow => write!(f, "bencode integer overflow"),
            Error::UnexpectedEof => write!(f, "unexpected end of input"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
