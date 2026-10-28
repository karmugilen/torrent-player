//! Live connection handle and event handler interface.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::RwLock;
use tokio::sync::{mpsc, Notify};
use wt_types::{InfoHash, PeerId, PeerSource, Transport};
use wt_wire::message::Message;

use crate::stats::{AtomicConnStats, PeerConnInfo};
use crate::{CoreError, Result};

/// Callback interface implemented by the peer state machine (e.g. `agy-2`).
pub trait ConnHandler: Send + 'static {
    /// Invoked whenever a new BitTorrent message arrives from the peer.
    fn on_message(&mut self, handle: &ConnHandle, msg: Message) -> Result<()>;

    /// Invoked when the connection closes, with an optional error description.
    fn on_closed(&mut self, handle: &ConnHandle, reason: &str);

    fn on_wake(&mut self, _handle: &ConnHandle) {}

    fn wake_interval(&self) -> Option<std::time::Duration> {
        None
    }
}

/// A cheaply cloneable handle to an active peer connection.
#[derive(Clone)]
pub struct ConnHandle {
    inner: Arc<ConnHandleInner>,
}

pub struct ConnHandleInner {
    pub remote_addr: SocketAddr,
    pub outgoing: bool,
    pub transport: Transport,
    pub encrypted: bool,
    pub info_hash: InfoHash,
    pub peer_id: Option<PeerId>,
    pub source: PeerSource,
    /// Peer's reserved handshake bits (LTEP / Fast / DHT).
    pub peer_extension_bits: [u8; 8],
    pub connected_at: Instant,
    pub closed: Arc<AtomicBool>,
    pub close_reason: RwLock<Option<String>>,
    pub stats: Arc<AtomicConnStats>,
    pub write_tx: mpsc::UnboundedSender<Message>,
    pub write_waker: Arc<Notify>,
    pub read_waker: Notify,
    refocus_requested: AtomicBool,
}

impl ConnHandle {
    /// Create a new handle instance.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        remote_addr: SocketAddr,
        outgoing: bool,
        transport: Transport,
        encrypted: bool,
        info_hash: InfoHash,
        peer_id: Option<PeerId>,
        source: PeerSource,
        peer_extension_bits: [u8; 8],
        write_tx: mpsc::UnboundedSender<Message>,
        write_waker: Arc<Notify>,
        closed: Arc<AtomicBool>,
        stats: Arc<AtomicConnStats>,
    ) -> Self {
        Self {
            inner: Arc::new(ConnHandleInner {
                remote_addr,
                outgoing,
                transport,
                encrypted,
                info_hash,
                peer_id,
                source,
                peer_extension_bits,
                connected_at: Instant::now(),
                closed,
                close_reason: RwLock::new(None),
                stats,
                write_tx,
                write_waker,
                read_waker: Notify::new(),
                refocus_requested: AtomicBool::new(false),
            }),
        }
    }

    /// Queue a message to be written to the peer (non-blocking).
    /// Returns `true` if queued, `false` if the connection is closed.
    /// Unbounded so Piece replies are never try_send-dropped under a full pipeline.
    pub fn send_message(&self, msg: Message) -> bool {
        if self.is_closed() {
            return false;
        }
        match self.inner.write_tx.send(msg) {
            Ok(()) => {
                self.inner.write_waker.notify_waiters();
                true
            }
            Err(_) => false,
        }
    }

    /// Queue a message (unbounded writer; fails only when closed).
    pub async fn send_message_async(&self, msg: Message) -> Result<()> {
        if self.send_message(msg) {
            Ok(())
        } else {
            Err(CoreError::Closed)
        }
    }

    /// Explicitly wake the connection writer to flush queued messages.
    pub fn flush(&self) {
        self.inner.write_waker.notify_waiters();
    }

    pub fn request_refocus(&self) {
        self.inner.refocus_requested.store(true, Ordering::Relaxed);
        self.wake();
    }

    pub(crate) fn refocus_pending(&self) -> bool {
        self.inner.refocus_requested.load(Ordering::Relaxed)
    }

    pub(crate) fn take_refocus(&self) -> bool {
        self.inner.refocus_requested.swap(false, Ordering::Relaxed)
    }

    /// Coalesce request-update notifications until the reader handles them.
    pub fn wake(&self) {
        self.inner.read_waker.notify_one();
    }

    pub(crate) fn read_waker(&self) -> &Notify {
        &self.inner.read_waker
    }

    /// Close the connection with a reason.
    pub fn close(&self, reason: impl Into<String>) {
        let r = reason.into();
        let was_closed = self.inner.closed.swap(true, Ordering::SeqCst);
        if !was_closed {
            *self.inner.close_reason.write() = Some(r);
            self.inner.write_waker.notify_waiters();
            self.wake();
        }
    }

    /// Check if the connection has been closed.
    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::Relaxed)
    }

    /// The reason the connection was closed, if set.
    pub fn close_reason(&self) -> Option<String> {
        self.inner.close_reason.read().clone()
    }

    /// Remote peer address.
    pub fn remote_addr(&self) -> SocketAddr {
        self.inner.remote_addr
    }

    /// Transport type (TCP or uTP).
    pub fn transport(&self) -> Transport {
        self.inner.transport
    }

    /// Whether this is an outbound connection.
    pub fn is_outgoing(&self) -> bool {
        self.inner.outgoing
    }

    /// Instant the connection completed its BitTorrent handshake.
    pub fn connected_at(&self) -> Instant {
        self.inner.connected_at
    }

    /// Whether Message Stream Encryption (MSE) is active.
    pub fn is_encrypted(&self) -> bool {
        self.inner.encrypted
    }

    /// Discovery / dial source for this connection.
    pub fn source(&self) -> PeerSource {
        self.inner.source
    }

    /// Peer's reserved BitTorrent handshake extension bits.
    pub fn peer_extension_bits(&self) -> [u8; 8] {
        self.inner.peer_extension_bits
    }

    /// InfoHash associated with this peer session.
    pub fn info_hash(&self) -> InfoHash {
        self.inner.info_hash
    }

    /// Remote Peer ID, if negotiated during handshake.
    pub fn peer_id(&self) -> Option<PeerId> {
        self.inner.peer_id
    }

    /// Live byte and chunk statistics counters.
    pub fn stats(&self) -> Arc<AtomicConnStats> {
        self.inner.stats.clone()
    }

    /// Connection metadata snapshot.
    pub fn info(&self) -> PeerConnInfo {
        let stats = self.inner.stats.snapshot();
        PeerConnInfo {
            remote_addr: self.inner.remote_addr,
            dial_addr: if self.inner.outgoing {
                Some(self.inner.remote_addr)
            } else {
                None
            },
            peer_id: self.inner.peer_id,
            client_name: String::new(),
            transport: self.inner.transport,
            source: self.inner.source,
            encrypted: self.inner.encrypted,
            outgoing: self.inner.outgoing,
            peer_choking: true,
            am_choking: true,
            peer_interested: false,
            am_interested: false,
            conn_stats: stats,
            download_rate: 0.0,
            upload_rate: 0.0,
            peer_completion: 0.0,
            pending_requests: 0,
            peer_max_requests: crate::peer_conn::DEFAULT_PEER_MAX_REQUESTS,
            peak_requests: 0,
            pipeline_limit: 0,
            nominal_max_requests: 1,
            rtt: std::time::Duration::from_millis(200),
            queued: 0,
            cancelled: 0,
            connected_for: self.inner.connected_at.elapsed(),
            last_useful_data: None,
        }
    }
}
