use bytes::{BufMut, BytesMut};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;
use wt_wire::utp::{
    split_extensions, PacketHeader, UtpSocket, EXT_NONE, EXT_SELECTIVE_ACK, HEADER_SIZE, ST_DATA,
    ST_STATE, ST_SYN,
};

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
    let mut stream =
        match tokio::time::timeout(std::time::Duration::from_secs(8), sock.connect(addr)).await {
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
    match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        stream.read_exact(&mut buf),
    )
    .await
    {
        Ok(Ok(_)) => println!(
            "read hs ok in {:?} reserved={:?} peer_id={:?}",
            t0.elapsed(),
            &buf[20..28],
            String::from_utf8_lossy(&buf[48..])
        ),
        Ok(Err(e)) => panic!("read err {e} after {:?}", t0.elapsed()),
        Err(_) => panic!("read timeout after {:?}", t0.elapsed()),
    }
}

/// Concurrent bidirectional multi-chunk transfer (Request/Piece-like).
/// Reproduces stalls that unidirectional 64 MiB misses.
#[tokio::test]
async fn test_utp_bidirectional_multichunk() {
    let server_sock = UtpSocket::bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .await
        .unwrap();
    let server_addr = server_sock.local_addr().unwrap();
    let client_sock = UtpSocket::bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .await
        .unwrap();

    const CHUNK: usize = 16 * 1024;
    const N: usize = 64; // 1 MiB each direction
    const TOTAL: usize = CHUNK * N;

    let server_task = tokio::spawn(async move {
        let stream = server_sock.accept().await.unwrap();
        let (mut rh, mut wh) = tokio::io::split(stream);

        let writer = tokio::spawn(async move {
            let mut sent = 0usize;
            let mut payload = vec![0u8; CHUNK];
            while sent < TOTAL {
                for (i, b) in payload.iter_mut().enumerate() {
                    *b = ((sent + i) % 251) as u8;
                }
                tokio::io::AsyncWriteExt::write_all(&mut wh, &payload)
                    .await
                    .unwrap();
                sent += CHUNK;
            }
            tokio::io::AsyncWriteExt::flush(&mut wh).await.unwrap();
            sent
        });

        let reader = tokio::spawn(async move {
            let mut got = 0usize;
            let mut buf = vec![0u8; CHUNK];
            while got < TOTAL {
                let n = tokio::io::AsyncReadExt::read(&mut rh, &mut buf)
                    .await
                    .unwrap();
                assert!(n > 0, "unexpected EOF at {got}");
                for (i, b) in buf.iter().enumerate().take(n) {
                    assert_eq!(*b, ((got + i) % 251) as u8, "mismatch at {got}+{i}");
                }
                got += n;
            }
            got
        });

        let (w, r) = tokio::join!(writer, reader);
        assert_eq!(w.unwrap(), TOTAL);
        assert_eq!(r.unwrap(), TOTAL);
    });

    let client_task = tokio::spawn(async move {
        let stream = client_sock.connect(server_addr).await.unwrap();
        let (mut rh, mut wh) = tokio::io::split(stream);

        let writer = tokio::spawn(async move {
            let mut sent = 0usize;
            let mut payload = vec![0u8; CHUNK];
            while sent < TOTAL {
                for (i, b) in payload.iter_mut().enumerate() {
                    *b = ((sent + i) % 251) as u8;
                }
                tokio::io::AsyncWriteExt::write_all(&mut wh, &payload)
                    .await
                    .unwrap();
                sent += CHUNK;
            }
            tokio::io::AsyncWriteExt::flush(&mut wh).await.unwrap();
            sent
        });

        let reader = tokio::spawn(async move {
            let mut got = 0usize;
            let mut buf = vec![0u8; CHUNK];
            while got < TOTAL {
                let n = tokio::io::AsyncReadExt::read(&mut rh, &mut buf)
                    .await
                    .unwrap();
                assert!(n > 0, "unexpected EOF at {got}");
                for (i, b) in buf.iter().enumerate().take(n) {
                    assert_eq!(*b, ((got + i) % 251) as u8, "mismatch at {got}+{i}");
                }
                got += n;
            }
            got
        });

        let (w, r) = tokio::join!(writer, reader);
        assert_eq!(w.unwrap(), TOTAL);
        assert_eq!(r.unwrap(), TOTAL);
    });

    let result = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let (r1, r2) = tokio::join!(server_task, client_task);
        r1.unwrap();
        r2.unwrap();
    })
    .await;
    assert!(
        result.is_ok(),
        "bidirectional multi-chunk uTP transfer timed out"
    );
}

