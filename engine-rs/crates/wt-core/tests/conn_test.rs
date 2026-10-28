//! Comprehensive acceptance tests for `wt-core::conn`.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use tokio::sync::{mpsc, Notify};
use wt_core::conn::{
    dial_peer, spawn_connection_driver, ConnHandle, ConnHandler, DialParams, HalfOpenTracker,
    PeerListener, SharedDatagramAdapter,
};
use wt_core::ratelimit::{Limiter, DEFAULT_DOWNLOAD_BURST};
use wt_core::{ClientConfig, Result};
use wt_discovery::DatagramIo;
use wt_types::{InfoHash, PeerId, PeerSource, Transport};
use wt_wire::ext::{ExtendedHandshake, EXTENSION_NAME_METADATA, EXTENSION_NAME_PEX};
use wt_wire::message::Message;
use wt_wire::utp::UtpSocket;

/// Simulate `PeerConnHandler::start` LTEP (dialer/listener no longer write it).
fn send_test_ltep(handle: &ConnHandle, version: &str) {
    let mut m = std::collections::BTreeMap::new();
    m.insert(EXTENSION_NAME_METADATA.to_string(), 1);
    m.insert(EXTENSION_NAME_PEX.to_string(), 2);
    let ext = ExtendedHandshake {
        m,
        v: Some(version.to_string()),
        p: Some(0),
        yourip: None,
        ipv4: None,
        ipv6: None,
        reqq: Some(1024),
        metadata_size: None,
        upload_only: Some(false),
        encryption: Some(false),
    };
    assert!(handle.send_message(Message::Extended {
        id: 0,
        payload: ext.encode(),
    }));
}

struct ChannelHandler {
    tx: mpsc::UnboundedSender<Message>,
}

impl ConnHandler for ChannelHandler {
    fn on_message(&mut self, _handle: &ConnHandle, msg: Message) -> Result<()> {
        let _ = self.tx.send(msg);
        Ok(())
    }

    fn on_closed(&mut self, _handle: &ConnHandle, _reason: &str) {}
}

struct FastCountingHandler {
    bytes_received: Arc<AtomicUsize>,
    notify: Arc<Notify>,
    target_bytes: usize,
}

impl ConnHandler for FastCountingHandler {
    fn on_message(&mut self, _handle: &ConnHandle, msg: Message) -> Result<()> {
        if let Message::Piece { piece, .. } = msg {
            let prev = self
                .bytes_received
                .fetch_add(piece.len(), Ordering::Relaxed);
            if prev + piece.len() >= self.target_bytes {
                self.notify.notify_waiters();
            }
        }
        Ok(())
    }

    fn on_closed(&mut self, _handle: &ConnHandle, _reason: &str) {
        self.notify.notify_waiters();
    }
}

/// Helper to configure a standard test ClientConfig on ephemeral port.
fn test_config() -> ClientConfig {
    ClientConfig {
        listen_port: 0,
        disable_tcp: false,
        disable_utp: false,
        nominal_dial_timeout: Duration::from_secs(5),
        min_dial_timeout: Duration::from_millis(500),
        handshakes_timeout: Duration::from_secs(4),
        total_half_open_conns: 50,
        half_open_conns_per_torrent: 10,
        extended_handshake_client_version: "WebtorCoreTest/1.0".to_string(),
        ..Default::default()
    }
}

