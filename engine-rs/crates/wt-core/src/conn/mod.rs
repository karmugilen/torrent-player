//! Peer connection I/O driver and shared UDP datagram socket.
pub mod dialer;
pub mod driver;
pub mod half_open;
pub mod handle;
pub mod listener;
pub mod shared_udp;

pub(crate) use dialer::forget_first_try_stats;
pub use dialer::{dial_peer, reduced_dial_timeout, DialOutcome, DialParams};
pub use driver::{spawn_connection_driver, DEFAULT_KEEPALIVE_TIMEOUT, DEFAULT_READ_DEADLINE};
pub use half_open::{GlobalHalfOpenGuard, HalfOpenTracker, TorrentHalfOpenGuard};
pub use handle::{ConnHandle, ConnHandler};
pub use listener::{AcceptedConn, InfoHashLookup, PeerListener, SkeyProvider};
pub use shared_udp::SharedDatagramAdapter;
