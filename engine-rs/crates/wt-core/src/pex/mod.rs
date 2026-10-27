//! # Peer exchange / BEP 11 (agy-2 integration guide)
//!
//! Per-torrent PEX event feed + per-connection send/receive gating, ported
//! from `pex.go` / `pexconn.go`.
//!
//! ## What agy-2 should call
//!
//! ```ignore
//! use wt_core::pex::{PexState, PexConnView, PEX_INTERVAL, PEX_RETRY_DELAY};
//! use wt_types::{PeerInfo, PeerSource, PexFlags};
//! use wt_wire::ext::PexMsg;
//!
//! let mut torrent_pex = PexState::new();
//! // On conn established / closed:
//! torrent_pex.add(addr, flags);
//! torrent_pex.drop_listed(addr, flags); // only if previously added
//!
//! // Writer wake: build incremental message from this conn's cursor
//! if let Some((msg, new_cursor)) = torrent_pex.genmsg(conn.pex_cursor.as_ref()) {
//!     if !msg.is_empty() {
//!         send PexMsg encoded as Extended(ut_pex id);
//!         conn.pex_cursor = Some(new_cursor);
//!         schedule PEX_INTERVAL;
//!     } else {
//!         schedule PEX_RETRY_DELAY;
//!     }
//! }
//!
//! // Reader: ingest with rate limits; returns PeerInfo with PeerSource::Pex
//! match torrent_pex.ingest(&msg, Instant::now(), &mut conn.last_pex_recv) {
//!     Ok(peers) => add_peers(peers),
//!     Err(PexIngestError::TooFrequent { .. }) => disconnect,
//!     Err(PexIngestError::Cooldown) => { /* discard */ }
//! }
//! ```

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::time::{Duration, Instant};

use wt_types::{PeerInfo, PeerSource, PexFlags};
use wt_wire::ext::PexMsg;

/// Writer interval after a successful PEX send (Go `pexInterval`).
pub const PEX_INTERVAL: Duration = Duration::from_secs(60);
/// Retry when there was nothing to send (Go `pexRetryDelay`).
pub const PEX_RETRY_DELAY: Duration = Duration::from_secs(10);
/// Minimum gap between inbound PEX on one connection (Go uses 45s).
pub const PEX_RECV_MIN_INTERVAL: Duration = Duration::from_secs(45);

const PEX_TARG_ADDED: i32 = 25;
const PEX_MAX_HOLD: usize = 25;
/// Upper bound on `max(added, dropped)` map sizes in one incremental message.
pub const PEX_MAX_DELTA: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EventType {
    Add,
    Drop,
}

#[derive(Clone, Debug)]
struct PexEvent {
    kind: EventType,
    addr: SocketAddr,
    flags: PexFlags,
}

/// Cursor into the event feed for incremental `genmsg`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PexCursor {
    /// Index of the last event already consumed (`usize::MAX` = none / start).
    index: usize,
}

impl PexCursor {
    pub fn start() -> Self {
        Self { index: usize::MAX }
    }
}

/// Per-torrent PEX state.
#[derive(Debug, Default)]
pub struct PexState {
    events: Vec<PexEvent>,
    hold: Vec<PexEvent>,
    /// Torrent-wide inbound cooldown deadline.
    rest_until: Option<Instant>,
    /// Net number of alive listed connections.
    nc: i32,
}