#[tokio::test]
async fn test_tcp_dial_and_accept_roundtrip() {
    let cfg = test_config();
    let our_ih = InfoHash([0x42; 20]);
    let server_peer_id: PeerId = [0x11; 20];
    let client_peer_id: PeerId = [0x22; 20];

    let listener = PeerListener::bind(&cfg).await.expect("bind listener");
    let listen_port = listener.listen_port();
    assert!(listen_port > 0);

    let (accepted_tx, mut accepted_rx) = mpsc::channel(16);
    let server_tracker = HalfOpenTracker::new();
    let skey_ih = our_ih;
    let skey_provider = Arc::new(move || vec![skey_ih.0]);
    let lk_ih = our_ih;
    let lookup = Arc::new(move |ih: &InfoHash| *ih == lk_ih);

    listener.spawn_accept_loop(
        cfg.clone(),
        server_peer_id,
        server_tracker,
        skey_provider,
        lookup,
        accepted_tx,
    );

    // Client dials server over TCP
    let client_tracker = HalfOpenTracker::new();
    let dial_addr: SocketAddr = format!("127.0.0.1:{listen_port}").parse().unwrap();

    let dial_params = DialParams {
        remote_addr: dial_addr,
        info_hash: our_ih,
        our_peer_id: client_peer_id,
        transport: Some(Transport::Tcp),
        source: PeerSource::Direct,
        metadata_size: None,
        cfg: &cfg,
        tracker: &client_tracker,
        dial_limiter: None,
        tcp_conn_hook: None,
        utp_socket: None,
        require_plain: false,
    };

    let client_outcome = dial_peer(dial_params).await.expect("dial_peer failed");
    let server_conn = tokio::time::timeout(Duration::from_secs(3), accepted_rx.recv())
        .await
        .expect("accept timed out")
        .expect("channel closed");

    assert_eq!(server_conn.info_hash, our_ih);
    assert_eq!(server_conn.peer_id, client_peer_id);
    assert_eq!(client_outcome.peer_id, server_peer_id);

    // Spawn drivers on both sides and verify message transfer
    let (s_msg_tx, mut s_msg_rx) = mpsc::unbounded_channel();
    let (c_msg_tx, mut c_msg_rx) = mpsc::unbounded_channel();

    let (_s_r, _s_w) = spawn_connection_driver(
        server_conn.stream,
        ChannelHandler { tx: s_msg_tx },
        server_conn.handle.clone(),
        server_conn.write_rx,
        server_conn.write_waker,
        None,
        None,
        Duration::from_secs(60),
        Duration::from_secs(60),
    );

    let (_c_r, _c_w) = spawn_connection_driver(
        client_outcome.stream,
        ChannelHandler { tx: c_msg_tx },
        client_outcome.handle.clone(),
        client_outcome.write_rx,
        client_outcome.write_waker,
        None,
        None,
        Duration::from_secs(60),
        Duration::from_secs(60),
    );

    // PeerConnHandler::start owns LTEP; exercise that path via handles.
    send_test_ltep(&client_outcome.handle, "WebtorCoreTest/1.0");
    send_test_ltep(&server_conn.handle, "WebtorCoreTest/1.0");

    let s_ext = tokio::time::timeout(Duration::from_secs(2), s_msg_rx.recv())
        .await
        .expect("server rx timeout")
        .expect("server msg");
    if let Message::Extended { id, payload } = s_ext {
        assert_eq!(id, 0);
        let ext = ExtendedHandshake::decode(&payload).expect("decode ext");
        assert_eq!(ext.v.as_deref(), Some("WebtorCoreTest/1.0"));
        assert_eq!(ext.reqq, Some(1024));
    } else {
        panic!("expected ExtendedHandshake, got {s_ext:?}");
    }

    let c_ext = tokio::time::timeout(Duration::from_secs(2), c_msg_rx.recv())
        .await
        .expect("client rx timeout")
        .expect("client msg");
    if let Message::Extended { id, payload } = c_ext {
        assert_eq!(id, 0);
        let ext = ExtendedHandshake::decode(&payload).expect("decode ext");
        assert_eq!(ext.v.as_deref(), Some("WebtorCoreTest/1.0"));
    } else {
        panic!("expected ExtendedHandshake, got {c_ext:?}");
    }

    // Client sends Interested, server sends Unchoke
    assert!(client_outcome.handle.send_message(Message::Interested));
    assert!(server_conn.handle.send_message(Message::Unchoke));

    let s_received = tokio::time::timeout(Duration::from_secs(2), s_msg_rx.recv())
        .await
        .expect("server rx timeout")
        .expect("server msg");
    assert_eq!(s_received, Message::Interested);

    let c_received = tokio::time::timeout(Duration::from_secs(2), c_msg_rx.recv())
        .await
        .expect("client rx timeout")
        .expect("client msg");
    assert_eq!(c_received, Message::Unchoke);

    client_outcome.handle.close("done");
    server_conn.handle.close("done");
    listener.close();
}

