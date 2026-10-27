//! BEP 9 magnet xs/as metainfo sources (Go patches/torrent/sources.go).
use std::collections::HashSet;
use std::sync::{Arc, Weak};
use std::time::Duration;

use super::EngineServer;

const MAX_METAINFO_BYTES: usize = 16 << 20;

impl EngineServer {
    pub(super) fn start_metadata_sources(&self, torrent: &wt_core::Torrent, sources: Vec<String>) {
        if sources.is_empty() || torrent.info().is_some() {
            return;
        }
        let Some(server) = self.self_ref.get().cloned() else {
            return;
        };
        let Ok(http) = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent(&self.client.inner.config.http_user_agent)
            .build()
        else {
            return;
        };
        let serial = Arc::new(tokio::sync::Mutex::new(()));
        let mut seen = HashSet::new();
        for source in sources {
            if !seen.insert(source.clone()) {
                continue;
            }
            let torrent = torrent.clone();
            let server = server.clone();
            let serial = serial.clone();
            let http = http.clone();
            self.client.inner.tokio_handle.spawn(async move {
                loop {
                    let fetch = async {
                        let _guard = serial.lock().await;
                        if torrent.closed() || torrent.info().is_some() {
                            return Ok(None);
                        }
                        fetch_metainfo(&http, &source).await.map(Some)
                    };
                    let result = tokio::select! {
                        _ = no_longer_needed(&server, &torrent) => return,
                        result = fetch => result,
                    };
                    match result {
                        Ok(None) => return,
                        Ok(Some(meta)) => {
                            // A source URL is untrusted: validate both magnet hashes
                            // before giving its info bytes to the core merge API.
                            if !matches_magnet(&torrent, &meta.info_bytes) {
                                tracing::warn!("magnet metadata source infohash mismatch");
                                return;
                            }
                            let Some(server) = server.upgrade() else {
                                return;
                            };
                            if torrent.closed() || torrent.info().is_some() {
                                return;
                            }
                            let trackers =
                                if meta.announce_list.is_empty() && !meta.announce.is_empty() {
                                    vec![vec![meta.announce]]
                                } else {
                                    meta.announce_list
                                };
                            let spec = wt_core::TorrentSpec {
                                info_hash: Some(torrent.info_hash()),
                                info_hash_v2: torrent.info_hash_v2(),
                                info_bytes: Some(meta.info_bytes),
                                trackers,
                                web_seeds: meta.url_list,
                                piece_layers: meta.piece_layers,
                                ..Default::default()
                            };
                            if server.client.add_torrent_spec(spec).is_ok() {
                                if let Some(rec) =
                                    server.record_by_id(&torrent.info_hash().to_hex())
                                {
                                    server.supplement_trackers(&rec);
                                }
                                server.notify_change();
                            }
                            return;
                        }
                        Err(error) => tracing::warn!(%error, "error using torrent metadata source"),
                    }
                    // Go retries transient source failures after 60–120 seconds.
                    let delay = Duration::from_secs(60 + rand::random::<u64>() % 60);
                    tokio::select! {
                        _ = no_longer_needed(&server, &torrent) => return,
                        _ = tokio::time::sleep(delay) => {}
                    }
                }
            });
        }
    }
}