impl PexState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn alive_conns(&self) -> i32 {
        self.nc
    }

    /// Record a new live connection.
    pub fn add(&mut self, addr: SocketAddr, flags: PexFlags) {
        if !usable_addr(addr) {
            return;
        }
        self.nc += 1;
        if self.nc >= PEX_TARG_ADDED {
            for e in self.hold.drain(..) {
                self.events.push(e);
            }
        }
        self.events.push(PexEvent {
            kind: EventType::Add,
            addr,
            flags,
        });
    }

    /// Record a disconnect for a connection that was previously `add`ed.
    pub fn drop_listed(&mut self, addr: SocketAddr, flags: PexFlags) {
        if !usable_addr(addr) {
            return;
        }
        self.nc -= 1;
        let e = PexEvent {
            kind: EventType::Drop,
            addr,
            flags,
        };
        if self.nc < PEX_TARG_ADDED && self.hold.len() < PEX_MAX_HOLD {
            self.hold.push(e);
        } else {
            self.events.push(e);
        }
    }

    /// Full snapshot message (no delta cap) — Go `Genmsg(nil)`.
    pub fn initial_msg(&self) -> PexMsg {
        let mut factory = MsgFactory::default();
        for e in &self.events {
            factory.append(e);
        }
        factory.into_msg()
    }

    /// Incremental message from `cursor`. Returns `None` only when disabled path
    /// wants a scheduled retry with empty tip; empty msg still returns Some.
    ///
    /// Returns `(msg, new_cursor)`. Initial call: pass `None` for full snapshot
    /// (uncapped). Subsequent: pass previous cursor for capped delta.
    pub fn genmsg(&self, cursor: Option<PexCursor>) -> (PexMsg, PexCursor) {
        match cursor {
            None => {
                let msg = self.initial_msg();
                let cur = if self.events.is_empty() {
                    PexCursor::start()
                } else {
                    PexCursor {
                        index: self.events.len() - 1,
                    }
                };
                (msg, cur)
            }
            Some(start) => {
                let mut factory = MsgFactory::default();
                let begin = if start.index == usize::MAX {
                    0
                } else {
                    start.index.saturating_add(1)
                };
                let mut last = start.index;
                for (i, e) in self.events.iter().enumerate().skip(begin) {
                    if factory.delta_len() >= PEX_MAX_DELTA {
                        break;
                    }
                    factory.append(e);
                    last = i;
                }
                (factory.into_msg(), PexCursor { index: last })
            }
        }
    }

    pub fn num_pending(&self, cursor: Option<PexCursor>) -> usize {
        match cursor {
            None => self.initial_msg().len(),
            Some(start) => {
                let begin = if start.index == usize::MAX {
                    0
                } else {
                    start.index.saturating_add(1)
                };
                self.events.len().saturating_sub(begin)
            }
        }
    }

    /// Ingest a received PEX message.
    ///
    /// Updates `last_recv` on success. Enforces per-conn 45s spacing and
    /// torrent-wide cooldown. Returns peers to add (`PeerSource::Pex`).
    pub fn ingest(
        &mut self,
        msg: &PexMsg,
        now: Instant,
        last_recv: &mut Option<Instant>,
    ) -> Result<Vec<PeerInfo>, PexIngestError> {
        if let Some(prev) = *last_recv {
            let since = now.saturating_duration_since(prev);
            if since < PEX_RECV_MIN_INTERVAL {
                return Err(PexIngestError::TooFrequent { since });
            }
        }
        *last_recv = Some(now);

        if self.rest_until.is_some_and(|d| now < d) {
            return Err(PexIngestError::Cooldown);
        }

        let mut peers = Vec::with_capacity(msg.added.len() + msg.added6.len());
        for (i, addr) in msg.added.iter().enumerate() {
            let flags = msg.added_flags.get(i).copied().unwrap_or_default();
            peers.push(peer_from_v4(*addr, flags));
        }
        for (i, addr) in msg.added6.iter().enumerate() {
            let flags = msg.added6_flags.get(i).copied().unwrap_or_default();
            peers.push(peer_from_v6(*addr, flags));
        }

        if !peers.is_empty() {
            self.rest_until = Some(now + PEX_INTERVAL);
        }
        Ok(peers)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PexIngestError {
    TooFrequent { since: Duration },
    Cooldown,
}

fn peer_from_v4(addr: SocketAddrV4, flags: PexFlags) -> PeerInfo {
    PeerInfo {
        id: None,
        addr: SocketAddr::V4(addr),
        source: PeerSource::Pex,
        supports_encryption: flags.prefers_encryption,
        pex: flags,
        trusted: false,
    }
}

fn peer_from_v6(addr: SocketAddrV6, flags: PexFlags) -> PeerInfo {
    PeerInfo {
        id: None,
        addr: SocketAddr::V6(addr),
        source: PeerSource::Pex,
        supports_encryption: flags.prefers_encryption,
        pex: flags,
        trusted: false,
    }
}

fn usable_addr(addr: SocketAddr) -> bool {
    let ip_ok = match addr.ip() {
        IpAddr::V4(v4) => !v4.is_unspecified(),
        IpAddr::V6(v6) => !v6.is_unspecified(),
    };
    ip_ok && addr.port() != 0
}

#[derive(Default)]
struct MsgFactory {
    msg: PexMsg,
    added: HashSet<SocketAddr>,
    dropped: HashSet<SocketAddr>,
}

impl MsgFactory {
    fn delta_len(&self) -> usize {
        self.added.len().max(self.dropped.len())
    }

    fn append(&mut self, e: &PexEvent) {
        match e.kind {
            EventType::Add => self.add(e),
            EventType::Drop => self.drop(e),
        }
    }

    fn add(&mut self, e: &PexEvent) {
        let key = e.addr;
        if self.added.contains(&key) {
            return;
        }
        if self.dropped.remove(&key) {
            remove_dropped(&mut self.msg, key);
            return;
        }
        match key {
            SocketAddr::V4(a) => {
                self.msg.added.push(a);
                self.msg.added_flags.push(e.flags);
            }
            SocketAddr::V6(a) => {
                self.msg.added6.push(a);
                self.msg.added6_flags.push(e.flags);
            }
        }
        self.added.insert(key);
    }

    fn drop(&mut self, e: &PexEvent) {
        let key = e.addr;
        if self.dropped.contains(&key) {
            return;
        }
        if self.added.remove(&key) {
            remove_added(&mut self.msg, key);
            return;
        }
        match key {
            SocketAddr::V4(a) => self.msg.dropped.push(a),
            SocketAddr::V6(a) => self.msg.dropped6.push(a),
        }
        self.dropped.insert(key);
    }

    fn into_msg(self) -> PexMsg {
        self.msg
    }
}

fn remove_added(msg: &mut PexMsg, key: SocketAddr) {
    match key {
        SocketAddr::V4(a) => {
            if let Some(i) = msg.added.iter().position(|x| *x == a) {
                msg.added.remove(i);
                if i < msg.added_flags.len() {
                    msg.added_flags.remove(i);
                }
            }
        }
        SocketAddr::V6(a) => {
            if let Some(i) = msg.added6.iter().position(|x| *x == a) {
                msg.added6.remove(i);
                if i < msg.added6_flags.len() {
                    msg.added6_flags.remove(i);
                }
            }
        }
    }
}

fn remove_dropped(msg: &mut PexMsg, key: SocketAddr) {
    match key {
        SocketAddr::V4(a) => {
            if let Some(i) = msg.dropped.iter().position(|x| *x == a) {
                msg.dropped.remove(i);
            }
        }
        SocketAddr::V6(a) => {
            if let Some(i) = msg.dropped6.iter().position(|x| *x == a) {
                msg.dropped6.remove(i);
            }
        }
    }
}

/// Per-connection bookkeeping helpers (pure; timer/IO stay in peer_conn).
#[derive(Debug, Default)]
pub struct PexConnView {
    pub enabled: bool,
    pub ext_id: u8,
    pub cursor: Option<PexCursor>,
    pub listed: bool,
    pub last_recv: Option<Instant>,
    pub remote_live: HashMap<SocketAddr, PexFlags>,
}

impl PexConnView {
    pub fn enable(&mut self, ext_id: u8) {
        if ext_id == 0 {
            return;
        }
        self.enabled = true;
        self.ext_id = ext_id;
        self.cursor = None;
    }

    pub fn update_remote_live(&mut self, msg: &PexMsg) {
        for a in &msg.dropped {
            self.remote_live.remove(&SocketAddr::V4(*a));
        }
        for a in &msg.dropped6 {
            self.remote_live.remove(&SocketAddr::V6(*a));
        }
        for (i, a) in msg.added.iter().enumerate() {
            let f = msg.added_flags.get(i).copied().unwrap_or_default();
            self.remote_live.insert(SocketAddr::V4(*a), f);
        }
        for (i, a) in msg.added6.iter().enumerate() {
            let f = msg.added6_flags.get(i).copied().unwrap_or_default();
            self.remote_live.insert(SocketAddr::V6(*a), f);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn v4(port: u16) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), port))
    }
    fn v6(port: u16) -> SocketAddr {
        SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, port, 0, 0))
    }

    #[test]
    fn reset_clears() {
        let mut s = PexState::new();
        s.add(v4(4747), PexFlags::default());
        s.reset();
        assert_eq!(s.alive_conns(), 0);
        assert!(s.initial_msg().is_empty());
    }

    #[test]
    fn add_and_genmsg_mixed() {
        let mut s = PexState::new();
        let f = PexFlags {
            reachable: true,
            ..Default::default()
        };
        s.add(v6(4747), PexFlags::default());
        s.add(v6(4748), f);
        s.add(v4(4747), f);
        s.add(v4(4748), PexFlags::default());
        let (m, _) = s.genmsg(None);
        assert_eq!(m.added6.len(), 2);
        assert_eq!(m.added.len(), 2);
        assert!(m.added_flags[0].reachable);
    }

    #[test]
    fn add_drop_cancel_out() {
        let mut s = PexState::new();
        // Keep nc high so drops aren't held.
        for p in 5000..5030 {
            s.add(v4(p), PexFlags::default());
        }
        s.add(v4(4747), PexFlags::default());
        s.drop_listed(v4(4747), PexFlags::default());
        let (m, _) = s.genmsg(None);
        assert!(!m.added.iter().any(|a| a.port() == 4747));
        assert!(!m.dropped.iter().any(|a| a.port() == 4747));
    }

    #[test]
    fn incremental_respects_max_delta() {
        let mut s = PexState::new();
        let n = 2 * PEX_MAX_DELTA;
        for i in 0..n {
            s.add(v4(1000 + i as u16), PexFlags::default());
        }
        // Initial uncapped
        let (m0, cur) = s.genmsg(None);
        assert_eq!(m0.added.len(), n);
        // Fresh incremental from start would cap — use cursor before any events
        let mut s2 = PexState::new();
        for i in 0..n {
            s2.add(v4(2000 + i as u16), PexFlags::default());
        }
        let (_full, cur0) = s2.genmsg(None);
        // Simulate a late joiner: walk from start cursor
        let (delta, _) = s2.genmsg(Some(PexCursor::start()));
        // From start, factory still caps at 50 adds
        assert!(delta.added.len() <= PEX_MAX_DELTA);
        let _ = (cur, cur0);
    }

    #[test]
    fn ingest_too_frequent_and_cooldown() {
        let mut s = PexState::new();
        let now = Instant::now();
        let mut last = None;
        let mut msg = PexMsg::new();
        msg.added
            .push(SocketAddrV4::new(Ipv4Addr::new(1, 2, 3, 4), 6881));
        let peers = s.ingest(&msg, now, &mut last).unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].source, PeerSource::Pex);

        let err = s
            .ingest(&msg, now + Duration::from_secs(10), &mut last)
            .unwrap_err();
        assert!(matches!(err, PexIngestError::TooFrequent { .. }));

        // After spacing, torrent cooldown still active
        let mut last2 = Some(now);
        let err = s
            .ingest(&msg, now + Duration::from_secs(50), &mut last2)
            .unwrap_err();
        assert_eq!(err, PexIngestError::Cooldown);
    }

    #[test]
    fn unspecified_ignored() {
        let mut s = PexState::new();
        s.add(
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 1)),
            PexFlags::default(),
        );
        assert!(s.initial_msg().is_empty());
    }
}
