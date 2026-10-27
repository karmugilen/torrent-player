//! Integration tests ported from `engine-go/seek_http_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use base64::Engine as _;
use std::io::{Read, Write};
use std::net::TcpStream;
use support::TestEngine;

fn send_get(addr: &str, path: &str, range: Option<&str>) -> std::io::Result<(u16, Vec<u8>)> {
    let mut stream = TcpStream::connect(addr)?;
    let mut req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    if let Some(r) = range {
        req.push_str(&format!("Range: {r}\r\n"));
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes())?;
    let mut resp = Vec::new();
    stream.read_to_end(&mut resp)?;
    let pos = resp
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(resp.len());
    let header = String::from_utf8_lossy(&resp[..pos]);
    let status = header
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .unwrap_or(0);
    let body = if pos + 4 <= resp.len() {
        resp[pos + 4..].to_vec()
    } else {
        Vec::new()
    };
    Ok((status, body))
}

#[ignore = "needs: multi-peer simulated seek swarm HTTP comparison"]
#[test]
fn test_seek_http_comparison() {
    // Port of Go `TestSeekHTTPComparison`.
    if std::env::var("SEEK_SEARCH").as_deref() != Ok("http") {
        return;
    }
    let rounds = std::env::var("SEEK_ROUNDS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(10);
    let piece_len = 256 * 1024i64;
    let file_len = 192 * 1024 * 1024i64;
    let num_pieces = (file_len / piece_len) as usize;

    let eng = TestEngine::new();
    let (torrent, _data, _hash) =
        support::build_single_file_torrent("movie.mkv", piece_len, num_pieces);
    let encoded = base64::engine::general_purpose::STANDARD.encode(&torrent);
    let (code, raw) = eng.dispatch(
        "POST",
        "/add",
        &serde_json::json!({"prepare": true, "torrentData": encoded}).to_string(),
    );
    assert_eq!(code, 200, "add: {}", String::from_utf8_lossy(&raw));
    let added: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = added["id"].as_str().unwrap().to_owned();

    let configure_req = serde_json::json!({
        "id": id,
        "descriptors": [null],
        "storageMode": "stream",
        "streamCacheBytes": 64 << 20,
        "selected": [0],
    });
    let (code, resp) = eng.dispatch("POST", "/configure", &configure_req.to_string());
    assert_eq!(code, 200, "configure: {}", String::from_utf8_lossy(&resp));

    let stream_port = eng.server.stream_port.load(std::sync::atomic::Ordering::SeqCst);
    let addr = format!("127.0.0.1:{stream_port}");
    let path = format!("/stream/0?id={id}");

    let mut first_byte_times = Vec::new();
    let mut k64_times = Vec::new();
    let mut m1_times = Vec::new();
    let mut rates = Vec::new();

    for _round in 0..rounds {
        for _k in 0..3 {
            let offset = 10 * 1024 * 1024i64;
            let start = std::time::Instant::now();
            let (status, body) = send_get(&addr, &path, Some(&format!("bytes={offset}-"))).unwrap();
            assert_eq!(status, 206);
            if !body.is_empty() {
                first_byte_times.push(start.elapsed().as_secs_f64() * 1000.0);
            }
            if body.len() >= 64 * 1024 {
                k64_times.push(start.elapsed().as_secs_f64() * 1000.0);
            }
            if body.len() >= 1024 * 1024 {
                m1_times.push(start.elapsed().as_secs_f64() * 1000.0);
            }
        }

        let span = 8 * 1024 * 1024i64;
        let start = std::time::Instant::now();
        let (status, body) = send_get(&addr, &path, Some(&format!("bytes={span}-"))).unwrap();
        assert_eq!(status, 206);
        let elapsed = start.elapsed().as_secs_f64();
        rates.push((body.len() as f64 / (1024.0 * 1024.0)) / elapsed);
    }

    assert!(!first_byte_times.is_empty());
    println!(
        "seek http: first_byte={:?} ms, 64k={:?} ms, 1m={:?} ms, rate={:?} MiB/s",
        first_byte_times.iter().sum::<f64>() / first_byte_times.len() as f64,
        k64_times.iter().sum::<f64>() / k64_times.len() as f64,
        m1_times.iter().sum::<f64>() / m1_times.len() as f64,
        rates.iter().sum::<f64>() / rates.len() as f64,
    );
    eng.close();
}

#[ignore = "needs: seek tuning comparison across multi-peer swarm"]
#[test]
fn test_seek_resume_comparison() {
    // Port of Go `TestSeekResumeComparison`.
    if std::env::var("SEEK_SEARCH").as_deref() != Ok("resume") {
        return;
    }
    let caches: Vec<i64> = match std::env::var("SEEK_CACHE") {
        Ok(env) => env
            .split(',')
            .filter_map(|s| s.parse::<i64>().ok().map(|c| c << 20))
            .collect(),
        Err(_) => vec![16 << 20, 48 << 20],
    };
    let rounds = std::env::var("SEEK_ROUNDS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(8);
    let piece_len = 256 * 1024i64;
    let file_len = 192 * 1024 * 1024i64;
    let num_pieces = (file_len / piece_len) as usize;

    let eng = TestEngine::new();
    let (torrent, _data, _hash) =
        support::build_single_file_torrent("movie.mkv", piece_len, num_pieces);
    let encoded = base64::engine::general_purpose::STANDARD.encode(&torrent);
    let (code, raw) = eng.dispatch(
        "POST",
        "/add",
        &serde_json::json!({"prepare": true, "torrentData": encoded}).to_string(),
    );
    assert_eq!(code, 200, "add: {}", String::from_utf8_lossy(&raw));
    let added: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = added["id"].as_str().unwrap().to_owned();

    for &cache_limit in &caches {
        let configure_req = serde_json::json!({
            "id": id,
            "descriptors": [null],
            "storageMode": "stream",
            "streamCacheBytes": cache_limit,
            "selected": [0],
        });
        let (code, resp) = eng.dispatch("POST", "/configure", &configure_req.to_string());
        assert_eq!(code, 200, "configure: {}", String::from_utf8_lossy(&resp));

        let stream_port = eng.server.stream_port.load(std::sync::atomic::Ordering::SeqCst);
        let addr = format!("127.0.0.1:{stream_port}");
        let path = format!("/stream/0?id={id}");

        let mut first_vals = Vec::new();
        let mut m1_vals = Vec::new();
        let mut m2_vals = Vec::new();
        let mut stream_rates = Vec::new();

        for _round in 0..rounds {
            for _k in 0..3 {
                let offset = 20 * 1024 * 1024i64;
                let start = std::time::Instant::now();
                let (status, body) = send_get(&addr, &path, Some(&format!("bytes={offset}-"))).unwrap();
                assert_eq!(status, 206);
                if !body.is_empty() {
                    first_vals.push(start.elapsed().as_secs_f64() * 1000.0);
                }
                if body.len() >= 1024 * 1024 {
                    m1_vals.push(start.elapsed().as_secs_f64() * 1000.0);
                }
                if body.len() >= 2 * 1024 * 1024 {
                    m2_vals.push(start.elapsed().as_secs_f64() * 1000.0);
                }
            }

            let span = 8 * 1024 * 1024i64;
            let start = std::time::Instant::now();
            let (status, body) = send_get(&addr, &path, Some(&format!("bytes={span}-"))).unwrap();
            assert_eq!(status, 206);
            let elapsed = start.elapsed().as_secs_f64();
            stream_rates.push((body.len() as f64 / (1024.0 * 1024.0)) / elapsed);
        }

        println!(
            "cache={cache_limit} first={:?} ms 1m={:?} ms 2m={:?} ms stream={:?} MiB/s",
            first_vals.iter().sum::<f64>() / first_vals.len() as f64,
            m1_vals.iter().sum::<f64>() / m1_vals.len() as f64,
            m2_vals.iter().sum::<f64>() / m2_vals.len() as f64,
            stream_rates.iter().sum::<f64>() / stream_rates.len() as f64,
        );
    }
    eng.close();
}
