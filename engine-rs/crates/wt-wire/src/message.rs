use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::fmt;
use std::io::{self, Error, ErrorKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum MessageType {
    Choke = 0,
    Unchoke = 1,
    Interested = 2,
    NotInterested = 3,
    Have = 4,
    Bitfield = 5,
    Request = 6,
    Piece = 7,
    Cancel = 8,
    Port = 9,
    Suggest = 13,
    HaveAll = 14,
    HaveNone = 15,
    Reject = 16,
    AllowedFast = 17,
    Extended = 20,
    HashRequest = 21,
    Hashes = 22,
    HashReject = 23,
}

impl MessageType {
    pub fn from_u8(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::Choke),
            1 => Some(Self::Unchoke),
            2 => Some(Self::Interested),
            3 => Some(Self::NotInterested),
            4 => Some(Self::Have),
            5 => Some(Self::Bitfield),
            6 => Some(Self::Request),
            7 => Some(Self::Piece),
            8 => Some(Self::Cancel),
            9 => Some(Self::Port),
            13 => Some(Self::Suggest),
            14 => Some(Self::HaveAll),
            15 => Some(Self::HaveNone),
            16 => Some(Self::Reject),
            17 => Some(Self::AllowedFast),
            20 => Some(Self::Extended),
            21 => Some(Self::HashRequest),
            22 => Some(Self::Hashes),
            23 => Some(Self::HashReject),
            _ => None,
        }
    }

}

impl fmt::Display for MessageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum Message {
    Keepalive,
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have {
        index: u32,
    },
    Bitfield(Bytes),
    Request {
        index: u32,
        begin: u32,
        length: u32,
    },
    Piece {
        index: u32,
        begin: u32,
        piece: Bytes,
    },
    Cancel {
        index: u32,
        begin: u32,
        length: u32,
    },
    Port(u16),
    Suggest {
        index: u32,
    },
    HaveAll,
    HaveNone,
    Reject {
        index: u32,
        begin: u32,
        length: u32,
    },
    AllowedFast {
        index: u32,
    },
    Extended {
        id: u8,
        payload: Bytes,
    },
    HashRequest {
        pieces_root: [u8; 32],
        base_layer: u32,
        index: u32,
        length: u32,
        proof_layers: u32,
    },
    Hashes {
        pieces_root: [u8; 32],
        base_layer: u32,
        index: u32,
        length: u32,
        proof_layers: u32,
        hashes: Vec<[u8; 32]>,
    },
    HashReject {
        pieces_root: [u8; 32],
        base_layer: u32,
        index: u32,
        length: u32,
        proof_layers: u32,
    },
}

impl fmt::Debug for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Keepalive => write!(f, "Keepalive"),
            Self::Choke => write!(f, "Choke"),
            Self::Unchoke => write!(f, "Unchoke"),
            Self::Interested => write!(f, "Interested"),
            Self::NotInterested => write!(f, "NotInterested"),
            Self::Have { index } => write!(f, "Have({index})"),
            Self::Bitfield(b) => write!(f, "Bitfield(len={})", b.len()),
            Self::Request {
                index,
                begin,
                length,
            } => {
                write!(f, "Request(idx={index}, off={begin}, len={length})")
            }
            Self::Piece {
                index,
                begin,
                piece,
            } => {
                write!(f, "Piece(idx={index}, off={begin}, len={})", piece.len())
            }
            Self::Cancel {
                index,
                begin,
                length,
            } => {
                write!(f, "Cancel(idx={index}, off={begin}, len={length})")
            }
            Self::Port(p) => write!(f, "Port({p})"),
            Self::Suggest { index } => write!(f, "Suggest({index})"),
            Self::HaveAll => write!(f, "HaveAll"),
            Self::HaveNone => write!(f, "HaveNone"),
            Self::Reject {
                index,
                begin,
                length,
            } => {
                write!(f, "Reject(idx={index}, off={begin}, len={length})")
            }
            Self::AllowedFast { index } => write!(f, "AllowedFast({index})"),
            Self::Extended { id, payload } => {
                write!(f, "Extended(id={id}, len={})", payload.len())
            }
            Self::HashRequest {
                pieces_root,
                base_layer,
                index,
                length,
                proof_layers,
            } => {
                write!(f, "HashRequest(root={}, base={base_layer}, idx={index}, len={length}, proof={proof_layers})", hex::encode(pieces_root))
            }
            Self::Hashes {
                pieces_root,
                base_layer,
                index,
                length,
                proof_layers,
                hashes,
            } => {
                write!(f, "Hashes(root={}, count={}, base={base_layer}, idx={index}, len={length}, proof={proof_layers})", hex::encode(pieces_root), hashes.len())
            }
            Self::HashReject {
                pieces_root,
                base_layer,
                index,
                length,
                proof_layers,
            } => {
                write!(f, "HashReject(root={}, base={base_layer}, idx={index}, len={length}, proof={proof_layers})", hex::encode(pieces_root))
            }
        }
    }
}