#[tokio::test]
async fn test_tcp_dial_and_accept_plain_no_mse() {
    let cfg = test_config();
    let our_ih = InfoHash([0x33; 20]);
    let server_peer_id: PeerId = [0x77; 20];
    let client_peer_id: PeerId = [0x88; 20];

    let listener = PeerListener::bind(&cfg).await.expect("bind listener");
    let listen_port = listener.listen_port();

    let (accepted_tx, mut accepted_rx) = mpsc::channel(16);
    let server_tracker = HalfOpenTracker::new();
    let skey_ih = our_ih;
    let skey_provider = Arc::new(move || vec![skey_ih.0]);
    let lk_ih = our_ih;
    let lookup = Arc::new(move |ih: &InfoHash| *ih == lk_ih);

    listener.spawn_accept_loop(
        cfg.clone(),
        server_peer_id,
        server_tracker,
        skey_provider,
        lookup,
        accepted_tx,
    );

    let client_tracker = HalfOpenTracker::new();
    let dial_addr: SocketAddr = format!("127.0.0.1:{listen_port}").parse().unwrap();

    let dial_params = DialParams {
        remote_addr: dial_addr,
        info_hash: our_ih,
        our_peer_id: client_peer_id,
        transport: Some(Transport::Tcp),
        source: PeerSource::Direct,
        metadata_size: None,
        cfg: &cfg,
        tracker: &client_tracker,
        dial_limiter: None,
        tcp_conn_hook: None,
        utp_socket: None,
        require_plain: true, // No MSE
    };

    let client_outcome = dial_peer(dial_params).await.expect("dial_peer failed");
    let server_conn = tokio::time::timeout(Duration::from_secs(3), accepted_rx.recv())
        .await
        .expect("accept timed out")
        .expect("channel closed");

    assert!(!client_outcome.handle.is_encrypted());
    assert!(!server_conn.handle.is_encrypted());

    client_outcome.handle.close("done");
    server_conn.handle.close("done");
    listener.close();
}

#[tokio::test]
async fn test_utp_dial_and_accept_roundtrip() {
    let mut cfg = test_config();
    cfg.disable_tcp = true; // force uTP

    let our_ih = InfoHash([0x55; 20]);
    let server_peer_id: PeerId = [0xaa; 20];
    let client_peer_id: PeerId = [0xbb; 20];

    let listener = PeerListener::bind(&cfg).await.expect("bind uTP listener");
    let listen_port = listener.listen_port();
    assert!(listen_port > 0);

    let (accepted_tx, mut accepted_rx) = mpsc::channel(16);
    let server_tracker = HalfOpenTracker::new();
    let skey_ih = our_ih;
    let skey_provider = Arc::new(move || vec![skey_ih.0]);
    let lk_ih = our_ih;
    let lookup = Arc::new(move |ih: &InfoHash| *ih == lk_ih);

    listener.spawn_accept_loop(
        cfg.clone(),
        server_peer_id,
        server_tracker,
        skey_provider,
        lookup,
        accepted_tx,
    );

    let client_tracker = HalfOpenTracker::new();
    let dial_addr: SocketAddr = format!("127.0.0.1:{listen_port}").parse().unwrap();

    let dial_params = DialParams {
        remote_addr: dial_addr,
        info_hash: our_ih,
        our_peer_id: client_peer_id,
        transport: Some(Transport::Utp),
        source: PeerSource::Direct,
        metadata_size: None,
        cfg: &cfg,
        tracker: &client_tracker,
        dial_limiter: None,
        tcp_conn_hook: None,
        utp_socket: None,
        require_plain: false,
    };

    let client_outcome = dial_peer(dial_params).await.expect("dial uTP peer failed");
    let server_conn = tokio::time::timeout(Duration::from_secs(4), accepted_rx.recv())
        .await
        .expect("uTP accept timed out")
        .expect("channel closed");

    assert_eq!(server_conn.info_hash, our_ih);
    assert_eq!(server_conn.peer_id, client_peer_id);
    assert_eq!(client_outcome.peer_id, server_peer_id);

    // Verify roundtrip message over uTP
    let (s_msg_tx, mut s_msg_rx) = mpsc::unbounded_channel();
    let (_s_r, _s_w) = spawn_connection_driver(
        server_conn.stream,
        ChannelHandler { tx: s_msg_tx },
        server_conn.handle.clone(),
        server_conn.write_rx,
        server_conn.write_waker,
        None,
        None,
        Duration::from_secs(60),
        Duration::from_secs(60),
    );

    let (c_msg_tx, mut c_msg_rx) = mpsc::unbounded_channel();
    let (_c_r, _c_w) = spawn_connection_driver(
        client_outcome.stream,
        ChannelHandler { tx: c_msg_tx },
        client_outcome.handle.clone(),
        client_outcome.write_rx,
        client_outcome.write_waker,
        None,
        None,
        Duration::from_secs(60),
        Duration::from_secs(60),
    );

    send_test_ltep(&client_outcome.handle, "WebtorCoreTest/1.0");
    send_test_ltep(&server_conn.handle, "WebtorCoreTest/1.0");

    let c_ext = tokio::time::timeout(Duration::from_secs(3), c_msg_rx.recv())
        .await
        .expect("client rx timeout")
        .expect("client msg");
    if let Message::Extended { id, payload } = c_ext {
        assert_eq!(id, 0);
        let ext = ExtendedHandshake::decode(&payload).expect("decode ext");
        assert_eq!(ext.v.as_deref(), Some("WebtorCoreTest/1.0"));
    } else {
        panic!("expected ExtendedHandshake, got {c_ext:?}");
    }

    let s_ext = tokio::time::timeout(Duration::from_secs(3), s_msg_rx.recv())
        .await
        .expect("server rx timeout")
        .expect("server msg");
    if let Message::Extended { id, payload } = s_ext {
        assert_eq!(id, 0);
        let ext = ExtendedHandshake::decode(&payload).expect("decode ext");
        assert_eq!(ext.v.as_deref(), Some("WebtorCoreTest/1.0"));
    } else {
        panic!("expected ExtendedHandshake, got {s_ext:?}");
    }

    assert!(client_outcome
        .handle
        .send_message(Message::Have { index: 42 }));
    let s_received = tokio::time::timeout(Duration::from_secs(3), s_msg_rx.recv())
        .await
        .expect("server rx timeout")
        .expect("server msg");
    assert_eq!(s_received, Message::Have { index: 42 });

    client_outcome.handle.close("done");
    server_conn.handle.close("done");
    listener.close();
}

