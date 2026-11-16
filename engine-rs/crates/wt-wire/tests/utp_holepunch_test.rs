//! uTP behaviour BEP 55 hole punching relies on: both ends dial each other
//! from their listen sockets at once, and SYNs are resent until answered.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;
use wt_wire::utp::{
    PacketHeader, UtpSocket, UtpStream, HEADER_SIZE, MAX_PACKET_SIZE, ST_DATA, ST_STATE, ST_SYN,
};

async fn bind() -> Arc<UtpSocket> {
    Arc::new(UtpSocket::bind("127.0.0.1:0".parse::<SocketAddr>().unwrap()).await.unwrap())
}

/// Both sides connect at once. Exactly one uTP connection must come out of
/// it, the same one on both ends, and carry data both ways.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_open_yields_one_shared_connection() {
    for _ in 0..20 {
        let a = bind().await;
        let b = bind().await;
        let (a_addr, b_addr) = (a.local_addr().unwrap(), b.local_addr().unwrap());

        let resend = Some(Duration::from_millis(100));
        let (ra, rb) = tokio::join!(
            a.connect_with_syn_resend(b_addr, resend),
            b.connect_with_syn_resend(a_addr, resend)
        );
        // One side's dial wins, the other side yields and accepts.
        let (mut dialer, accept_side): (UtpStream, &Arc<UtpSocket>) = match (ra, rb) {
            (Ok(s), Err(e)) => {
                assert!(e.to_string().contains("simultaneous open"), "{e}");
                (s, &b)
            }
            (Err(e), Ok(s)) => {
                assert!(e.to_string().contains("simultaneous open"), "{e}");
                (s, &a)
            }
            (Ok(_), Ok(_)) => panic!("both dials succeeded: two connections"),
            (Err(x), Err(y)) => panic!("both dials failed: {x} / {y}"),
        };
        let mut accepted = tokio::time::timeout(Duration::from_secs(2), accept_side.accept())
            .await
            .expect("yielding side accepts the winner")
            .unwrap();

        dialer.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        accepted.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
        accepted.write_all(b"pong").await.unwrap();
        dialer.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"pong");

        // No second connection lingers on the accepting side.
        assert!(
            tokio::time::timeout(Duration::from_millis(50), accept_side.accept()).await.is_err(),
            "second stream accepted"
        );
    }
}

/// The first SYN is lost (the remote NAT had no mapping yet); a resend gets
/// through. Without resending, the dial waits out its whole timeout.
#[tokio::test]
async fn syn_resend_survives_lost_first_syn_and_duplicate_syn_is_one_stream() {
    let raw = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let raw_addr = raw.local_addr().unwrap();
    let client = bind().await;

    let peer = tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        // Drop the first SYN on the floor.
        let (_, _) = raw.recv_from(&mut buf).await.unwrap();
        let (n, from) = raw.recv_from(&mut buf).await.unwrap();
        let mut b = bytes::Bytes::copy_from_slice(&buf[..n]);
        let syn = PacketHeader::decode(&mut b).unwrap();
        assert_eq!(syn.type_ver >> 4, ST_SYN, "resent packet is a SYN");
        let resp = PacketHeader {
            type_ver: (ST_STATE << 4) | 1,
            extension: 0,
            connection_id: syn.connection_id,
            timestamp_micros: 0,
            timestamp_diff_micros: 0,
            wnd_size: 1 << 20,
            seq_nr: 7,
            ack_nr: syn.seq_nr,
        };
        let mut out = bytes::BytesMut::with_capacity(HEADER_SIZE);
        resp.encode(&mut out);
        raw.send_to(&out, from).await.unwrap();
    });

    let started = std::time::Instant::now();
    let stream = client
        .connect_with_syn_resend(raw_addr, Some(Duration::from_millis(100)))
        .await
        .expect("resent SYN connects");
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(client.has_stream_to(raw_addr));
    drop(stream);
    peer.await.unwrap();

    // Acceptor side: the same SYN twice must give one stream, two SYN-ACKs.
    let server = bind().await;
    let server_addr = server.local_addr().unwrap();
    let raw = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let syn = PacketHeader {
        type_ver: (ST_SYN << 4) | 1,
        extension: 0,
        connection_id: 1234,
        timestamp_micros: 0,
        timestamp_diff_micros: 0,
        wnd_size: 1 << 20,
        seq_nr: 1,
        ack_nr: 0,
    };
    let mut out = bytes::BytesMut::with_capacity(HEADER_SIZE);
    syn.encode(&mut out);
    raw.send_to(&out, server_addr).await.unwrap();
    raw.send_to(&out, server_addr).await.unwrap();
    let mut buf = [0u8; 2048];
    for _ in 0..2 {
        let (n, _) = tokio::time::timeout(Duration::from_secs(2), raw.recv_from(&mut buf))
            .await
            .expect("SYN-ACK")
            .unwrap();
        let mut b = bytes::Bytes::copy_from_slice(&buf[..n]);
        let h = PacketHeader::decode(&mut b).unwrap();
        assert_eq!(h.type_ver >> 4, ST_STATE);
        assert_eq!(h.connection_id, 1234);
    }
    let _first = tokio::time::timeout(Duration::from_secs(1), server.accept())
        .await
        .unwrap()
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), server.accept()).await.is_err(),
        "duplicate SYN opened a second stream"
    );
}