impl Message {

    /// Calculate the length of the message payload (excluding the 4-byte length prefix).
    pub fn payload_len(&self) -> usize {
        match self {
            Self::Keepalive => 0,
            Self::Choke
            | Self::Unchoke
            | Self::Interested
            | Self::NotInterested
            | Self::HaveAll
            | Self::HaveNone => 1,
            Self::Have { .. } | Self::Suggest { .. } | Self::AllowedFast { .. } => 1 + 4,
            Self::Port(_) => 1 + 2,
            Self::Request { .. } | Self::Cancel { .. } | Self::Reject { .. } => 1 + 12,
            Self::Bitfield(b) => 1 + b.len(),
            Self::Piece { piece, .. } => 1 + 8 + piece.len(),
            Self::Extended { payload, .. } => 1 + 1 + payload.len(),
            Self::HashRequest { .. } | Self::HashReject { .. } => 1 + 32 + 16,
            Self::Hashes { hashes, .. } => 1 + 32 + 16 + hashes.len() * 32,
        }
    }

    /// Encode message into a `BytesMut` buffer.
    pub fn encode(&self, dst: &mut BytesMut) {
        let payload_len = self.payload_len();
        dst.put_u32(payload_len as u32);
        if payload_len == 0 {
            return;
        }

        match self {
            Self::Keepalive => {}
            Self::Choke => dst.put_u8(MessageType::Choke as u8),
            Self::Unchoke => dst.put_u8(MessageType::Unchoke as u8),
            Self::Interested => dst.put_u8(MessageType::Interested as u8),
            Self::NotInterested => dst.put_u8(MessageType::NotInterested as u8),
            Self::Have { index } => {
                dst.put_u8(MessageType::Have as u8);
                dst.put_u32(*index);
            }
            Self::Bitfield(b) => {
                dst.put_u8(MessageType::Bitfield as u8);
                dst.put_slice(b);
            }
            Self::Request {
                index,
                begin,
                length,
            } => {
                dst.put_u8(MessageType::Request as u8);
                dst.put_u32(*index);
                dst.put_u32(*begin);
                dst.put_u32(*length);
            }
            Self::Piece {
                index,
                begin,
                piece,
            } => {
                dst.put_u8(MessageType::Piece as u8);
                dst.put_u32(*index);
                dst.put_u32(*begin);
                dst.put_slice(piece);
            }
            Self::Cancel {
                index,
                begin,
                length,
            } => {
                dst.put_u8(MessageType::Cancel as u8);
                dst.put_u32(*index);
                dst.put_u32(*begin);
                dst.put_u32(*length);
            }
            Self::Port(p) => {
                dst.put_u8(MessageType::Port as u8);
                dst.put_u16(*p);
            }
            Self::Suggest { index } => {
                dst.put_u8(MessageType::Suggest as u8);
                dst.put_u32(*index);
            }
            Self::HaveAll => dst.put_u8(MessageType::HaveAll as u8),
            Self::HaveNone => dst.put_u8(MessageType::HaveNone as u8),
            Self::Reject {
                index,
                begin,
                length,
            } => {
                dst.put_u8(MessageType::Reject as u8);
                dst.put_u32(*index);
                dst.put_u32(*begin);
                dst.put_u32(*length);
            }
            Self::AllowedFast { index } => {
                dst.put_u8(MessageType::AllowedFast as u8);
                dst.put_u32(*index);
            }
            Self::Extended { id, payload } => {
                dst.put_u8(MessageType::Extended as u8);
                dst.put_u8(*id);
                dst.put_slice(payload);
            }
            Self::HashRequest {
                pieces_root,
                base_layer,
                index,
                length,
                proof_layers,
            } => {
                dst.put_u8(MessageType::HashRequest as u8);
                dst.put_slice(pieces_root);
                dst.put_u32(*base_layer);
                dst.put_u32(*index);
                dst.put_u32(*length);
                dst.put_u32(*proof_layers);
            }
            Self::Hashes {
                pieces_root,
                base_layer,
                index,
                length,
                proof_layers,
                hashes,
            } => {
                dst.put_u8(MessageType::Hashes as u8);
                dst.put_slice(pieces_root);
                dst.put_u32(*base_layer);
                dst.put_u32(*index);
                dst.put_u32(*length);
                dst.put_u32(*proof_layers);
                for h in hashes {
                    dst.put_slice(h);
                }
            }
            Self::HashReject {
                pieces_root,
                base_layer,
                index,
                length,
                proof_layers,
            } => {
                dst.put_u8(MessageType::HashReject as u8);
                dst.put_slice(pieces_root);
                dst.put_u32(*base_layer);
                dst.put_u32(*index);
                dst.put_u32(*length);
                dst.put_u32(*proof_layers);
            }
        }
    }