/// uTP piece path multi-chunk transfer (SACK strip + first-wins dial race).
#[tokio::test]
async fn test_utp_piece_multichunk_transfer() {
    let mut cfg = test_config();
    cfg.disable_tcp = true;

    let our_ih = InfoHash([0x56; 20]);
    let server_peer_id: PeerId = [0xcc; 20];
    let client_peer_id: PeerId = [0xdd; 20];

    let listener = PeerListener::bind(&cfg).await.expect("bind uTP listener");
    let listen_port = listener.listen_port();

    let (accepted_tx, mut accepted_rx) = mpsc::channel(16);
    let server_tracker = HalfOpenTracker::new();
    let skey_ih = our_ih;
    let skey_provider = Arc::new(move || vec![skey_ih.0]);
    let lk_ih = our_ih;
    let lookup = Arc::new(move |ih: &InfoHash| *ih == lk_ih);

    listener.spawn_accept_loop(
        cfg.clone(),
        server_peer_id,
        server_tracker,
        skey_provider,
        lookup,
        accepted_tx,
    );

    let client_tracker = HalfOpenTracker::new();
    let dial_addr: SocketAddr = format!("127.0.0.1:{listen_port}").parse().unwrap();
    let dial_params = DialParams {
        remote_addr: dial_addr,
        info_hash: our_ih,
        our_peer_id: client_peer_id,
        transport: Some(Transport::Utp),
        source: PeerSource::Direct,
        metadata_size: None,
        cfg: &cfg,
        tracker: &client_tracker,
        dial_limiter: None,
        tcp_conn_hook: None,
        utp_socket: None,
        require_plain: false,
    };

    let client_outcome = dial_peer(dial_params).await.expect("dial uTP peer failed");
    let server_conn = tokio::time::timeout(Duration::from_secs(4), accepted_rx.recv())
        .await
        .expect("uTP accept timed out")
        .expect("channel closed");

    const TOTAL: usize = 256 * 1024; // 256 KiB of piece payload
    const CHUNK: usize = 16 * 1024;
    let num_chunks = TOTAL / CHUNK;

    let bytes_received = Arc::new(AtomicUsize::new(0));
    let done_notify = Arc::new(Notify::new());
    let receiver = FastCountingHandler {
        bytes_received: bytes_received.clone(),
        notify: done_notify.clone(),
        target_bytes: TOTAL,
    };

    let (_s_r, _s_w) = spawn_connection_driver(
        server_conn.stream,
        ChannelHandler {
            tx: mpsc::unbounded_channel().0,
        },
        server_conn.handle.clone(),
        server_conn.write_rx,
        server_conn.write_waker,
        None,
        None,
        Duration::from_secs(60),
        Duration::from_secs(60),
    );

    let (_c_r, _c_w) = spawn_connection_driver(
        client_outcome.stream,
        receiver,
        client_outcome.handle.clone(),
        client_outcome.write_rx,
        client_outcome.write_waker,
        None,
        None,
        Duration::from_secs(60),
        Duration::from_secs(60),
    );

    let piece_payload = Bytes::from(vec![0x5au8; CHUNK]);
    let seeder = server_conn.handle.clone();
    let send_task = tokio::spawn(async move {
        for i in 0..num_chunks {
            let msg = Message::Piece {
                index: 0,
                begin: (i * CHUNK) as u32,
                piece: piece_payload.clone(),
            };
            seeder.send_message_async(msg).await.unwrap();
        }
    });

    tokio::time::timeout(Duration::from_secs(10), done_notify.notified())
        .await
        .expect("uTP piece multi-chunk transfer timed out");
    send_task.await.unwrap();

    let received = bytes_received.load(Ordering::Relaxed);
    assert_eq!(
        received, TOTAL,
        "uTP piece path must deliver all payload bytes"
    );

    client_outcome.handle.close("done");
    server_conn.handle.close("done");
    listener.close();
}

