//! Peer connection I/O driver and shared UDP datagram socket.
//!
//! # Guide for `agy-2` (Peer State Machine Integration)
//!
//! The `conn` module provides the transport, encryption, framing, and I/O loops
//! for incoming and outgoing BitTorrent peer connections.
//!
//! ## Public API Signatures
//!
//! ```ignore
//! // 1. Connection Handle and Event Handler:
//! pub trait ConnHandler: Send + 'static {
//!     fn on_message(&mut self, handle: &ConnHandle, msg: Message) -> Result<()>;
//!     fn on_closed(&mut self, handle: &ConnHandle, reason: &str);
//! }
//!
//! impl ConnHandle {
//!     pub fn send_message(&self, msg: Message) -> bool;
//!     pub async fn send_message_async(&self, msg: Message) -> Result<()>;
//!     pub fn flush(&self);
//!     pub fn close(&self, reason: impl Into<String>);
//!     pub fn is_closed(&self) -> bool;
//!     pub fn close_reason(&self) -> Option<String>;
//!     pub fn remote_addr(&self) -> SocketAddr;
//!     pub fn transport(&self) -> Transport;
//!     pub fn is_outgoing(&self) -> bool;
//!     pub fn is_encrypted(&self) -> bool;
//!     pub fn info_hash(&self) -> InfoHash;
//!     pub fn peer_id(&self) -> Option<PeerId>;
//!     pub fn stats(&self) -> Arc<AtomicConnStats>;
//!     pub fn info(&self) -> PeerConnInfo;
//! }
//!
//! // 2. Outgoing Dialing:
//! pub async fn dial_peer(params: DialParams<'_>) -> Result<DialOutcome>;
//!
//! // 3. Connection Driver:
//! pub fn spawn_connection_driver<H, S>(
//!     stream: S,
//!     handler: H,
//!     handle: ConnHandle,
//!     write_rx: mpsc::Receiver<Message>,
//!     write_waker: Arc<Notify>,
//!     download_limiter: Option<Limiter>,
//!     upload_limiter: Option<Limiter>,
//!     keepalive_timeout: Duration,
//!     read_deadline: Duration,
//! ) -> (JoinHandle<()>, JoinHandle<()>);
//!
//! // 4. Inbound Listener:
//! impl PeerListener {
//!     pub async fn bind(cfg: &ClientConfig) -> Result<Self>;
//!     pub fn listen_port(&self) -> u16;
//!     pub fn utp_socket(&self) -> Option<&Arc<UtpSocket>>;
//!     pub fn spawn_accept_loop(...);
//! }
//!
//! // 5. Shared UDP Socket for Discovery (DHT/UDP tracker):
//! impl SharedDatagramAdapter {
//!     pub fn new(sock: Arc<UtpSocket>) -> Self;
//!     pub fn utp_socket(&self) -> &Arc<UtpSocket>;
//! }
//! // SharedDatagramAdapter implements wt_discovery::DatagramIo.
//! ```

pub mod dialer;
pub mod driver;
pub mod half_open;
pub mod handle;
pub mod listener;
pub mod shared_udp;

pub use dialer::{dial_peer, reduced_dial_timeout, DialOutcome, DialParams, OUTBOUND_MSG_QUEUE};
pub use driver::{spawn_connection_driver, DEFAULT_KEEPALIVE_TIMEOUT, DEFAULT_READ_DEADLINE};
pub use half_open::{GlobalHalfOpenGuard, HalfOpenTracker, TorrentHalfOpenGuard};
pub use handle::{ConnHandle, ConnHandler};
pub use listener::{AcceptedConn, InfoHashLookup, PeerListener, SkeyProvider};
pub use shared_udp::SharedDatagramAdapter;