    /// Helper to serialize message to `Bytes`.
    pub fn to_bytes(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(4 + self.payload_len());
        self.encode(&mut buf);
        buf.freeze()
    }

    /// Decode from raw message payload (excluding the 4-byte length prefix).
    /// `payload` is zero-copy partitioned when extracting `Piece` data or `Extended` payload.
    pub fn decode_payload(mut payload: Bytes) -> io::Result<Self> {
        if payload.is_empty() {
            return Ok(Self::Keepalive);
        }

        let id = payload.get_u8();
        let msg_type = MessageType::from_u8(id).ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidData,
                format!("unhandled message type: {id}"),
            )
        })?;

        match msg_type {
            MessageType::Choke => {
                if !payload.is_empty() {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        format!("{} unused bytes in message type Choke", payload.len()),
                    ));
                }
                Ok(Self::Choke)
            }
            MessageType::Unchoke => {
                if !payload.is_empty() {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        format!("{} unused bytes in message type Unchoke", payload.len()),
                    ));
                }
                Ok(Self::Unchoke)
            }
            MessageType::Interested => {
                if !payload.is_empty() {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        format!("{} unused bytes in message type Interested", payload.len()),
                    ));
                }
                Ok(Self::Interested)
            }
            MessageType::NotInterested => {
                if !payload.is_empty() {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        format!(
                            "{} unused bytes in message type NotInterested",
                            payload.len()
                        ),
                    ));
                }
                Ok(Self::NotInterested)
            }
            MessageType::Have => {
                if payload.len() != 4 {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        format!("expected 4 bytes for Have, got {}", payload.len()),
                    ));
                }
                Ok(Self::Have {
                    index: payload.get_u32(),
                })
            }
            MessageType::Bitfield => Ok(Self::Bitfield(payload)),
            MessageType::Request => {
                if payload.len() != 12 {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        format!("expected 12 bytes for Request, got {}", payload.len()),
                    ));
                }
                Ok(Self::Request {
                    index: payload.get_u32(),
                    begin: payload.get_u32(),
                    length: payload.get_u32(),
                })
            }
            MessageType::Piece => {
                if payload.len() < 8 {
                    return Err(Error::new(
                        ErrorKind::UnexpectedEof,
                        format!(
                            "expected at least 8 bytes for Piece header, got {}",
                            payload.len()
                        ),
                    ));
                }
                let index = payload.get_u32();
                let begin = payload.get_u32();
                Ok(Self::Piece {
                    index,
                    begin,
                    piece: payload, // zero-copy slice of remaining bytes!
                })
            }
            MessageType::Cancel => {
                if payload.len() != 12 {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        format!("expected 12 bytes for Cancel, got {}", payload.len()),
                    ));
                }
                Ok(Self::Cancel {
                    index: payload.get_u32(),
                    begin: payload.get_u32(),
                    length: payload.get_u32(),
                })
            }
            MessageType::Port => {
                if payload.len() != 2 {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        format!("expected 2 bytes for Port, got {}", payload.len()),
                    ));
                }
                Ok(Self::Port(payload.get_u16()))
            }
            MessageType::Suggest => {
                if payload.len() != 4 {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        format!("expected 4 bytes for Suggest, got {}", payload.len()),
                    ));
                }
                Ok(Self::Suggest {
                    index: payload.get_u32(),
                })
            }
            MessageType::HaveAll => {
                if !payload.is_empty() {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        format!("{} unused bytes in message type HaveAll", payload.len()),
                    ));
                }
                Ok(Self::HaveAll)
            }
            MessageType::HaveNone => {
                if !payload.is_empty() {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        format!("{} unused bytes in message type HaveNone", payload.len()),
                    ));
                }
                Ok(Self::HaveNone)
            }
            MessageType::Reject => {
                if payload.len() != 12 {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        format!("expected 12 bytes for Reject, got {}", payload.len()),
                    ));
                }
                Ok(Self::Reject {
                    index: payload.get_u32(),
                    begin: payload.get_u32(),
                    length: payload.get_u32(),
                })
            }
            MessageType::AllowedFast => {
                if payload.len() != 4 {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        format!("expected 4 bytes for AllowedFast, got {}", payload.len()),
                    ));
                }
                Ok(Self::AllowedFast {
                    index: payload.get_u32(),
                })
            }
            MessageType::Extended => {
                if payload.is_empty() {
                    return Err(Error::new(
                        ErrorKind::UnexpectedEof,
                        "expected extended id byte",
                    ));
                }
                let ext_id = payload.get_u8();
                Ok(Self::Extended {
                    id: ext_id,
                    payload, // zero-copy remaining payload
                })
            }
            MessageType::HashRequest => {
                if payload.len() != 32 + 16 {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        format!("expected 48 bytes for HashRequest, got {}", payload.len()),
                    ));
                }
                let mut pieces_root = [0u8; 32];
                payload.copy_to_slice(&mut pieces_root);
                let base_layer = payload.get_u32();
                let index = payload.get_u32();
                let length = payload.get_u32();
                let proof_layers = payload.get_u32();
                Ok(Self::HashRequest {
                    pieces_root,
                    base_layer,
                    index,
                    length,
                    proof_layers,
                })
            }
            MessageType::Hashes => {
                if payload.len() < 32 + 16 || !(payload.len() - (32 + 16)).is_multiple_of(32) {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        format!("invalid length for Hashes: {}", payload.len()),
                    ));
                }
                let mut pieces_root = [0u8; 32];
                payload.copy_to_slice(&mut pieces_root);
                let base_layer = payload.get_u32();
                let index = payload.get_u32();
                let length = payload.get_u32();
                let proof_layers = payload.get_u32();
                let num_hashes = payload.len() / 32;
                let mut hashes = Vec::with_capacity(num_hashes);
                for _ in 0..num_hashes {
                    let mut h = [0u8; 32];
                    payload.copy_to_slice(&mut h);
                    hashes.push(h);
                }
                Ok(Self::Hashes {
                    pieces_root,
                    base_layer,
                    index,
                    length,
                    proof_layers,
                    hashes,
                })
            }
            MessageType::HashReject => {
                if payload.len() != 32 + 16 {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        format!("expected 48 bytes for HashReject, got {}", payload.len()),
                    ));
                }
                let mut pieces_root = [0u8; 32];
                payload.copy_to_slice(&mut pieces_root);
                let base_layer = payload.get_u32();
                let index = payload.get_u32();
                let length = payload.get_u32();
                let proof_layers = payload.get_u32();
                Ok(Self::HashReject {
                    pieces_root,
                    base_layer,
                    index,
                    length,
                    proof_layers,
                })
            }
        }
    }
}

pub fn marshal_bitfield(bf: &[bool]) -> Bytes {
    let mut b = vec![0u8; bf.len().div_ceil(8)];
    for (i, &have) in bf.iter().enumerate() {
        if have {
            b[i / 8] |= 1 << (7 - (i % 8));
        }
    }
    Bytes::from(b)
}

pub fn unmarshal_bitfield(b: &[u8]) -> Vec<bool> {
    let mut bf = Vec::with_capacity(b.len() * 8);
    for &c in b {
        for i in (0..8).rev() {
            bf.push((c >> i) & 1 == 1);
        }
    }
    bf
}