/// Go dialPool.getFirst returns on the first success and drains losers. A prior
/// Rust 750ms TCP-prefer after a fast uTP SYN-ACK starved uTP-established peers
/// (Bunny retain2: Go ~15 uTP vs Rust ~5 while TCP counts matched).
#[tokio::test]
async fn dial_race_first_wins_finishes_promptly() {
    let mut cfg = test_config();
    cfg.disable_tcp = false;
    cfg.disable_utp = false; // race both; uTP to a TCP-only peer fails slowly
    cfg.header_obfuscation_policy.preferred = false; // plaintext HS
    cfg.handshakes_timeout = Duration::from_secs(4);
    cfg.nominal_dial_timeout = Duration::from_secs(2);
    cfg.min_dial_timeout = Duration::from_millis(500);

    let our_ih = InfoHash([0x43; 20]);
    let server_peer_id: PeerId = [0x31; 20];
    let client_peer_id: PeerId = [0x32; 20];

    let listener = PeerListener::bind(&cfg).await.expect("bind");
    let listen_port = listener.listen_port();
    let (accepted_tx, mut accepted_rx) = mpsc::channel(4);
    let server_tracker = HalfOpenTracker::new();
    let skey_ih = our_ih;
    let skey_provider = Arc::new(move || vec![skey_ih.0]);
    let lk_ih = our_ih;
    let lookup = Arc::new(move |ih: &InfoHash| *ih == lk_ih);
    listener.spawn_accept_loop(
        cfg.clone(),
        server_peer_id,
        server_tracker,
        skey_provider,
        lookup,
        accepted_tx,
    );

    let client_tracker = HalfOpenTracker::new();
    let dial_addr: SocketAddr = format!("127.0.0.1:{listen_port}").parse().unwrap();
    let utp = listener.utp_socket().cloned();
    let t0 = Instant::now();
    let outcome = dial_peer(DialParams {
        remote_addr: dial_addr,
        info_hash: our_ih,
        our_peer_id: client_peer_id,
        transport: None, // race TCP+uTP
        source: PeerSource::Direct,
        metadata_size: None,
        cfg: &cfg,
        tracker: &client_tracker,
        dial_limiter: None,
        tcp_conn_hook: None,
        utp_socket: utp.as_ref(),
        require_plain: true,
    })
    .await
    .expect("dial_peer");
    let elapsed = t0.elapsed();
    // Spare-wait is 250ms max; localhost first-wins must finish promptly.
    assert!(
        elapsed < Duration::from_millis(1500),
        "dial race first-wins must finish promptly (elapsed {elapsed:?})"
    );
    assert!(
        matches!(
            outcome.handle.transport(),
            Transport::Tcp | Transport::Utp
        ),
        "first connected transport wins, got {:?}",
        outcome.handle.transport()
    );

    let _server = tokio::time::timeout(Duration::from_secs(2), accepted_rx.recv())
        .await
        .expect("accept")
        .expect("conn");
    outcome.handle.close("done");
    listener.close();
}

