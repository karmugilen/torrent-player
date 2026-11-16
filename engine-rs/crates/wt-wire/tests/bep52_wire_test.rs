//! BEP 52 wire format: hash request (21), hashes (22), hash reject (23) and
//! the v2 handshake reserved bit. Byte layouts are written out by hand from
//! the BEP so an encoder/decoder that agree with each other but not with
//! libtorrent would still fail.

use bytes::{Bytes, BytesMut};
use wt_wire::codec::decode_message;
use wt_wire::handshake::{
    default_peer_extension_bytes, PeerExtensionBits, EXTENSION_BIT_V2_UPGRADE,
};
use wt_wire::{validate_hash_request, Message};

const ROOT: [u8; 32] = [0xab; 32];

/// `<len u32><id u8><pieces root 32><base layer u32><index u32><length u32><proof layers u32>`
fn header(id: u8, extra: usize) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&((1 + 32 + 16 + extra) as u32).to_be_bytes());
    v.push(id);
    v.extend_from_slice(&ROOT);
    v.extend_from_slice(&3u32.to_be_bytes()); // base layer
    v.extend_from_slice(&512u32.to_be_bytes()); // index
    v.extend_from_slice(&256u32.to_be_bytes()); // length
    v.extend_from_slice(&2u32.to_be_bytes()); // proof layers
    v
}

#[test]
fn hash_request_byte_layout() {
    let msg = Message::HashRequest {
        pieces_root: ROOT,
        base_layer: 3,
        index: 512,
        length: 256,
        proof_layers: 2,
    };
    let bytes = msg.to_bytes();
    assert_eq!(&bytes[..], &header(21, 0)[..]);
    let mut buf = BytesMut::from(&bytes[..]);
    assert_eq!(decode_message(&mut buf, 1 << 20).unwrap(), Some(msg));
    assert!(buf.is_empty());
}

#[test]
fn hash_reject_byte_layout() {
    let msg = Message::HashReject {
        pieces_root: ROOT,
        base_layer: 3,
        index: 512,
        length: 256,
        proof_layers: 2,
    };
    assert_eq!(&msg.to_bytes()[..], &header(23, 0)[..]);
    let mut buf = BytesMut::from(&header(23, 0)[..]);
    assert_eq!(decode_message(&mut buf, 1 << 20).unwrap(), Some(msg));
}

#[test]
fn hashes_byte_layout_with_trailing_hashes() {
    let hashes: Vec<[u8; 32]> = (0..5u8).map(|i| [i; 32]).collect();
    let msg = Message::Hashes {
        pieces_root: ROOT,
        base_layer: 3,
        index: 512,
        length: 256,
        proof_layers: 2,
        hashes: hashes.clone(),
    };
    let mut expected = header(22, 5 * 32);
    for h in &hashes {
        expected.extend_from_slice(h);
    }
    assert_eq!(&msg.to_bytes()[..], &expected[..]);
    let mut buf = BytesMut::from(&expected[..]);
    assert_eq!(decode_message(&mut buf, 1 << 20).unwrap(), Some(msg));
}

#[test]
fn malformed_hash_messages_are_errors_not_panics() {
    // Every truncation and every non-multiple-of-32 tail.
    for id in [21u8, 22, 23] {
        let full = header(id, 0);
        let body = &full[4..];
        for cut in 1..body.len() {
            let r = Message::decode_payload(Bytes::copy_from_slice(&body[..cut]));
            assert!(r.is_err(), "id {id} truncated to {cut} decoded");
        }
    }
    for tail in 1..32 {
        let mut body = header(22, 0)[4..].to_vec();
        body.extend(std::iter::repeat_n(0u8, 32 + tail));
        assert!(Message::decode_payload(Bytes::from(body)).is_err());
    }
    // Request/reject must be exactly 48 bytes after the id.
    for id in [21u8, 23] {
        let mut body = header(id, 0)[4..].to_vec();
        body.extend_from_slice(&[0u8; 32]);
        assert!(Message::decode_payload(Bytes::from(body)).is_err());
    }
}

#[test]
fn hashes_count_is_bounded_by_max_message_length() {
    // A peer cannot make us allocate for hashes it did not send: the frame
    // length is checked before the body is buffered.
    let mut frame = Vec::new();
    frame.extend_from_slice(&u32::MAX.to_be_bytes());
    frame.push(22);
    let mut buf = BytesMut::from(&frame[..]);
    assert!(decode_message(&mut buf, 256 * 1024).is_err());
}

#[test]
fn hash_request_bounds() {
    assert!(validate_hash_request(0, 0, 2, 0).is_ok());
    assert!(validate_hash_request(2, 512, 512, 5).is_ok());
    assert!(validate_hash_request(0, 8192, 8192, 0).is_ok());
    // length: power of two in 2..=8192
    for len in [0u32, 1, 3, 6, 513, 16384, u32::MAX] {
        assert!(validate_hash_request(0, 0, len, 0).is_err(), "length {len}");
    }
    // index must be a multiple of length
    assert!(validate_hash_request(0, 3, 2, 0).is_err());
    assert!(validate_hash_request(0, 256, 512, 0).is_err());
    // layers beyond any i64-sized tree
    assert!(validate_hash_request(64, 0, 2, 0).is_err());
    assert!(validate_hash_request(0, 0, 2, 64).is_err());
    assert!(validate_hash_request(40, 0, 2, 30).is_err());
    assert!(validate_hash_request(u32::MAX, 0, 2, u32::MAX).is_err());
}

#[test]
fn v2_upgrade_reserved_bit_is_byte7_0x10() {
    // BEP 52: "bit 4 of the last reserved byte" (reserved[7] & 0x10).
    let bits = PeerExtensionBits::with_bits(&[EXTENSION_BIT_V2_UPGRADE]);
    assert_eq!(bits.0, [0, 0, 0, 0, 0, 0, 0, 0x10]);
    assert!(bits.supports_v2_upgrade());
    // Neighbouring bits are DHT (0x01) and Fast (0x04), not v2.
    assert!(!PeerExtensionBits([0, 0, 0, 0, 0, 0, 0, 0x05]).supports_v2_upgrade());
    assert!(!PeerExtensionBits([0x10, 0, 0, 0, 0, 0, 0, 0]).supports_v2_upgrade());
    // The default set does not advertise v2; dialers add it per-torrent.
    assert!(!default_peer_extension_bytes().supports_v2_upgrade());
}

#[test]
fn extended_handshake_hostile_payloads() {
    use wt_wire::ExtendedHandshake;
    // Regression: deep nesting from a peer aborted the process (stack overflow).
    let n = 500_000;
    let mut deep = b"d1:m".to_vec();
    deep.extend(std::iter::repeat_n(b'l', n));
    deep.extend(std::iter::repeat_n(b'e', n));
    deep.push(b'e');
    assert!(ExtendedHandshake::decode(&deep).is_err());

    // Regression: out-of-range integers wrapped through `as u32`.
    let h = ExtendedHandshake::decode(b"d13:metadata_sizei4294967396e4:reqqi-1ee").unwrap();
    assert_eq!(h.metadata_size, None);
    assert_eq!(h.reqq, None);
    let h = ExtendedHandshake::decode(b"d13:metadata_sizei31235e4:reqqi250ee").unwrap();
    assert_eq!(h.metadata_size, Some(31235));
    assert_eq!(h.reqq, Some(250));
}
