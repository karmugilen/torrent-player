//! Shared UDP datagram adapter multiplexing uTP and non-uTP traffic (DHT / UDP trackers).

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use wt_discovery::datagram::{DatagramIo, IoFut};
use wt_wire::utp::UtpSocket;

/// Adapter implementing [`DatagramIo`] over a shared [`UtpSocket`].
///
/// Packets matching uTP protocol headers are processed by uTP state machines,
/// while non-uTP datagrams (e.g. BEP 5 DHT and BEP 15 UDP tracker queries/replies)
/// are routed through [`UtpSocket::recv_non_utp`] and [`UtpSocket::send_non_utp`].
#[derive(Clone)]
pub struct SharedDatagramAdapter {
    sock: Arc<UtpSocket>,
}

impl SharedDatagramAdapter {
    /// Create a new adapter wrapping an existing uTP socket.
    pub fn new(sock: Arc<UtpSocket>) -> Self {
        Self { sock }
    }

    /// Access the underlying uTP socket.
    pub fn utp_socket(&self) -> &Arc<UtpSocket> {
        &self.sock
    }
}

impl DatagramIo for SharedDatagramAdapter {
    fn send_to<'a>(&'a self, buf: &'a [u8], addr: SocketAddr) -> IoFut<'a, usize> {
        Box::pin(async move { self.sock.send_non_utp(buf, addr).await })
    }

    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> IoFut<'a, (usize, SocketAddr)> {
        Box::pin(async move {
            let (bytes, from) = self.sock.recv_non_utp().await?;
            let to_copy = bytes.len().min(buf.len());
            buf[..to_copy].copy_from_slice(&bytes[..to_copy]);
            Ok((to_copy, from))
        })
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.sock.local_addr()
    }
}