async fn no_longer_needed(server: &Weak<EngineServer>, torrent: &wt_core::Torrent) {
    loop {
        if server.strong_count() == 0 || torrent.closed() || torrent.info().is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn fetch_metainfo(
    http: &reqwest::Client,
    source: &str,
) -> Result<wt_types::MetaInfo, String> {
    let mut response = http.get(source).send().await.map_err(|e| e.to_string())?;
    if response.status() != reqwest::StatusCode::OK {
        return Err(format!(
            "unexpected response status code: {}",
            response.status()
        ));
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_METAINFO_BYTES as u64)
    {
        return Err("torrent metadata source exceeds size limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if bytes.len().saturating_add(chunk.len()) > MAX_METAINFO_BYTES {
            return Err("torrent metadata source exceeds size limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    wt_meta::parse_metainfo(&bytes).map_err(|e| e.to_string())
}

fn matches_magnet(torrent: &wt_core::Torrent, info_bytes: &[u8]) -> bool {
    let Ok(info) = wt_meta::parse_info(info_bytes) else {
        return false;
    };
    if let Some(v2) = torrent.info_hash_v2() {
        if wt_meta::info_hash_v2(info_bytes) != v2 {
            return false;
        }
        if torrent.info_hash() == v2.to_short() {
            return info.has_v2();
        }
    }
    info.has_v1() && wt_meta::info_hash_v1(info_bytes) == torrent.info_hash()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn fixture() -> (Vec<u8>, String) {
        let info = wt_types::Info {
            name: "source.mp4".into(),
            piece_length: 16384,
            pieces: vec![0; 20],
            files: vec![wt_types::FileInfo {
                length: 16384,
                ..Default::default()
            }],
            ..Default::default()
        };
        let info_bytes = wt_meta::encode_info(&info).unwrap();
        let hash = wt_meta::info_hash_v1(&info_bytes).to_hex();
        (
            wt_meta::encode_metainfo(&wt_types::MetaInfo {
                info_bytes,
                ..Default::default()
            })
            .unwrap(),
            hash,
        )
    }

    fn engine() -> (tempfile::TempDir, Arc<EngineServer>) {
        let root = "/home/kar/.cache/wt-tmp/codex-engine";
        std::fs::create_dir_all(root).unwrap();
        let dir = tempfile::tempdir_in(root).unwrap();
        let server = EngineServer::new_with_config(0, dir.path(), 10, |config| {
            config.no_dht = true;
            config.disable_utp = true;
            config.disable_webtorrent = true;
        })
        .unwrap();
        (dir, server)
    }

    #[tokio::test]
    async fn magnet_xs_and_as_load_validated_metadata() {
        for key in ["xs", "as"] {
            let (meta, hash) = fixture();
            let (_dir, server) = engine();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let source = format!("http://{}/source.torrent", listener.local_addr().unwrap());
            let endpoint = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                assert!(socket.read(&mut request).await.unwrap() > 0);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    meta.len()
                );
                socket.write_all(head.as_bytes()).await.unwrap();
                socket.write_all(&meta).await.unwrap();
            });
            let magnet = format!(
                "magnet:?xt=urn:btih:{hash}&{key}={}",
                url::form_urlencoded::byte_serialize(source.as_bytes()).collect::<String>()
            );
            assert_eq!(
                server
                    .request("POST", "/add", &json!({"torrentId": magnet}).to_string())
                    .0,
                200
            );
            let rec = server.records_by_hash.read().get(&hash).unwrap().clone();
            let torrent = rec.read().torrent.clone();
            tokio::time::timeout(Duration::from_secs(3), async {
                while torrent.info().is_none() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("magnet metadata never arrived from source");
            endpoint.await.unwrap();
            assert_eq!(torrent.info().unwrap().name, "source.mp4");
            server.close();
        }
    }

    #[tokio::test]
    async fn source_response_is_cancelled_when_torrent_is_removed() {
        let (_, hash) = fixture();
        let (_dir, server) = engine();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let source = format!("http://{}/source", listener.local_addr().unwrap());
        let magnet = format!("magnet:?xt=urn:btih:{hash}&xs={source}");
        assert_eq!(
            server
                .request("POST", "/add", &json!({"torrentId": magnet}).to_string())
                .0,
            200
        );
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut request = [0; 4096];
        assert!(socket.read(&mut request).await.unwrap() > 0);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\nd")
            .await
            .unwrap();
        let id = server.records_by_hash.read().get(&hash).unwrap().read().id.clone();
        assert_eq!(
            server
                .request("POST", "/remove", &json!({"id": id}).to_string())
                .0,
            200
        );
        let n = tokio::time::timeout(Duration::from_secs(2), socket.read(&mut request))
            .await
            .expect("source connection was not cancelled");
        assert!(n.is_err() || n.unwrap() == 0);
        server.close();
    }

    #[test]
    fn mismatched_source_info_never_replaces_magnet_identity() {
        let (_dir, server) = engine();
        let (meta, _) = fixture();
        let hash = "0123456789012345678901234567890123456789";
        server.request(
            "POST",
            "/add",
            &json!({"torrentId": format!("magnet:?xt=urn:btih:{hash}")}).to_string(),
        );
        let rec = server.records_by_hash.read().get(hash).unwrap().clone();
        let torrent = rec.read().torrent.clone();
        assert!(!matches_magnet(
            &torrent,
            &wt_meta::parse_metainfo(&meta).unwrap().info_bytes
        ));
        assert!(torrent.info().is_none());
        server.close();
    }
}