#[test]
fn test_split_extensions_selective_ack() {
    let mut body = BytesMut::new();
    body.put_u8(EXT_NONE);
    body.put_u8(4);
    body.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]);
    body.extend_from_slice(b"PAYLOAD");
    let (payload, sack) = split_extensions(EXT_SELECTIVE_ACK, body.freeze()).expect("split");
    assert_eq!(&payload[..], b"PAYLOAD");
    assert_eq!(sack.as_deref(), Some(&[0x01, 0x00, 0x00, 0x00][..]));
}

/// Peer DATA packets may carry selective-ACK extensions; payload must exclude them.
#[tokio::test]
async fn test_utp_skips_selective_ack_extension_on_data() {
    use wt_wire::utp::{wall_micros, UTP_VERSION};

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
        let mut slice = bytes::Bytes::copy_from_slice(&buf[..n]);
        let syn = PacketHeader::decode(&mut slice).unwrap();
        assert_eq!(syn.type_ver >> 4, ST_SYN);
        let a = syn.connection_id;
        let send_id = a;
        // SYN-ACK
        let resp = PacketHeader {
            type_ver: (ST_STATE << 4) | UTP_VERSION,
            extension: EXT_NONE,
            connection_id: send_id,
            timestamp_micros: wall_micros(),
            timestamp_diff_micros: 0,
            wnd_size: 1_048_576,
            seq_nr: 42,
            ack_nr: syn.seq_nr,
        };
        let mut out = BytesMut::with_capacity(HEADER_SIZE);
        resp.encode(&mut out);
        raw.send_to(&out, peer).await.unwrap();

        // Wait for initiator DATA (BT-like), ACK it
        let (n, _) = raw.recv_from(&mut buf).await.unwrap();
        let mut slice = bytes::Bytes::copy_from_slice(&buf[..n]);
        let data = PacketHeader::decode(&mut slice).unwrap();
        assert_eq!(data.type_ver >> 4, ST_DATA);

        // Reply with DATA that includes a selective-ACK extension before payload.
        // Format after 20-byte header: next_ext_type (0), len, bitmask..., then payload.
        let payload = b"PIECE_OK_PAYLOAD!!"; // 18 bytes
        let mut pkt = BytesMut::new();
        let hdr = PacketHeader {
            type_ver: (ST_DATA << 4) | UTP_VERSION,
            extension: EXT_SELECTIVE_ACK,
            connection_id: send_id,
            timestamp_micros: wall_micros(),
            timestamp_diff_micros: 0,
            wnd_size: 1_048_576,
            seq_nr: 42, // first DATA = STATE.seq (libutp)
            ack_nr: data.seq_nr,
        };
        hdr.encode(&mut pkt);
        pkt.put_u8(EXT_NONE); // next extension type
        pkt.put_u8(4); // sack len
        pkt.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]); // bitmask
        pkt.extend_from_slice(payload);
        raw.send_to(&pkt, peer).await.unwrap();
        payload.len()
    });

    let client_task = tokio::spawn(async move {
        let mut stream = client.connect(raw_addr).await.unwrap();
        stream.write_all(b"request-bytes-here").await.unwrap();
        let mut buf = vec![0u8; 18];
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            stream.read_exact(&mut buf),
        )
        .await
        .expect("read timeout — extension bytes likely poisoned payload framing")
        .expect("read err");
        assert_eq!(&buf[..], b"PIECE_OK_PAYLOAD!!");
    });

    let (a, c) = tokio::join!(acceptor, client_task);
    a.unwrap();
    c.unwrap();
}

