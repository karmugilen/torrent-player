use bytes::Bytes;
use std::collections::BTreeMap;
use std::io::{self, Error, ErrorKind};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};
use wt_meta::bencode::{self, Value};
use wt_types::PexFlags;

use crate::message::Message;

pub const EXTENSION_NAME_METADATA: &str = "ut_metadata";
pub const EXTENSION_NAME_PEX: &str = "ut_pex";

pub const METADATA_PIECE_SIZE: usize = 16 * 1024; // 16 KiB

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataMsgType {
    Request = 0,
    Data = 1,
    Reject = 2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataMsg {
    pub msg_type: MetadataMsgType,
    pub piece: u32,
    pub total_size: Option<u32>,
    pub data: Option<Bytes>,
}

impl MetadataMsg {
    pub fn request(piece: u32) -> Self {
        Self {
            msg_type: MetadataMsgType::Request,
            piece,
            total_size: None,
            data: None,
        }
    }

    pub fn data(piece: u32, total_size: u32, data: Bytes) -> Self {
        Self {
            msg_type: MetadataMsgType::Data,
            piece,
            total_size: Some(total_size),
            data: Some(data),
        }
    }

    pub fn reject(piece: u32) -> Self {
        Self {
            msg_type: MetadataMsgType::Reject,
            piece,
            total_size: None,
            data: None,
        }
    }

    pub fn encode(&self) -> Bytes {
        let mut dict = BTreeMap::new();
        dict.insert(b"msg_type".to_vec(), Value::Int(self.msg_type as i64));
        dict.insert(b"piece".to_vec(), Value::Int(self.piece as i64));
        if let Some(total) = self.total_size {
            dict.insert(b"total_size".to_vec(), Value::Int(total as i64));
        }
        let dict_val = Value::Dict(dict);
        let mut encoded = bencode::encode(&dict_val);
        if let Some(ref d) = self.data {
            encoded.extend_from_slice(d);
        }
        Bytes::from(encoded)
    }

    pub fn decode(payload: Bytes) -> io::Result<Self> {
        let (val, prefix_len) =
            bencode::decode_prefix(&payload).map_err(|e| Error::new(ErrorKind::InvalidData, e))?;
        let dict = val
            .as_dict()
            .ok_or_else(|| Error::new(ErrorKind::InvalidData, "metadata payload must be a dict"))?;

        let msg_type_raw = dict
            .get(b"msg_type".as_slice())
            .and_then(|v| v.as_int())
            .ok_or_else(|| Error::new(ErrorKind::InvalidData, "missing msg_type"))?;

        let msg_type = match msg_type_raw {
            0 => MetadataMsgType::Request,
            1 => MetadataMsgType::Data,
            2 => MetadataMsgType::Reject,
            other => {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    format!("unknown metadata msg_type: {other}"),
                ))
            }
        };

        let piece = dict
            .get(b"piece".as_slice())
            .and_then(|v| v.as_int())
            .ok_or_else(|| Error::new(ErrorKind::InvalidData, "missing piece"))?
            as u32;

        let total_size = dict
            .get(b"total_size".as_slice())
            .and_then(|v| v.as_int())
            .map(|n| n as u32);

        let data = if msg_type == MetadataMsgType::Data && payload.len() > prefix_len {
            Some(payload.slice(prefix_len..))
        } else {
            None
        };

        Ok(Self {
            msg_type,
            piece,
            total_size,
            data,
        })
    }

    pub fn to_message(&self, ext_id: u8) -> Message {
        Message::Extended {
            id: ext_id,
            payload: self.encode(),
        }
    }
}

/// BEP 10 Extended Handshake Dictionary
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtendedHandshake {
    pub m: BTreeMap<String, u8>,
    pub v: Option<String>,
    pub p: Option<u16>,
    pub yourip: Option<IpAddr>,
    pub ipv4: Option<Ipv4Addr>,
    pub ipv6: Option<Ipv6Addr>,
    pub reqq: Option<u32>,
    pub metadata_size: Option<u32>,
    pub upload_only: Option<bool>,
    pub encryption: Option<bool>,
}

impl ExtendedHandshake {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn encode(&self) -> Bytes {
        let mut dict = BTreeMap::new();
        let mut m_dict = BTreeMap::new();
        for (name, &id) in &self.m {
            m_dict.insert(name.as_bytes().to_vec(), Value::Int(id as i64));
        }
        dict.insert(b"m".to_vec(), Value::Dict(m_dict));

