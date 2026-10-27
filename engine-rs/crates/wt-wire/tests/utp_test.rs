use bytes::BytesMut;
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;
use wt_wire::utp::{PacketHeader, UtpSocket, EXT_NONE, HEADER_SIZE, ST_DATA, ST_STATE, ST_SYN};

/// BEP 29: a raw UDP peer that speaks correct connection IDs must complete
/// handshake with our initiator (send=A+1, recv=A after SYN with A).
#[tokio::test]
async fn test_utp_bep29_ids_against_raw_acceptor() {
    let raw = UdpSocket::bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .await
        .unwrap();
    let raw_addr = raw.local_addr().unwrap();

    let client = UtpSocket::bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .await
        .unwrap();

    let acceptor = tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        let (n, peer) = raw.recv_from(&mut buf).await.unwrap();
        assert!(n >= HEADER_SIZE);
        let mut slice = bytes::Bytes::copy_from_slice(&buf[..n]);
        let syn = PacketHeader::decode(&mut slice).unwrap();
        assert_eq!(syn.type_ver >> 4, ST_SYN);
        let a = syn.connection_id;
        // Acceptor: send=A, recv=A+1 (libutp / BEP diagram)
        let send_id = a;
        let _recv_id = a.wrapping_add(1);
        let resp = PacketHeader {
            type_ver: (ST_STATE << 4) | 1,
            extension: EXT_NONE,
            connection_id: send_id,
            timestamp_micros: 0,
            timestamp_diff_micros: 0,
            wnd_size: 1_048_576,
            seq_nr: 42,
            ack_nr: syn.seq_nr,
        };
        let mut out = BytesMut::with_capacity(HEADER_SIZE);
        resp.encode(&mut out);
        raw.send_to(&out, peer).await.unwrap();

        // First DATA from initiator must use connection_id = A+1
        let (n, _) = raw.recv_from(&mut buf).await.unwrap();
        let mut slice = bytes::Bytes::copy_from_slice(&buf[..n]);
        let data = PacketHeader::decode(&mut slice).unwrap();
        assert_eq!(
            data.type_ver >> 4,
            ST_DATA,
            "expected ST_DATA after connect"
        );
        assert_eq!(
            data.connection_id,
            a.wrapping_add(1),
            "initiator must send with conn_id_recv+1 (BEP 29)"
        );
        a
    });

    let client_task = tokio::spawn(async move {
        let mut stream = client.connect(raw_addr).await.unwrap();
        stream.write_all(b"ping").await.unwrap();
        stream.flush().await.unwrap();
    });

    let (a, _) = tokio::join!(acceptor, client_task);
    a.unwrap();
}

#[tokio::test]
async fn test_utp_demux_non_utp() {
    let utp_sock = UtpSocket::bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .await
        .unwrap();
    let utp_addr = utp_sock.local_addr().unwrap();

    let raw_sock = UdpSocket::bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .await
        .unwrap();
    let raw_addr = raw_sock.local_addr().unwrap();

    // Send a non-uTP packet (e.g. DHT ping) to the uTP socket
    let dht_packet = b"d1:ad2:id20:abcdefghij0123456789e1:q4:ping1:t2:aa1:y1:qe";
    raw_sock.send_to(dht_packet, utp_addr).await.unwrap();

    // The uTP socket should demux this non-uTP packet to recv_non_utp()
    let (received, sender) = utp_sock.recv_non_utp().await.unwrap();
    assert_eq!(&received[..], dht_packet);
    assert_eq!(sender, raw_addr);

    // Send a raw UDP response back via send_non_utp
    let dht_response = b"d1:rd2:id20:0123456789abcdefghije1:t2:aa1:y1:re";
    utp_sock.send_non_utp(dht_response, sender).await.unwrap();

    let mut buf = [0u8; 1024];
    let (n, from) = raw_sock.recv_from(&mut buf).await.unwrap();
    assert_eq!(&buf[..n], dht_response);
    assert_eq!(from, utp_addr);
}

#[tokio::test]
async fn test_utp_basic_transfer() {
    let server_sock = UtpSocket::bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .await
        .unwrap();
    let server_addr = server_sock.local_addr().unwrap();

    let client_sock = UtpSocket::bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .await
        .unwrap();

    let server_task = tokio::spawn(async move {
        let mut stream = server_sock.accept().await.unwrap();
        let mut buf = vec![0u8; 16];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf[..], b"hello utp world!");
        stream.write_all(b"greetings client").await.unwrap();
        stream.flush().await.unwrap();
    });

    let client_task = tokio::spawn(async move {
        let mut stream = client_sock.connect(server_addr).await.unwrap();
        stream.write_all(b"hello utp world!").await.unwrap();
        stream.flush().await.unwrap();

        let mut buf = vec![0u8; 16];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf[..], b"greetings client");
    });

    let (r1, r2) = tokio::join!(server_task, client_task);
    r1.unwrap();
    r2.unwrap();
}

