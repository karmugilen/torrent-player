use bytes::{Buf, BufMut, Bytes, BytesMut};
use parking_lot::Mutex;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::Future;
use std::io::{self, Error, ErrorKind};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot, watch};

pub const UTP_VERSION: u8 = 1;

pub const ST_DATA: u8 = 0;
pub const ST_FIN: u8 = 1;
pub const ST_STATE: u8 = 2;
pub const ST_RESET: u8 = 3;
pub const ST_SYN: u8 = 4;

pub const EXT_NONE: u8 = 0;
pub const EXT_SELECTIVE_ACK: u8 = 1;

pub const HEADER_SIZE: usize = 20;
pub const MAX_PACKET_SIZE: usize = 1432;
pub const MAX_PAYLOAD_SIZE: usize = MAX_PACKET_SIZE - HEADER_SIZE; // 1412 bytes

pub const LEDBAT_TARGET_MICROS: u32 = 100_000; // 100ms
pub const INITIAL_CWND: usize = 512 * 1024; // 512 KiB
pub const MAX_CWND: usize = 8 * 1024 * 1024; // 8 MiB for high loopback throughput
pub const DEFAULT_RECV_WND: u32 = 8 * 1024 * 1024;
pub const DUP_ACKS_BEFORE_RESEND: u16 = 3;
const WINDOW_UPDATE_BYTES: u32 = 64 * 1024;

/// Skip linked uTP extension headers after the 20-byte packet header.
/// `first_ext` is `PacketHeader.extension`. Returns remaining payload and
/// selective-ACK bitmask (if present). Matches libutp/BEP 29 layout:
/// each extension is `[next_type][len][len bytes…]` where the *current*
/// type came from the previous `next_type` (or the header for the first).
pub fn split_extensions(first_ext: u8, mut data: Bytes) -> io::Result<(Bytes, Option<Bytes>)> {
    let mut ext = first_ext;
    let mut sack: Option<Bytes> = None;
    while ext != EXT_NONE {
        if data.len() < 2 {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "uTP extension header truncated",
            ));
        }
        let next = data[0];
        let len = data[1] as usize;
        data.advance(2);
        if data.len() < len {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "uTP extension body truncated",
            ));
        }
        let body = data.split_to(len);
        if ext == EXT_SELECTIVE_ACK {
            sack = Some(body);
        }
        ext = next;
    }
    Ok((data, sack))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PacketHeader {
    pub type_ver: u8,
    pub extension: u8,
    pub connection_id: u16,
    pub timestamp_micros: u32,
    pub timestamp_diff_micros: u32,
    pub wnd_size: u32,
    pub seq_nr: u16,
    pub ack_nr: u16,
}

impl PacketHeader {
    pub fn get_type(&self) -> u8 {
        self.type_ver >> 4
    }

    pub fn get_version(&self) -> u8 {
        self.type_ver & 0x0f
    }

    pub fn encode(&self, dst: &mut BytesMut) {
        dst.put_u8(self.type_ver);
        dst.put_u8(self.extension);
        dst.put_u16(self.connection_id);
        dst.put_u32(self.timestamp_micros);
        dst.put_u32(self.timestamp_diff_micros);
        dst.put_u32(self.wnd_size);
        dst.put_u16(self.seq_nr);
        dst.put_u16(self.ack_nr);
    }

    pub fn decode(src: &mut Bytes) -> io::Result<Self> {
        if src.len() < HEADER_SIZE {
            return Err(Error::new(
                ErrorKind::UnexpectedEof,
                "uTP packet header too short",
            ));
        }
        let type_ver = src.get_u8();
        let extension = src.get_u8();
        let connection_id = src.get_u16();
        let timestamp_micros = src.get_u32();
        let timestamp_diff_micros = src.get_u32();
        let wnd_size = src.get_u32();
        let seq_nr = src.get_u16();
        let ack_nr = src.get_u16();

        Ok(Self {
            type_ver,
            extension,
            connection_id,
            timestamp_micros,
            timestamp_diff_micros,
            wnd_size,
            seq_nr,
            ack_nr,
        })
    }
}

pub fn now_micros(start: Instant) -> u32 {
    (start.elapsed().as_micros() & 0xffff_ffff) as u32
}

/// Wall-clock microseconds for uTP header timestamps (libtorrent gettimeofday).
/// Relative elapsed timestamps make peer delay samples ~unix-epoch-sized and
/// collapse LEDBAT windows on public swarms.
pub fn wall_micros() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_micros() & 0xffff_ffff) as u32)
        .unwrap_or(0)
}

struct OutgoingPacket {
    seq_nr: u16,
    len: usize,
    payload: Bytes,
    sent_at: Instant,
    /// Sent more than once, so its ACK says nothing about the round trip.
    retransmitted: bool,
}

/// Retransmit timeout before a connection has a round-trip sample.
const INITIAL_RTO: Duration = Duration::from_millis(500);
const MIN_RTO: Duration = Duration::from_millis(200);
const MAX_RTO: Duration = Duration::from_secs(4);
/// How long a base delay is trusted: clocks drift and routes change.
const BASE_DELAY_WINDOW: Duration = Duration::from_secs(60);

static UTP_TRACE: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(|| std::env::var_os("WT_UTP_TRACE").is_some());

