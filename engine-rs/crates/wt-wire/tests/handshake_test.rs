use tokio::io::duplex;
use wt_types::{InfoHash, PeerId};
use wt_wire::handshake::{default_peer_extension_bytes, handshake, handshake_incoming};

#[tokio::test]
async fn test_handshake_concurrent_exchange() {
    let (mut client, mut server) = duplex(1024);

    let info_hash = InfoHash([0xab; 20]);
    let client_id: PeerId = [0x01; 20];
    let server_id: PeerId = [0x02; 20];
    let ext = default_peer_extension_bytes();

    let client_fut = handshake(&mut client, &info_hash, &client_id, &ext);
    let server_fut = handshake(&mut server, &info_hash, &server_id, &ext);

    let (c_res, s_res) = tokio::try_join!(client_fut, server_fut).unwrap();

    assert_eq!(c_res.peer_id, server_id);
    assert_eq!(c_res.info_hash, info_hash);
    assert_eq!(s_res.peer_id, client_id);
    assert_eq!(s_res.info_hash, info_hash);
}

#[tokio::test]
async fn test_handshake_incoming_lookup_success() {
    let (mut client, mut server) = duplex(1024);

    let info_hash = InfoHash([0xcd; 20]);
    let client_id: PeerId = [0x11; 20];
    let server_id: PeerId = [0x22; 20];
    let ext = default_peer_extension_bytes();

    let expected_ih = info_hash;
    let client_fut = handshake(&mut client, &info_hash, &client_id, &ext);
    let server_fut =
        handshake_incoming(&mut server, &server_id, &ext, move |ih| *ih == expected_ih);

    let (c_res, s_res) = tokio::try_join!(client_fut, server_fut).unwrap();

    assert_eq!(c_res.peer_id, server_id);
    assert_eq!(s_res.peer_id, client_id);
    assert_eq!(s_res.info_hash, info_hash);
}

#[tokio::test]
async fn test_handshake_incoming_lookup_rejected() {
    let (mut client, mut server) = duplex(1024);

    let info_hash = InfoHash([0xef; 20]);
    let client_id: PeerId = [0x33; 20];
    let server_id: PeerId = [0x44; 20];
    let ext = default_peer_extension_bytes();

    let client_task =
        tokio::spawn(async move { handshake(&mut client, &info_hash, &client_id, &ext).await });
    let server_task = tokio::spawn(async move {
        handshake_incoming(&mut server, &server_id, &ext, |_ih| false).await
    });

    let (c_res, s_res) = tokio::join!(client_task, server_task);
    assert!(
        s_res.unwrap().is_err(),
        "expected rejection from lookup callback"
    );
    assert!(
        c_res.unwrap().is_err(),
        "client should receive error due to closed server"
    );
}