        if let Some(ref v) = self.v {
            dict.insert(b"v".to_vec(), Value::Bytes(v.as_bytes().to_vec()));
        }
        if let Some(p) = self.p {
            dict.insert(b"p".to_vec(), Value::Int(p as i64));
        }
        if let Some(yourip) = self.yourip {
            match yourip {
                IpAddr::V4(ip) => {
                    dict.insert(b"yourip".to_vec(), Value::Bytes(ip.octets().to_vec()));
                }
                IpAddr::V6(ip) => {
                    dict.insert(b"yourip".to_vec(), Value::Bytes(ip.octets().to_vec()));
                }
            }
        }
        if let Some(ipv4) = self.ipv4 {
            dict.insert(b"ipv4".to_vec(), Value::Bytes(ipv4.octets().to_vec()));
        }
        if let Some(ipv6) = self.ipv6 {
            dict.insert(b"ipv6".to_vec(), Value::Bytes(ipv6.octets().to_vec()));
        }
        if let Some(reqq) = self.reqq {
            dict.insert(b"reqq".to_vec(), Value::Int(reqq as i64));
        }
        if let Some(size) = self.metadata_size {
            dict.insert(b"metadata_size".to_vec(), Value::Int(size as i64));
        }
        if let Some(ro) = self.upload_only {
            dict.insert(b"upload_only".to_vec(), Value::Int(if ro { 1 } else { 0 }));
        }
        if let Some(e) = self.encryption {
            dict.insert(b"e".to_vec(), Value::Int(if e { 1 } else { 0 }));
        }

        Bytes::from(bencode::encode(&Value::Dict(dict)))
    }

    pub fn decode(payload: &[u8]) -> io::Result<Self> {
        let val = bencode::decode(payload).map_err(|e| Error::new(ErrorKind::InvalidData, e))?;
        let dict = val
            .as_dict()
            .ok_or_else(|| Error::new(ErrorKind::InvalidData, "expected dict"))?;

        let mut handshake = Self::default();

        if let Some(m_val) = dict.get(b"m".as_slice()).and_then(|v| v.as_dict()) {
            for (k, v) in m_val {
                if let (Ok(name), Some(id)) = (std::str::from_utf8(k), v.as_int()) {
                    handshake.m.insert(name.to_string(), id as u8);
                }
            }
        }

        if let Some(v_bytes) = dict.get(b"v".as_slice()).and_then(|v| v.as_bytes()) {
            handshake.v = std::str::from_utf8(v_bytes).ok().map(|s| s.to_string());
        }

        if let Some(p) = dict.get(b"p".as_slice()).and_then(|v| v.as_int()) {
            handshake.p = Some(p as u16);
        }

        if let Some(yourip_bytes) = dict.get(b"yourip".as_slice()).and_then(|v| v.as_bytes()) {
            if yourip_bytes.len() == 4 {
                let oct: [u8; 4] = yourip_bytes.try_into().unwrap();
                handshake.yourip = Some(IpAddr::V4(Ipv4Addr::from(oct)));
            } else if yourip_bytes.len() == 16 {
                let oct: [u8; 16] = yourip_bytes.try_into().unwrap();
                handshake.yourip = Some(IpAddr::V6(Ipv6Addr::from(oct)));
            }
        }

        if let Some(ipv4_bytes) = dict.get(b"ipv4".as_slice()).and_then(|v| v.as_bytes()) {
            if ipv4_bytes.len() == 4 {
                let oct: [u8; 4] = ipv4_bytes.try_into().unwrap();
                handshake.ipv4 = Some(Ipv4Addr::from(oct));
            }
        }

        if let Some(ipv6_bytes) = dict.get(b"ipv6".as_slice()).and_then(|v| v.as_bytes()) {
            if ipv6_bytes.len() == 16 {
                let oct: [u8; 16] = ipv6_bytes.try_into().unwrap();
                handshake.ipv6 = Some(Ipv6Addr::from(oct));
            }
        }

        if let Some(reqq) = dict.get(b"reqq".as_slice()).and_then(|v| v.as_int()) {
            handshake.reqq = Some(reqq as u32);
        }

        if let Some(size) = dict
            .get(b"metadata_size".as_slice())
            .and_then(|v| v.as_int())
        {
            handshake.metadata_size = Some(size as u32);
        }

        if let Some(ro) = dict.get(b"upload_only".as_slice()).and_then(|v| v.as_int()) {
            handshake.upload_only = Some(ro != 0);
        }

        if let Some(e) = dict.get(b"e".as_slice()).and_then(|v| v.as_int()) {
            handshake.encryption = Some(e != 0);
        }

        Ok(handshake)
    }

    pub fn to_message(&self) -> Message {
        Message::Extended {
            id: 0,
            payload: self.encode(),
        }
    }
}