struct StreamState {
    send_id: u16,
    _recv_id: u16,
    remote_addr: SocketAddr,
    #[allow(dead_code)]
    start_time: Instant,

    // Send state
    seq_nr: u16,
    ack_nr: u16, // remote's latest acked seq_nr
    last_remote_ack: u16,
    remote_wnd: u32,
    cwnd: usize,
    unacked: VecDeque<OutgoingPacket>,
    bytes_in_flight: usize,
    write_closed: bool,
    write_waker: Option<Waker>,
    dup_ack_count: u16,
    fast_resend_seq: Option<u16>,

    // Recv state
    expected_seq_nr: u16,
    recv_buf: BytesMut,
    out_of_order: BTreeMap<u16, Bytes>,
    read_closed: bool,
    read_waker: Option<Waker>,
    last_advertised_window: u32,

    // Congestion / RTT
    base_delay_micros: u32,
    /// Lowest outbound delay in the current window, next window's base.
    recent_min_delay: u32,
    base_delay_since: Instant,
    last_measured_delay: u32,
    srtt: Option<Duration>,
    rttvar: Duration,
    rto: Duration,
    unacked_recv_count: usize,

    // Reset / Fin
    is_reset: bool,
}

impl StreamState {
    fn new(send_id: u16, recv_id: u16, remote_addr: SocketAddr, start_time: Instant) -> Self {
        Self {
            send_id,
            _recv_id: recv_id,
            remote_addr,
            start_time,
            seq_nr: 1,
            ack_nr: 0,
            last_remote_ack: 0,
            remote_wnd: DEFAULT_RECV_WND,
            cwnd: INITIAL_CWND,
            recent_min_delay: u32::MAX,
            base_delay_since: Instant::now(),
            srtt: None,
            rttvar: Duration::ZERO,
            rto: INITIAL_RTO,
            unacked: VecDeque::new(),
            bytes_in_flight: 0,
            write_closed: false,
            write_waker: None,
            dup_ack_count: 0,
            fast_resend_seq: None,
            expected_seq_nr: 0,
            recv_buf: BytesMut::with_capacity(256 * 1024),
            out_of_order: BTreeMap::new(),
            read_closed: false,
            read_waker: None,
            last_advertised_window: DEFAULT_RECV_WND,
            base_delay_micros: u32::MAX,
            last_measured_delay: 0,
            unacked_recv_count: 0,
            is_reset: false,
        }
    }

    /// Build selective-ACK bitmask for packets past `ack_nr` that sit in the OOO buffer.
    /// RFC 6298 estimate from an ACK of a packet sent once.
    fn on_rtt_sample(&mut self, sample: Duration) {
        match self.srtt {
            None => {
                self.srtt = Some(sample);
                self.rttvar = sample / 2;
            }
            Some(srtt) => {
                self.rttvar = (self.rttvar * 3 + srtt.abs_diff(sample)) / 4;
                self.srtt = Some((srtt * 7 + sample) / 8);
            }
        }
        let srtt = self.srtt.unwrap_or(sample);
        self.rto = (srtt + self.rttvar * 4).clamp(MIN_RTO, MAX_RTO);
    }

    /// The oldest packet went unanswered for a whole timeout. The window stays:
    /// requests share it, and a shrunken one held up seeks.
    fn on_timeout(&mut self) {
        self.rto = (self.rto * 2).min(MAX_RTO);
    }

    /// Grow the window by how far the delay of what we send, reported back by
    /// the peer, is below the LEDBAT target. It never shrinks on delay: requests
    /// share this window, and comparing builds showed seeks got slower.
    fn on_send_delay(&mut self, delay: u32) {
        if delay == 0 {
            return;
        }
        if self.base_delay_since.elapsed() >= BASE_DELAY_WINDOW {
            self.base_delay_micros = self.recent_min_delay;
            self.recent_min_delay = u32::MAX;
            self.base_delay_since = Instant::now();
        }
        self.recent_min_delay = self.recent_min_delay.min(delay);
        self.base_delay_micros = self.base_delay_micros.min(delay);
        let queue = u64::from(delay.saturating_sub(self.base_delay_micros));
        let target = u64::from(LEDBAT_TARGET_MICROS);
        let mss = MAX_PAYLOAD_SIZE as u64;
        if queue < target {
            let grow = (mss * (target - queue) / target) as usize;
            self.cwnd = (self.cwnd + grow).min(MAX_CWND);
        }
    }

    fn selective_ack_bitmask(&self) -> Option<Vec<u8>> {
        if self.out_of_order.is_empty() {
            return None;
        }
        let mut max_offset = 0u16;
        for &seq in self.out_of_order.keys() {
            let offset = seq.wrapping_sub(self.ack_nr);
            // bit 0 = ack_nr+2 => offset 2
            if offset >= 2 {
                max_offset = max_offset.max(offset);
            }
        }
        if max_offset < 2 {
            return None;
        }
        let bits_needed = (max_offset - 1) as usize; // offset 2 => bit 0
        let bytes_needed = bits_needed.div_ceil(32) * 4; // multiple of 4
        let mut mask = vec![0u8; bytes_needed.max(4)];
        for &seq in self.out_of_order.keys() {
            let offset = seq.wrapping_sub(self.ack_nr);
            if offset < 2 {
                continue;
            }
            let bit = (offset - 2) as usize;
            let byte = bit / 8;
            let bit_in_byte = bit % 8;
            if byte < mask.len() {
                mask[byte] |= 1 << bit_in_byte;
            }
        }
        Some(mask)
    }
}

