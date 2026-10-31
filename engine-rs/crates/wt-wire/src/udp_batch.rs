//! Send and receive queued UDP datagrams together without delaying sparse traffic.
use bytes::{Bytes, BytesMut};
use std::collections::VecDeque;
use std::net::SocketAddr;
use tokio::net::UdpSocket;

/// Datagrams taken from the socket in one system call.
const RECV_BATCH: usize = 32;
/// Room for a datagram. uTP packets stop at 1432 bytes; a longer one is dropped.
const RECV_SLOT: usize = 2048;
const RECV_CHUNK: usize = 64 * RECV_SLOT;

/// Reads datagrams into one shared buffer, so a packet costs neither an
/// allocation nor a copy. Each packet keeps its part of the buffer alive until
/// it has been consumed; a new buffer starts when this one runs out.
pub(crate) struct DatagramReader {
    chunk: BytesMut,
    batching: bool,
}

impl DatagramReader {
    pub(crate) fn new() -> Self {
        Self { chunk: BytesMut::new(), batching: true }
    }

    /// Wait for traffic, then queue everything ready, up to one batch.
    pub(crate) async fn read(
        &mut self,
        socket: &UdpSocket,
        out: &mut VecDeque<(Bytes, SocketAddr)>,
    ) -> std::io::Result<()> {
        if self.chunk.len() < RECV_BATCH * RECV_SLOT {
            self.chunk = BytesMut::zeroed(RECV_CHUNK);
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        if self.batching {
            use std::io::ErrorKind;
            use std::os::fd::AsRawFd;
            use tokio::io::Interest;
            let buf = &mut self.chunk[..RECV_BATCH * RECV_SLOT];
            let mut found = Vec::with_capacity(RECV_BATCH);
            loop {
                let result = socket
                    .async_io(Interest::READABLE, || recv_many(socket.as_raw_fd(), buf, &mut found))
                    .await;
                match result {
                    Ok(()) => break,
                    Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(e) if e.raw_os_error() == Some(nix::errno::Errno::ENOSYS as i32) => {
                        self.batching = false;
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }
            if self.batching {
                for datagram in found {
                    let mut slot = self.chunk.split_to(RECV_SLOT);
                    if let Some((len, from)) = datagram {
                        slot.truncate(len);
                        out.push_back((slot.freeze(), from));
                    }
                }
                return Ok(());
            }
        }
        let (len, from) = socket.recv_from(&mut self.chunk[..RECV_SLOT]).await?;
        let mut slot = self.chunk.split_to(RECV_SLOT);
        slot.truncate(len);
        out.push_back((slot.freeze(), from));
        Ok(())
    }
}

/// Receive up to one batch, one slot per datagram. `None` marks a datagram that
/// did not fit or has no usable sender.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn recv_many(
    fd: std::os::fd::RawFd,
    buf: &mut [u8],
    found: &mut Vec<Option<(usize, SocketAddr)>>,
) -> std::io::Result<()> {
    use nix::sys::socket::{recvmmsg, MsgFlags, MultiHeaders, SockaddrStorage};
    use std::cell::RefCell;
    use std::io::IoSliceMut;
    thread_local! {
        // Kept per thread: these headers cannot cross an await, and are too big
        // to rebuild on every wakeup.
        static HEADERS: RefCell<MultiHeaders<SockaddrStorage>> =
            RefCell::new(MultiHeaders::preallocate(RECV_BATCH, None));
    }
    HEADERS.with_borrow_mut(|headers| {
        let mut slices: Vec<[IoSliceMut<'_>; 1]> =
            buf.chunks_exact_mut(RECV_SLOT).map(|slot| [IoSliceMut::new(slot)]).collect();
        let received = recvmmsg(fd, headers, slices.iter_mut(), MsgFlags::MSG_DONTWAIT, None)
            .map_err(|error| std::io::Error::from_raw_os_error(error as i32))?;
        found.clear();
        found.extend(received.map(|message| {
            let from = message.address.as_ref().and_then(socket_addr)?;
            (!message.flags.contains(MsgFlags::MSG_TRUNC)).then_some((message.bytes, from))
        }));
        Ok(())
    })
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn socket_addr(address: &nix::sys::socket::SockaddrStorage) -> Option<SocketAddr> {
    use std::net::{SocketAddrV4, SocketAddrV6};
    address
        .as_sockaddr_in()
        .map(|v4| SocketAddr::V4(SocketAddrV4::from(*v4)))
        .or_else(|| address.as_sockaddr_in6().map(|v6| SocketAddr::V6(SocketAddrV6::from(*v6))))
}

pub(crate) async fn send_datagrams(socket: &UdpSocket, packets: &[(Bytes, SocketAddr)]) {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use std::io::ErrorKind;
        use std::os::fd::AsRawFd;
        use tokio::io::Interest;
        let mut sent = 0;
        while sent < packets.len() {
            let result = socket
                .async_io(Interest::WRITABLE, || {
                    send_many(socket.as_raw_fd(), &packets[sent..])
                })
                .await;
            match result {
                Ok(n) if n > 0 => sent += n,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) if e.raw_os_error() == Some(nix::errno::Errno::ENOSYS as i32) => {
                    for (data, target) in &packets[sent..] {
                        let _ = socket.send_to(data, target).await;
                    }
                    break;
                }
                // Match the existing pump: a failed datagram must not stop
                // subsequent packets or other connections sharing this socket.
                _ => sent += 1,
            }
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    for (data, target) in packets {
        let _ = socket.send_to(data, target).await;
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn send_many(fd: std::os::fd::RawFd, packets: &[(Bytes, SocketAddr)]) -> std::io::Result<usize> {
    use nix::sys::socket::{sendmmsg, MsgFlags, MultiHeaders, SockaddrStorage};
    use std::io::IoSlice;
    let addresses: Vec<Option<SockaddrStorage>> = packets
        .iter()
        .map(|(_, address)| Some((*address).into()))
        .collect();
    let slices: Vec<_> = packets
        .iter()
        .map(|(data, _)| [IoSlice::new(data)])
        .collect();
    let mut headers = MultiHeaders::<SockaddrStorage>::preallocate(packets.len(), None);
    // nix owns and validates the syscall buffers, keeping this crate's
    // forbid(unsafe_code) policy intact.
    sendmmsg(
        fd,
        &mut headers,
        &slices,
        &addresses,
        [],
        MsgFlags::MSG_DONTWAIT,
    )
    .map(|results| results.count())
    .map_err(|error| std::io::Error::from_raw_os_error(error as i32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn reader_returns_each_datagram_and_sender_and_drops_oversize() {
        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let receiver = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let to = receiver.local_addr().unwrap();
        for packet in [&b"one"[..], &[0u8; RECV_SLOT + 1], b"three"] {
            sender.send_to(packet, to).await.unwrap();
        }
        let mut reader = DatagramReader::new();
        let mut out = VecDeque::new();
        while out.len() < 2 {
            tokio::time::timeout(Duration::from_secs(2), reader.read(&receiver, &mut out))
                .await
                .unwrap()
                .unwrap();
        }
        let from = sender.local_addr().unwrap();
        assert_eq!(out[0], (Bytes::from_static(b"one"), from));
        assert_eq!(out[1], (Bytes::from_static(b"three"), from));
    }

    #[tokio::test]
    async fn batch_preserves_datagrams_destinations_and_order() {
        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let packets: Vec<_> = (0..64u32)
            .map(|i| {
                (
                    Bytes::copy_from_slice(&i.to_be_bytes()),
                    if i % 2 == 0 {
                        a.local_addr().unwrap()
                    } else {
                        b.local_addr().unwrap()
                    },
                )
            })
            .collect();
        send_datagrams(&sender, &packets).await;
        for i in 0..64u32 {
            let mut bytes = [0; 8];
            let receiver = if i % 2 == 0 { &a } else { &b };
            let (n, _) =
                tokio::time::timeout(Duration::from_secs(2), receiver.recv_from(&mut bytes))
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(n, 4);
            assert_eq!(&bytes[..n], &i.to_be_bytes());
        }
    }

    #[tokio::test]
    async fn bad_destination_does_not_drop_remaining_packets() {
        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let receiver = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let target = receiver.local_addr().unwrap();
        let packets = vec![
            (Bytes::from_static(b"first"), target),
            (
                Bytes::from_static(b"invalid family"),
                "[::1]:9000".parse().unwrap(),
            ),
            (Bytes::from_static(b"last"), target),
        ];
        send_datagrams(&sender, &packets).await;
        for expected in [b"first".as_slice(), b"last".as_slice()] {
            let mut bytes = [0; 32];
            let (n, _) =
                tokio::time::timeout(Duration::from_secs(2), receiver.recv_from(&mut bytes))
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(&bytes[..n], expected);
        }
    }
}