/// BEP 11 Peer Exchange (PEX) Message
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PexMsg {
    pub added: Vec<SocketAddrV4>,
    pub added_flags: Vec<PexFlags>,
    pub added6: Vec<SocketAddrV6>,
    pub added6_flags: Vec<PexFlags>,
    pub dropped: Vec<SocketAddrV4>,
    pub dropped6: Vec<SocketAddrV6>,
}

pub const PEX_PREFERS_ENCRYPTION: u8 = 0x01;
pub const PEX_SEED_UPLOAD_ONLY: u8 = 0x02;
pub const PEX_SUPPORTS_UTP: u8 = 0x04;
pub const PEX_HOLEPUNCH_SUPPORT: u8 = 0x08;
pub const PEX_OUTGOING_CONN: u8 = 0x10;

impl PexMsg {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.added.len() + self.added6.len() + self.dropped.len() + self.dropped6.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn encode(&self) -> Bytes {
        let mut dict = BTreeMap::new();

        let mut added_bytes = Vec::with_capacity(self.added.len() * 6);
        for addr in &self.added {
            added_bytes.extend_from_slice(&addr.ip().octets());
            added_bytes.extend_from_slice(&addr.port().to_be_bytes());
        }
        dict.insert(b"added".to_vec(), Value::Bytes(added_bytes));

        let mut added_f_bytes = Vec::with_capacity(self.added_flags.len());
        for f in &self.added_flags {
            let mut byte = 0u8;
            if f.prefers_encryption {
                byte |= PEX_PREFERS_ENCRYPTION;
            }
            if f.seed_only {
                byte |= PEX_SEED_UPLOAD_ONLY;
            }
            if f.supports_utp {
                byte |= PEX_SUPPORTS_UTP;
            }
            if f.supports_holepunch {
                byte |= PEX_HOLEPUNCH_SUPPORT;
            }
            if f.reachable {
                byte |= PEX_OUTGOING_CONN;
            }
            added_f_bytes.push(byte);
        }
        dict.insert(b"added.f".to_vec(), Value::Bytes(added_f_bytes));

        let mut added6_bytes = Vec::with_capacity(self.added6.len() * 18);
        for addr in &self.added6 {
            added6_bytes.extend_from_slice(&addr.ip().octets());
            added6_bytes.extend_from_slice(&addr.port().to_be_bytes());
        }
        dict.insert(b"added6".to_vec(), Value::Bytes(added6_bytes));

        let mut added6_f_bytes = Vec::with_capacity(self.added6_flags.len());
        for f in &self.added6_flags {
            let mut byte = 0u8;
            if f.prefers_encryption {
                byte |= PEX_PREFERS_ENCRYPTION;
            }
            if f.seed_only {
                byte |= PEX_SEED_UPLOAD_ONLY;
            }
            if f.supports_utp {
                byte |= PEX_SUPPORTS_UTP;
            }
            if f.supports_holepunch {
                byte |= PEX_HOLEPUNCH_SUPPORT;
            }
            if f.reachable {
                byte |= PEX_OUTGOING_CONN;
            }
            added6_f_bytes.push(byte);
        }
        dict.insert(b"added6.f".to_vec(), Value::Bytes(added6_f_bytes));

        let mut dropped_bytes = Vec::with_capacity(self.dropped.len() * 6);
        for addr in &self.dropped {
            dropped_bytes.extend_from_slice(&addr.ip().octets());
            dropped_bytes.extend_from_slice(&addr.port().to_be_bytes());
        }
        dict.insert(b"dropped".to_vec(), Value::Bytes(dropped_bytes));

        let mut dropped6_bytes = Vec::with_capacity(self.dropped6.len() * 18);
        for addr in &self.dropped6 {
            dropped6_bytes.extend_from_slice(&addr.ip().octets());
            dropped6_bytes.extend_from_slice(&addr.port().to_be_bytes());
        }
        dict.insert(b"dropped6".to_vec(), Value::Bytes(dropped6_bytes));

        Bytes::from(bencode::encode(&Value::Dict(dict)))
    }