fn encode_ack_packet(st: &mut StreamState, delay: u32, sack: Option<&[u8]>) -> Bytes {
    let (ext, ext_overhead) = match sack {
        Some(m) if !m.is_empty() => (EXT_SELECTIVE_ACK, 2 + m.len()),
        _ => (EXT_NONE, 0),
    };
    let mut b = BytesMut::with_capacity(HEADER_SIZE + ext_overhead);
    let window = DEFAULT_RECV_WND.saturating_sub(st.recv_buf.len() as u32);
    st.last_advertised_window = window;
    let resp = PacketHeader {
        type_ver: (ST_STATE << 4) | UTP_VERSION,
        extension: ext,
        connection_id: st.send_id,
        timestamp_micros: wall_micros(),
        timestamp_diff_micros: delay,
        wnd_size: window,
        seq_nr: st.seq_nr,
        ack_nr: st.ack_nr,
    };
    resp.encode(&mut b);
    if let Some(m) = sack {
        if !m.is_empty() {
            b.put_u8(EXT_NONE);
            b.put_u8(m.len() as u8);
            b.extend_from_slice(m);
        }
    }
    b.freeze()
}

/// A uTP Stream socket implementing `AsyncRead + AsyncWrite`.
pub struct UtpStream {
    inner: Arc<UtpStreamInner>,
    registry: Weak<StreamRegistry>,
    /// Keeps the owning `UtpSocket` alive for ephemeral dial sockets so the
    /// recv/send pumps are not left only on detached task Arcs.
    _socket: Option<Arc<UtpSocket>>,
    read_timer: Option<Pin<Box<tokio::time::Sleep>>>,
}

pub struct UtpStreamInner {
    state: Mutex<StreamState>,
    socket_tx: mpsc::UnboundedSender<(Bytes, SocketAddr)>,
    _closed: AtomicBool,
}

impl UtpStream {
    pub fn peer_addr(&self) -> SocketAddr {
        self.inner.state.lock().remote_addr
    }

    fn close_write(&self) {
        let mut st = self.inner.state.lock();
        if !st.write_closed && !st.is_reset {
            st.write_closed = true;
            let window = DEFAULT_RECV_WND.saturating_sub(st.recv_buf.len() as u32);
            st.last_advertised_window = window;
            let hdr = PacketHeader {
                type_ver: (ST_FIN << 4) | UTP_VERSION,
                extension: EXT_NONE,
                connection_id: st.send_id,
                timestamp_micros: wall_micros(),
                timestamp_diff_micros: st.last_measured_delay,
                wnd_size: window,
                seq_nr: st.seq_nr,
                ack_nr: st.ack_nr,
            };
            st.seq_nr = st.seq_nr.wrapping_add(1);
            let mut pkt = BytesMut::with_capacity(HEADER_SIZE);
            hdr.encode(&mut pkt);
            let target = st.remote_addr;
            let _ = self.inner.socket_tx.send((pkt.freeze(), target));
        }
    }

    /// Attach a strong ref to the parent socket (ephemeral outgoing dials).
    pub fn with_socket(mut self, socket: Arc<UtpSocket>) -> Self {
        self._socket = Some(socket);
        self
    }
}

impl Drop for UtpStream {
    fn drop(&mut self) {
        self.close_write();
        let key = {
            let state = self.inner.state.lock();
            (state.remote_addr, state._recv_id)
        };
        if let Some(registry) = self.registry.upgrade() {
            let mut streams = registry.lock();
            if streams
                .get(&key)
                .is_some_and(|inner| Arc::ptr_eq(inner, &self.inner))
            {
                streams.remove(&key);
            }
        }
    }
}

