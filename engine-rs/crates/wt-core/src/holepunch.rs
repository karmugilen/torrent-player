//! BEP 55 hole punching (`ut_holepunch`): relay, initiator and target policy.
//!
//! The three roles:
//! - **Initiator**: a dial to a PEX peer flagged 0x08 failed. Ask a connected
//!   `ut_holepunch` peer (preferably the one that told us about the target)
//!   to relay a *rendezvous*; on its *connect*, dial the target over uTP.
//! - **Relay**: on a rendezvous from A naming B, send *connect* to both A
//!   (with B's endpoint) and B (with A's endpoint), or answer with an error.
//! - **Target**: on a *connect*, dial the endpoint over uTP at once, so both
//!   sides send through their NATs at the same time.
//!
//! Punch dials go out through the uTP listen socket: the NAT mapping the
//! remote reaches is that socket's. All decisions here are pure and take the
//! clock as an argument; the torrent glue lives at the bottom of this file.

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use wt_types::{PeerInfo, PeerSource, Transport};
use wt_wire::holepunch::{HolepunchError, HolepunchMsg, HolepunchMsgType};

use crate::torrent::TorrentInner;

/// A connect answering our rendezvous counts as solicited for this long.
pub const PENDING_TTL: Duration = Duration::from_secs(30);
/// Rendezvous attempts per target before giving up on it.
pub const MAX_ATTEMPTS_PER_TARGET: u8 = 2;
/// Least time between two rendezvous for one target.
pub const RETRY_GAP: Duration = Duration::from_secs(20);
/// An exhausted target may be tried again after this long.
pub const ATTEMPT_MEMORY: Duration = Duration::from_secs(10 * 60);
/// Sliding window for every rate limit below.
pub const RATE_WINDOW: Duration = Duration::from_secs(60);
/// Rendezvous we send through one relay per window.
pub const RENDEZVOUS_PER_RELAY: usize = 4;
/// Rendezvous one torrent sends per window (punch-first sends more of them).
pub const RENDEZVOUS_PER_TORRENT: usize = 32;
/// After a punch-first rendezvous, the direct dial waits this long so the
/// punch can land first; if it does not, the peer is dialed normally.
pub const PUNCH_FIRST_WAIT: Duration = Duration::from_secs(8);

/// Punch first instead of dialing: PEX says the peer speaks ut_holepunch and
/// nobody reported an outgoing connection to it (BEP 11 flag 0x10).
pub fn punch_first_candidate(peer: &wt_types::PeerInfo) -> bool {
    peer.source == wt_types::PeerSource::Pex
        && peer.pex.supports_holepunch
        && !peer.pex.reachable
        && usable_endpoint(peer.addr)
}
/// Rendezvous we relay for one sender per window.
pub const RELAY_PER_SENDER: usize = 8;
/// Rendezvous one torrent relays per window.
pub const RELAY_PER_TORRENT: usize = 32;
/// Connects we act on per relay per window when we did not ask for them.
pub const UNSOLICITED_PER_RELAY: usize = 2;
/// Unsolicited connects one torrent acts on per window.
pub const UNSOLICITED_PER_TORRENT: usize = 6;
/// A connection from a punched endpoint counts as punched for this long.
pub const PUNCH_TTL: Duration = Duration::from_secs(30);
/// SYN resend interval of punch dials.
pub const PUNCH_SYN_RESEND: Duration = Duration::from_millis(500);
/// Bound on every per-address map.
const MAP_CAP: usize = 512;

// ---------------------------------------------------------------- counters

/// Per-torrent counters. Monotonic for the life of that torrent.
#[derive(Default)]
pub struct HolepunchCounters {
    pub rendezvous_sent: AtomicU64,
    pub connect_received: AtomicU64,
    pub relayed_for_others: AtomicU64,
    pub errors_received: AtomicU64,
    pub punch_dials_attempted: AtomicU64,
    pub punch_dials_succeeded: AtomicU64,
}

impl HolepunchCounters {
    pub fn snapshot(&self) -> HolepunchStats {
        let l = |a: &AtomicU64| a.load(Ordering::Relaxed);
        HolepunchStats {
            rendezvous_sent: l(&self.rendezvous_sent),
            connect_received: l(&self.connect_received),
            relayed_for_others: l(&self.relayed_for_others),
            errors_received: l(&self.errors_received),
            punch_dials_attempted: l(&self.punch_dials_attempted),
            punch_dials_succeeded: l(&self.punch_dials_succeeded),
        }
    }

