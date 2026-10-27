use bytes::BytesMut;
use wt_wire::handshake::{
    default_peer_extension_bytes, PeerExtensionBits, EXTENSION_BIT_V2_UPGRADE,
};
use wt_wire::message::{marshal_bitfield, unmarshal_bitfield, Message, MessageType};

#[test]
fn test_constants() {
    assert_eq!(MessageType::NotInterested as u8, 3);
    assert_eq!(MessageType::HaveAll as u8, 14);
}

#[test]
fn test_bitfield_encode() {
    let mut bf = vec![false; 37];
    bf[2] = true;
    bf[7] = true;
    bf[32] = true;
    let b = marshal_bitfield(&bf);
    let expected = b"\x21\x00\x00\x00\x80";
    assert_eq!(&b[..], expected);
}

#[test]
fn test_bitfield_unmarshal() {
    let bf = unmarshal_bitfield(b"\x81\x06");
    let mut expected = vec![false; 16];
    expected[0] = true;
    expected[7] = true;
    expected[13] = true;
    expected[14] = true;
    assert_eq!(bf, expected);
}

#[test]
fn test_have_encode() {
    let msg = Message::Have { index: 42 };
    let bytes = msg.to_bytes();
    let expected = b"\x00\x00\x00\x05\x04\x00\x00\x00\x2a";
    assert_eq!(&bytes[..], expected);
}

#[test]
fn test_marshal_keepalive() {
    let msg = Message::Keepalive;
    let bytes = msg.to_bytes();
    let expected = b"\x00\x00\x00\x00";
    assert_eq!(&bytes[..], expected);
}

#[test]
fn test_marshal_port_msg() {
    let msg = Message::Port(0xaabb);
    let bytes = msg.to_bytes();
    let expected = b"\x00\x00\x00\x03\x09\xaa\xbb";
    assert_eq!(&bytes[..], expected);
}

#[test]
fn test_unmarshal_port_msg() {
    let mut buf = BytesMut::from(&b"\x00\x00\x00\x03\x09\xaa\xbb"[..]);
    let msg = wt_wire::codec::decode_message(&mut buf, 8)
        .unwrap()
        .unwrap();
    assert_eq!(msg, Message::Port(0xaabb));
}

#[test]
fn test_v2_bit_location() {
    let mut bits = PeerExtensionBits::new();
    bits.set_bit(EXTENSION_BIT_V2_UPGRADE, true);
    assert_eq!(bits.0[7], 0x10);
}

#[test]
fn test_default_extension_bytes() {
    let pex = default_peer_extension_bytes();
    assert!(pex.supports_dht());
    assert!(pex.supports_extended());
    assert!(pex.supports_fast());
    assert!(!pex.get_bit(63));
}