impl AsyncRead for UtpStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let mut st = this.inner.state.lock();
        if st.unacked.is_empty() {
            this.read_timer = None;
        }
        // Retransmit while blocked on read — otherwise a lost first DATA
        // after SYN-ACK never retries (poll_write is not called again).
        let rto = st.rto;
        let retransmit = st.unacked.front().and_then(|front| {
            if front.sent_at.elapsed() > rto {
                Some((
                    front.seq_nr,
                    front.len,
                    front.payload.clone(),
                    st.send_id,
                    wall_micros(),
                    st.last_measured_delay,
                    DEFAULT_RECV_WND.saturating_sub(st.recv_buf.len() as u32),
                    st.ack_nr,
                    st.remote_addr,
                ))
            } else {
                None
            }
        });
        if let Some((seq, len, payload, send_id, ts, delay, wnd, ack, target)) = retransmit {
            if let Some(front) = st.unacked.front_mut() {
                front.sent_at = Instant::now();
                front.retransmitted = true;
            }
            st.on_timeout();
            let hdr = PacketHeader {
                type_ver: (ST_DATA << 4) | UTP_VERSION,
                extension: EXT_NONE,
                connection_id: send_id,
                timestamp_micros: ts,
                timestamp_diff_micros: delay,
                wnd_size: wnd,
                seq_nr: seq,
                ack_nr: ack,
            };
            let mut pkt = BytesMut::with_capacity(HEADER_SIZE + len);
            hdr.encode(&mut pkt);
            pkt.extend_from_slice(&payload);
            st.last_advertised_window = wnd;
            let _ = this.inner.socket_tx.send((pkt.freeze(), target));
        }

        if !st.recv_buf.is_empty() {
            let to_read = std::cmp::min(buf.remaining(), st.recv_buf.len());
            let slice = st.recv_buf.split_to(to_read);
            buf.put_slice(&slice);

            // Receiving already ACKed these packets, but draining the buffer
            // must also advertise newly available space. Otherwise a sender
            // that saw a zero window can never send the next packet.
            let window = DEFAULT_RECV_WND.saturating_sub(st.recv_buf.len() as u32);
            let reopened = window.saturating_sub(st.last_advertised_window);
            if st.unacked_recv_count > 0
                || reopened >= WINDOW_UPDATE_BYTES
                || (st.last_advertised_window == 0 && window > 0)
            {
                st.unacked_recv_count = 0;
                let delay = st.last_measured_delay;
                let sack = st.selective_ack_bitmask();
                let packet = encode_ack_packet(&mut st, delay, sack.as_deref());
                let target = st.remote_addr;
                let _ = this.inner.socket_tx.send((packet, target));
            }

            return Poll::Ready(Ok(()));
        }

        if st.read_closed || st.is_reset {
            return Poll::Ready(Ok(()));
        }

        st.read_waker = Some(cx.waker().clone());
        // Reuse one timer for the oldest unacked packet. Spawning a fresh
        // sleeping task on every Pending poll accumulated thousands of stale
        // wakes under load, including when there was nothing to retransmit.
        if let Some(front) = st.unacked.front() {
            let deadline =
                tokio::time::Instant::from_std(front.sent_at + st.rto);
            let timer = this
                .read_timer
                .get_or_insert_with(|| Box::pin(tokio::time::sleep_until(deadline)));
            timer.as_mut().reset(deadline);
            if timer.as_mut().poll(cx).is_ready() {
                cx.waker().wake_by_ref();
            }
        }
        Poll::Pending
    }
}

impl AsyncWrite for UtpStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut st = self.inner.state.lock();
        if st.write_closed || st.is_reset {
            return Poll::Ready(Err(Error::new(
                ErrorKind::BrokenPipe,
                "uTP connection closed",
            )));
        }

        let mut total_sent = 0;
        let mut packets_to_send = Vec::new();

        let max_payload = if st.remote_addr.ip().is_loopback() {
            8192
        } else {
            MAX_PAYLOAD_SIZE
        };

        while total_sent < buf.len() {
            let max_flight = std::cmp::min(st.cwnd, st.remote_wnd as usize);
            if st.bytes_in_flight >= max_flight && max_flight > 0 {
                break;
            }

            let can_send = max_flight.saturating_sub(st.bytes_in_flight);
            let chunk_len =
                std::cmp::min(std::cmp::min(buf.len() - total_sent, can_send), max_payload);
            if chunk_len == 0 {
                break;
            }

            let payload = Bytes::copy_from_slice(&buf[total_sent..total_sent + chunk_len]);
            let seq = st.seq_nr;
            st.seq_nr = st.seq_nr.wrapping_add(1);

            let hdr = PacketHeader {
                type_ver: (ST_DATA << 4) | UTP_VERSION,
                extension: EXT_NONE,
                connection_id: st.send_id,
                timestamp_micros: wall_micros(),
                timestamp_diff_micros: st.last_measured_delay,
                wnd_size: DEFAULT_RECV_WND.saturating_sub(st.recv_buf.len() as u32),
                seq_nr: seq,
                ack_nr: st.ack_nr,
            };

            let mut pkt = BytesMut::with_capacity(HEADER_SIZE + chunk_len);
            st.last_advertised_window = hdr.wnd_size;
            hdr.encode(&mut pkt);
            pkt.extend_from_slice(&payload);
            let pkt_bytes = pkt.freeze();

            st.bytes_in_flight += chunk_len;
            st.unacked.push_back(OutgoingPacket {
                seq_nr: seq,
                len: chunk_len,
                payload: payload.clone(),
                sent_at: Instant::now(),
                retransmitted: false,
            });

            packets_to_send.push(pkt_bytes);
            total_sent += chunk_len;
        }

        if total_sent == 0 {
            // Retransmission check if waiting for window
            let my_wnd = DEFAULT_RECV_WND.saturating_sub(st.recv_buf.len() as u32);
            let send_id = st.send_id;
            let ts = wall_micros();
            let delay = st.last_measured_delay;
            let ack = st.ack_nr;
            let target = st.remote_addr;

            if let Some(front) = st.unacked.front_mut() {
                if front.sent_at.elapsed() > Duration::from_millis(50) {
                    front.sent_at = Instant::now();
                    front.retransmitted = true;
                    let hdr = PacketHeader {
                        type_ver: (ST_DATA << 4) | UTP_VERSION,
                        extension: EXT_NONE,
                        connection_id: send_id,
                        timestamp_micros: ts,
                        timestamp_diff_micros: delay,
                        wnd_size: my_wnd,
                        seq_nr: front.seq_nr,
                        ack_nr: ack,
                    };
                    let mut pkt = BytesMut::with_capacity(HEADER_SIZE + front.len);
                    hdr.encode(&mut pkt);
                    pkt.extend_from_slice(&front.payload);
                    let _ = self.inner.socket_tx.send((pkt.freeze(), target));
                }
            }

            st.write_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }

        let target = st.remote_addr;
        // A read may already be waiting with no retransmission timer. New
        // unacked data must wake it to register the first packet's deadline.
        if let Some(waker) = st.read_waker.take() {
            waker.wake();
        }
        drop(st);

        for pkt in packets_to_send {
            let _ = self.inner.socket_tx.send((pkt, target));
        }

        Poll::Ready(Ok(total_sent))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.close_write();

        Poll::Ready(Ok(()))
    }
}

