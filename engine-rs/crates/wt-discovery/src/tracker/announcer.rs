//! Per-torrent tracker announce dispatcher.
//!
//! Ports the scheduling behaviour of `client-tracker-announcer.go`:
//! tier/url keyed announces, Started/Completed/Stopped/regular events,
//! max 2 concurrent announces per tracker URL, interval backoff after
//! success, and [`TrackerStatusEvent`] emissions.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use wt_types::InfoHash;

use super::{
    AnnounceEvent, AnnounceOpt, AnnounceRequest, AnnounceResponse, Client, ClientOpts, Peer,
    TrackerError,
};
use crate::status::{StatusCallback, TrackerStatusEvent};

/// Go `maxConcurrentAnnouncesPerTracker`.
pub const MAX_CONCURRENT_ANNOUNCES_PER_TRACKER: usize = 2;

/// Default re-announce when the tracker returns interval 0 / missing.
pub const DEFAULT_ANNOUNCE_INTERVAL: Duration = Duration::from_secs(30 * 60);
/// Backoff after a failed announce (before retry).
pub const ANNOUNCE_ERROR_BACKOFF: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TorrentAnnounceKey {
    pub url: String,
    pub info_hash: InfoHash,
}

/// Snapshot the core supplies so the dispatcher can build announce requests
/// without holding torrent locks across the network call.
#[derive(Clone, Debug)]
pub struct TorrentAnnounceSnapshot {
    pub info_hash: InfoHash,
    pub peer_id: [u8; 20],
    pub downloaded: i64,
    pub left: i64,
    pub uploaded: i64,
    pub port: u16,
    pub key: i32,
    pub need_data: bool,
    pub want_peers: bool,
    pub completed: bool,
    /// True once the torrent has finished downloading (for Completed event).
    pub newly_completed: bool,
}

pub type AnnounceRequestFactory =
    Arc<dyn Fn(&TorrentAnnounceKey, AnnounceEvent) -> Option<AnnounceRequest> + Send + Sync>;

#[derive(Clone, Default)]
pub struct AnnounceDispatcherConfig {
    pub announce_opt: AnnounceOpt,
    pub client_opts: ClientOpts,
    pub status: Option<StatusCallback>,
    /// Channel that receives peers from successful announces.
    pub peers_tx: Option<mpsc::UnboundedSender<(InfoHash, Vec<Peer>)>>,
}

#[derive(Clone, Copy, Debug)]
struct SlotState {
    active: bool,
    next_at: Instant,
    event: AnnounceEvent,
    last_failed: bool,
    sent_completed: bool,
    interval: Duration,
}

impl Default for SlotState {
    fn default() -> Self {
        Self {
            active: false,
            next_at: Instant::now(),
            event: AnnounceEvent::Started,
            last_failed: false,
            sent_completed: false,
            interval: DEFAULT_ANNOUNCE_INTERVAL,
        }
    }
}

struct Inner {
    cfg: AnnounceDispatcherConfig,
    slots: HashMap<TorrentAnnounceKey, SlotState>,
    /// Concurrent announces in flight per tracker URL.
    url_inflight: HashMap<String, usize>,
    clients: HashMap<String, Arc<Client>>,
    snapshots: HashMap<InfoHash, TorrentAnnounceSnapshot>,
    closed: bool,
}