/// A rejecting uTP endpoint is a failed raw dial, not a connected transport
/// that may win the race and occupy a half-open slot for a full BT handshake.
#[tokio::test]
async fn test_utp_connect_reset_is_refused() {
    use wt_wire::utp::ST_RESET;
    let raw = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let remote = raw.local_addr().unwrap();
    let client = UtpSocket::bind("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let peer = tokio::spawn(async move {
        let mut buf = [0; 2048];
        let (n, addr) = raw.recv_from(&mut buf).await.unwrap();
        let mut packet = bytes::Bytes::copy_from_slice(&buf[..n]);
        let mut syn = PacketHeader::decode(&mut packet).unwrap();
        syn.type_ver = (ST_RESET << 4) | 1;
        let mut response = BytesMut::new();
        syn.encode(&mut response);
        raw.send_to(&response, addr).await.unwrap();
    });
    let result = tokio::time::timeout(std::time::Duration::from_secs(1), client.connect(remote))
        .await
        .expect("RST must fail the dial promptly");
    assert!(
        matches!(result, Err(ref e) if e.kind() == std::io::ErrorKind::ConnectionRefused),
        "RST must be ConnectionRefused, never a successful connect"
    );
    assert_eq!(client.active_streams_count(), 0);
    peer.await.unwrap();
}

/// Cancellation by a reduced dial timeout or a winning TCP handshake must
/// remove the pending SYN so a late SYN-ACK cannot create an orphan stream.
#[tokio::test]
async fn test_utp_cancelled_connect_ignores_late_syn_ack() {
    let raw = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let remote = raw.local_addr().unwrap();
    let client = UtpSocket::bind("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let (mut syn, addr) = {
        let connect = client.connect(remote);
        tokio::pin!(connect);
        let mut buf = [0; 2048];
        tokio::select! {
            _ = &mut connect => panic!("silent peer cannot connect"),
            received = raw.recv_from(&mut buf) => {
                let (n, addr) = received.unwrap();
                let mut packet = bytes::Bytes::copy_from_slice(&buf[..n]);
                (PacketHeader::decode(&mut packet).unwrap(), addr)
            }
        }
        // Drop the in-flight connect future, just as dial_utp's timeout does.
    };
    syn.type_ver = (ST_STATE << 4) | 1;
    syn.ack_nr = syn.seq_nr;
    let mut response = BytesMut::new();
    syn.encode(&mut response);
    raw.send_to(&response, addr).await.unwrap();
    // In-order datagram barrier: observe that the late response was processed.
    raw.send_to(b"barrier", addr).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), client.recv_non_utp())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        client.active_streams_count(),
        0,
        "cancelled dials must not resurrect streams"
    );
}

#[tokio::test]
async fn test_utp_dropped_socket_releases_udp_port() {
    let socket = UtpSocket::bind("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let addr = socket.local_addr().unwrap();
    drop(socket);
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if UdpSocket::bind(addr).await.is_ok() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("dropped uTP socket must stop its pumps and release its UDP port");
}

#[tokio::test]
async fn test_utp_unknown_reset_is_not_echoed() {
    use wt_wire::utp::ST_RESET;
    let raw = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let client = UtpSocket::bind("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let reset = PacketHeader {
        type_ver: (ST_RESET << 4) | 1,
        extension: EXT_NONE,
        connection_id: 456,
        timestamp_micros: 0,
        timestamp_diff_micros: 0,
        wnd_size: 0,
        seq_nr: 1,
        ack_nr: 0,
    };
    let mut response = BytesMut::new();
    reset.encode(&mut response);
    raw.send_to(&response, client.local_addr().unwrap())
        .await
        .unwrap();
    raw.send_to(b"barrier", client.local_addr().unwrap())
        .await
        .unwrap();
    client.recv_non_utp().await.unwrap();
    let mut buf = [0; 2048];
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(50),
            raw.recv_from(&mut buf)
        )
        .await
        .is_err(),
        "echoing an unknown RST creates a reset loop"
    );
}

#[tokio::test]
async fn test_utp_dropped_stream_leaves_shared_socket_usable() {
    let server = UtpSocket::bind("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let client = UtpSocket::bind("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let stream = client.connect(server.local_addr().unwrap()).await.unwrap();
    let incoming = server.accept().await.unwrap();
    assert_eq!(client.active_streams_count(), 1);
    drop(stream);
    assert_eq!(
        client.active_streams_count(),
        0,
        "dropping a race loser must remove its stream registration"
    );
    let again = client.connect(server.local_addr().unwrap()).await.unwrap();
    let accepted = server.accept().await.unwrap();
    assert_eq!(client.active_streams_count(), 1);
    drop((again, incoming, accepted));
}