type StreamRegistry = Mutex<HashMap<(SocketAddr, u16), Arc<UtpStreamInner>>>;
pub type StreamMap = Arc<StreamRegistry>;
pub type SynMap =
    Arc<Mutex<HashMap<(SocketAddr, u16), oneshot::Sender<io::Result<Arc<UtpStreamInner>>>>>>;

/// Removing an in-flight connect future (timeout or losing dial) must also
/// remove its SYN registration. Keep the same lock order as the receive pump
/// so cancellation cannot race a late SYN-ACK into leaving an orphan stream.
struct PendingConnect<'a> {
    socket: &'a UtpSocket,
    key: (SocketAddr, u16),
    armed: bool,
}

impl Drop for PendingConnect<'_> {
    fn drop(&mut self) {
        if self.armed {
            let mut pending = self.socket.syn_senders.lock();
            pending.remove(&self.key);
            self.socket.streams.lock().remove(&self.key);
        }
    }
}

/// Shared uTP socket managing multiplexing of uTP streams and non-uTP packets.
pub struct UtpSocket {
    udp: Arc<UdpSocket>,
    streams: StreamMap,
    syn_senders: SynMap,
    accept_rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<UtpStream>>>,
    non_utp_rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<(Bytes, SocketAddr)>>>,
    out_tx: mpsc::UnboundedSender<(Bytes, SocketAddr)>,
    closed: Arc<AtomicBool>,
    shutdown: watch::Sender<bool>,
    recv_task: tokio::task::JoinHandle<()>,
}

