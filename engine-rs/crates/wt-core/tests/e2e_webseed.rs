//! Production BEP 19 source scheduling, without tracker peers.
mod e2e_support;

use e2e_support::*;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use wt_core::{Client, MemoryClientStorage};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn webseed_downloads_verified_selected_data_and_respects_payload_gate() {
    let (info, ih, data, info_bytes) = greeting_torrent();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/payload", listener.local_addr().unwrap());
    let body = data.clone();
    let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = requests.clone();
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let n = socket.read(&mut request).await.unwrap();
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let request = String::from_utf8_lossy(&request[..n]).to_ascii_lowercase();
            let (start, end) = request
                .lines()
                .find_map(|line| line.strip_prefix("range: bytes="))
                .map(|range| {
                    let (a, b) = range.trim().split_once('-').unwrap();
                    (a.parse::<usize>().unwrap(), b.parse::<usize>().unwrap())
                })
                .unwrap_or((0, body.len() - 1));
            let headers = format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{}\r\nConnection: close\r\n\r\n", end-start+1,body.len());
            socket.write_all(headers.as_bytes()).await.unwrap();
            socket.write_all(&body[start..=end]).await.unwrap();
        }
    });
    let storage = Arc::new(MemoryClientStorage::new());
    let client = Client::new(testing_config(storage.clone(), false)).unwrap();
    let mut spec = torrent_spec(ih, Some(info_bytes), "webseed", Some(storage.clone()));
    spec.web_seeds = vec![url];
    let (torrent, _) = client.add_torrent_spec(spec).unwrap();
    torrent.disallow_data_download();
    torrent.download_all();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        requests.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "payload gate fetched bytes"
    );
    torrent.allow_data_download();
    wait_bytes_completed(&torrent, data.len() as i64, Duration::from_secs(3)).await;
    assert_eq!(read_all_from_storage(&storage, &info, ih), data);
    assert!(requests.load(std::sync::atomic::Ordering::SeqCst) > 0);
    client.close();
    server.abort();
}