/// Schedules announces across tracker URLs for many torrents.
pub struct AnnounceDispatcher {
    inner: Arc<Mutex<Inner>>,
    wake: tokio::sync::Notify,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl AnnounceDispatcher {
    pub fn new(cfg: AnnounceDispatcherConfig) -> Arc<Self> {
        let me = Arc::new(Self {
            inner: Arc::new(Mutex::new(Inner {
                cfg,
                slots: HashMap::new(),
                url_inflight: HashMap::new(),
                clients: HashMap::new(),
                snapshots: HashMap::new(),
                closed: false,
            })),
            wake: tokio::sync::Notify::new(),
            task: Mutex::new(None),
        });
        let run = Arc::clone(&me);
        let handle = tokio::spawn(async move { run.run_loop().await });
        *me.task.lock() = Some(handle);
        me
    }

    /// Register / refresh torrent stats used to build announce requests.
    pub fn update_torrent(&self, snap: TorrentAnnounceSnapshot) {
        let mut g = self.inner.lock();
        let ih = snap.info_hash;
        let newly = snap.newly_completed;
        g.snapshots.insert(ih, snap);
        if newly {
            for (key, slot) in g.slots.iter_mut() {
                if key.info_hash == ih && !slot.sent_completed {
                    slot.event = AnnounceEvent::Completed;
                    slot.next_at = Instant::now();
                }
            }
        }
        drop(g);
        self.wake.notify_one();
    }

    /// Add tracker URLs (flat; tiers are ordered by the caller if needed).
    pub fn add_trackers(&self, info_hash: InfoHash, urls: impl IntoIterator<Item = String>) {
        let mut g = self.inner.lock();
        for url in urls {
            let key = TorrentAnnounceKey {
                url: url.clone(),
                info_hash,
            };
            g.slots.entry(key).or_insert_with(|| SlotState {
                event: AnnounceEvent::Started,
                next_at: Instant::now(),
                ..SlotState::default()
            });
        }
        drop(g);
        self.wake.notify_one();
    }

    /// Schedule Stopped announces then drop slots for the torrent.
    pub fn remove_torrent(&self, info_hash: InfoHash) {
        let mut g = self.inner.lock();
        for (key, slot) in g.slots.iter_mut() {
            if key.info_hash == info_hash {
                slot.event = AnnounceEvent::Stopped;
                slot.next_at = Instant::now();
            }
        }
        g.snapshots.remove(&info_hash);
        drop(g);
        self.wake.notify_one();
    }

    pub fn close(&self) {
        let mut g = self.inner.lock();
        g.closed = true;
        drop(g);
        self.wake.notify_one();
        if let Some(h) = self.task.lock().take() {
            h.abort();
        }
    }

    async fn run_loop(self: Arc<Self>) {
        loop {
            if self.inner.lock().closed {
                break;
            }
            self.dispatch_once().await;
            let sleep_for = {
                let g = self.inner.lock();
                g.slots
                    .values()
                    .filter(|s| !s.active)
                    .map(|s| s.next_at.saturating_duration_since(Instant::now()))
                    .min()
                    .unwrap_or(Duration::from_secs(5))
            };
            tokio::select! {
                _ = self.wake.notified() => {}
                _ = tokio::time::sleep(sleep_for) => {}
            }
        }
    }

    async fn dispatch_once(self: &Arc<Self>) {
        let due: Vec<(
            TorrentAnnounceKey,
            AnnounceEvent,
            AnnounceRequest,
            AnnounceOpt,
        )> = {
            let mut g = self.inner.lock();
            let now = Instant::now();
            let mut out = Vec::new();
            // Collect keys first to satisfy borrow checker.
            let mut candidates: Vec<TorrentAnnounceKey> = g
                .slots
                .iter()
                .filter(|(_, s)| !s.active && s.next_at <= now)
                .map(|(k, _)| k.clone())
                .collect();
            // Prefer Started/Completed over regular (match Go eventOrdering).
            candidates.sort_by_key(|k| {
                let e = g
                    .slots
                    .get(k)
                    .map(|s| s.event)
                    .unwrap_or(AnnounceEvent::None);
                event_order(e)
            });

            for key in candidates {
                let inflight = g.url_inflight.get(&key.url).copied().unwrap_or(0);
                if inflight >= MAX_CONCURRENT_ANNOUNCES_PER_TRACKER {
                    continue;
                }
                let active = g.slots.get(&key).map(|s| s.active).unwrap_or(true);
                if active {
                    continue;
                }
                let event = g
                    .slots
                    .get(&key)
                    .map(|s| s.event)
                    .unwrap_or(AnnounceEvent::None);
                let Some(snap) = g.snapshots.get(&key.info_hash).cloned() else {
                    continue;
                };
                // Skip regular announces when we neither need data nor want peers,
                // except Stopped/Completed/Started.
                if event == AnnounceEvent::None && !snap.need_data && !snap.want_peers {
                    if let Some(slot) = g.slots.get_mut(&key) {
                        slot.next_at = now + Duration::from_secs(60);
                    }
                    continue;
                }
                let req = AnnounceRequest {
                    info_hash: snap.info_hash,
                    peer_id: snap.peer_id,
                    downloaded: snap.downloaded,
                    left: snap.left,
                    uploaded: snap.uploaded,
                    event,
                    ip_address: 0,
                    key: snap.key,
                    num_want: if snap.want_peers { -1 } else { 0 },
                    port: snap.port,
                };
                if let Some(slot) = g.slots.get_mut(&key) {
                    slot.active = true;
                }
                *g.url_inflight.entry(key.url.clone()).or_insert(0) += 1;
                let opt = g.cfg.announce_opt.clone();
                out.push((key, event, req, opt));
            }
            out
        };

        for (key, event, req, opt) in due {
            let this = Arc::clone(self);
            tokio::spawn(async move {
                this.run_announce(key, event, req, opt).await;
            });
        }
    }

    async fn run_announce(
        self: Arc<Self>,
        key: TorrentAnnounceKey,
        event: AnnounceEvent,
        req: AnnounceRequest,
        opt: AnnounceOpt,
    ) {
        let existing = {
            let g = self.inner.lock();
            g.clients.get(&key.url).cloned()
        };
        let client = if let Some(c) = existing {
            c
        } else {
            let opts = self.inner.lock().cfg.client_opts.clone();
            match Client::new(&key.url, opts).await {
                Ok(c) => {
                    let c = Arc::new(c);
                    self.inner
                        .lock()
                        .clients
                        .insert(key.url.clone(), Arc::clone(&c));
                    c
                }
                Err(e) => {
                    self.finish_announce(&key, event, Err(e));
                    return;
                }
            }
        };

        let result = client.announce(req, opt).await;
        self.finish_announce(&key, event, result);
    }

    fn finish_announce(
        &self,
        key: &TorrentAnnounceKey,
        event: AnnounceEvent,
        result: std::result::Result<AnnounceResponse, TrackerError>,
    ) {
        let mut g = self.inner.lock();
        if let Some(n) = g.url_inflight.get_mut(&key.url) {
            *n = n.saturating_sub(1);
        }
        let status_cb = g.cfg.status.clone();
        let peers_tx = g.cfg.peers_tx.clone();
        let now = Instant::now();

        match &result {
            Ok(resp) => {
                if let Some(slot) = g.slots.get_mut(key) {
                    slot.active = false;
                    slot.last_failed = false;
                    let interval = if resp.interval > 0 {
                        Duration::from_secs(resp.interval as u64)
                    } else {
                        DEFAULT_ANNOUNCE_INTERVAL
                    };
                    let min_i = resp
                        .min_interval
                        .filter(|&m| m > 0)
                        .map(|m| Duration::from_secs(m as u64));
                    slot.interval = min_i.map(|m| m.max(interval)).unwrap_or(interval);
                    slot.next_at = now + slot.interval;
                    if event == AnnounceEvent::Completed {
                        slot.sent_completed = true;
                    }
                    if event == AnnounceEvent::Stopped {
                        // Drop after stopped.
                    } else if event == AnnounceEvent::Started || event == AnnounceEvent::Completed {
                        slot.event = AnnounceEvent::None;
                    }
                    let peers = resp.peers.clone();
                    drop(g);
                    if let Some(cb) = &status_cb {
                        if event == AnnounceEvent::Started {
                            cb(&TrackerStatusEvent::Connected {
                                url: key.url.clone(),
                                info_hash: key.info_hash,
                            });
                        }
                        cb(&TrackerStatusEvent::AnnounceSuccessful {
                            url: key.url.clone(),
                            info_hash: key.info_hash,
                        });
                    }
                    if let Some(tx) = peers_tx {
                        let _ = tx.send((key.info_hash, peers));
                    }
                    if event == AnnounceEvent::Stopped {
                        self.inner.lock().slots.remove(key);
                    }
                    return;
                }
            }
            Err(err) => {
                let msg = err.to_string();
                if let Some(slot) = g.slots.get_mut(key) {
                    slot.active = false;
                    slot.last_failed = true;
                    slot.next_at = now + ANNOUNCE_ERROR_BACKOFF;
                    // Keep Started so we retry as Started until first success.
                    if event != AnnounceEvent::Started && event != AnnounceEvent::Completed {
                        slot.event = AnnounceEvent::None;
                    }
                }
                drop(g);
                if let Some(cb) = &status_cb {
                    cb(&TrackerStatusEvent::AnnounceError {
                        url: key.url.clone(),
                        info_hash: key.info_hash,
                        error: msg.clone(),
                    });
                    cb(&TrackerStatusEvent::Disconnected {
                        url: key.url.clone(),
                        info_hash: key.info_hash,
                        error: Some(msg),
                    });
                }
                return;
            }
        }
        // Slot vanished.
        if let Some(slot) = self.inner.lock().slots.get_mut(key) {
            slot.active = false;
        }
    }
}

fn event_order(e: AnnounceEvent) -> i32 {
    // Lower sorts first. Matches Go eventOrdering preference.
    match e {
        AnnounceEvent::Started => -4,
        AnnounceEvent::Completed => -3,
        AnnounceEvent::Stopped => -2,
        AnnounceEvent::None => 0,
    }
}