impl UtpSocket {
    pub async fn bind(addr: SocketAddr) -> io::Result<Self> {
        let std_sock = std::net::UdpSocket::bind(addr)?;
        let sock2 = socket2::SockRef::from(&std_sock);
        let _ = sock2.set_recv_buffer_size(16 * 1024 * 1024);
        let _ = sock2.set_send_buffer_size(16 * 1024 * 1024);
        let _ = std_sock.set_nonblocking(true);
        let udp = Arc::new(UdpSocket::from_std(std_sock)?);
        let streams: StreamMap = Arc::new(Mutex::new(HashMap::new()));
        let syn_senders: SynMap = Arc::new(Mutex::new(HashMap::new()));
        let (accept_tx, accept_rx) = mpsc::unbounded_channel();
        let (non_utp_tx, non_utp_rx) = mpsc::unbounded_channel();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<(Bytes, SocketAddr)>();
        let closed = Arc::new(AtomicBool::new(false));

        let (shutdown, mut send_shutdown) = watch::channel(false);

        // Outgoing pump
        let udp_out = udp.clone();
        tokio::spawn(async move {
            let mut batch = Vec::with_capacity(64);
            let mut draining = false;
            loop {
                let count = if draining {
                    out_rx.recv_many(&mut batch, 64).await
                } else {
                    tokio::select! {
                        biased;
                        _ = send_shutdown.changed() => {
                            // Stop new writes, but flush datagrams already
                            // queued by Write/Shutdown before dropping UDP.
                            out_rx.close();
                            draining = true;
                            continue;
                        }
                        count = out_rx.recv_many(&mut batch, 64) => count,
                    }
                };
                if count == 0 {
                    break;
                }
                for (buf, target) in &batch {
                    if *UTP_TRACE && buf.len() >= HEADER_SIZE {
                        let t = buf[0] >> 4;
                        let cid = u16::from_be_bytes([buf[2], buf[3]]);
                        let seq = u16::from_be_bytes([buf[16], buf[17]]);
                        let ack = u16::from_be_bytes([buf[18], buf[19]]);
                        eprintln!(
                            "utp OUT ->{target} type={t} cid={cid} seq={seq} ack={ack} len={}",
                            buf.len()
                        );
                    }
                }
                crate::udp_batch::send_datagrams(&udp_out, &batch).await;
                batch.clear();
            }
        });

        // Incoming reader & demux pump
        let udp_in = udp.clone();
        let streams_in = streams.clone();
        let syn_senders_in = syn_senders.clone();
        let out_tx_in = out_tx.clone();
        let closed_in = closed.clone();

        let recv_task = tokio::spawn(async move {
            let mut buf = [0u8; 65535];
            let start_time = Instant::now();
            let mut drained = 0u32;

            while !closed_in.load(Ordering::Relaxed) {
                // Take what is already queued without a trip through the
                // scheduler, yielding now and then so other tasks still run.
                let received = match udp_in.try_recv_from(&mut buf) {
                    Ok(res) if drained < 64 => {
                        drained += 1;
                        Ok(res)
                    }
                    Ok(res) => {
                        drained = 0;
                        tokio::task::yield_now().await;
                        Ok(res)
                    }
                    Err(err) if err.kind() == ErrorKind::WouldBlock => {
                        drained = 0;
                        udp_in.recv_from(&mut buf).await
                    }
                    Err(err) => Err(err),
                };
                let (len, peer) = match received {
                    Ok(res) => res,
                    Err(_) => break,
                };

                let mut raw = Bytes::copy_from_slice(&buf[..len]);
                if len < HEADER_SIZE {
                    let _ = non_utp_tx.send((raw, peer));
                    continue;
                }

                let type_ver = raw[0];
                let ver = type_ver & 0x0f;
                let pkt_type = type_ver >> 4;

                if ver != UTP_VERSION || pkt_type > ST_SYN {
                    // Non-uTP packet (e.g. DHT / UDP tracker), pass to non_utp demux channel!
                    let _ = non_utp_tx.send((raw, peer));
                    continue;
                }

                let original = raw.clone();
                let hdr = match PacketHeader::decode(&mut raw) {
                    Ok(h) => h,
                    Err(_) => {
                        let _ = non_utp_tx.send((original, peer));
                        continue;
                    }
                };

                let now_u = wall_micros();
                let delay = now_u.wrapping_sub(hdr.timestamp_micros);

                if *UTP_TRACE {
                    eprintln!(
                        "utp IN  <-{peer} type={pkt_type} cid={} seq={} ack={} len={len}",
                        hdr.connection_id, hdr.seq_nr, hdr.ack_nr
                    );
                }

                if pkt_type == ST_SYN {
                    // Incoming connection (BEP 29 / libutp):
                    // SYN carries initiator recv_id (=A). Acceptor send_id=A,
                    // recv_id=A+1. Response ST_STATE uses send_id=A.
                    // Previous code swapped send/recv; local loopback still
                    // worked because both ends were inverted, but public
                    // libtorrent/qBittorrent peers rejected post-SYN data.
                    let send_id = hdr.connection_id;
                    let recv_id = send_id.wrapping_add(1);

                    let mut st = StreamState::new(send_id, recv_id, peer, start_time);
                    st.ack_nr = hdr.seq_nr;
                    st.expected_seq_nr = hdr.seq_nr.wrapping_add(1);

                    let inner = Arc::new(UtpStreamInner {
                        state: Mutex::new(st),
                        socket_tx: out_tx_in.clone(),
                        _closed: AtomicBool::new(false),
                    });

                    streams_in.lock().insert((peer, recv_id), inner.clone());

                    // Send ST_STATE response
                    let resp = PacketHeader {
                        type_ver: (ST_STATE << 4) | UTP_VERSION,
                        extension: EXT_NONE,
                        connection_id: send_id,
                        timestamp_micros: wall_micros(),
                        timestamp_diff_micros: delay,
                        wnd_size: DEFAULT_RECV_WND,
                        seq_nr: 1,
                        ack_nr: hdr.seq_nr,
                    };
                    let mut b = BytesMut::with_capacity(HEADER_SIZE);
                    resp.encode(&mut b);
                    let _ = out_tx_in.send((b.freeze(), peer));

                    let stream = UtpStream {
                        inner,
                        registry: Arc::downgrade(&streams_in),
                        _socket: None,
                        read_timer: None,
                    };
                    let _ = accept_tx.send(stream);
                    continue;
                }

                // Lookup existing connection
                let stream_opt = {
                    let map = streams_in.lock();
                    map.get(&(peer, hdr.connection_id)).cloned()
                };

                let inner = match stream_opt {
                    Some(s) => s,
                    None => {
                        // SYN-ACK for an ongoing connect(): peer echoes our
                        // SYN connection_id (= our recv_id A). Initiator
                        // send_id = A+1, recv_id = A (BEP 29 diagram).
                        let mut syn_map = syn_senders_in.lock();
                        if pkt_type == ST_RESET {
                            if let Some(sender) = syn_map.remove(&(peer, hdr.connection_id)) {
                                let _ = sender.send(Err(Error::new(
                                    ErrorKind::ConnectionRefused,
                                    "uTP connect reset by peer",
                                )));
                            }
                            // Go/libutp ignores unknown RESETs. Replying with
                            // RESET creates a reset loop between closed peers.
                            continue;
                        }
                        if pkt_type != ST_STATE || hdr.ack_nr != 1 {
                            continue;
                        }
                        if let Some(sender) = syn_map.remove(&(peer, hdr.connection_id)) {
                            let recv_id = hdr.connection_id;
                            let send_id = recv_id.wrapping_add(1);
                            let mut st = StreamState::new(send_id, recv_id, peer, start_time);
                            st.seq_nr = 2; // ST_SYN consumed seq 1
                                           // libtorrent/libutp: ST_STATE does not consume seq_nr. Init
                                           // ack_nr to seq-1; first peer DATA still uses seq (== STATE.seq).
                                           // Using ack_nr=STATE.seq makes libtorrent ignore our DATA
                                           // (ack_nr > m_seq_nr-1) — public uTP HS then stalls forever.
                            st.ack_nr = hdr.seq_nr.wrapping_sub(1);
                            st.expected_seq_nr = hdr.seq_nr;
                            st.last_remote_ack = hdr.ack_nr;

                            let inner = Arc::new(UtpStreamInner {
                                state: Mutex::new(st),
                                socket_tx: out_tx_in.clone(),
                                _closed: AtomicBool::new(false),
                            });
                            streams_in.lock().insert((peer, recv_id), inner.clone());
                            let _ = sender.send(Ok(inner.clone()));
                            continue;
                        }

                        // Unknown uTP connection, send ST_RESET
                        let reset = PacketHeader {
                            type_ver: (ST_RESET << 4) | UTP_VERSION,
                            extension: EXT_NONE,
                            connection_id: hdr.connection_id,
                            timestamp_micros: wall_micros(),
                            timestamp_diff_micros: 0,
                            wnd_size: 0,
                            seq_nr: 1,
                            ack_nr: hdr.seq_nr,
                        };
                        let mut b = BytesMut::with_capacity(HEADER_SIZE);
                        reset.encode(&mut b);
                        let _ = out_tx_in.send((b.freeze(), peer));
                        continue;
                    }
                };

                // Strip extension headers before touching the payload. Public
                // libtorrent/qBittorrent peers attach selective-ACK on DATA;
                // treating those bytes as BT payload poisons length-prefix
                // framing and leaves established uTP peers at downloaded=0.
                let (payload, sack) = match split_extensions(hdr.extension, raw) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                // Process packet on stream
                let mut st = inner.state.lock();
                st.remote_wnd = hdr.wnd_size;
                st.last_measured_delay = delay;

                // Handle cumulative ACKs
                let mut acked_any = false;
                let prev_ack = st.last_remote_ack;
                let mut rtt_sample = None;
                while let Some(front) = st.unacked.front() {
                    let diff = hdr.ack_nr.wrapping_sub(front.seq_nr) as i16;
                    if diff >= 0 {
                        let len = front.len;
                        if !front.retransmitted {
                            rtt_sample = Some(front.sent_at.elapsed());
                        }
                        st.bytes_in_flight = st.bytes_in_flight.saturating_sub(len);
                        st.unacked.pop_front();
                        acked_any = true;
                    } else {
                        break;
                    }
                }
                if let Some(sample) = rtt_sample {
                    st.on_rtt_sample(sample);
                }
                if acked_any {
                    st.on_send_delay(hdr.timestamp_diff_micros);
                    st.last_remote_ack = hdr.ack_nr;
                    st.dup_ack_count = 0;
                } else if hdr.ack_nr == prev_ack && !st.unacked.is_empty() {
                    st.dup_ack_count = st.dup_ack_count.saturating_add(1);
                }

                // Selective ACK: count sacked packets past the cumulative ack
                // and fast-retransmit the HOL packet once we see enough evidence.
                // Packets stay in `unacked` until the cumulative ack advances
                // (libutp also frees sacked slots; cumulative path is sufficient).
                let mut sacked_after = 0u16;
                if let Some(ref mask) = sack {
                    for byte in mask.iter() {
                        sacked_after = sacked_after.saturating_add(byte.count_ones() as u16);
                    }
                }

                let front_seq = st.unacked.front().map(|p| p.seq_nr);
                let need_fast_resend = (st.dup_ack_count >= DUP_ACKS_BEFORE_RESEND
                    || sacked_after >= DUP_ACKS_BEFORE_RESEND)
                    && front_seq.is_some()
                    && st.fast_resend_seq != front_seq;

                let resend = if need_fast_resend {
                    let front = st.unacked.front_mut().expect("checked is_some");
                    front.sent_at = Instant::now();
                    front.retransmitted = true;
                    let seq = front.seq_nr;
                    let len = front.len;
                    let payload = front.payload.clone();
                    st.fast_resend_seq = Some(seq);
                    st.cwnd = (st.cwnd / 2).max(MAX_PAYLOAD_SIZE);
                    Some((
                        seq,
                        len,
                        payload,
                        st.send_id,
                        wall_micros(),
                        st.last_measured_delay,
                        DEFAULT_RECV_WND.saturating_sub(st.recv_buf.len() as u32),
                        st.ack_nr,
                        st.remote_addr,
                    ))
                } else {
                    None
                };

                if let Some(w) = st.write_waker.take() {
                    w.wake();
                }

                if let Some((seq, len, payload, send_id, ts, dly, wnd, ack, target)) = resend {
                    let rhdr = PacketHeader {
                        type_ver: (ST_DATA << 4) | UTP_VERSION,
                        extension: EXT_NONE,
                        connection_id: send_id,
                        timestamp_micros: ts,
                        timestamp_diff_micros: dly,
                        wnd_size: wnd,
                        seq_nr: seq,
                        ack_nr: ack,
                    };
                    let mut pkt = BytesMut::with_capacity(HEADER_SIZE + len);
                    rhdr.encode(&mut pkt);
                    pkt.extend_from_slice(&payload);
                    let _ = out_tx_in.send((pkt.freeze(), target));
                }

                match pkt_type {
                    ST_DATA => {
                        if hdr.seq_nr == st.expected_seq_nr {
                            st.recv_buf.extend_from_slice(&payload);
                            st.expected_seq_nr = st.expected_seq_nr.wrapping_add(1);

                            // Check out of order queue
                            loop {
                                let next_seq = st.expected_seq_nr;
                                if let Some(next_payload) = st.out_of_order.remove(&next_seq) {
                                    st.recv_buf.extend_from_slice(&next_payload);
                                    st.expected_seq_nr = st.expected_seq_nr.wrapping_add(1);
                                } else {
                                    break;
                                }
                            }
                            if let Some(w) = st.read_waker.take() {
                                w.wake();
                            }
                        } else if hdr.seq_nr.wrapping_sub(st.expected_seq_nr) as i16 > 0 {
                            st.out_of_order.insert(hdr.seq_nr, payload);
                        }

                        st.ack_nr = st.expected_seq_nr.wrapping_sub(1);
                        st.unacked_recv_count += 1;

                        // ACK every DATA promptly — delayed ACK (>=2) starved
                        // public-swarm peers that wait for ack before more data.
                        let should_ack = st.unacked_recv_count >= 1 || !st.out_of_order.is_empty();
                        if should_ack {
                            st.unacked_recv_count = 0;
                            let sack_bytes = st.selective_ack_bitmask();
                            let pkt = encode_ack_packet(&mut st, delay, sack_bytes.as_deref());
                            let _ = out_tx_in.send((pkt, peer));
                        }
                    }
                    ST_FIN => {
                        st.read_closed = true;
                        st.ack_nr = hdr.seq_nr;
                        st.unacked_recv_count = 0;
                        if let Some(w) = st.read_waker.take() {
                            w.wake();
                        }
                        let pkt = encode_ack_packet(&mut st, delay, None);
                        let _ = out_tx_in.send((pkt, peer));
                    }
                    ST_RESET => {
                        st.is_reset = true;
                        if let Some(w) = st.read_waker.take() {
                            w.wake();
                        }
                        if let Some(w) = st.write_waker.take() {
                            w.wake();
                        }
                    }
                    ST_STATE => {
                        // Cumulative + selective ACKs handled above.
                    }
                    _ => {}
                }
            }
        });

        Ok(Self {
            udp,
            streams,
            syn_senders,
            accept_rx: Arc::new(tokio::sync::Mutex::new(accept_rx)),
            non_utp_rx: Arc::new(tokio::sync::Mutex::new(non_utp_rx)),
            out_tx,
            closed,
            shutdown,
            recv_task,
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.udp.local_addr()
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        self.recv_task.abort();
        self.shutdown.send_replace(true);
        self.syn_senders.lock().clear();
        for (_, inner) in self.streams.lock().drain() {
            let mut state = inner.state.lock();
            state.is_reset = true;
            if let Some(waker) = state.read_waker.take() {
                waker.wake();
            }
            if let Some(waker) = state.write_waker.take() {
                waker.wake();
            }
        }
    }

    pub fn active_streams_count(&self) -> usize {
        self.streams.lock().len()
    }

    pub async fn accept(&self) -> io::Result<UtpStream> {
        let mut rx = self.accept_rx.lock().await;
        rx.recv()
            .await
            .ok_or_else(|| Error::new(ErrorKind::ConnectionAborted, "uTP socket closed"))
    }

    pub async fn connect(&self, remote: SocketAddr) -> io::Result<UtpStream> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(Error::new(
                ErrorKind::ConnectionAborted,
                "uTP socket closed",
            ));
        }
        // BEP 29: SYN carries conn_id_recv; subsequent packets use recv+1.
        let conn_id: u16 = rand::random();
        let (tx, rx) = oneshot::channel();
        self.syn_senders.lock().insert((remote, conn_id), tx);
        let mut pending = PendingConnect {
            socket: self,
            key: (remote, conn_id),
            armed: true,
        };

        let syn = PacketHeader {
            type_ver: (ST_SYN << 4) | UTP_VERSION,
            extension: EXT_NONE,
            connection_id: conn_id,
            // Wall-clock micros like libtorrent gettimeofday; relative
            // timestamps make peer delay samples nonsensical.
            timestamp_micros: wall_micros(),
            timestamp_diff_micros: 0,
            wnd_size: DEFAULT_RECV_WND,
            seq_nr: 1,
            ack_nr: 0,
        };

        let mut b = BytesMut::with_capacity(HEADER_SIZE);
        syn.encode(&mut b);
        self.out_tx
            .send((b.freeze(), remote))
            .map_err(|_| Error::new(ErrorKind::BrokenPipe, "socket closed"))?;

        match tokio::time::timeout(Duration::from_secs(5), rx).await {
            Ok(Ok(Ok(inner))) => {
                pending.armed = false;
                Ok(UtpStream {
                    inner,
                    registry: Arc::downgrade(&self.streams),
                    _socket: None,
                    read_timer: None,
                })
            }
            Ok(Ok(Err(error))) => Err(error),
            Ok(Err(_)) => Err(Error::new(
                ErrorKind::ConnectionAborted,
                "uTP socket closed",
            )),
            Err(_) => Err(Error::new(ErrorKind::TimedOut, "uTP connect timed out")),
        }
    }

    /// Read non-uTP packet (demuxed from shared UDP socket for DHT/tracker).
    pub async fn recv_non_utp(&self) -> io::Result<(Bytes, SocketAddr)> {
        let mut rx = self.non_utp_rx.lock().await;
        rx.recv()
            .await
            .ok_or_else(|| Error::new(ErrorKind::ConnectionAborted, "uTP socket closed"))
    }

    /// Send raw non-uTP UDP packet through the shared socket.
    pub async fn send_non_utp(&self, buf: &[u8], target: SocketAddr) -> io::Result<usize> {
        self.udp.send_to(buf, target).await
    }
}

impl Drop for UtpSocket {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
#[path = "utp_flow_tests.rs"]
mod flow_tests;
