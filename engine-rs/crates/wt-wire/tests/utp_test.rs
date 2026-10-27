use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;
use wt_wire::utp::UtpSocket;

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
