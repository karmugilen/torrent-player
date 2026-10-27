//! Client runtime, peer networking listener & dialer, and swarm coordinator.
//!
//! Ports the Client side of `engine-go/patches/torrent/client.go`: one Tokio
//! runtime, hybrid short-hash registry, PeerListener (TCP+uTP), half-open /
//! dial limiters, Sources (DHT/trackers/WebTorrent), and legacy PeerConn attach
//! after `conn::dial_peer` / accept (until `PeerTorrent` is wired by agy-2).

use bytes::{Bytes, BytesMut};
use parking_lot::RwLock;
use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Notify};
use wt_types::{InfoHash, PeerId, PeerInfo, PeerSource, PieceIndex};

use crate::arrival::ArrivalModel;
use crate::conn::{
    dial_peer as conn_dial_peer, AcceptedConn, DialParams, HalfOpenTracker, PeerListener,
    SharedDatagramAdapter,
};
use crate::peer_conn::PeerConn;
use crate::ratelimit::{Limiter, DEFAULT_DOWNLOAD_BURST};
use crate::sources::{
    RegisterTorrent, Sources, SourcesConfig, TorrentAnnounceInfo, TorrentKeys, PeerSink,
};
use crate::torrent::TorrentInner;
use crate::wire::{
    decode_message, ExtendedHandshake, Message, MetadataMsg, MetadataMsgType,
    DEFAULT_MAX_MESSAGE_LENGTH, DEFAULT_UT_METADATA_ID, DEFAULT_UT_PEX_ID, EXTENSION_NAME_METADATA,
    EXTENSION_NAME_PEX, EXT_HANDSHAKE_ID,
};
use crate::{ClientConfig, CoreError, PeerStream, Result, TcpConnHook, TorrentSpec};

/// Thin Sources sink / announce adapter over [`TorrentInner`] (agy-2 owns the
/// eventual first-class impls; Client registers torrents immediately).
struct TorrentSourcesAdapter {
    torrent: Arc<TorrentInner>,
    peer_id: PeerId,
    listen_port: Arc<RwLock<u16>>,
    peers_low_water: usize,
}

impl PeerSink for TorrentSourcesAdapter {
    fn add_peers(&self, _info_hash: InfoHash, peers: Vec<PeerInfo>) {
        let mut known = self.torrent.known_peers.write();
        for p in peers {
            known.insert(p);
        }
        self.torrent.bump_change();
    }

    fn add_web_seeds(&self, _info_hash: InfoHash, urls: Vec<String>) {
        let mut ws = self.torrent.web_seeds.write();
        for u in urls {
            if !ws.contains(&u) {
                ws.push(u);
            }
        }
        self.torrent.bump_change();
    }
}

impl TorrentAnnounceInfo for TorrentSourcesAdapter {
    fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    fn downloaded(&self) -> i64 {
        let Some(info) = self.torrent.info.read().clone() else {
            return 0;
        };
        let pieces = self.torrent.pieces.read();
        let mut completed = 0i64;
        for p in pieces.iter() {
            if p.complete {
                completed += info.piece(p.index).length;
            }
        }
        completed
    }

    fn uploaded(&self) -> i64 {
        0
    }

    fn left(&self) -> i64 {
        let Some(info) = self.torrent.info.read().clone() else {
            return -1;
        };
        let pieces = self.torrent.pieces.read();
        let mut completed = 0i64;
        for p in pieces.iter() {
            if p.complete {
                completed += info.piece(p.index).length;
            }
        }
        (info.total_length() - completed).max(0)
    }

    fn port(&self) -> u16 {
        *self.listen_port.read()
    }

    fn announce_key(&self) -> i32 {
        // Stable-ish per-torrent key from infohash bytes.
        let b = self.torrent.info_hash.0;
        i32::from_be_bytes([b[0], b[1], b[2], b[3]])
    }

    fn need_data(&self) -> bool {
        !self.torrent.data_download_disallowed.load(Ordering::Relaxed)
            && !self.torrent.seeding.load(Ordering::Relaxed)
    }

    fn want_peers(&self) -> bool {
        self.torrent.known_peers.read().len() <= self.peers_low_water
    }

    fn completed(&self) -> bool {
        self.torrent.seeding.load(Ordering::Relaxed)
            || self
                .torrent
                .info
                .read()
                .as_ref()
                .map(|_info| {
                    let pieces = self.torrent.pieces.read();
                    !pieces.is_empty() && pieces.iter().all(|p| p.complete)
                })
                .unwrap_or(false)
    }