#[tokio::test]
async fn test_half_open_limits_enforced() {
    let tracker = HalfOpenTracker::new();
    let ih1 = InfoHash([1u8; 20]);
    let ih2 = InfoHash([2u8; 20]);

    // Test total limit
    let max_total = 2;
    let g1 = tracker.acquire_global(max_total).expect("g1 acquired");
    let g2 = tracker.acquire_global(max_total).expect("g2 acquired");
    assert!(tracker.acquire_global(max_total).is_none());
    assert_eq!(tracker.current_total(), 2);

    drop(g1);
    assert_eq!(tracker.current_total(), 1);
    let _g3 = tracker.acquire_global(max_total).expect("g3 acquired");
    assert_eq!(tracker.current_total(), 2);
    drop(g2);
    assert_eq!(tracker.current_total(), 1);

    // Test per-torrent limit
    let max_per_torrent = 1;
    let t1 = tracker
        .acquire_torrent(ih1, max_per_torrent)
        .expect("t1 acquired");
    assert!(tracker.acquire_torrent(ih1, max_per_torrent).is_none());
    let t2 = tracker
        .acquire_torrent(ih2, max_per_torrent)
        .expect("t2 acquired");
    assert_eq!(tracker.current_for_torrent(&ih1), 1);
    assert_eq!(tracker.current_for_torrent(&ih2), 1);

    drop(t1);
    assert_eq!(tracker.current_for_torrent(&ih1), 0);
    assert_eq!(tracker.current_for_torrent(&ih2), 1);
    drop(t2);
    assert_eq!(tracker.current_for_torrent(&ih2), 0);
}

