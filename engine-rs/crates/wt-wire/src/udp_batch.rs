//! Send already-queued UDP datagrams together without delaying sparse traffic.
use bytes::Bytes;
use std::net::SocketAddr;
use tokio::net::UdpSocket;

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