    fn newly_completed(&self) -> bool {
        false
    }
}

pub struct ClientInner {
    pub config: ClientConfig,
    pub peer_id: PeerId,
    pub listen_port: Arc<RwLock<u16>>,
    pub listen_addrs: RwLock<Vec<SocketAddr>>,
    pub torrents: RwLock<HashMap<InfoHash, Arc<TorrentInner>>>,
    /// Hybrid magnet fix: both v1 and v2 short hashes (WEBTOR_PATCH).
    pub torrents_by_short: RwLock<HashMap<InfoHash, Arc<TorrentInner>>>,
    pub outgoing_tcp_conn_hook: RwLock<Option<TcpConnHook>>,
    pub urgent_steal_after: RwLock<Duration>,
    pub arrival_model: Arc<RwLock<ArrivalModel>>,
    pub dial_limiter: Limiter,
    pub download_limiter: Limiter,
    pub upload_limiter: Limiter,
    pub half_open: HalfOpenTracker,
    /// Mirrored for `ClientStats` / older readers; prefer [`HalfOpenTracker::current_total`].
    pub active_half_open: AtomicUsize,
    pub closed: AtomicBool,
    pub tokio_handle: tokio::runtime::Handle,
    pub _runtime: Option<tokio::runtime::Runtime>,
    listener: RwLock<Option<Arc<PeerListener>>>,
    sources: RwLock<Option<Arc<Sources>>>,
    accept_tx_closed: AtomicBool,
}

impl ClientInner {
    pub fn new(cfg: ClientConfig) -> Result<Arc<Self>> {
        let (handle, rt) = match tokio::runtime::Handle::try_current() {
            Ok(h) => (h, None),
            Err(_) => {
                // Android-sized: few async workers; blocking pool for storage/hash.
                let blocking = cfg.piece_hashers_per_torrent.saturating_mul(4).max(8);
                let r = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(4)
                    .max_blocking_threads(blocking)
                    .thread_name("wt-client")
                    .enable_all()
                    .build()?;
                let h = r.handle().clone();
                (h, Some(r))
            }
        };

        let mut peer_id = [0u8; 20];
        peer_id[..8].copy_from_slice(b"-WT0100-");
        for b in &mut peer_id[8..] {
            *b = rand::random::<u8>();
        }

        let dial_rate = if cfg.dial_rate > 0.0 {
            cfg.dial_rate
        } else {
            100.0
        };
        let dial_burst = dial_rate.max(1.0) as usize;
        let dial_limiter = Limiter::new(dial_rate, dial_burst);

        let download_limiter = match cfg.download_rate {
            Some(r) if r > 0.0 && r.is_finite() => Limiter::new(r, DEFAULT_DOWNLOAD_BURST),
            _ => {
                let l = Limiter::infinite();
                l.ensure_default_burst(DEFAULT_DOWNLOAD_BURST);
                l
            }
        };
        let upload_limiter = match cfg.upload_rate {
            Some(r) if r > 0.0 && r.is_finite() => Limiter::new(r, DEFAULT_DOWNLOAD_BURST),
            _ => {
                let l = Limiter::infinite();
                l.ensure_default_burst(DEFAULT_DOWNLOAD_BURST);
                l
            }
        };

        let listen_port = Arc::new(RwLock::new(cfg.listen_port));
        let half_open = HalfOpenTracker::new();
        let owns_runtime = rt.is_some();

        let client = Arc::new(Self {
            peer_id,
            listen_port: listen_port.clone(),
            listen_addrs: RwLock::new(Vec::new()),
            torrents: RwLock::new(HashMap::new()),
            torrents_by_short: RwLock::new(HashMap::new()),
            outgoing_tcp_conn_hook: RwLock::new(None),
            urgent_steal_after: RwLock::new(Duration::ZERO),
            arrival_model: Arc::new(RwLock::new(ArrivalModel::new())),
            dial_limiter,
            download_limiter,
            upload_limiter,
            half_open,
            active_half_open: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            tokio_handle: handle.clone(),
            _runtime: rt,
            listener: RwLock::new(None),
            sources: RwLock::new(None),
            accept_tx_closed: AtomicBool::new(false),
            config: cfg,
        });

        // Bind listeners + Sources on the client runtime.
        let cl = client.clone();
        let boot = async move { cl.bootstrap_networking().await };
        let boot_res = if owns_runtime {
            // We own the runtime and are not inside it yet.
            handle.block_on(boot)
        } else {
            // Already on a runtime (e.g. #[tokio::test]): allow nested block_on.
            tokio::task::block_in_place(|| handle.block_on(boot))
        };
        boot_res.map_err(|e| CoreError::Other(format!("client bootstrap: {e}")))?;

        Ok(client)
    }