    pub fn decode(payload: &[u8]) -> io::Result<Self> {
        let val = bencode::decode(payload).map_err(|e| Error::new(ErrorKind::InvalidData, e))?;
        let dict = val
            .as_dict()
            .ok_or_else(|| Error::new(ErrorKind::InvalidData, "expected dict"))?;

        let mut msg = Self::default();

        if let Some(bytes) = dict.get(b"added".as_slice()).and_then(|v| v.as_bytes()) {
            for chunk in bytes.as_chunks::<6>().0 {
                let ip = Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
                let port = u16::from_be_bytes([chunk[4], chunk[5]]);
                msg.added.push(SocketAddrV4::new(ip, port));
            }
        }

        if let Some(bytes) = dict.get(b"added.f".as_slice()).and_then(|v| v.as_bytes()) {
            for &byte in bytes {
                msg.added_flags.push(PexFlags {
                    prefers_encryption: (byte & PEX_PREFERS_ENCRYPTION) != 0,
                    seed_only: (byte & PEX_SEED_UPLOAD_ONLY) != 0,
                    supports_utp: (byte & PEX_SUPPORTS_UTP) != 0,
                    supports_holepunch: (byte & PEX_HOLEPUNCH_SUPPORT) != 0,
                    reachable: (byte & PEX_OUTGOING_CONN) != 0,
                });
            }
        }

        if let Some(bytes) = dict.get(b"added6".as_slice()).and_then(|v| v.as_bytes()) {
            for chunk in bytes.as_chunks::<18>().0 {
                let mut octets = [0u8; 16];
                octets.copy_from_slice(&chunk[..16]);
                let ip = Ipv6Addr::from(octets);
                let port = u16::from_be_bytes([chunk[16], chunk[17]]);
                msg.added6.push(SocketAddrV6::new(ip, port, 0, 0));
            }
        }

        if let Some(bytes) = dict.get(b"added6.f".as_slice()).and_then(|v| v.as_bytes()) {
            for &byte in bytes {
                msg.added6_flags.push(PexFlags {
                    prefers_encryption: (byte & PEX_PREFERS_ENCRYPTION) != 0,
                    seed_only: (byte & PEX_SEED_UPLOAD_ONLY) != 0,
                    supports_utp: (byte & PEX_SUPPORTS_UTP) != 0,
                    supports_holepunch: (byte & PEX_HOLEPUNCH_SUPPORT) != 0,
                    reachable: (byte & PEX_OUTGOING_CONN) != 0,
                });
            }
        }

        if let Some(bytes) = dict.get(b"dropped".as_slice()).and_then(|v| v.as_bytes()) {
            for chunk in bytes.as_chunks::<6>().0 {
                let ip = Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
                let port = u16::from_be_bytes([chunk[4], chunk[5]]);
                msg.dropped.push(SocketAddrV4::new(ip, port));
            }
        }

        if let Some(bytes) = dict.get(b"dropped6".as_slice()).and_then(|v| v.as_bytes()) {
            for chunk in bytes.as_chunks::<18>().0 {
                let mut octets = [0u8; 16];
                octets.copy_from_slice(&chunk[..16]);
                let ip = Ipv6Addr::from(octets);
                let port = u16::from_be_bytes([chunk[16], chunk[17]]);
                msg.dropped6.push(SocketAddrV6::new(ip, port, 0, 0));
            }
        }

        Ok(msg)
    }

    pub fn to_message(&self, pex_ext_id: u8) -> Message {
        Message::Extended {
            id: pex_ext_id,
            payload: self.encode(),
        }
    }
}

/// Local LTEP protocol map managing extension IDs.
#[derive(Debug, Clone, Default)]
pub struct LocalLtepProtocolMap {
    pub index: Vec<String>,
    pub num_builtin: usize,
}

impl LocalLtepProtocolMap {
    pub fn new() -> Self {
        Self {
            index: Vec::new(),
            num_builtin: 0,
        }
    }

    pub fn to_supported_extension_dict(&self) -> BTreeMap<String, u8> {
        let mut map = BTreeMap::new();
        for (i, name) in self.index.iter().enumerate() {
            map.insert(name.clone(), (i + 1) as u8);
        }
        map
    }

    pub fn lookup_id(&self, id: u8) -> io::Result<(String, bool)> {
        if id == 0 {
            return Ok(("handshake".to_string(), true));
        }
        let protocol_idx = (id - 1) as usize;
        if protocol_idx >= self.index.len() {
            return Err(Error::new(
                ErrorKind::NotFound,
                format!("unexpected extended message ID: {id}"),
            ));
        }
        let builtin = protocol_idx < self.num_builtin;
        let name = self.index[protocol_idx].clone();
        Ok((name, builtin))
    }

    pub fn add_user_protocol(&mut self, name: &str) {
        self.index.retain(|n| n != name);
        self.index.push(name.to_string());
    }

    pub fn add_builtin_protocol(&mut self, name: &str) {
        self.index.retain(|n| n != name);
        self.index.insert(self.num_builtin, name.to_string());
        self.num_builtin += 1;
    }
}
