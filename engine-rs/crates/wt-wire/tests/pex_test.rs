use std::net::{Ipv4Addr, SocketAddrV4};
use wt_types::PexFlags;
use wt_wire::codec::decode_message;
use wt_wire::ext::PexMsg;
use wt_wire::message::Message;

#[test]
fn test_unmarshal_pex() {
    let payload = b"d5:added12:\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0ce";
    let pex = PexMsg::decode(payload).unwrap();
    assert_eq!(pex.added.len(), 2);
    assert_eq!(pex.added[0].port(), 1286);
    assert_eq!(pex.added[1].port(), 0x100 * 0xb + 0xc);
}

#[test]
fn test_empty_pex_msg() {
    let pex = PexMsg::new();
    let encoded = pex.encode();
    let decoded = PexMsg::decode(&encoded).unwrap();
    assert_eq!(pex, decoded);
}

#[test]
fn test_marshal_pex_message() {
    let mut pm = PexMsg::new();
    let addr = SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 0x55aa);
    let flags = PexFlags {
        prefers_encryption: true,
        seed_only: false,
        supports_utp: false,
        supports_holepunch: false,
        reachable: true,
    };
    pm.added.push(addr);
    pm.added_flags.push(flags);

    let pex_ext_id = 7u8;
    let msg = pm.to_message(pex_ext_id);

    let expected = b"\x00\x00\x00\x4c\x14\x07d5:added6:\x7f\x00\x00\x01\x55\xaa7:added.f1:\x116:added60:8:added6.f0:7:dropped0:8:dropped60:e";
    let bytes = msg.to_bytes();
    assert_eq!(&bytes[..], expected);

    // Decode back
    let mut buf = bytes::BytesMut::from(&bytes[..]);
    let decoded_msg = decode_message(&mut buf, 128).unwrap().unwrap();
    match decoded_msg {
        Message::Extended { id, payload } => {
            assert_eq!(id, pex_ext_id);
            let pm_out = PexMsg::decode(&payload).unwrap();
            assert_eq!(pm.added.len(), pm_out.added.len());
            assert_eq!(pm.added[0].ip(), pm_out.added[0].ip());
            assert_eq!(pm.added[0].port(), pm_out.added[0].port());
            assert_eq!(pm.added_flags[0], pm_out.added_flags[0]);
        }
        other => panic!("unexpected message: {other:?}"),
    }
}