#[tokio::test]
async fn test_utp_loopback_64mib() {
    let server_sock = UtpSocket::bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .await
        .unwrap();
    let server_addr = server_sock.local_addr().unwrap();

    let client_sock = UtpSocket::bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .await
        .unwrap();

    const TRANSFER_SIZE: usize = 64 * 1024 * 1024; // 64 MiB

    // Prepare patterned data
    let mut data = Vec::with_capacity(TRANSFER_SIZE);
    for i in 0..TRANSFER_SIZE {
        data.push((i % 251) as u8);
    }
    let expected_hash = Sha256::digest(&data);

    let server_task = tokio::spawn(async move {
        let mut stream = server_sock.accept().await.unwrap();
        let mut received = Vec::with_capacity(TRANSFER_SIZE);
        let mut chunk = vec![0u8; 64 * 1024];

        while received.len() < TRANSFER_SIZE {
            let to_read = std::cmp::min(chunk.len(), TRANSFER_SIZE - received.len());
            let n = stream.read(&mut chunk[..to_read]).await.unwrap();
            if n == 0 {
                break;
            }
            received.extend_from_slice(&chunk[..n]);
        }
        assert_eq!(
            received.len(),
            TRANSFER_SIZE,
            "server received incomplete data"
        );
        let hash = Sha256::digest(&received);
        assert_eq!(hash, expected_hash, "data hash mismatch");
    });

    let client_task = tokio::spawn(async move {
        let mut stream = client_sock.connect(server_addr).await.unwrap();
        let start = Instant::now();

        let mut offset = 0;
        let chunk_size = 64 * 1024;
        while offset < TRANSFER_SIZE {
            let end = std::cmp::min(offset + chunk_size, TRANSFER_SIZE);
            stream.write_all(&data[offset..end]).await.unwrap();
            offset = end;
        }
        stream.shutdown().await.unwrap();

        let elapsed = start.elapsed();
        let mb = TRANSFER_SIZE as f64 / (1024.0 * 1024.0);
        let mbps = mb / elapsed.as_secs_f64();
        println!("uTP 64 MiB transfer took {elapsed:?}, throughput: {mbps:.2} MB/s");
        assert!(
            mbps >= 100.0,
            "throughput {mbps:.2} MB/s < 100 MB/s requirement"
        );
    });

    let (r1, r2) = tokio::join!(server_task, client_task);
    r1.unwrap();
    r2.unwrap();
}

/// Manual public-swarm probe (ignored). Run with:
/// PUBLIC_UTP_PEER=107.175.93.162:6881 cargo test -p wt-wire --test utp_test test_utp_public_peer_hs -- --ignored --nocapture
#[tokio::test]
#[ignore]
async fn test_utp_public_peer_hs() {
    let addr: SocketAddr = std::env::var("PUBLIC_UTP_PEER")
        .unwrap_or_else(|_| "107.175.93.162:6881".into())
        .parse()
        .unwrap();
    let sock = UtpSocket::bind("0.0.0.0:0".parse::<SocketAddr>().unwrap())
        .await
        .unwrap();
    println!("local {:?}", sock.local_addr().unwrap());
    let t0 = Instant::now();
    let mut stream = match tokio::time::timeout(std::time::Duration::from_secs(8), sock.connect(addr)).await {
        Ok(Ok(s)) => {
            println!("utp connect ok in {:?}", t0.elapsed());
            s
        }
        Ok(Err(e)) => panic!("connect err {e} in {:?}", t0.elapsed()),
        Err(_) => panic!("connect timeout in {:?}", t0.elapsed()),
    };
    let mut hs = vec![19u8];
    hs.extend_from_slice(b"BitTorrent protocol");
    hs.extend_from_slice(&[0u8; 8]);
    hs.extend_from_slice(&hex::decode("d160b8d8ea35a5b4e52837468fc8f03d55cef1f7").unwrap());
    hs.extend_from_slice(b"-WW0001-012345678901");
    tokio::time::timeout(std::time::Duration::from_secs(10), stream.write_all(&hs))
        .await
        .expect("write timeout")
        .expect("write err");
    let _ = stream.flush().await;
    println!("wrote hs in {:?}", t0.elapsed());
    let mut buf = [0u8; 68];
    match tokio::time::timeout(std::time::Duration::from_secs(10), stream.read_exact(&mut buf)).await {
        Ok(Ok(_)) => println!("read hs ok in {:?} reserved={:?} peer_id={:?}", t0.elapsed(), &buf[20..28], String::from_utf8_lossy(&buf[48..])),
        Ok(Err(e)) => panic!("read err {e} after {:?}", t0.elapsed()),
        Err(_) => panic!("read timeout after {:?}", t0.elapsed()),
    }
}
