//! BEP 55 `ut_holepunch` message codec.
//!
//! Binary layout (all integers big-endian):
//!
//! ```text
//! msg_type:u8  addr_type:u8  addr:4|16  port:u16  err_code:u32
//! ```
//!
//! `msg_type` is rendezvous (0), connect (1) or error (2). `addr_type` is
//! IPv4 (0) or IPv6 (1). `err_code` is 0 except in error messages.

use bytes::{BufMut, Bytes, BytesMut};
use std::io::{self, Error, ErrorKind};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crate::message::Message;

/// LTEP name advertised in the extended handshake `m` dictionary.
pub const EXTENSION_NAME_HOLEPUNCH: &str = "ut_holepunch";

pub const HOLEPUNCH_ADDR_IPV4: u8 = 0;
pub const HOLEPUNCH_ADDR_IPV6: u8 = 1;

/// Encoded size of an IPv4 message.
pub const HOLEPUNCH_V4_LEN: usize = 1 + 1 + 4 + 2 + 4;
/// Encoded size of an IPv6 message.
pub const HOLEPUNCH_V6_LEN: usize = 1 + 1 + 16 + 2 + 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HolepunchMsgType {
    /// Ask the receiver (relay) to introduce us to `addr`.
    Rendezvous = 0,
    /// Sent by the relay: start a uTP connection to `addr` now.
    Connect = 1,
    /// Sent by the relay: the rendezvous for `addr` failed with `err_code`.
    Error = 2,
}

/// BEP 55 error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HolepunchError {
    /// The target endpoint is invalid.
    NoSuchPeer = 1,
    /// The relaying peer is not connected to the target peer.
    NotConnected = 2,
    /// The target peer does not support the holepunch extension.
    NoSupport = 3,
    /// The target endpoint belongs to the relaying peer (or is the sender).
    NoSelf = 4,
}

impl HolepunchError {
    pub fn from_code(code: u32) -> Option<Self> {
        Some(match code {
            1 => Self::NoSuchPeer,
            2 => Self::NotConnected,
            3 => Self::NoSupport,
            4 => Self::NoSelf,
            _ => return None,
        })
    }