    pub(crate) fn bump(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

/// Snapshot of [`HolepunchCounters`], serialized on `/connections/{id}` and
/// aggregated on engine `/stats`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HolepunchStats {
    /// Rendezvous messages we sent to relays.
    pub rendezvous_sent: u64,
    /// Connect messages relays sent us (solicited or not, acted on or not).
    pub connect_received: u64,
    /// Rendezvous we relayed for other peers (connect sent to both sides).
    pub relayed_for_others: u64,
    /// Error messages relays sent us in answer to our rendezvous.
    pub errors_received: u64,
    /// uTP punch dials started after a connect message.
    pub punch_dials_attempted: u64,
    /// Punched endpoints that ended up as an established peer connection.
    /// Bumped only when [`TorrentInner::note_attached_punch`] consumes a
    /// punch record — the same path that marks the peer source `"C"`.
    pub punch_dials_succeeded: u64,
}

impl std::ops::Add for HolepunchStats {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self {
            rendezvous_sent: self.rendezvous_sent + rhs.rendezvous_sent,
            connect_received: self.connect_received + rhs.connect_received,
            relayed_for_others: self.relayed_for_others + rhs.relayed_for_others,
            errors_received: self.errors_received + rhs.errors_received,
            punch_dials_attempted: self.punch_dials_attempted + rhs.punch_dials_attempted,
            punch_dials_succeeded: self.punch_dials_succeeded + rhs.punch_dials_succeeded,
        }
    }
}

impl std::ops::AddAssign for HolepunchStats {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

// ---------------------------------------------------------------- relay

/// What the relay knows about one of its connections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelayPeerView {
    /// Key of the connection (its remote address).
    pub conn_addr: SocketAddr,
    /// Where a punch must aim: the uTP source address, or for TCP the IP
    /// with the peer's advertised listen port.
    pub endpoint: SocketAddr,
    /// Peer's `ut_holepunch` id from its extended handshake.
    pub ext_id: Option<u8>,
    /// Our connection to this peer is uTP. A relay tells the target to dial
    /// the address it sees us at, and only on uTP is that our punchable UDP
    /// endpoint; over TCP it is an ephemeral TCP port.
    pub utp: bool,
}

