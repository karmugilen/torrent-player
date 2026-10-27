//! Shared UDP datagram IO for DHT and UDP trackers.
//!
//! When uTP is enabled the engine owns one UDP socket (`wt-wire::UtpSocket`) and
//! demuxes non-uTP packets via `recv_non_utp` / `send_non_utp`. Core implements
//! [`DatagramIo`] over that socket and passes it into discovery. Dedicated /
//! standalone mode uses [`StandaloneDatagram`] wrapping a plain `UdpSocket`.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use tokio::net::UdpSocket;

/// Future returned by [`DatagramIo`] helpers.
pub type IoFut<'a, T> = Pin<Box<dyn Future<Output = std::io::Result<T>> + Send + 'a>>;

/// Send / receive datagrams — shared (non-uTP demux) or standalone.
///
/// Core implements this by wrapping `wt_wire::UtpSocket::{send_non_utp,
/// recv_non_utp}`. Discovery never depends on wt-wire directly.
pub trait DatagramIo: Send + Sync + 'static {
    fn send_to<'a>(&'a self, buf: &'a [u8], addr: SocketAddr) -> IoFut<'a, usize>;

    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> IoFut<'a, (usize, SocketAddr)>;

    fn local_addr(&self) -> std::io::Result<SocketAddr>;
}

/// Dedicated UDP socket fallback (no uTP sharing).
#[derive(Clone)]
pub struct StandaloneDatagram {
    sock: Arc<UdpSocket>,
}

impl StandaloneDatagram {
    pub fn new(sock: Arc<UdpSocket>) -> Self {
        Self { sock }
    }

    pub async fn bind(addr: &str) -> std::io::Result<Self> {
        Ok(Self {
            sock: Arc::new(UdpSocket::bind(addr).await?),
        })
    }

    pub fn socket(&self) -> &Arc<UdpSocket> {
        &self.sock
    }
}

impl DatagramIo for StandaloneDatagram {
    fn send_to<'a>(&'a self, buf: &'a [u8], addr: SocketAddr) -> IoFut<'a, usize> {
        Box::pin(async move { self.sock.send_to(buf, addr).await })
    }

    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> IoFut<'a, (usize, SocketAddr)> {
        Box::pin(async move { self.sock.recv_from(buf).await })
    }

    fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.sock.local_addr()
    }
}

impl DatagramIo for Arc<UdpSocket> {
    fn send_to<'a>(&'a self, buf: &'a [u8], addr: SocketAddr) -> IoFut<'a, usize> {
        Box::pin(async move { UdpSocket::send_to(self, buf, addr).await })
    }

    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> IoFut<'a, (usize, SocketAddr)> {
        Box::pin(async move { UdpSocket::recv_from(self, buf).await })
    }

    fn local_addr(&self) -> std::io::Result<SocketAddr> {
        UdpSocket::local_addr(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn standalone_roundtrip() {
        let a = StandaloneDatagram::bind("127.0.0.1:0").await.unwrap();
        let b = StandaloneDatagram::bind("127.0.0.1:0").await.unwrap();
        let dest = b.local_addr().unwrap();
        a.send_to(b"ping", dest).await.unwrap();
        let mut buf = [0u8; 16];
        let (n, from) = b.recv_from(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"ping");
        assert_eq!(from, a.local_addr().unwrap());
    }
}
