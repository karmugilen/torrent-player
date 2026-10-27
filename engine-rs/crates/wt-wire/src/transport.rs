use std::io::{self, Error, ErrorKind};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::pin::Pin;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use wt_types::Transport;

use crate::utp::{UtpSocket, UtpStream};

pub async fn dial_tcp(addr: SocketAddr, timeout: Duration) -> io::Result<TcpStream> {
    tokio::time::timeout(timeout, TcpStream::connect(addr))
        .await
        .map_err(|_| Error::new(ErrorKind::TimedOut, format!("dial TCP {addr} timed out")))?
}

pub async fn listen_tcp(addr: SocketAddr) -> io::Result<TcpListener> {
    TcpListener::bind(addr).await
}

pub async fn dial_utp(
    socket: &UtpSocket,
    addr: SocketAddr,
    timeout: Duration,
) -> io::Result<UtpStream> {
    tokio::time::timeout(timeout, socket.connect(addr))
        .await
        .map_err(|_| Error::new(ErrorKind::TimedOut, format!("dial uTP {addr} timed out")))?
}

pub async fn listen_utp(addr: SocketAddr) -> io::Result<UtpSocket> {
    UtpSocket::bind(addr).await
}

/// Helper to bind TCP on both IPv4 and IPv6 (dual stack) for a given port.
/// If port is 0, a free ephemeral port is selected and used for both where possible.
pub async fn listen_dual_stack_tcp(port: u16) -> io::Result<Vec<TcpListener>> {
    let mut listeners = Vec::new();

    let v4_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port));
    let l_v4 = TcpListener::bind(v4_addr).await?;
    let bound_port = l_v4.local_addr()?.port();
    listeners.push(l_v4);

    let v6_addr = SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, bound_port, 0, 0));
    if let Ok(l_v6) = TcpListener::bind(v6_addr).await {
        listeners.push(l_v6);
    }

    Ok(listeners)
}

/// Helper to bind uTP on both IPv4 and IPv6 (dual stack) for a given port.
pub async fn listen_dual_stack_utp(port: u16) -> io::Result<Vec<UtpSocket>> {
    let mut sockets = Vec::new();

    let v4_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port));
    let s_v4 = UtpSocket::bind(v4_addr).await?;
    let bound_port = s_v4.local_addr()?.port();
    sockets.push(s_v4);

    let v6_addr = SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, bound_port, 0, 0));
    if let Ok(s_v6) = UtpSocket::bind(v6_addr).await {
        sockets.push(s_v6);
    }

    Ok(sockets)
}

/// Unified connection wrapper for TCP or uTP.
pub enum WireStream {
    Tcp(TcpStream),
    Utp(UtpStream),
}

impl AsyncRead for WireStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            WireStream::Tcp(s) => Pin::new(s).poll_read(cx, buf),
            WireStream::Utp(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for WireStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        match self.get_mut() {
            WireStream::Tcp(s) => Pin::new(s).poll_write(cx, buf),
            WireStream::Utp(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            WireStream::Tcp(s) => Pin::new(s).poll_flush(cx),
            WireStream::Utp(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            WireStream::Tcp(s) => Pin::new(s).poll_shutdown(cx),
            WireStream::Utp(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

/// Dial peer using specified transport, honoring timeout.
pub async fn dial_peer(
    addr: SocketAddr,
    transport: Transport,
    timeout: Duration,
    utp_socket: Option<&UtpSocket>,
) -> io::Result<WireStream> {
    match transport {
        Transport::Tcp => {
            let tcp = dial_tcp(addr, timeout).await?;
            Ok(WireStream::Tcp(tcp))
        }
        Transport::Utp => {
            let sock = utp_socket.ok_or_else(|| {
                Error::new(ErrorKind::InvalidInput, "uTP socket required to dial uTP")
            })?;
            let utp = dial_utp(sock, addr, timeout).await?;
            Ok(WireStream::Utp(utp))
        }
        _ => Err(Error::new(
            ErrorKind::Unsupported,
            format!("unsupported transport: {transport:?}"),
        )),
    }
}
