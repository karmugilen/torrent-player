//! BEP 55 end to end: initiator A reaches target B through relay R.
//!
//! Loopback has no NAT, so "unreachable" is staged: R listens on TCP only,
//! so A and B reach it from ephemeral TCP ports, and that ephemeral port is
//! what R's PEX hands out. A's ordinary dial to it fails; the PEX flag 0x08
//! says B can be punched, so A sends R a rendezvous. R knows B's real listen
//! port from B's extended handshake and sends connect to both. A and B dial
//! each other over uTP from their listen sockets and one connection results.

mod e2e_support;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use e2e_support::{
    greeting_torrent, peer_info, read_all_from_storage, seed_memory, testing_config, torrent_spec,
    wait_bytes_completed, wait_local_port,
};
use wt_core::{Client, MemoryClientStorage, PeerInfo, PeerSource, PexFlags, Torrent, Transport};

async fn wait_until(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + timeout;
    while !f() {
        if tokio::time::Instant::now() >= deadline {
            panic!("timed out waiting for {what}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn punched_peer(t: &Torrent, port: u16) -> bool {
    t.peer_conns().iter().any(|p| {
        p.source == PeerSource::UtHolepunch
            && p.transport == Transport::Utp
            && p.remote_addr.port() == port
    })
}

fn tcp_only_relay() -> (Client, SocketAddr) {
    let storage = Arc::new(MemoryClientStorage::new());
    let mut cfg = testing_config(storage, false);
    cfg.disable_utp = true;
    let relay = Client::new(cfg).unwrap();
    let port = relay.local_port();
    (relay, format!("127.0.0.1:{port}").parse().unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn initiator_reaches_target_through_relay() {
    let (info, ih, data, info_bytes) = greeting_torrent();

    let (relay, relay_addr) = tcp_only_relay();
    wait_local_port(&relay).await;
    let (relay_t, _) = relay
        .add_torrent_spec(torrent_spec(ih, Some(info_bytes.clone()), "hp-relay", None))
        .unwrap();
    relay_t.inner.set_info(Arc::new(info.clone())).unwrap();

    // B: the target, holding the data.
    let b_storage = Arc::new(MemoryClientStorage::new());
    let b = Client::new(testing_config(b_storage.clone(), true)).unwrap();
    let b_port = wait_local_port(&b).await;
    let (b_t, _) = b
        .add_torrent_spec(torrent_spec(ih, Some(info_bytes.clone()), "hp-b", Some(b_storage.clone())))
        .unwrap();
    b_t.inner.set_info(Arc::new(info.clone())).unwrap();
    seed_memory(&b_storage, &b_t, &info, ih, &data);
    b_t.add_peers(vec![peer_info(relay_addr)]);
    // B is in R's PEX snapshot before A arrives.
    wait_until("B connected to R", Duration::from_secs(10), || relay_t.stats().active_peers >= 1).await;

    // A: the initiator.
    let a_storage = Arc::new(MemoryClientStorage::new());
    let a = Client::new(testing_config(a_storage.clone(), false)).unwrap();
    let a_port = wait_local_port(&a).await;
    let (a_t, _) = a
        .add_torrent_spec(torrent_spec(ih, Some(info_bytes), "hp-a", Some(a_storage.clone())))
        .unwrap();
    a_t.inner.set_info(Arc::new(info.clone())).unwrap();
    a_t.download_all();
    a_t.add_peers(vec![peer_info(relay_addr)]);

    wait_until("A and B connected by a punch", Duration::from_secs(40), || {
        punched_peer(&a_t, b_port) && punched_peer(&b_t, a_port)
    })
    .await;
    wait_bytes_completed(&a_t, info.total_length(), Duration::from_secs(20)).await;
    assert_eq!(read_all_from_storage(&a_storage, &info, ih), data);

    // Per-torrent counters (same values as client aggregate with one torrent each).
    let (ha, hb, hr) = (
        a_t.holepunch_stats(),
        b_t.holepunch_stats(),
        relay_t.holepunch_stats(),
    );
    assert_eq!(ha, a.holepunch_stats());
    assert_eq!(hb, b.holepunch_stats());
    assert_eq!(hr, relay.holepunch_stats());
    assert!(ha.rendezvous_sent >= 1, "A sent a rendezvous: {ha:?}");
    assert!(hr.relayed_for_others >= 1, "R relayed: {hr:?}");
    assert!(ha.connect_received >= 1 && hb.connect_received >= 1, "{ha:?} {hb:?}");
    assert!(ha.punch_dials_attempted >= 1 && hb.punch_dials_attempted >= 1, "{ha:?} {hb:?}");
    assert!(ha.punch_dials_succeeded >= 1 && hb.punch_dials_succeeded >= 1, "{ha:?} {hb:?}");
    assert_eq!(hr.rendezvous_sent, 0, "R never needed a punch itself: {hr:?}");
    assert!(ha.punch_dials_succeeded <= ha.punch_dials_attempted + 1);
    // Successful punch attach marks peer source UtHolepunch ("C" on /connections).
    assert!(punched_peer(&a_t, b_port));
    assert!(punched_peer(&b_t, a_port));

    // Exactly one A-B connection on each side.
    let count = |t: &Torrent, port: u16| {
        t.peer_conns().iter().filter(|p| p.remote_addr.port() == port).count()
    };
    assert_eq!(count(&a_t, b_port), 1);
    assert_eq!(count(&b_t, a_port), 1);

    a.close();
    b.close();
    relay.close();
}

/// The relay is not connected to the target: A gets NotConnected back and
/// does not punch.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relay_without_target_answers_error() {
    let (info, ih, _data, info_bytes) = greeting_torrent();
    let (relay, relay_addr) = tcp_only_relay();
    wait_local_port(&relay).await;
    let (relay_t, _) = relay
        .add_torrent_spec(torrent_spec(ih, Some(info_bytes.clone()), "hp-relay2", None))
        .unwrap();
    relay_t.inner.set_info(Arc::new(info.clone())).unwrap();

    let mut cfg = testing_config(Arc::new(MemoryClientStorage::new()), false);
    cfg.nominal_dial_timeout = Duration::from_secs(2);
    let a = Client::new(cfg).unwrap();
    wait_local_port(&a).await;
    let (a_t, _) = a
        .add_torrent_spec(torrent_spec(ih, Some(info_bytes), "hp-a2", None))
        .unwrap();
    a_t.inner.set_info(Arc::new(info.clone())).unwrap();
    a_t.download_all();
    a_t.add_peers(vec![peer_info(relay_addr)]);
    wait_until("A connected to R", Duration::from_secs(10), || a_t.stats().active_peers >= 1).await;

    // A punchable peer nobody is connected to (nothing listens there).
    let dead = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let dead_addr = dead.local_addr().unwrap();
    drop(dead);
    a_t.add_peers(vec![PeerInfo {
        id: None,
        addr: dead_addr,
        source: PeerSource::Pex,
        supports_encryption: false,
        pex: PexFlags { supports_holepunch: true, supports_utp: true, ..Default::default() },
        trusted: false,
    }]);

    wait_until("relay error", Duration::from_secs(20), || a_t.holepunch_stats().errors_received >= 1).await;
    let ha = a_t.holepunch_stats();
    assert_eq!(ha, a.holepunch_stats());
    assert!(ha.rendezvous_sent >= 1, "{ha:?}");
    assert_eq!(ha.connect_received, 0, "{ha:?}");
    assert_eq!(ha.punch_dials_attempted, 0, "{ha:?}");
    assert_eq!(ha.punch_dials_succeeded, 0, "{ha:?}");
    assert_eq!(relay_t.holepunch_stats().relayed_for_others, 0);

    a.close();
    relay.close();
}

/// `disable_holepunch` keeps the extension out of the handshake: nothing is
/// relayed or sent even when a punchable peer fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disabled_holepunch_sends_nothing() {
    let (info, ih, _data, info_bytes) = greeting_torrent();
    let (relay, relay_addr) = tcp_only_relay();
    wait_local_port(&relay).await;
    let (relay_t, _) = relay
        .add_torrent_spec(torrent_spec(ih, Some(info_bytes.clone()), "hp-relay3", None))
        .unwrap();
    relay_t.inner.set_info(Arc::new(info.clone())).unwrap();

    let mut cfg = testing_config(Arc::new(MemoryClientStorage::new()), false);
    cfg.disable_holepunch = true;
    cfg.nominal_dial_timeout = Duration::from_secs(2);
    let a = Client::new(cfg).unwrap();
    wait_local_port(&a).await;
    let (a_t, _) = a
        .add_torrent_spec(torrent_spec(ih, Some(info_bytes), "hp-a3", None))
        .unwrap();
    a_t.inner.set_info(Arc::new(info.clone())).unwrap();
    a_t.download_all();
    a_t.add_peers(vec![peer_info(relay_addr)]);
    wait_until("A connected to R", Duration::from_secs(10), || a_t.stats().active_peers >= 1).await;
    a_t.add_peers(vec![PeerInfo {
        id: None,
        addr: "127.0.0.1:9".parse().unwrap(),
        source: PeerSource::Pex,
        supports_encryption: false,
        pex: PexFlags { supports_holepunch: true, supports_utp: true, ..Default::default() },
        trusted: false,
    }]);
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(a_t.holepunch_stats(), Default::default());
    assert_eq!(relay_t.holepunch_stats(), Default::default());
    assert_eq!(a.holepunch_stats(), Default::default());
    assert_eq!(relay.holepunch_stats(), Default::default());
    a.close();
    relay.close();
}

/// Two torrents on one client keep independent holepunch counters.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_torrents_do_not_share_holepunch_counters() {
    let (info_a, ih_a, _data_a, info_bytes_a) = greeting_torrent();
    let (info_b, ih_b, _data_b, info_bytes_b) = patterned_second_torrent();

    let (relay, relay_addr) = tcp_only_relay();
    wait_local_port(&relay).await;
    let (relay_ta, _) = relay
        .add_torrent_spec(torrent_spec(ih_a, Some(info_bytes_a.clone()), "hp-r-a", None))
        .unwrap();
    relay_ta.inner.set_info(Arc::new(info_a.clone())).unwrap();
    let (relay_tb, _) = relay
        .add_torrent_spec(torrent_spec(ih_b, Some(info_bytes_b.clone()), "hp-r-b", None))
        .unwrap();
    relay_tb.inner.set_info(Arc::new(info_b.clone())).unwrap();

    let mut cfg = testing_config(Arc::new(MemoryClientStorage::new()), false);
    cfg.nominal_dial_timeout = Duration::from_secs(2);
    let a = Client::new(cfg).unwrap();
    wait_local_port(&a).await;
    let (t_active, _) = a
        .add_torrent_spec(torrent_spec(ih_a, Some(info_bytes_a), "hp-active", None))
        .unwrap();
    t_active.inner.set_info(Arc::new(info_a.clone())).unwrap();
    t_active.download_all();
    t_active.add_peers(vec![peer_info(relay_addr)]);
    wait_until("active connected to R", Duration::from_secs(10), || {
        t_active.stats().active_peers >= 1
    })
    .await;

    let (t_idle, _) = a
        .add_torrent_spec(torrent_spec(ih_b, Some(info_bytes_b), "hp-idle", None))
        .unwrap();
    t_idle.inner.set_info(Arc::new(info_b.clone())).unwrap();
    // Idle torrent never dials or punches.

    let dead = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let dead_addr = dead.local_addr().unwrap();
    drop(dead);
    t_active.add_peers(vec![PeerInfo {
        id: None,
        addr: dead_addr,
        source: PeerSource::Pex,
        supports_encryption: false,
        pex: PexFlags {
            supports_holepunch: true,
            supports_utp: true,
            ..Default::default()
        },
        trusted: false,
    }]);

    wait_until("active got relay error", Duration::from_secs(20), || {
        t_active.holepunch_stats().errors_received >= 1
    })
    .await;

    let active = t_active.holepunch_stats();
    let idle = t_idle.holepunch_stats();
    assert!(active.rendezvous_sent >= 1, "{active:?}");
    assert!(active.errors_received >= 1, "{active:?}");
    assert_eq!(idle, Default::default(), "idle torrent must stay at zero: {idle:?}");
    assert_eq!(
        a.holepunch_stats(),
        active + idle,
        "client aggregate is the sum of live torrents"
    );
    assert_eq!(relay_tb.holepunch_stats(), Default::default());

    a.close();
    relay.close();
}

/// Second greeting-sized torrent with a distinct infohash for isolation tests.
fn patterned_second_torrent() -> (wt_core::Info, wt_core::InfoHash, Vec<u8>, Vec<u8>) {
    e2e_support::patterned_torrent("greeting-b", e2e_support::GREETING_CONTENTS.len(), 5)
}