    async fn bootstrap_networking(self: &Arc<Self>) -> Result<()> {
        let mut utp_for_sources: Option<Arc<wt_wire::utp::UtpSocket>> = None;

        if self.config.accept_peer_connections
            && (!self.config.disable_tcp || !self.config.disable_utp)
        {
            let listener = PeerListener::bind(&self.config).await?;
            let port = listener.listen_port();
            *self.listen_port.write() = port;
            {
                let mut addrs = self.listen_addrs.write();
                addrs.clear();
                addrs.push(SocketAddr::from(([0, 0, 0, 0], port)));
                addrs.push(SocketAddr::from(([127, 0, 0, 1], port)));
            }
            if let Some(utp) = listener.utp_socket() {
                utp_for_sources = Some(Arc::clone(utp));
            }

            let (accepted_tx, mut accepted_rx) = mpsc::channel::<AcceptedConn>(64);
            let weak_lookup_client = Arc::clone(self);
            let lookup: crate::conn::InfoHashLookup = Arc::new(move |ih: &InfoHash| {
                weak_lookup_client.torrents_by_short.read().contains_key(ih)
            });
            let skey_client = Arc::clone(self);
            let skey_provider: crate::conn::SkeyProvider = Arc::new(move || {
                skey_client
                    .torrents_by_short
                    .read()
                    .keys()
                    .map(|ih| ih.0)
                    .collect()
            });

            listener.spawn_accept_loop(
                self.config.clone(),
                self.peer_id,
                self.half_open.clone(),
                skey_provider,
                lookup,
                accepted_tx,
            );

            let accept_client = Arc::clone(self);
            self.tokio_handle.spawn(async move {
                while let Some(accepted) = accepted_rx.recv().await {
                    if accept_client.closed.load(Ordering::Relaxed) {
                        break;
                    }
                    let cl = accept_client.clone();
                    accept_client.tokio_handle.spawn(async move {
                        let _ = cl.attach_accepted(accepted).await;
                    });
                }
                accept_client
                    .accept_tx_closed
                    .store(true, Ordering::Relaxed);
            });

            *self.listener.write() = Some(Arc::new(listener));
        }

        // Discovery (trackers always; DHT/WebTorrent per config).
        let mut src_cfg = SourcesConfig::from_client_config(&self.config);
        src_cfg.peer_id = self.peer_id;
        src_cfg.listen_port = *self.listen_port.read();
        src_cfg.status_callbacks = self.config.callbacks.status_updated.clone();
        if let Some(utp) = utp_for_sources {
            src_cfg.datagram = Some(Arc::new(SharedDatagramAdapter::new(utp)));
        }
        match Sources::start(src_cfg).await {
            Ok(sources) => {
                *self.sources.write() = Some(sources);
            }
            Err(e) => {
                // Non-fatal when DHT bind fails in constrained test envs.
                tracing::warn!("Sources::start failed: {e}");
            }
        }

        Ok(())
    }

    fn torrent_keys_for(t: &TorrentInner) -> TorrentKeys {
        TorrentKeys {
            v1: Some(t.info_hash),
            v2_short: t.info_hash_v2.map(|v| v.to_short()),
        }
    }

    fn register_sources(&self, torrent: &Arc<TorrentInner>, trackers: Vec<Vec<String>>, web_seeds: Vec<String>) {
        let Some(sources) = self.sources.read().clone() else {
            return;
        };
        let adapter = Arc::new(TorrentSourcesAdapter {
            torrent: Arc::clone(torrent),
            peer_id: self.peer_id,
            listen_port: Arc::clone(&self.listen_port),
            peers_low_water: self.config.torrent_peers_low_water,
        });
        let reg = RegisterTorrent {
            keys: Self::torrent_keys_for(torrent),
            trackers,
            web_seeds,
            info: adapter.clone() as Arc<dyn TorrentAnnounceInfo>,
            sink: adapter as Arc<dyn PeerSink>,
        };
        if let Err(e) = sources.register_torrent(reg) {
            tracing::debug!("register_torrent: {e}");
        }
    }

