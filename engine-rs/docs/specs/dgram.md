wt-discovery publishes crate::datagram::DatagramIo (also re-exported as dht::DhtPacketIo / crate::DatagramIo).

```rust
pub type IoFut<'a, T> = Pin<Box<dyn Future<Output = std::io::Result<T>> + Send + 'a>>;

pub trait DatagramIo: Send + Sync + 'static {
    fn send_to<'a>(&'a self, buf: &'a [u8], addr: SocketAddr) -> IoFut<'a, usize>;
    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> IoFut<'a, (usize, SocketAddr)>;
    fn local_addr(&self) -> std::io::Result<SocketAddr>;
}
```

Standalone fallback: `StandaloneDatagram` wrapping `Arc<UdpSocket>` (+ `impl DatagramIo for Arc<UdpSocket>`).

Core/agy-1 should implement this over wt-wire `UtpSocket::{send_non_utp, recv_non_utp}` and pass `Arc<dyn DatagramIo>` into:
- `tracker::ClientOpts.datagram` / `UdpClientOpts.datagram`
- `dht::DhtNode::with_datagram_io` / `with_packet_io`

DHT KRPC tip: packets typically start with `d` (`looks_like_krpc`). UDP trackers and DHT both consume the same trait; dedicated socket mode remains the default when no shared io is supplied.