#[tokio::test]
async fn test_rate_limiter_caps_throughput_within_10_percent() {
    let (pipe_a, pipe_b) = tokio::io::duplex(256 * 1024);

    let (write_tx_a, write_rx_a) = mpsc::unbounded_channel();
    let write_waker_a = Arc::new(Notify::new());
    let handle_a = ConnHandle::new(
        "127.0.0.1:1000".parse().unwrap(),
        true,
        Transport::Tcp,
        false,
        InfoHash([0; 20]),
        None,
        PeerSource::Direct,
        [0u8; 8],
        write_tx_a,
        write_waker_a.clone(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(wt_core::stats::AtomicConnStats::default()),
    );

    let (write_tx_b, write_rx_b) = mpsc::unbounded_channel();
    let write_waker_b = Arc::new(Notify::new());
    let handle_b = ConnHandle::new(
        "127.0.0.1:2000".parse().unwrap(),
        false,
        Transport::Tcp,
        false,
        InfoHash([0; 20]),
        None,
        PeerSource::Incoming,
        [0u8; 8],
        write_tx_b,
        write_waker_b.clone(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(wt_core::stats::AtomicConnStats::default()),
    );

    // Limit download to 8 MiB/s (8,388,608 B/s) with 1 MiB burst
    let target_rate = 8.0 * 1024.0 * 1024.0;
    let download_limiter = Limiter::new(target_rate, DEFAULT_DOWNLOAD_BURST);

    let total_transfer_bytes = 4 * 1024 * 1024; // 4 MiB
    let bytes_received = Arc::new(AtomicUsize::new(0));
    let done_notify = Arc::new(Notify::new());

    let receiver_handler = FastCountingHandler {
        bytes_received: bytes_received.clone(),
        notify: done_notify.clone(),
        target_bytes: total_transfer_bytes,
    };

    let (_r_b, _w_b) = spawn_connection_driver(
        pipe_b,
        receiver_handler,
        handle_b.clone(),
        write_rx_b,
        write_waker_b,
        Some(download_limiter),
        None,
        Duration::from_secs(60),
        Duration::from_secs(60),
    );

    let (_r_a, _w_a) = spawn_connection_driver(
        pipe_a,
        ChannelHandler {
            tx: mpsc::unbounded_channel().0,
        },
        handle_a.clone(),
        write_rx_a,
        write_waker_a,
        None,
        None,
        Duration::from_secs(60),
        Duration::from_secs(60),
    );

    let piece_payload = Bytes::from(vec![0x77u8; 16384]); // 16 KiB chunks
    let num_chunks = total_transfer_bytes / 16384;

    let start = Instant::now();
    let handle_a_clone = handle_a.clone();
    let piece_clone = piece_payload.clone();
    let send_task = tokio::spawn(async move {
        for i in 0..num_chunks {
            let msg = Message::Piece {
                index: 0,
                begin: (i * 16384) as u32,
                piece: piece_clone.clone(),
            };
            handle_a_clone.send_message_async(msg).await.unwrap();
        }
    });

    tokio::time::timeout(Duration::from_secs(5), done_notify.notified())
        .await
        .expect("transfer timed out");
    send_task.await.unwrap();

    let elapsed = start.elapsed().as_secs_f64();
    let received = bytes_received.load(Ordering::Relaxed);
    assert_eq!(received, total_transfer_bytes);

    // Theoretical minimum time: (4 MiB - 1 MiB burst) / 8 MiB/s = 3 / 8 = 0.375s
    // Rate limiter should enforce pacing around 0.35s - 0.55s
    let measured_rate = total_transfer_bytes as f64 / elapsed;
    println!(
        "Rate limiter: transferred {total_transfer_bytes} B in {elapsed:.3}s => {measured_rate:.2} B/s (limit: {target_rate} B/s)"
    );
    // Rate limiter must keep transfer from completing instantly (e.g. >= 0.3s)
    assert!(
        elapsed >= 0.30,
        "Transfer finished too fast ({elapsed:.3}s), rate limiter did not throttle!"
    );

    handle_a.close("done");
    handle_b.close("done");
}

#[tokio::test]
async fn test_conn_handle_64mib_transfer_throughput() {
    let (pipe_a, pipe_b) = tokio::io::duplex(1024 * 1024);

    let (write_tx_a, write_rx_a) = mpsc::unbounded_channel();
    let write_waker_a = Arc::new(Notify::new());
    let handle_a = ConnHandle::new(
        "127.0.0.1:3000".parse().unwrap(),
        true,
        Transport::Tcp,
        false,
        InfoHash([0; 20]),
        None,
        PeerSource::Direct,
        [0u8; 8],
        write_tx_a,
        write_waker_a.clone(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(wt_core::stats::AtomicConnStats::default()),
    );

    let (write_tx_b, write_rx_b) = mpsc::unbounded_channel();
    let write_waker_b = Arc::new(Notify::new());
    let handle_b = ConnHandle::new(
        "127.0.0.1:4000".parse().unwrap(),
        false,
        Transport::Tcp,
        false,
        InfoHash([0; 20]),
        None,
        PeerSource::Incoming,
        [0u8; 8],
        write_tx_b,
        write_waker_b.clone(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(wt_core::stats::AtomicConnStats::default()),
    );

    let total_bytes = 64 * 1024 * 1024; // 64 MiB
    let bytes_received = Arc::new(AtomicUsize::new(0));
    let done_notify = Arc::new(Notify::new());

    let receiver_handler = FastCountingHandler {
        bytes_received: bytes_received.clone(),
        notify: done_notify.clone(),
        target_bytes: total_bytes,
    };

    let (_r_b, _w_b) = spawn_connection_driver(
        pipe_b,
        receiver_handler,
        handle_b.clone(),
        write_rx_b,
        write_waker_b,
        None,
        None,
        Duration::from_secs(60),
        Duration::from_secs(60),
    );

    let (_r_a, _w_a) = spawn_connection_driver(
        pipe_a,
        ChannelHandler {
            tx: mpsc::unbounded_channel().0,
        },
        handle_a.clone(),
        write_rx_a,
        write_waker_a,
        None,
        None,
        Duration::from_secs(60),
        Duration::from_secs(60),
    );

    let chunk_size = 16384;
    let num_chunks = total_bytes / chunk_size;
    let piece_payload = Bytes::from(vec![0xccu8; chunk_size]);

    let start = Instant::now();
    for i in 0..num_chunks {
        let msg = Message::Piece {
            index: (i / 16) as u32,
            begin: ((i % 16) * chunk_size) as u32,
            piece: piece_payload.clone(),
        };
        handle_a.send_message_async(msg).await.unwrap();
    }

    tokio::time::timeout(Duration::from_secs(10), done_notify.notified())
        .await
        .expect("64 MiB transfer timed out");

    let elapsed = start.elapsed();
    let mb_per_sec = (total_bytes as f64 / (1024.0 * 1024.0)) / elapsed.as_secs_f64();
    println!("64 MiB ConnHandle transfer achieved: {mb_per_sec:.2} MB/s in {elapsed:?}");

    assert_eq!(bytes_received.load(Ordering::Relaxed), total_bytes);
    // Acceptance criterion: >= order of magnitude of wt-wire codec throughput (>= 100 MB/s)
    assert!(
        mb_per_sec >= 100.0,
        "Throughput {mb_per_sec:.2} MB/s below required >= 100 MB/s"
    );

    handle_a.close("done");
    handle_b.close("done");
}

#[tokio::test]
async fn test_shared_socket_demux_delivers_dht_while_utp_runs() {
    // Bind shared uTP socket
    let bind_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let utp_sock = Arc::new(UtpSocket::bind(bind_addr).await.expect("bind utp socket"));
    let shared_adapter = SharedDatagramAdapter::new(utp_sock.clone());
    let local_addr = shared_adapter.local_addr().expect("local addr");

    // Bind a standalone UDP socket simulating a remote DHT node
    let dht_peer = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind dht peer");
    let dht_addr = dht_peer.local_addr().unwrap();

    // Spawn a background task sending/receiving DHT datagrams through shared_adapter
    let dht_payload = b"d1:ad2:id20:abcdefghij0123456789e1:q4:ping1:t2:011:y1:qe"; // KRPC ping
    let shared_for_task = shared_adapter.clone();
    let dht_received = Arc::new(AtomicUsize::new(0));
    let dht_rx_clone = dht_received.clone();

    let dht_task = tokio::spawn(async move {
        let mut buf = vec![0u8; 2048];
        let (n, from) = shared_for_task
            .recv_from(&mut buf)
            .await
            .expect("recv_from shared");
        assert_eq!(from, dht_addr);
        assert_eq!(&buf[..n], dht_payload);
        dht_rx_clone.fetch_add(1, Ordering::Relaxed);

        // Echo response back
        let resp = b"d1:rd2:id20:0123456789abcdefghije1:t2:011:y1:re";
        shared_for_task
            .send_to(resp, from)
            .await
            .expect("send_to shared");
    });

    // Concurrently send DHT packet from remote peer to shared adapter
    dht_peer
        .send_to(dht_payload, local_addr)
        .await
        .expect("dht send");

    // Wait for DHT exchange
    dht_task.await.expect("dht task finished");
    assert_eq!(dht_received.load(Ordering::Relaxed), 1);

    let mut resp_buf = vec![0u8; 512];
    let (rn, rfrom) = dht_peer
        .recv_from(&mut resp_buf)
        .await
        .expect("dht recv resp");
    assert_eq!(rfrom, local_addr);
    assert_eq!(
        &resp_buf[..rn],
        b"d1:rd2:id20:0123456789abcdefghije1:t2:011:y1:re"
    );
}

#[tokio::test]
async fn request_wake_is_coalesced_and_does_not_extend_read_deadline() {
    struct WakeHandler(Arc<AtomicUsize>);
    impl ConnHandler for WakeHandler {
        fn on_message(&mut self, _: &ConnHandle, _: Message) -> Result<()> {
            Ok(())
        }
        fn on_closed(&mut self, _: &ConnHandle, _: &str) {}
        fn on_wake(&mut self, _: &ConnHandle) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let (stream, _peer) = tokio::io::duplex(1024);
    let (tx, rx) = mpsc::unbounded_channel();
    let waker = Arc::new(Notify::new());
    let handle = ConnHandle::new(
        "127.0.0.1:1".parse().unwrap(),
        true,
        Transport::Tcp,
        false,
        InfoHash([0; 20]),
        None,
        PeerSource::Direct,
        [0u8; 8],
        tx,
        waker.clone(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(wt_core::stats::AtomicConnStats::default()),
    );
    let count = Arc::new(AtomicUsize::new(0));
    handle.wake();
    handle.wake();
    let (reader, writer) = spawn_connection_driver(
        stream,
        WakeHandler(count.clone()),
        handle.clone(),
        rx,
        waker,
        None,
        None,
        Duration::from_secs(60),
        Duration::from_millis(150),
    );
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(count.load(Ordering::SeqCst), 1);
    handle.wake();
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(count.load(Ordering::SeqCst), 2);
    tokio::time::timeout(Duration::from_millis(200), reader)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        handle.close_reason().as_deref(),
        Some("read deadline exceeded (keepalive timeout)")
    );
    writer.abort();
}