impl RelayPeerView {
    fn is(&self, addr: SocketAddr) -> bool {
        self.conn_addr == addr || self.endpoint == addr
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayAction {
    /// Send `connect(sender_gets)` to the sender and `connect(target_gets)`
    /// to the connection `target_conn`.
    Connect {
        target_conn: SocketAddr,
        sender_gets: SocketAddr,
        target_gets: SocketAddr,
    },
    /// Answer the sender with this error.
    Error(HolepunchError),
}

/// Endpoint a punch could plausibly reach: unicast with a port.
pub fn usable_endpoint(addr: SocketAddr) -> bool {
    let ip_ok = match addr.ip() {
        IpAddr::V4(v4) => !(v4.is_unspecified() || v4.is_broadcast() || v4.is_multicast()),
        IpAddr::V6(v6) => !(v6.is_unspecified() || v6.is_multicast()),
    };
    ip_ok && addr.port() != 0
}

/// BEP 55 relay decision for a rendezvous from `sender` naming `target`.
pub fn relay_decision(
    sender: &RelayPeerView,
    target: SocketAddr,
    peers: &[RelayPeerView],
) -> RelayAction {
    if !usable_endpoint(target) {
        return RelayAction::Error(HolepunchError::NoSuchPeer);
    }
    if sender.is(target) {
        return RelayAction::Error(HolepunchError::NoSelf);
    }
    let Some(found) = peers
        .iter()
        .find(|p| p.conn_addr != sender.conn_addr && p.is(target))
    else {
        return RelayAction::Error(HolepunchError::NotConnected);
    };
    if found.ext_id.is_none() {
        return RelayAction::Error(HolepunchError::NoSupport);
    }
    RelayAction::Connect {
        target_conn: found.conn_addr,
        sender_gets: found.endpoint,
        target_gets: sender.endpoint,
    }
}

// ---------------------------------------------------------------- state

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectVerdict {
    /// Dial `endpoint` now.
    Punch { solicited: bool },
    /// Do nothing.
    Ignore(&'static str),
}

#[derive(Debug, Default)]
struct Attempt {
    count: u8,
    last: Option<Instant>,
    tried: Vec<SocketAddr>,
}

#[derive(Debug, Default)]
struct Window(VecDeque<Instant>);

impl Window {
    fn prune(&mut self, now: Instant) {
        while self
            .0
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= RATE_WINDOW)
        {
            self.0.pop_front();
        }
    }
    fn has_room(&mut self, now: Instant, max: usize) -> bool {
        self.prune(now);
        self.0.len() < max
    }
    fn take(&mut self, now: Instant, max: usize) -> bool {
        if !self.has_room(now, max) {
            return false;
        }
        self.0.push_back(now);
        true
    }
}

fn keyed_take(
    map: &mut HashMap<SocketAddr, Window>,
    key: SocketAddr,
    now: Instant,
    max: usize,
) -> bool {
    if !map.contains_key(&key) && map.len() >= MAP_CAP {
        map.retain(|_, w| {
            w.prune(now);
            !w.0.is_empty()
        });
        if map.len() >= MAP_CAP {
            return false;
        }
    }
    map.entry(key).or_default().take(now, max)
}

fn keyed_has_room(map: &mut HashMap<SocketAddr, Window>, key: SocketAddr, now: Instant, max: usize) -> bool {
    map.get_mut(&key).is_none_or(|w| w.has_room(now, max))
}

/// Per-torrent hole punching bookkeeping and rate limits.
#[derive(Debug, Default)]
pub struct HolepunchState {
    /// Target endpoint → connection whose PEX reported it with flag 0x08.
    pex_origin: HashMap<SocketAddr, SocketAddr>,
    attempts: HashMap<SocketAddr, Attempt>,
    /// Target → (relay, when) of rendezvous awaiting an answer.
    pending: HashMap<SocketAddr, (SocketAddr, Instant)>,
    rendezvous_torrent: Window,
    rendezvous_relay: HashMap<SocketAddr, Window>,
    relay_torrent: Window,
    relay_sender: HashMap<SocketAddr, Window>,
    unsolicited_torrent: Window,
    unsolicited_relay: HashMap<SocketAddr, Window>,
    /// Endpoints we punched toward, until when a connection counts as punched.
    punched: HashMap<SocketAddr, Instant>,
}

impl HolepunchState {
    pub fn new() -> Self {
        Self::default()
    }

    /// `from` reported `target` in PEX with the holepunch flag.
    pub fn note_pex_origin(&mut self, target: SocketAddr, from: SocketAddr) {
        if self.pex_origin.len() >= MAP_CAP && !self.pex_origin.contains_key(&target) {
            return;
        }
        self.pex_origin.insert(target, from);
    }

    pub fn pex_origin(&self, target: SocketAddr) -> Option<SocketAddr> {
        self.pex_origin.get(&target).copied()
    }

    /// A connection closed: forget it as a PEX origin.
    pub fn forget_conn(&mut self, conn: SocketAddr) {
        self.pex_origin.retain(|_, from| *from != conn);
    }

    fn prune(&mut self, now: Instant) {
        self.pending
            .retain(|_, (_, at)| now.saturating_duration_since(*at) < PENDING_TTL);
        self.attempts.retain(|_, a| {
            a.last
                .is_some_and(|t| now.saturating_duration_since(t) < ATTEMPT_MEMORY)
        });
        self.punched.retain(|_, until| *until > now);
    }

    /// Initiator: pick a relay for `target` among connected `relays` (all of
    /// which support `ut_holepunch`) and record the rendezvous. `None` when
    /// limits or the absence of a usable relay forbid one.
    pub fn plan_rendezvous(
        &mut self,
        target: SocketAddr,
        relays: &[RelayPeerView],
        now: Instant,
    ) -> Option<SocketAddr> {
        self.prune(now);
        if !usable_endpoint(target) || self.pending.contains_key(&target) {
            return None;
        }
        if self.attempts.len() >= MAP_CAP && !self.attempts.contains_key(&target) {
            return None;
        }
        let attempt = self.attempts.entry(target).or_default();
        if attempt.count >= MAX_ATTEMPTS_PER_TARGET
            || attempt
                .last
                .is_some_and(|t| now.saturating_duration_since(t) < RETRY_GAP)
        {
            return None;
        }
        if !self.rendezvous_torrent.has_room(now, RENDEZVOUS_PER_TORRENT) {
            return None;
        }
        let origin = self.pex_origin.get(&target).copied();
        let usable = |r: &&RelayPeerView| {
            r.ext_id.is_some() && !r.is(target) && !attempt.tried.contains(&r.conn_addr)
        };
        // The peer that told us about the target is the one likely connected
        // to it; any other relay is a guess, tried only after it.
        let mut ordered: Vec<&RelayPeerView> = relays.iter().filter(usable).collect();
        // Among equals a uTP relay goes first: see `RelayPeerView::utp`.
        ordered.sort_by_key(|r| (Some(r.conn_addr) != origin, !r.utp, r.conn_addr));
        let relay = ordered.into_iter().find(|r| {
            keyed_has_room(&mut self.rendezvous_relay, r.conn_addr, now, RENDEZVOUS_PER_RELAY)
        })?;
        let relay = relay.conn_addr;
        if !keyed_take(&mut self.rendezvous_relay, relay, now, RENDEZVOUS_PER_RELAY) {
            return None;
        }
        self.rendezvous_torrent.take(now, RENDEZVOUS_PER_TORRENT);
        let attempt = self.attempts.entry(target).or_default();
        attempt.count += 1;
        attempt.last = Some(now);
        attempt.tried.push(relay);
        if self.pending.len() < MAP_CAP {
            self.pending.insert(target, (relay, now));
        }
        Some(relay)
    }

    /// Target/initiator: a connect for `endpoint` arrived from `relay`.
    pub fn on_connect(&mut self, relay: SocketAddr, endpoint: SocketAddr, now: Instant) -> ConnectVerdict {
        self.prune(now);
        if !usable_endpoint(endpoint) {
            return ConnectVerdict::Ignore("unusable endpoint");
        }
        // The relay answers with the endpoint it knows for the target, which
        // can differ in port from the address PEX gave us (an ephemeral TCP
        // port), so match our rendezvous by relay and IP.
        let ours = self
            .pending
            .iter()
            .filter(|(t, (r, _))| *r == relay && t.ip() == endpoint.ip())
            .min_by_key(|(t, _)| **t != endpoint)
            .map(|(t, _)| *t);
        if let Some(target) = ours {
            self.pending.remove(&target);
            return ConnectVerdict::Punch { solicited: true };
        }
        // Unsolicited: we are the target. A relay could aim us at anything,
        // so it gets a small budget of dials.
        if !keyed_has_room(&mut self.unsolicited_relay, relay, now, UNSOLICITED_PER_RELAY)
            || !self.unsolicited_torrent.has_room(now, UNSOLICITED_PER_TORRENT)
        {
            return ConnectVerdict::Ignore("unsolicited connect rate limit");
        }
        if !keyed_take(&mut self.unsolicited_relay, relay, now, UNSOLICITED_PER_RELAY) {
            return ConnectVerdict::Ignore("unsolicited connect rate limit");
        }
        self.unsolicited_torrent.take(now, UNSOLICITED_PER_TORRENT);
        ConnectVerdict::Punch { solicited: false }
    }

    /// Initiator: the relay answered our rendezvous for `endpoint` with an error.
    pub fn on_error(&mut self, relay: SocketAddr, endpoint: SocketAddr) {
        // Error messages echo the address from our rendezvous.
        if self.pending.get(&endpoint).is_some_and(|(r, _)| *r == relay) {
            self.pending.remove(&endpoint);
        }
    }

    pub fn is_pending(&self, target: SocketAddr) -> bool {
        self.pending.contains_key(&target)
    }

    /// Relay: may we relay one more rendezvous from `sender`?
    pub fn allow_relay(&mut self, sender: SocketAddr, now: Instant) -> bool {
        if !self.relay_torrent.has_room(now, RELAY_PER_TORRENT)
            || !keyed_has_room(&mut self.relay_sender, sender, now, RELAY_PER_SENDER)
        {
            return false;
        }
        if !keyed_take(&mut self.relay_sender, sender, now, RELAY_PER_SENDER) {
            return false;
        }
        self.relay_torrent.take(now, RELAY_PER_TORRENT)
    }

    /// A punch dial toward `endpoint` started.
    pub fn note_punch(&mut self, endpoint: SocketAddr, now: Instant) {
        self.prune(now);
        if self.punched.len() < MAP_CAP || self.punched.contains_key(&endpoint) {
            self.punched.insert(endpoint, now + PUNCH_TTL);
        }
    }

    /// Whether a connection from `endpoint` is the result of a recent punch.
    pub fn is_punched(&self, endpoint: SocketAddr, now: Instant) -> bool {
        self.punched.get(&endpoint).is_some_and(|until| *until > now)
    }

    /// Consume the punch record for `endpoint`; true once per punch.
    pub fn take_punched(&mut self, endpoint: SocketAddr, now: Instant) -> bool {
        match self.punched.remove(&endpoint) {
            Some(until) => until > now,
            None => false,
        }
    }
}

// ---------------------------------------------------------------- torrent glue

impl TorrentInner {
    /// `ut_holepunch` is offered on public torrents with PEX, unless disabled.
    pub(crate) fn holepunch_allowed(&self) -> bool {
        use crate::peer_conn::PeerTorrent;
        !self.pex_disabled()
            && self
                .client
                .read()
                .upgrade()
                .is_some_and(|c| !c.config.disable_holepunch)
    }

    /// Relay view of a live connection.
    pub(crate) fn relay_view(&self, conn_addr: SocketAddr) -> Option<RelayPeerView> {
        let transport = self.conn_handles.read().get(&conn_addr).map(|h| h.transport())?;
        let endpoint = match transport {
            // uTP dials leave from the listen socket: the source is the endpoint.
            Transport::Utp => conn_addr,
            _ => self
                .peer_listen_ports
                .read()
                .get(&conn_addr)
                .map(|p| SocketAddr::new(conn_addr.ip(), *p))
                .unwrap_or(conn_addr),
        };
        Some(RelayPeerView {
            conn_addr,
            endpoint,
            ext_id: self.holepunch_peers.read().get(&conn_addr).copied(),
            utp: matches!(transport, Transport::Utp),
        })
    }

    fn relay_views(&self) -> Vec<RelayPeerView> {
        let addrs: Vec<SocketAddr> = self
            .conn_handles
            .read()
            .iter()
            .filter(|(_, h)| !h.is_closed())
            .map(|(a, _)| *a)
            .collect();
        addrs.into_iter().filter_map(|a| self.relay_view(a)).collect()
    }

    /// Already connected to, or dialing, whatever lives at `endpoint`.
    pub(crate) fn has_peer_at(&self, endpoint: SocketAddr) -> bool {
        if self.conn_handles.read().contains_key(&endpoint)
            || self.dialing_peers.lock().contains(&endpoint)
        {
            return true;
        }
        self.relay_views().iter().any(|v| v.is(endpoint))
    }

    fn send_holepunch(&self, conn: SocketAddr, ext_id: u8, msg: HolepunchMsg) -> bool {
        match self.conn_handles.read().get(&conn) {
            Some(h) => h.send_message(msg.to_message(ext_id)),
            None => false,
        }
    }

    /// Initiator: a dial to `peer` failed. Ask a relay to introduce us when
    /// the peer said (via PEX) it can be punched. Returns whether a
    /// rendezvous went out.
    pub(crate) fn maybe_rendezvous(&self, peer: &PeerInfo) -> bool {
        if !peer.pex.supports_holepunch || !self.holepunch_allowed() {
            return false;
        }
        let Some(client) = self.client.read().upgrade() else {
            return false;
        };
        if !client.can_punch(peer.addr)
            || self.wants_conns() == 0
            || self.has_peer_at(peer.addr)
        {
            return false;
        }
        let relays: Vec<RelayPeerView> = self
            .relay_views()
            .into_iter()
            .filter(|v| v.ext_id.is_some())
            .collect();
        let now = Instant::now();
        let Some(relay) = self.holepunch.lock().plan_rendezvous(peer.addr, &relays, now) else {
            return false;
        };
        let Some(ext_id) = relays.iter().find(|r| r.conn_addr == relay).and_then(|r| r.ext_id) else {
            return false;
        };
        if !self.send_holepunch(relay, ext_id, HolepunchMsg::rendezvous(peer.addr)) {
            self.holepunch.lock().on_error(relay, peer.addr);
            return false;
        }
        HolepunchCounters::bump(&self.holepunch_stats.rendezvous_sent);
        wt_types::events::record("holepunch_rendezvous", format!("relay={relay} target={}", peer.addr));
        true
    }

    /// A `ut_holepunch` message arrived on the connection `from`.
    pub(crate) fn handle_holepunch(&self, from: SocketAddr, msg: HolepunchMsg) {
        if !self.holepunch_allowed() {
            return;
        }
        let counters = &self.holepunch_stats;
        let now = Instant::now();
        match msg.msg_type {
            HolepunchMsgType::Rendezvous => {
                let Some(sender) = self.relay_view(from) else { return };
                // We can only answer a peer that told us its extension id.
                let Some(reply_id) = sender.ext_id else { return };
                if !self.holepunch.lock().allow_relay(from, now) {
                    return;
                }
                match relay_decision(&sender, msg.addr, &self.relay_views()) {
                    RelayAction::Connect { target_conn, sender_gets, target_gets } => {
                        let Some(target_id) = self.holepunch_peers.read().get(&target_conn).copied() else {
                            return;
                        };
                        // Target first: it is the side that has to open its NAT.
                        let sent = self.send_holepunch(target_conn, target_id, HolepunchMsg::connect(target_gets))
                            && self.send_holepunch(from, reply_id, HolepunchMsg::connect(sender_gets));
                        if sent {
                            HolepunchCounters::bump(&counters.relayed_for_others);
                        }
                    }
                    RelayAction::Error(err) => {
                        self.send_holepunch(from, reply_id, HolepunchMsg::error(msg.addr, err));
                    }
                }
            }
            HolepunchMsgType::Connect => {
                HolepunchCounters::bump(&counters.connect_received);
                let endpoint = msg.addr;
                let Some(client) = self.client.read().upgrade() else { return };
                if !client.can_punch(endpoint) || self.has_peer_at(endpoint) || self.wants_conns() == 0 {
                    self.holepunch.lock().on_error(from, endpoint);
                    return;
                }
                let verdict = self.holepunch.lock().on_connect(from, endpoint, now);
                if let ConnectVerdict::Punch { solicited } = verdict {
                    wt_types::events::record(
                        "holepunch_connect",
                        format!("relay={from} endpoint={endpoint} solicited={solicited}"),
                    );
                    if let Some(me) = client.torrent(self.info_hash) {
                        client.spawn_punch_dial(me, endpoint);
                    }
                }
            }
            HolepunchMsgType::Error => {
                HolepunchCounters::bump(&counters.errors_received);
                self.holepunch.lock().on_error(from, msg.addr);
                wt_types::events::record(
                    "holepunch_error",
                    format!("relay={from} target={} code={}", msg.addr, msg.err_code),
                );
            }
        }
    }

    /// A connection attached; count it when it came from a punch.
    ///
    /// Callers: every successful [`Self::attach_conn`] (outbound punch dial
    /// and inbound simultaneous-open). `punch_dials_succeeded` and peer
    /// source `"C"` stay aligned: inbound attach rewrites the source to
    /// [`PeerSource::UtHolepunch`] only when [`HolepunchState::is_punched`]
    /// is true, and this method bumps the counter only when
    /// [`HolepunchState::take_punched`] consumes that same record.
    pub(crate) fn note_attached_punch(&self, remote: SocketAddr, source: PeerSource) {
        // Peer hints restore the UtHolepunch source on ordinary dials, so the
        // source alone does not prove a punch: the punch record does.
        let _ = source;
        if self.holepunch.lock().take_punched(remote, Instant::now()) {
            HolepunchCounters::bump(&self.holepunch_stats.punch_dials_succeeded);
        }
    }
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    fn view(conn: &str, endpoint: &str, ext: Option<u8>) -> RelayPeerView {
        RelayPeerView { conn_addr: a(conn), endpoint: a(endpoint), ext_id: ext, utp: false }
    }

    // ------------------------------------------------ relay

    #[test]
    fn relay_connects_both_sides_with_each_others_endpoint() {
        // A reached us over TCP from an ephemeral port; it listens on 6881.
        let sender = view("198.51.100.1:40000", "198.51.100.1:6881", Some(3));
        let target = view("203.0.113.9:51413", "203.0.113.9:51413", Some(7));
        let peers = [sender, target];
        assert_eq!(
            relay_decision(&sender, a("203.0.113.9:51413"), &peers),
            RelayAction::Connect {
                target_conn: a("203.0.113.9:51413"),
                sender_gets: a("203.0.113.9:51413"),
                target_gets: a("198.51.100.1:6881"),
            }
        );
    }

    #[test]
    fn relay_finds_target_by_listen_endpoint_of_tcp_conn() {
        let sender = view("198.51.100.1:6881", "198.51.100.1:6881", Some(3));
        let target = view("203.0.113.9:40001", "203.0.113.9:51413", Some(7));
        match relay_decision(&sender, a("203.0.113.9:51413"), &[sender, target]) {
            RelayAction::Connect { target_conn, sender_gets, .. } => {
                assert_eq!(target_conn, a("203.0.113.9:40001"));
                assert_eq!(sender_gets, a("203.0.113.9:51413"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn relay_error_codes() {
        let sender = view("198.51.100.1:6881", "198.51.100.1:6881", Some(3));
        let nosupport = view("203.0.113.9:51413", "203.0.113.9:51413", None);
        let peers = [sender, nosupport];
        let err = |t: &str| relay_decision(&sender, a(t), &peers);
        assert_eq!(err("0.0.0.0:6881"), RelayAction::Error(HolepunchError::NoSuchPeer));
        assert_eq!(err("203.0.113.9:0"), RelayAction::Error(HolepunchError::NoSuchPeer));
        assert_eq!(err("[::]:6881"), RelayAction::Error(HolepunchError::NoSuchPeer));
        assert_eq!(err("224.0.0.1:6881"), RelayAction::Error(HolepunchError::NoSuchPeer));
        assert_eq!(err("192.0.2.200:6881"), RelayAction::Error(HolepunchError::NotConnected));
        assert_eq!(err("203.0.113.9:51413"), RelayAction::Error(HolepunchError::NoSupport));
        assert_eq!(err("198.51.100.1:6881"), RelayAction::Error(HolepunchError::NoSelf));
    }

    #[test]
    fn relay_rate_limits_per_sender_and_torrent() {
        let mut st = HolepunchState::new();
        let now = Instant::now();
        let s1 = a("198.51.100.1:6881");
        for _ in 0..RELAY_PER_SENDER {
            assert!(st.allow_relay(s1, now));
        }
        assert!(!st.allow_relay(s1, now), "per-sender cap");
        assert!(st.allow_relay(s1, now + RATE_WINDOW), "window slides");
        let mut st = HolepunchState::new();
        for i in 0..RELAY_PER_TORRENT {
            let s = SocketAddr::new(s1.ip(), 1000 + i as u16);
            assert!(st.allow_relay(s, now));
        }
        assert!(!st.allow_relay(a("198.51.100.2:1"), now), "torrent cap");
    }

    // ------------------------------------------------ initiator

    #[test]
    fn initiator_prefers_pex_origin_and_bounds_retries() {
        let mut st = HolepunchState::new();
        let now = Instant::now();
        let target = a("203.0.113.9:51413");
        let r1 = view("198.51.100.1:6881", "198.51.100.1:6881", Some(3));
        let r2 = view("198.51.100.2:6881", "198.51.100.2:6881", Some(3));
        st.note_pex_origin(target, r2.conn_addr);
        let relays = [r1, r2];
        assert_eq!(st.plan_rendezvous(target, &relays, now), Some(r2.conn_addr));
        // In flight: no second rendezvous.
        assert_eq!(st.plan_rendezvous(target, &relays, now), None);
        st.on_error(r2.conn_addr, target);
        // Too soon after the first.
        assert_eq!(st.plan_rendezvous(target, &relays, now + Duration::from_secs(1)), None);
        // Next attempt uses a relay not yet tried.
        let later = now + RETRY_GAP;
        assert_eq!(st.plan_rendezvous(target, &relays, later), Some(r1.conn_addr));
        st.on_error(r1.conn_addr, target);
        // Attempts exhausted.
        assert_eq!(st.plan_rendezvous(target, &relays, later + RETRY_GAP), None);
        // Forgotten after a long while.
        assert!(st.plan_rendezvous(target, &relays, later + ATTEMPT_MEMORY).is_some());
    }

    #[test]
    fn initiator_needs_a_supporting_relay_that_is_not_the_target() {
        let mut st = HolepunchState::new();
        let now = Instant::now();
        let target = a("203.0.113.9:51413");
        let nosupport = view("198.51.100.1:6881", "198.51.100.1:6881", None);
        let is_target = view("203.0.113.9:51413", "203.0.113.9:51413", Some(3));
        assert_eq!(st.plan_rendezvous(target, &[nosupport, is_target], now), None);
        assert_eq!(st.plan_rendezvous(a("0.0.0.0:1"), &[], now), None);
    }

    #[test]
    fn initiator_rate_limits_per_relay_and_torrent() {
        let mut st = HolepunchState::new();
        let now = Instant::now();
        let r = view("198.51.100.1:6881", "198.51.100.1:6881", Some(3));
        let mut sent = 0;
        for i in 0..50u16 {
            let t = SocketAddr::new("203.0.113.9".parse().unwrap(), 2000 + i);
            if st.plan_rendezvous(t, &[r], now).is_some() {
                sent += 1;
            }
        }
        assert_eq!(sent, RENDEZVOUS_PER_RELAY, "one relay is not flooded");
        let mut st = HolepunchState::new();
        let mut sent = 0;
        for i in 0..50u16 {
            let relay = view(&format!("198.51.100.{}:6881", i + 1), &format!("198.51.100.{}:6881", i + 1), Some(3));
            let t = SocketAddr::new("203.0.113.9".parse().unwrap(), 2000 + i);
            if st.plan_rendezvous(t, &[relay], now).is_some() {
                sent += 1;
            }
        }
        assert_eq!(sent, RENDEZVOUS_PER_TORRENT);
    }

    #[test]
    fn solicited_connect_punches_and_clears_pending() {
        let mut st = HolepunchState::new();
        let now = Instant::now();
        let target = a("203.0.113.9:51413");
        let r = view("198.51.100.1:6881", "198.51.100.1:6881", Some(3));
        st.plan_rendezvous(target, &[r], now).unwrap();
        assert!(st.is_pending(target));
        assert_eq!(
            st.on_connect(r.conn_addr, target, now),
            ConnectVerdict::Punch { solicited: true }
        );
        assert!(!st.is_pending(target));
    }

    #[test]
    fn connect_with_listen_port_matches_rendezvous_for_ephemeral_port() {
        let mut st = HolepunchState::new();
        let now = Instant::now();
        let r = view("198.51.100.1:6881", "198.51.100.1:6881", Some(3));
        st.plan_rendezvous(a("203.0.113.9:40001"), &[r], now).unwrap();
        assert_eq!(
            st.on_connect(r.conn_addr, a("203.0.113.9:51413"), now),
            ConnectVerdict::Punch { solicited: true }
        );
        assert!(!st.is_pending(a("203.0.113.9:40001")));
    }

    #[test]
    fn connect_from_other_relay_is_unsolicited() {
        let mut st = HolepunchState::new();
        let now = Instant::now();
        let target = a("203.0.113.9:51413");
        let r = view("198.51.100.1:6881", "198.51.100.1:6881", Some(3));
        st.plan_rendezvous(target, &[r], now).unwrap();
        assert_eq!(
            st.on_connect(a("198.51.100.99:1"), target, now),
            ConnectVerdict::Punch { solicited: false }
        );
        assert!(st.is_pending(target), "the real relay may still answer");
    }

    // ------------------------------------------------ target

    #[test]
    fn unsolicited_connects_are_bounded() {
        let mut st = HolepunchState::new();
        let now = Instant::now();
        let relay = a("198.51.100.1:6881");
        let mut punched = 0;
        for i in 0..20u16 {
            let ep = SocketAddr::new("203.0.113.9".parse().unwrap(), 3000 + i);
            if matches!(st.on_connect(relay, ep, now), ConnectVerdict::Punch { .. }) {
                punched += 1;
            }
        }
        assert_eq!(punched, UNSOLICITED_PER_RELAY, "one hostile relay");
        let mut st = HolepunchState::new();
        let mut punched = 0;
        for i in 0..20u16 {
            let relay = SocketAddr::new("198.51.100.1".parse().unwrap(), 4000 + i);
            let ep = SocketAddr::new("203.0.113.9".parse().unwrap(), 3000 + i);
            if matches!(st.on_connect(relay, ep, now), ConnectVerdict::Punch { .. }) {
                punched += 1;
            }
        }
        assert_eq!(punched, UNSOLICITED_PER_TORRENT, "many relays");
        assert!(matches!(
            st.on_connect(relay, SocketAddr::new("203.0.113.9".parse().unwrap(), 9), now + RATE_WINDOW),
            ConnectVerdict::Punch { solicited: false }
        ));
    }

    #[test]
    fn connect_to_unusable_endpoint_ignored() {
        let mut st = HolepunchState::new();
        let now = Instant::now();
        for ep in ["0.0.0.0:6881", "1.2.3.4:0", "255.255.255.255:6881", "[ff02::1]:6881"] {
            assert!(matches!(
                st.on_connect(a("198.51.100.1:6881"), a(ep), now),
                ConnectVerdict::Ignore(_)
            ));
        }
    }

    #[test]
    fn punch_records_expire_and_are_consumed_once() {
        let mut st = HolepunchState::new();
        let now = Instant::now();
        let ep = a("203.0.113.9:51413");
        st.note_punch(ep, now);
        assert!(st.is_punched(ep, now));
        assert!(st.take_punched(ep, now));
        assert!(!st.take_punched(ep, now));
        st.note_punch(ep, now);
        assert!(!st.is_punched(ep, now + PUNCH_TTL));
        assert!(!st.take_punched(ep, now + PUNCH_TTL));
    }

    #[test]
    fn initiator_prefers_utp_relay_when_no_origin_is_known() {
        let mut st = HolepunchState::new();
        let tcp = view("192.0.2.1:6881", "192.0.2.1:6881", Some(1));
        let utp = RelayPeerView { utp: true, ..view("192.0.2.9:6881", "192.0.2.9:6881", Some(1)) };
        let got = st.plan_rendezvous(a("203.0.113.9:51413"), &[tcp, utp], Instant::now());
        assert_eq!(got, Some(utp.conn_addr));
    }

    #[test]
    fn forget_conn_drops_pex_origin() {
        let mut st = HolepunchState::new();
        let t = a("203.0.113.9:51413");
        let r = a("198.51.100.1:6881");
        st.note_pex_origin(t, r);
        assert_eq!(st.pex_origin(t), Some(r));
        st.forget_conn(r);
        assert_eq!(st.pex_origin(t), None);
    }
}
