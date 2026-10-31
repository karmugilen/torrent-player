use std::fmt;
use std::io::{self, Error, ErrorKind};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use wt_types::{InfoHash, PeerId};

pub const PROTOCOL_STR: &[u8; 19] = b"BitTorrent protocol";
pub const PROTOCOL_HEADER: &[u8; 20] = b"\x13BitTorrent protocol";
pub const HANDSHAKE_LEN: usize = 68;

pub const EXTENSION_BIT_DHT: u8 = 0;
pub const EXTENSION_BIT_FAST: u8 = 2;
pub const EXTENSION_BIT_V2_UPGRADE: u8 = 4;
pub const EXTENSION_BIT_LTEP: u8 = 20;

#[derive(Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct PeerExtensionBits(pub [u8; 8]);

impl PeerExtensionBits {
    pub fn new() -> Self {
        Self([0u8; 8])
    }

    pub fn with_bits(bits: &[u8]) -> Self {
        let mut pex = Self::new();
        for &b in bits {
            pex.set_bit(b, true);
        }
        pex
    }

    pub fn get_bit(&self, bit: u8) -> bool {
        if bit > 63 {
            panic!("bit out of range: {bit}");
        }
        let byte_idx = 7 - (bit / 8) as usize;
        let bit_mask = 1 << (bit % 8);
        (self.0[byte_idx] & bit_mask) != 0
    }

    pub fn set_bit(&mut self, bit: u8, on: bool) {
        if bit > 63 {
            panic!("bit out of range: {bit}");
        }
        let byte_idx = 7 - (bit / 8) as usize;
        let bit_mask = 1 << (bit % 8);
        if on {
            self.0[byte_idx] |= bit_mask;
        } else {
            self.0[byte_idx] &= !bit_mask;
        }
    }

    pub fn supports_extended(&self) -> bool {
        self.get_bit(EXTENSION_BIT_LTEP)
    }

    pub fn supports_dht(&self) -> bool {
        self.get_bit(EXTENSION_BIT_DHT)
    }

    pub fn supports_fast(&self) -> bool {
        self.get_bit(EXTENSION_BIT_FAST)
    }

}

pub fn default_peer_extension_bytes() -> PeerExtensionBits {
    PeerExtensionBits::with_bits(&[EXTENSION_BIT_DHT, EXTENSION_BIT_LTEP, EXTENSION_BIT_FAST])
}

impl fmt::Debug for PeerExtensionBits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PeerExtensionBits({})", hex::encode(self.0))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandshakeResult {
    pub extensions: PeerExtensionBits,
    pub info_hash: InfoHash,
    pub peer_id: PeerId,
}

pub async fn write_handshake<W: AsyncWrite + Unpin>(
    writer: &mut W,
    info_hash: &InfoHash,
    peer_id: &PeerId,
    extensions: &PeerExtensionBits,
) -> io::Result<()> {
    let mut buf = [0u8; HANDSHAKE_LEN];
    buf[0] = 19;
    buf[1..20].copy_from_slice(PROTOCOL_STR);
    buf[20..28].copy_from_slice(&extensions.0);
    buf[28..48].copy_from_slice(&info_hash.0);
    buf[48..68].copy_from_slice(peer_id);
    writer.write_all(&buf).await
}

pub async fn read_handshake<R: AsyncRead + Unpin>(
    reader: &mut R,
    expected_info_hash: Option<&InfoHash>,
) -> io::Result<HandshakeResult> {
    let mut buf = [0u8; HANDSHAKE_LEN];
    reader.read_exact(&mut buf).await?;

    if buf[0] != 19 || &buf[1..20] != PROTOCOL_STR {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("unexpected protocol header: {:?}", &buf[..20]),
        ));
    }

    let mut ext = [0u8; 8];
    ext.copy_from_slice(&buf[20..28]);

    let mut ih = [0u8; 20];
    ih.copy_from_slice(&buf[28..48]);
    let info_hash = InfoHash(ih);

    if let Some(expected) = expected_info_hash {
        if &info_hash != expected {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!("mismatched infohash: expected {expected}, got {info_hash}"),
            ));
        }
    }

    let mut peer_id = [0u8; 20];
    peer_id.copy_from_slice(&buf[48..68]);

    Ok(HandshakeResult {
        extensions: PeerExtensionBits(ext),
        info_hash,
        peer_id,
    })
}

/// Outgoing handshake (or handshake with known InfoHash).
/// Sends our handshake and reads the peer's handshake concurrently.
pub async fn handshake<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    info_hash: &InfoHash,
    peer_id: &PeerId,
    extensions: &PeerExtensionBits,
) -> io::Result<HandshakeResult> {
    let (mut r, mut w) = tokio::io::split(stream);
    let write_fut = write_handshake(&mut w, info_hash, peer_id, extensions);
    let read_fut = read_handshake(&mut r, Some(info_hash));
    let (_, res) = tokio::try_join!(write_fut, read_fut)?;
    Ok(res)
}

/// Incoming handshake where the peer may be connecting to any of our torrents.
/// Sends protocol header and extensions, reads the peer's 68-byte handshake,
/// verifies the infohash with the `lookup` callback, and if valid, replies
/// with our infohash and peer_id.
pub async fn handshake_incoming<S: AsyncRead + AsyncWrite + Unpin, F>(
    stream: &mut S,
    peer_id: &PeerId,
    extensions: &PeerExtensionBits,
    lookup: F,
) -> io::Result<HandshakeResult>
where
    F: FnOnce(&InfoHash) -> bool,
{
    let mut buf = [0u8; HANDSHAKE_LEN];
    stream.read_exact(&mut buf).await?;

    if buf[0] != 19 || &buf[1..20] != PROTOCOL_STR {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("unexpected protocol header: {:?}", &buf[..20]),
        ));
    }

    let mut ext = [0u8; 8];
    ext.copy_from_slice(&buf[20..28]);

    let mut ih = [0u8; 20];
    ih.copy_from_slice(&buf[28..48]);
    let info_hash = InfoHash(ih);

    if !lookup(&info_hash) {
        let _ = stream.shutdown().await;
        return Err(Error::new(
            ErrorKind::NotFound,
            format!("infohash not found: {info_hash}"),
        ));
    }

    let mut peer_pid = [0u8; 20];
    peer_pid.copy_from_slice(&buf[48..68]);

    // Send our handshake back
    write_handshake(stream, &info_hash, peer_id, extensions).await?;

    Ok(HandshakeResult {
        extensions: PeerExtensionBits(ext),
        info_hash,
        peer_id: peer_pid,
    })
}