    /// Add or merge a torrent (Go `Client.AddTorrentSpec` + `Torrent.MergeSpec`).
    pub fn add_torrent_spec(self: &Arc<Self>, mut spec: TorrentSpec) -> Result<(Arc<TorrentInner>, bool)> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(CoreError::Closed);
        }

        let mut info_obj: Option<wt_types::Info> = None;
        if let Some(ref ib) = spec.info_bytes {
            if let Ok(parsed) = wt_meta::parse_info(ib) {
                if spec.info_hash.is_none() {
                    use sha1::{Digest, Sha1};
                    let mut hasher = Sha1::new();
                    hasher.update(ib);
                    let digest: [u8; 20] = hasher.finalize().into();
                    spec.info_hash = Some(InfoHash(digest));
                }
                info_obj = Some(parsed);
            }
        }

        let info_hash = spec
            .info_hash
            .or_else(|| spec.info_hash_v2.map(|v| v.to_short()))
            .ok_or_else(|| CoreError::Other("missing infohash".to_string()))?;

        // Existing by v1 map or short-hash map (hybrid).
        {
            let torrents_map = self.torrents.read();
            let short_map = self.torrents_by_short.read();
            let existing = torrents_map
                .get(&info_hash)
                .cloned()
                .or_else(|| short_map.get(&info_hash).cloned())
                .or_else(|| {
                    spec.info_hash_v2
                        .and_then(|v2| short_map.get(&v2.to_short()).cloned())
                });
            if let Some(existing) = existing {
                drop(short_map);
                drop(torrents_map);
                self.merge_spec(&existing, &spec, info_obj)?;
                return Ok((existing, false));
            }
        }

        let storage = spec
            .storage
            .clone()
            .unwrap_or_else(|| self.config.default_storage.clone());
        let torrent_inner = Arc::new(TorrentInner::new(
            info_hash,
            spec.info_hash_v2,
            spec.display_name.clone(),
            storage,
            self.arrival_model.clone(),
        ));
        torrent_inner
            .max_established_conns
            .store(self.config.established_conns_per_torrent, Ordering::Relaxed);
        if self.config.seed {
            torrent_inner.seeding.store(true, Ordering::Relaxed);
        }

        {
            let mut torrents_map = self.torrents.write();
            let mut short_map = self.torrents_by_short.write();
            torrents_map.insert(info_hash, torrent_inner.clone());
            short_map.insert(info_hash, torrent_inner.clone());
            if let Some(v2) = spec.info_hash_v2 {
                short_map.insert(v2.to_short(), torrent_inner.clone());
            }
        }

        if let Some(info) = info_obj {
            let _ = torrent_inner.set_info(Arc::new(info));
        }
        *torrent_inner.trackers.write() = spec.trackers.clone();
        *torrent_inner.web_seeds.write() = spec.web_seeds.clone();

        if !spec.dht_nodes.is_empty() {
            self.add_dht_nodes(&spec.dht_nodes);
        }

        self.register_sources(
            &torrent_inner,
            spec.trackers.clone(),
            spec.web_seeds.clone(),
        );

        // Dial known peers when dialing enabled (Go maybeNewConns).
        if self.config.dial_for_peer_conns {
            let peers: Vec<_> = torrent_inner.known_peers.read().iter().cloned().collect();
            for p in peers {
                let t = torrent_inner.clone();
                let cl = Arc::clone(self);
                self.tokio_handle.spawn(async move {
                    let _ = cl.dial_peer(t, p.addr, p.source).await;
                });
            }
        }

        Ok((torrent_inner, true))
    }

    fn merge_spec(
        &self,
        torrent: &Arc<TorrentInner>,
        spec: &TorrentSpec,
        info_obj: Option<wt_types::Info>,
    ) -> Result<()> {
        // display_name is not interior-mutable on TorrentInner; set only at create.
        if let Some(info) = info_obj {
            if torrent.info.read().is_none() {
                let _ = torrent.set_info(Arc::new(info));
            }
        }
        if !spec.dht_nodes.is_empty() {
            self.add_dht_nodes(&spec.dht_nodes);
        }
        {
            let mut ws = torrent.web_seeds.write();
            for u in &spec.web_seeds {
                if !ws.contains(u) {
                    ws.push(u.clone());
                }
            }
        }
        {
            let mut tr = torrent.trackers.write();
            for tier in &spec.trackers {
                if !tr.iter().any(|t| t == tier) {
                    tr.push(tier.clone());
                }
            }
        }

        if let Some(sources) = self.sources.read().clone() {
            let keys = Self::torrent_keys_for(torrent);
            let _ = sources.add_trackers(&keys, spec.trackers.clone());
            if !spec.web_seeds.is_empty() {
                // Sink path also used at register; merge into torrent above is enough for now.
            }
            sources.refresh_torrent(&keys);
        }

        torrent.bump_change();
        Ok(())
    }

    pub fn torrent(&self, ih: InfoHash) -> Option<Arc<TorrentInner>> {
        self.torrents
            .read()
            .get(&ih)
            .cloned()
            .or_else(|| self.torrents_by_short.read().get(&ih).cloned())
    }

    pub fn torrents(&self) -> Vec<Arc<TorrentInner>> {
        self.torrents.read().values().cloned().collect()
    }

    pub fn local_port(&self) -> u16 {
        *self.listen_port.read()
    }

    pub fn listen_addrs(&self) -> Vec<SocketAddr> {
        self.listen_addrs.read().clone()
    }

    pub fn add_dht_nodes(&self, nodes: &[String]) {
        if let Some(sources) = self.sources.read().clone() {
            sources.add_dht_nodes(nodes);
        }
    }

    pub fn dht_contacts(&self) -> Vec<SocketAddr> {
        let Some(sources) = self.sources.read().clone() else {
            return Vec::new();
        };
        self.block_on(async move { sources.dht_contacts().await })
    }

    fn block_on<F: std::future::Future>(&self, fut: F) -> F::Output {
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::block_in_place(|| self.tokio_handle.block_on(fut))
        } else {
            self.tokio_handle.block_on(fut)
        }
    }

    pub fn set_outgoing_tcp_conn_hook(&self, hook: Option<TcpConnHook>) {
        *self.outgoing_tcp_conn_hook.write() = hook;
    }

    pub fn set_urgent_request_stealing(&self, after: Duration) {
        *self.urgent_steal_after.write() = after;
    }

    pub fn close(self: &Arc<Self>) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }

        if let Some(listener) = self.listener.write().take() {
            listener.close();
        }

        if let Some(sources) = self.sources.write().take() {
            self.block_on(async move { sources.close().await });
        }

        let torrents: Vec<_> = {
            let mut map = self.torrents.write();
            let vals: Vec<_> = map.values().cloned().collect();
            map.clear();
            vals
        };
        self.torrents_by_short.write().clear();

        for t in torrents {
            t.closed.store(true, Ordering::Relaxed);
            let conns = t.conns.write();
            for c in conns.values() {
                c.close();
            }
        }
    }

    async fn attach_accepted(self: &Arc<Self>, accepted: AcceptedConn) -> Result<()> {
        let torrent = self
            .torrents_by_short
            .read()
            .get(&accepted.info_hash)
            .cloned()
            .ok_or_else(|| CoreError::Other("unknown infohash after accept".into()))?;

        // Handshake already completed by PeerListener; attach legacy PeerConn I/O.
        self.attach_handshaken_peer(
            torrent,
            accepted.stream,
            accepted.handle.remote_addr(),
            false,
            PeerSource::Incoming,
            accepted.handle.peer_id(),
        )
        .await
    }

    /// Dial a peer for `torrent` (used by e2e / transfer tests and maybeNewConns).
    pub async fn dial_peer(
        self: &Arc<Self>,
        torrent: Arc<TorrentInner>,
        addr: SocketAddr,
        source: PeerSource,
    ) -> Result<()> {
        if self.closed.load(Ordering::Relaxed) || torrent.closed.load(Ordering::Relaxed) {
            return Ok(());
        }
        if !self.config.dial_for_peer_conns {
            return Err(CoreError::Other("dial_for_peer_conns disabled".into()));
        }

        self.active_half_open
            .store(self.half_open.current_total(), Ordering::Relaxed);

        let metadata_size = torrent
            .info
            .read()
            .as_ref()
            .map(|i| i.total_length() as u32);
        let hook = self.outgoing_tcp_conn_hook.read().clone();
        let utp = self
            .listener
            .read()
            .as_ref()
            .and_then(|l| l.utp_socket().cloned());

        // Prefer plain when peer is a trusted Direct e2e peer and TCP is enabled —
        // still go through conn::dial_peer for half-open + rate limiting. MSE is
        // required to talk to our own PeerListener.
        let params = DialParams {
            remote_addr: addr,
            info_hash: torrent.info_hash,
            our_peer_id: self.peer_id,
            transport: if !self.config.disable_tcp {
                Some(wt_types::Transport::Tcp)
            } else {
                Some(wt_types::Transport::Utp)
            },
            source,
            metadata_size,
            cfg: &self.config,
            tracker: &self.half_open,
            dial_limiter: Some(&self.dial_limiter),
            tcp_conn_hook: hook.as_ref(),
            utp_socket: utp.as_ref(),
            require_plain: false,
        };

        let outcome = conn_dial_peer(params).await?;
        self.active_half_open
            .store(self.half_open.current_total(), Ordering::Relaxed);

        self.attach_handshaken_peer(
            torrent,
            outcome.stream,
            addr,
            true,
            source,
            Some(outcome.peer_id),
        )
        .await
    }

    /// Dial over an explicit transport (for TCP vs uTP handshake tests).
    pub async fn dial_peer_transport(
        self: &Arc<Self>,
        torrent: Arc<TorrentInner>,
        addr: SocketAddr,
        source: PeerSource,
        transport: wt_types::Transport,
        require_plain: bool,
    ) -> Result<()> {
        if self.closed.load(Ordering::Relaxed) || torrent.closed.load(Ordering::Relaxed) {
            return Ok(());
        }
        let metadata_size = torrent
            .info
            .read()
            .as_ref()
            .map(|i| i.total_length() as u32);
        let hook = self.outgoing_tcp_conn_hook.read().clone();
        let utp = self
            .listener
            .read()
            .as_ref()
            .and_then(|l| l.utp_socket().cloned());
        let params = DialParams {
            remote_addr: addr,
            info_hash: torrent.info_hash,
            our_peer_id: self.peer_id,
            transport: Some(transport),
            source,
            metadata_size,
            cfg: &self.config,
            tracker: &self.half_open,
            dial_limiter: Some(&self.dial_limiter),
            tcp_conn_hook: hook.as_ref(),
            utp_socket: utp.as_ref(),
            require_plain,
        };
        let outcome = conn_dial_peer(params).await?;
        self.attach_handshaken_peer(
            torrent,
            outcome.stream,
            addr,
            true,
            source,
            Some(outcome.peer_id),
        )
        .await
    }

    async fn attach_handshaken_peer(
        self: &Arc<Self>,
        torrent: Arc<TorrentInner>,
        stream: Box<dyn PeerStream>,
        remote_addr: SocketAddr,
        outgoing: bool,
        source: PeerSource,
        peer_id: Option<PeerId>,
    ) -> Result<()> {
        // Reuse legacy PeerConn reader/writer until PeerTorrent + ConnHandler are wired.
        self.spawn_peer_conn_handshaken(torrent, stream, remote_addr, outgoing, source, peer_id)
            .await
    }

    /// Legacy PeerConn attach for a stream that already completed BT (+ optional MSE) handshake.
    pub async fn spawn_peer_conn_handshaken(
        self: &Arc<Self>,
        torrent: Arc<TorrentInner>,
        stream: Box<dyn PeerStream>,
        remote_addr: SocketAddr,
        outgoing: bool,
        source: PeerSource,
        peer_id: Option<PeerId>,
    ) -> Result<()> {
        let (write_tx, mut write_rx) = mpsc::unbounded_channel::<Message>();
        let write_waker = Arc::new(Notify::new());

        let peer_conn = PeerConn::new(
            remote_addr,
            outgoing,
            source,
            write_tx.clone(),
            write_waker.clone(),
        );
        if let Some(pid) = peer_id {
            peer_conn.inner.write().peer_id = Some(pid);
        }

        // Send Extended Handshake (BEP 10) — may duplicate dialer's; peers tolerate it.
        let mut m_map = BTreeMap::new();
        m_map.insert(EXTENSION_NAME_METADATA.to_string(), DEFAULT_UT_METADATA_ID);
        if !self.config.disable_pex {
            m_map.insert(EXTENSION_NAME_PEX.to_string(), DEFAULT_UT_PEX_ID);
        }

        let ext_hs = ExtendedHandshake {
            m: m_map,
            v: Some(self.config.extended_handshake_client_version.clone()),
            p: Some(*self.listen_port.read()),
            yourip: None,
            ipv4: None,
            ipv6: None,
            reqq: Some(250),
            metadata_size: torrent
                .info
                .read()
                .as_ref()
                .map(|i| i.total_length() as u32),
            upload_only: None,
            encryption: None,
        };
        peer_conn.send_message(Message::Extended {
            id: EXT_HANDSHAKE_ID,
            payload: ext_hs.encode(),
        });

        let pieces_guard = torrent.pieces.read();
        if !pieces_guard.is_empty() {
            let num_pieces = pieces_guard.len();
            let num_bytes = num_pieces.div_ceil(8);
            let mut bf = vec![0u8; num_bytes];
            let mut any_complete = false;
            for (idx, p) in pieces_guard.iter().enumerate() {
                if p.complete {
                    bf[idx / 8] |= 1 << (7 - (idx % 8));
                    any_complete = true;
                }
            }
            if any_complete {
                peer_conn.send_message(Message::Bitfield(Bytes::from(bf)));
            }
        }
        drop(pieces_guard);

        if !torrent.data_upload_disallowed.load(Ordering::Relaxed) {
            peer_conn.send_message(Message::Unchoke);
            peer_conn.inner.write().am_choking = false;
        }

        if !torrent.data_download_disallowed.load(Ordering::Relaxed) {
            peer_conn.send_message(Message::Interested);
            peer_conn.inner.write().am_interested = true;
        }

        torrent.conns.write().insert(remote_addr, peer_conn.clone());
        torrent.bump_change();

        for cb in &self.config.callbacks.completed_handshake {
            cb(&peer_conn.peer_conn_info(), torrent.info_hash);
        }

        let (mut read_half, mut write_half) = tokio::io::split(stream);

        let waker_clone = write_waker.clone();
        let p_conn_w = peer_conn.clone();
        let _writer_handle = self.tokio_handle.spawn(async move {
            let mut write_buf = BytesMut::with_capacity(64 * 1024);
            while !p_conn_w.is_closed() {
                let notified = waker_clone.notified();
                tokio::pin!(notified);

                while let Ok(msg) = write_rx.try_recv() {
                    msg.encode(&mut write_buf);
                }

                if write_buf.is_empty() {
                    tokio::select! {
                        _ = notified => {},
                        msg = write_rx.recv() => {
                            if let Some(m) = msg {
                                m.encode(&mut write_buf);
                            } else {
                                break;
                            }
                        }
                    }
                }

                if !write_buf.is_empty() {
                    use tokio::io::AsyncWriteExt;
                    if write_half.write_all(&write_buf).await.is_err() {
                        break;
                    }
                    write_buf.clear();
                }
            }
            p_conn_w.close();
        });

        let p_conn_r = peer_conn.clone();
        let tor_r = torrent.clone();
        let _reader_handle = self.tokio_handle.spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut read_buf = BytesMut::with_capacity(64 * 1024);
            while !p_conn_r.is_closed() {
                let msg_res = decode_message(&mut read_buf, DEFAULT_MAX_MESSAGE_LENGTH);
                match msg_res {
                    Ok(Some(msg)) => {
                        match msg {
                            Message::Choke => {
                                p_conn_r.inner.write().peer_choking = true;
                                p_conn_r.inner.write().webtor_choked_since = Some(Instant::now());
                                tor_r.bump_change();
                            }
                            Message::Unchoke => {
                                p_conn_r.inner.write().peer_choking = false;
                                p_conn_r.inner.write().webtor_unchoked_at = Some(Instant::now());
                                tor_r.refill_peer_requests(&p_conn_r);
                                tor_r.bump_change();
                            }
                            Message::Interested => {
                                p_conn_r.inner.write().peer_interested = true;
                                tor_r.bump_change();
                            }
                            Message::NotInterested => {
                                p_conn_r.inner.write().peer_interested = false;
                                tor_r.bump_change();
                            }
                            Message::Have { index } => {
                                p_conn_r.inner.write().peer_pieces.insert(index);
                                tor_r.refill_peer_requests(&p_conn_r);
                                tor_r.bump_change();
                            }
                            Message::Bitfield(bf) => {
                                let mut inner = p_conn_r.inner.write();
                                for (byte_idx, &byte) in bf.iter().enumerate() {
                                    for bit_idx in 0..8 {
                                        if (byte & (1 << (7 - bit_idx))) != 0 {
                                            let p = (byte_idx * 8 + bit_idx) as PieceIndex;
                                            inner.peer_pieces.insert(p);
                                        }
                                    }
                                }
                                drop(inner);
                                tor_r.refill_peer_requests(&p_conn_r);
                                tor_r.bump_change();
                            }
                            Message::HaveAll => {
                                p_conn_r.inner.write().peer_sent_have_all = true;
                                tor_r.refill_peer_requests(&p_conn_r);
                                tor_r.bump_change();
                            }
                            Message::Piece {
                                index,
                                begin,
                                piece,
                            } => {
                                tor_r.on_block_received(remote_addr, index, begin, piece);
                                tor_r.refill_peer_requests(&p_conn_r);
                            }
                            Message::Request {
                                index,
                                begin,
                                length,
                            } => {
                                if !tor_r.data_upload_disallowed.load(Ordering::Relaxed) {
                                    if let Some(info) = tor_r.info.read().as_ref() {
                                        if let Some(storage) = tor_r.storage.read().as_ref() {
                                            let piece_obj = info.piece(index);
                                            let piece_storage =
                                                storage.piece(piece_obj, info.piece_hash_v1(index));
                                            let mut buf = vec![0u8; length as usize];
                                            if let Ok(n) =
                                                piece_storage.read_at(&mut buf, begin as i64)
                                            {
                                                if n == length as usize {
                                                    p_conn_r.send_message(Message::Piece {
                                                        index,
                                                        begin,
                                                        piece: Bytes::from(buf),
                                                    });
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            Message::Cancel { .. } => {}
                            Message::Extended { id, payload } => {
                                if id == EXT_HANDSHAKE_ID {
                                    if let Ok(ext) = ExtendedHandshake::decode(&payload) {
                                        let mut inner = p_conn_r.inner.write();
                                        inner.peer_ext_ut_metadata =
                                            ext.m.get(EXTENSION_NAME_METADATA).copied();
                                        inner.peer_ext_ut_pex =
                                            ext.m.get(EXTENSION_NAME_PEX).copied();
                                        inner.peer_metadata_size = ext.metadata_size;
                                        drop(inner);

                                        if tor_r.info.read().is_none() {
                                            if let Some(m_id) =
                                                ext.m.get(EXTENSION_NAME_METADATA).copied()
                                            {
                                                let req = MetadataMsg::request(0);
                                                p_conn_r.send_message(req.to_message(m_id));
                                            }
                                        }
                                    }
                                } else if id == DEFAULT_UT_METADATA_ID {
                                    if let Ok(meta_msg) = MetadataMsg::decode(payload) {
                                        match meta_msg.msg_type {
                                            MetadataMsgType::Request => {
                                                if let Some(mi) = tor_r.metainfo_raw.read().as_ref()
                                                {
                                                    let resp = MetadataMsg::data(
                                                        meta_msg.piece,
                                                        mi.info_bytes.len() as u32,
                                                        Bytes::copy_from_slice(&mi.info_bytes),
                                                    );
                                                    if let Some(peer_m_id) =
                                                        p_conn_r.inner.read().peer_ext_ut_metadata
                                                    {
                                                        p_conn_r.send_message(
                                                            resp.to_message(peer_m_id),
                                                        );
                                                    }
                                                }
                                            }
                                            MetadataMsgType::Data => {
                                                if tor_r.info.read().is_none() {
                                                    if let Some(data) = meta_msg.data {
                                                        if let Ok(info) = wt_meta::parse_info(&data)
                                                        {
                                                            let _ = tor_r.set_info(Arc::new(info));
                                                        }
                                                    }
                                                }
                                            }
                                            MetadataMsgType::Reject => {}
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }
                    Ok(None) => {}
                    Err(_) => {
                        break;
                    }
                }

                match read_half.read_buf(&mut read_buf).await {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            p_conn_r.close();
            tor_r.conns.write().remove(&remote_addr);
            tor_r.bump_change();
        });

        Ok(())
    }
}