/// Data packets stay MTU sized, loopback included. Loopback used to get
/// 8 KiB payloads, which the batched receive (2 KiB slots) drops as
/// truncated: no data ever arrived and every loopback transfer stalled.
#[tokio::test]
async fn data_packets_fit_the_mtu_on_loopback() {
    let raw = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let raw_addr = raw.local_addr().unwrap();
    let client = bind().await;
    let peer = tokio::spawn(async move {
        let mut buf = vec![0u8; 65_536];
        let (n, from) = raw.recv_from(&mut buf).await.unwrap();
        let mut b = bytes::Bytes::copy_from_slice(&buf[..n]);
        let syn = PacketHeader::decode(&mut b).unwrap();
        let resp = PacketHeader {
            type_ver: (ST_STATE << 4) | 1,
            extension: 0,
            connection_id: syn.connection_id,
            timestamp_micros: 0,
            timestamp_diff_micros: 0,
            wnd_size: 1 << 20,
            seq_nr: 7,
            ack_nr: syn.seq_nr,
        };
        let mut out = bytes::BytesMut::with_capacity(HEADER_SIZE);
        resp.encode(&mut out);
        raw.send_to(&out, from).await.unwrap();
        let mut payload = 0usize;
        while payload < 32 * 1024 {
            let (n, _) = tokio::time::timeout(Duration::from_secs(2), raw.recv_from(&mut buf))
                .await
                .expect("data packet")
                .unwrap();
            assert!(n <= MAX_PACKET_SIZE, "datagram of {n} bytes exceeds the uTP MTU");
            if buf[0] >> 4 == ST_DATA {
                payload += n - HEADER_SIZE;
            }
        }
    });
    let mut stream = client.connect(raw_addr).await.unwrap();
    stream.write_all(&vec![0x5a; 32 * 1024]).await.unwrap();
    peer.await.unwrap();
}

/// A punched connection must carry a real transfer both ways, not just a
/// ping: 1 MiB each direction over a simultaneous-open stream.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn punched_connection_carries_bulk_data_both_ways() {
    let a = bind().await;
    let b = bind().await;
    let (a_addr, b_addr) = (a.local_addr().unwrap(), b.local_addr().unwrap());
    let resend = Some(Duration::from_millis(100));
    let (ra, rb) = tokio::join!(
        a.connect_with_syn_resend(b_addr, resend),
        b.connect_with_syn_resend(a_addr, resend)
    );
    let (dialer, accept_side) = match (ra, rb) {
        (Ok(s), Err(_)) => (s, &b),
        (Err(_), Ok(s)) => (s, &a),
        other => panic!("expected one winning dial: {:?}", other.0.is_ok()),
    };
    let accepted = tokio::time::timeout(Duration::from_secs(2), accept_side.accept())
        .await
        .unwrap()
        .unwrap();

    const TOTAL: usize = 1 << 20;
    let data: Vec<u8> = (0..TOTAL).map(|i| (i % 251) as u8).collect();
    let pump = |s: UtpStream, data: Vec<u8>| async move {
        let (mut rh, mut wh) = tokio::io::split(s);
        let w = tokio::spawn(async move { wh.write_all(&data).await.unwrap(); wh });
        let mut got = vec![0u8; TOTAL];
        rh.read_exact(&mut got).await.unwrap();
        let _wh = w.await.unwrap();
        got
    };
    let (x, y) = tokio::time::timeout(
        Duration::from_secs(20),
        async { tokio::join!(pump(dialer, data.clone()), pump(accepted, data.clone())) },
    )
    .await
    .expect("bulk transfer over punched uTP stalled");
    assert!(x == data && y == data, "payload corrupted");
}
