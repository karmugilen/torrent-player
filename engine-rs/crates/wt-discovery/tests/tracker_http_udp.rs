//! In-process fake HTTP + UDP tracker announce tests.

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use wt_discovery::tracker::{
    announce, AnnounceEvent, AnnounceRequest, Client, ClientOpts, TrackerError,
};
use wt_types::InfoHash;

fn sample_request() -> AnnounceRequest {
    AnnounceRequest {
        info_hash: InfoHash([1u8; 20]),
        peer_id: [2u8; 20],
        downloaded: 0,
        left: 100,
        uploaded: 0,
        event: AnnounceEvent::Started,
        ip_address: 0,
        key: 42,
        num_want: -1,
        port: 6881,
    }
}

#[tokio::test]
async fn http_announce_compact_peers() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let n = sock.read(&mut buf).await.unwrap();
        let req = String::from_utf8_lossy(&buf[..n]);
        assert!(req.contains("GET "));
        assert!(req.contains("info_hash="));

        // d8:intervali1800e5:peers6: + 1.2.3.4:6881
        let mut body = Vec::new();
        body.extend_from_slice(b"d8:intervali1800e5:peers6:");
        body.extend_from_slice(&[1, 2, 3, 4, 0x1a, 0xe1]);
        body.push(b'e');

        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        sock.write_all(header.as_bytes()).await.unwrap();
        sock.write_all(&body).await.unwrap();
    });

    let url = format!("http://{addr}/announce");
    let resp = announce(&url, sample_request()).await.unwrap();
    assert_eq!(resp.interval, 1800);
    assert_eq!(resp.peers.len(), 1);
    assert_eq!(resp.peers[0].addr.to_string(), "1.2.3.4:6881");
}

#[tokio::test]
async fn http_failure_reason() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 2048];
        let _ = sock.read(&mut buf).await.unwrap();
        let body = b"d14:failure reason4:nopee";
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        sock.write_all(header.as_bytes()).await.unwrap();
        sock.write_all(body).await.unwrap();
    });
    let url = format!("http://{addr}/announce");
    let err = announce(&url, sample_request()).await.unwrap_err();
    assert!(matches!(err, TrackerError::FailureReason(_)));
}

#[tokio::test]
async fn udp_announce_roundtrip() {
    let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = sock.local_addr().unwrap();
    let sock = Arc::new(sock);

    let server = {
        let sock = Arc::clone(&sock);
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            // Connect request
            let (n, from) = sock.recv_from(&mut buf).await.unwrap();
            assert!(n >= 16);
            let action = i32::from_be_bytes(buf[8..12].try_into().unwrap());
            assert_eq!(action, 0); // connect
            let tid = i32::from_be_bytes(buf[12..16].try_into().unwrap());
            let conn_id: u64 = 0x1122334455667788;
            let mut resp = Vec::new();
            resp.extend_from_slice(&0i32.to_be_bytes()); // action connect
            resp.extend_from_slice(&tid.to_be_bytes());
            resp.extend_from_slice(&conn_id.to_be_bytes());
            sock.send_to(&resp, from).await.unwrap();

            // Announce
            let (n, from) = sock.recv_from(&mut buf).await.unwrap();
            assert!(n >= 16 + 82);
            let action = i32::from_be_bytes(buf[8..12].try_into().unwrap());
            assert_eq!(action, 1);
            let tid = i32::from_be_bytes(buf[12..16].try_into().unwrap());
            let mut resp = Vec::new();
            resp.extend_from_slice(&1i32.to_be_bytes()); // announce
            resp.extend_from_slice(&tid.to_be_bytes());
            resp.extend_from_slice(&900i32.to_be_bytes()); // interval
            resp.extend_from_slice(&1i32.to_be_bytes()); // leechers
            resp.extend_from_slice(&2i32.to_be_bytes()); // seeders
            resp.extend_from_slice(&[9, 9, 9, 9, 0x1a, 0xe1]); // peer
            sock.send_to(&resp, from).await.unwrap();
        })
    };

    let url = format!("udp://{addr}/announce");
    let cl = Client::new(&url, ClientOpts::default()).await.unwrap();
    let resp = cl
        .announce(sample_request(), Default::default())
        .await
        .unwrap();
    assert_eq!(resp.interval, 900);
    assert_eq!(resp.leechers, 1);
    assert_eq!(resp.seeders, 2);
    assert_eq!(resp.peers.len(), 1);
    assert_eq!(
        resp.peers[0].addr,
        "9.9.9.9:6881".parse::<SocketAddr>().unwrap()
    );
    server.await.unwrap();
}