    pub fn code(self) -> u32 {
        self as u32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HolepunchMsg {
    pub msg_type: HolepunchMsgType,
    pub addr: SocketAddr,
    /// Raw error code; 0 for rendezvous and connect. Unknown codes are kept
    /// so a newer peer's error is still counted.
    pub err_code: u32,
}

impl HolepunchMsg {
    pub fn rendezvous(addr: SocketAddr) -> Self {
        Self { msg_type: HolepunchMsgType::Rendezvous, addr, err_code: 0 }
    }

    pub fn connect(addr: SocketAddr) -> Self {
        Self { msg_type: HolepunchMsgType::Connect, addr, err_code: 0 }
    }

    pub fn error(addr: SocketAddr, err: HolepunchError) -> Self {
        Self { msg_type: HolepunchMsgType::Error, addr, err_code: err.code() }
    }

    /// Known error code of an error message.
    pub fn error_kind(&self) -> Option<HolepunchError> {
        HolepunchError::from_code(self.err_code)
    }

    pub fn encode(&self) -> Bytes {
        let mut b = BytesMut::with_capacity(HOLEPUNCH_V6_LEN);
        b.put_u8(self.msg_type as u8);
        match self.addr.ip() {
            IpAddr::V4(ip) => {
                b.put_u8(HOLEPUNCH_ADDR_IPV4);
                b.put_slice(&ip.octets());
            }
            IpAddr::V6(ip) => {
                b.put_u8(HOLEPUNCH_ADDR_IPV6);
                b.put_slice(&ip.octets());
            }
        }
        b.put_u16(self.addr.port());
        b.put_u32(self.err_code);
        b.freeze()
    }

    /// Decode a message. Unknown message or address types and input too
    /// short for the address and port are rejected.
    ///
    /// `err_code` is optional on rendezvous and connect: libtorrent (and so
    /// qBittorrent, Deluge and others) leaves it out and sends only
    /// `msg_type addr_type addr port`, while uTorrent sends the full layout.
    /// An error message must carry its code. Bytes after the known fields
    /// are ignored, as other clients do.
    pub fn decode(payload: &[u8]) -> io::Result<Self> {
        let bad = |what: &str| Error::new(ErrorKind::InvalidData, format!("ut_holepunch: {what}"));
        if payload.len() < 2 {
            return Err(bad("truncated header"));
        }
        let msg_type = match payload[0] {
            0 => HolepunchMsgType::Rendezvous,
            1 => HolepunchMsgType::Connect,
            2 => HolepunchMsgType::Error,
            _ => return Err(bad("unknown msg_type")),
        };
        let addr_len = match payload[1] {
            HOLEPUNCH_ADDR_IPV4 => 4,
            HOLEPUNCH_ADDR_IPV6 => 16,
            _ => return Err(bad("unknown addr_type")),
        };
        let rest = &payload[2..];
        if rest.len() < addr_len + 2 {
            return Err(bad("truncated address"));
        }
        let ip = if addr_len == 4 {
            let o: [u8; 4] = rest[..4].try_into().expect("length checked");
            IpAddr::V4(Ipv4Addr::from(o))
        } else {
            let o: [u8; 16] = rest[..16].try_into().expect("length checked");
            IpAddr::V6(Ipv6Addr::from(o))
        };
        let port = u16::from_be_bytes([rest[addr_len], rest[addr_len + 1]]);
        let tail = &rest[addr_len + 2..];
        let err_code = match tail.get(..4) {
            Some(c) => u32::from_be_bytes([c[0], c[1], c[2], c[3]]),
            None if msg_type == HolepunchMsgType::Error => return Err(bad("error without err_code")),
            None => 0,
        };
        Ok(Self { msg_type, addr: SocketAddr::new(ip, port), err_code })
    }

    pub fn to_message(&self, ext_id: u8) -> Message {
        Message::Extended { id: ext_id, payload: self.encode() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4() -> SocketAddr {
        "203.0.113.7:51413".parse().unwrap()
    }
    fn v6() -> SocketAddr {
        "[2001:db8::42]:6881".parse().unwrap()
    }

    #[test]
    fn ipv4_rendezvous_layout_and_roundtrip() {
        let m = HolepunchMsg::rendezvous(v4());
        let enc = m.encode();
        assert_eq!(
            enc.as_ref(),
            &[0, 0, 203, 0, 113, 7, 0xC8, 0xD5, 0, 0, 0, 0][..]
        );
        assert_eq!(HolepunchMsg::decode(&enc).unwrap(), m);
    }

    #[test]
    fn ipv6_connect_layout_and_roundtrip() {
        let m = HolepunchMsg::connect(v6());
        let enc = m.encode();
        assert_eq!(enc.len(), HOLEPUNCH_V6_LEN);
        assert_eq!(enc[0], 1);
        assert_eq!(enc[1], HOLEPUNCH_ADDR_IPV6);
        assert_eq!(&enc[2..18], &"2001:db8::42".parse::<Ipv6Addr>().unwrap().octets());
        assert_eq!(&enc[18..20], &6881u16.to_be_bytes());
        assert_eq!(&enc[20..24], &[0, 0, 0, 0]);
        assert_eq!(HolepunchMsg::decode(&enc).unwrap(), m);
    }

    #[test]
    fn every_error_code_roundtrips_v4_and_v6() {
        for (err, code) in [
            (HolepunchError::NoSuchPeer, 1u32),
            (HolepunchError::NotConnected, 2),
            (HolepunchError::NoSupport, 3),
            (HolepunchError::NoSelf, 4),
        ] {
            assert_eq!(err.code(), code);
            assert_eq!(HolepunchError::from_code(code), Some(err));
            for addr in [v4(), v6()] {
                let m = HolepunchMsg::error(addr, err);
                let enc = m.encode();
                assert_eq!(enc[0], 2);
                assert_eq!(&enc[enc.len() - 4..], &code.to_be_bytes());
                let dec = HolepunchMsg::decode(&enc).unwrap();
                assert_eq!(dec, m);
                assert_eq!(dec.error_kind(), Some(err));
            }
        }
        assert_eq!(HolepunchError::from_code(0), None);
        assert_eq!(HolepunchError::from_code(5), None);
    }

    #[test]
    fn unknown_error_code_is_kept_raw() {
        let mut enc = HolepunchMsg::error(v4(), HolepunchError::NoSelf).encode().to_vec();
        let n = enc.len();
        enc[n - 4..].copy_from_slice(&99u32.to_be_bytes());
        let dec = HolepunchMsg::decode(&enc).unwrap();
        assert_eq!(dec.err_code, 99);
        assert_eq!(dec.error_kind(), None);
    }

    #[test]
    fn malformed_input_rejected() {
        let good4 = HolepunchMsg::connect(v4()).encode().to_vec();
        let good6 = HolepunchMsg::connect(v6()).encode().to_vec();
        // Empty and header-only.
        assert!(HolepunchMsg::decode(&[]).is_err());
        assert!(HolepunchMsg::decode(&[0]).is_err());
        assert!(HolepunchMsg::decode(&[0, 0]).is_err());
        // Too short for the address and port.
        for n in 0..8 {
            assert!(HolepunchMsg::decode(&good4[..n]).is_err(), "v4 len {n}");
        }
        for n in 0..20 {
            assert!(HolepunchMsg::decode(&good6[..n]).is_err(), "v6 len {n}");
        }
        // An error message must carry its code.
        let err4 = HolepunchMsg::error(v4(), HolepunchError::NotConnected).encode().to_vec();
        for n in 0..err4.len() {
            assert!(HolepunchMsg::decode(&err4[..n]).is_err(), "error len {n}");
        }
        // Unknown msg_type.
        let mut bad = good4.clone();
        bad[0] = 3;
        assert!(HolepunchMsg::decode(&bad).is_err());
        bad[0] = 0xFF;
        assert!(HolepunchMsg::decode(&bad).is_err());
        // Unknown addr_type.
        let mut bad = good4.clone();
        bad[1] = 2;
        assert!(HolepunchMsg::decode(&bad).is_err());
        // addr_type says IPv6 but the body is IPv4-sized.
        let mut bad = good4.clone();
        bad[1] = HOLEPUNCH_ADDR_IPV6;
        assert!(HolepunchMsg::decode(&bad).is_err());
    }

    /// libtorrent omits `err_code` on rendezvous and connect.
    #[test]
    fn libtorrent_short_connect_and_rendezvous_decode() {
        let short4 = [1u8, 0, 203, 0, 113, 7, 0xC8, 0xD5];
        assert_eq!(HolepunchMsg::decode(&short4).unwrap(), HolepunchMsg::connect(v4()));
        let mut short6 = vec![0u8, 1];
        short6.extend_from_slice(&"2001:db8::42".parse::<Ipv6Addr>().unwrap().octets());
        short6.extend_from_slice(&6881u16.to_be_bytes());
        assert_eq!(HolepunchMsg::decode(&short6).unwrap(), HolepunchMsg::rendezvous(v6()));
    }

    #[test]
    fn trailing_bytes_are_ignored() {
        let mut long = HolepunchMsg::connect(v4()).encode().to_vec();
        long.extend_from_slice(&[9, 9, 9]);
        assert_eq!(HolepunchMsg::decode(&long).unwrap(), HolepunchMsg::connect(v4()));
    }

    #[test]
    fn to_message_uses_peer_ext_id() {
        let m = HolepunchMsg::rendezvous(v4());
        match m.to_message(9) {
            Message::Extended { id, payload } => {
                assert_eq!(id, 9);
                assert_eq!(payload, m.encode());
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
