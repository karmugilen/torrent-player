//! BEP 19 webseed range fetch against an in-process HTTP server.

use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use wt_discovery::webseed::{Extent, WebSeedClient};
use wt_types::{FileInfo, Info};

#[tokio::test]
async fn fetch_range_across_files() {
    let payload_a: Vec<u8> = (0u8..100).collect();
    let payload_b: Vec<u8> = (100u8..150).collect();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            let mut buf = [0u8; 4096];
            let n = sock.read(&mut buf).await.unwrap();
            let req = String::from_utf8_lossy(&buf[..n]);
            let path = if req.contains("/dir/a.txt") {
                "a"
            } else if req.contains("/dir/b.txt") {
                "b"
            } else {
                sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
                    .await
                    .ok();
                continue;
            };
            let range = req
                .lines()
                .find(|l| l.to_ascii_lowercase().starts_with("range:"))
                .and_then(|l| l.split_once('='))
                .map(|(_, r)| r.trim().to_string());

            let data = if path == "a" {
                payload_a.clone()
            } else {
                payload_b.clone()
            };
            let (status, body) = if let Some(r) = range {
                // bytes=start-end
                let r = r.trim_start_matches("bytes=");
                let mut parts = r.split('-');
                let start: usize = parts.next().unwrap().parse().unwrap();
                let end: usize = parts.next().unwrap().parse().unwrap();
                let slice = data[start..=end].to_vec();
                ("206 Partial Content", slice)
            } else {
                ("200 OK", data)
            };
            let header = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            sock.write_all(header.as_bytes()).await.unwrap();
            sock.write_all(&body).await.unwrap();
        }
    });

    let info = Info {
        name: "dir".into(),
        piece_length: 16384,
        pieces: vec![0u8; 20],
        files: vec![
            FileInfo {
                path: vec!["a.txt".into()],
                length: 100,
                torrent_offset: 0,
                ..Default::default()
            },
            FileInfo {
                path: vec!["b.txt".into()],
                length: 50,
                torrent_offset: 100,
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    let mut client = WebSeedClient::new(format!("http://{addr}/"), 2).unwrap();
    client.set_info(info);
    let bytes = client
        .fetch_extent(Extent {
            start: 90,
            length: 20,
        })
        .await
        .unwrap();
    assert_eq!(bytes.len(), 20);
    // last 10 of a (90..100) then first 10 of b (100..110)
    let expected: Vec<u8> = (90u8..110).collect();
    assert_eq!(&bytes[..], &expected[..]);
    let _ = Arc::new(());
}