#[tokio::test]
async fn unsupported_scheme() {
    let err = Client::new("lol://x", ClientOpts::default())
        .await
        .err()
        .unwrap();
    assert!(matches!(err, TrackerError::BadScheme));
}

#[tokio::test]
async fn http_announce_counting_dialer() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wt_discovery::tracker::{DialerHook, HttpClientOpts};

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let n = sock.read(&mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).contains("GET "));
        let mut body = Vec::new();
        body.extend_from_slice(b"d8:intervali600e5:peers6:");
        body.extend_from_slice(&[8, 8, 8, 8, 0x1a, 0xe1]);
        body.push(b'e');
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        sock.write_all(header.as_bytes()).await.unwrap();
        sock.write_all(&body).await.unwrap();
    });

    let dials = Arc::new(AtomicUsize::new(0));
    let dials_hook = Arc::clone(&dials);
    let dialer: DialerHook = Arc::new(move |_network, addr| {
        let dials_hook = Arc::clone(&dials_hook);
        Box::pin(async move {
            dials_hook.fetch_add(1, Ordering::SeqCst);
            tokio::net::TcpStream::connect(addr).await
        })
    });

    let url = format!("http://{addr}/announce");
    let cl = Client::new(
        &url,
        ClientOpts {
            http: HttpClientOpts {
                dialer: Some(dialer),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let resp = cl
        .announce(sample_request(), Default::default())
        .await
        .unwrap();
    assert_eq!(resp.interval, 600);
    assert_eq!(resp.peers.len(), 1);
    assert_eq!(resp.peers[0].addr.to_string(), "8.8.8.8:6881");
    assert_eq!(dials.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn udp_announce_via_datagram_io() {
    use wt_discovery::datagram::StandaloneDatagram;

    let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = sock.local_addr().unwrap();
    let sock = Arc::new(sock);

    let server = {
        let sock = Arc::clone(&sock);
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            let (n, from) = sock.recv_from(&mut buf).await.unwrap();
            assert!(n >= 16);
            let tid = i32::from_be_bytes(buf[12..16].try_into().unwrap());
            let conn_id: u64 = 0xaabbccddeeff0011;
            let mut resp = Vec::new();
            resp.extend_from_slice(&0i32.to_be_bytes());
            resp.extend_from_slice(&tid.to_be_bytes());
            resp.extend_from_slice(&conn_id.to_be_bytes());
            sock.send_to(&resp, from).await.unwrap();

            let (n, from) = sock.recv_from(&mut buf).await.unwrap();
            assert!(n >= 16 + 82);
            let tid = i32::from_be_bytes(buf[12..16].try_into().unwrap());
            let mut resp = Vec::new();
            resp.extend_from_slice(&1i32.to_be_bytes());
            resp.extend_from_slice(&tid.to_be_bytes());
            resp.extend_from_slice(&120i32.to_be_bytes());
            resp.extend_from_slice(&0i32.to_be_bytes());
            resp.extend_from_slice(&1i32.to_be_bytes());
            resp.extend_from_slice(&[7, 7, 7, 7, 0x1a, 0xe1]);
            sock.send_to(&resp, from).await.unwrap();
        })
    };

    let io = Arc::new(StandaloneDatagram::bind("127.0.0.1:0").await.unwrap());
    let url = format!("udp://{addr}/announce");
    let cl = Client::new(
        &url,
        ClientOpts {
            datagram: Some(io),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let resp = cl
        .announce(sample_request(), Default::default())
        .await
        .unwrap();
    assert_eq!(resp.interval, 120);
    assert_eq!(resp.peers[0].addr.to_string(), "7.7.7.7:6881");
    server.await.unwrap();
}
