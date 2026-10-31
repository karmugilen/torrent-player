use super::*;
use std::sync::atomic::AtomicUsize;
use std::task::Wake;

struct WakeCount(AtomicUsize);
impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn stream(state: StreamState) -> (UtpStream, mpsc::UnboundedReceiver<(Bytes, SocketAddr)>) {
    let (socket_tx, receiver) = mpsc::unbounded_channel();
    (
        UtpStream {
            inner: Arc::new(UtpStreamInner {
                state: Mutex::new(state),
                socket_tx,
                _closed: AtomicBool::new(false),
            }),
            registry: Weak::new(),
            _socket: None,
            read_timer: None,
        },
        receiver,
    )
}

fn state() -> StreamState {
    StreamState::new(10, 11, "127.0.0.1:9000".parse().unwrap(), Instant::now())
}

#[tokio::test]
async fn pending_reads_do_not_spawn_a_timer_task_per_poll() {
    let (mut stream, _receiver) = stream(state());
    let waker = Waker::from(Arc::new(WakeCount(AtomicUsize::new(0))));
    let mut cx = Context::from_waker(&waker);
    let handle = tokio::runtime::Handle::current();
    let before = handle.metrics().num_alive_tasks();
    for _ in 0..512 {
        let mut bytes = [0; 16];
        assert!(Pin::new(&mut stream)
            .poll_read(&mut cx, &mut ReadBuf::new(&mut bytes))
            .is_pending());
    }
    let after = handle.metrics().num_alive_tasks();
    assert!(
        after <= before + 1,
        "512 pending read polls spawned {} timer tasks",
        after - before
    );
}

#[tokio::test]
async fn draining_full_receive_buffer_announces_reopened_window() {
    let mut st = state();
    st.recv_buf.resize(DEFAULT_RECV_WND as usize, 7);
    // The sender has seen a zero-window ACK and cannot send more data.
    let mut ack = encode_ack_packet(&mut st, 0, None);
    assert_eq!(PacketHeader::decode(&mut ack).unwrap().wnd_size, 0);
    let (mut stream, mut receiver) = stream(st);
    let waker = Waker::from(Arc::new(WakeCount(AtomicUsize::new(0))));
    let mut cx = Context::from_waker(&waker);
    let mut bytes = vec![0; 64 * 1024];
    let mut buf = ReadBuf::new(&mut bytes);
    assert!(matches!(
        Pin::new(&mut stream).poll_read(&mut cx, &mut buf),
        Poll::Ready(Ok(()))
    ));
    assert_eq!(buf.filled().len(), 64 * 1024);
    let (mut ack, _) = receiver
        .try_recv()
        .expect("sender remains stuck at zero window after reader frees space");
    let ack = PacketHeader::decode(&mut ack).unwrap();
    assert_eq!(ack.get_type(), ST_STATE);
    assert_eq!(ack.wnd_size, 64 * 1024);
}

#[tokio::test]
async fn lost_outgoing_packet_is_retransmitted_while_waiting_for_read() {
    let mut st = state();
    st.bytes_in_flight = 4;
    st.unacked.push_back(OutgoingPacket {
        seq_nr: 1,
        len: 4,
        payload: Bytes::from_static(b"ping"),
        sent_at: Instant::now() - Duration::from_millis(501),
        retransmitted: false,
    });
    let (mut stream, mut receiver) = stream(st);
    let waker = Waker::from(Arc::new(WakeCount(AtomicUsize::new(0))));
    let mut cx = Context::from_waker(&waker);
    let mut bytes = [0; 4];
    assert!(Pin::new(&mut stream)
        .poll_read(&mut cx, &mut ReadBuf::new(&mut bytes))
        .is_pending());
    let (mut packet, _) = receiver.try_recv().unwrap();
    assert_eq!(
        PacketHeader::decode(&mut packet).unwrap().get_type(),
        ST_DATA
    );
    assert_eq!(&packet[..], b"ping");
}

#[tokio::test]
async fn waiting_read_is_woken_when_first_unacked_write_needs_a_timer() {
    let (mut stream, _receiver) = stream(state());
    let wakes = Arc::new(WakeCount(AtomicUsize::new(0)));
    let waker = Waker::from(wakes.clone());
    let mut cx = Context::from_waker(&waker);
    let mut bytes = [0; 16];
    assert!(Pin::new(&mut stream)
        .poll_read(&mut cx, &mut ReadBuf::new(&mut bytes))
        .is_pending());
    assert!(stream.read_timer.is_none());
    assert!(matches!(
        Pin::new(&mut stream).poll_write(&mut cx, b"ping"),
        Poll::Ready(Ok(4))
    ));
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    assert!(Pin::new(&mut stream)
        .poll_read(&mut cx, &mut ReadBuf::new(&mut bytes))
        .is_pending());
    assert!(stream.read_timer.is_some());
}

#[tokio::test]
async fn writes_advertise_actual_receive_capacity() {
    let mut st = state();
    st.recv_buf.resize(DEFAULT_RECV_WND as usize / 2, 7);
    let (mut stream, mut receiver) = stream(st);
    let waker = Waker::from(Arc::new(WakeCount(AtomicUsize::new(0))));
    let mut cx = Context::from_waker(&waker);
    assert!(matches!(
        Pin::new(&mut stream).poll_write(&mut cx, b"ping"),
        Poll::Ready(Ok(4))
    ));
    let (mut packet, _) = receiver.try_recv().unwrap();
    let header = PacketHeader::decode(&mut packet).unwrap();
    assert_eq!(
        header.wnd_size,
        DEFAULT_RECV_WND / 2,
        "outgoing requests must not claim a full window while receive data is buffered"
    );
}

#[tokio::test]
async fn receive_window_updates_are_coalesced_after_small_reads() {
    let mut st = state();
    st.recv_buf.resize(256 * 1024, 7);
    let _ = encode_ack_packet(&mut st, 0, None);
    let (mut stream, mut receiver) = stream(st);
    let waker = Waker::from(Arc::new(WakeCount(AtomicUsize::new(0))));
    let mut cx = Context::from_waker(&waker);
    let mut bytes = [0; 16 * 1024];
    for _ in 0..4 {
        let mut buffer = ReadBuf::new(&mut bytes);
        assert!(matches!(
            Pin::new(&mut stream).poll_read(&mut cx, &mut buffer),
            Poll::Ready(Ok(()))
        ));
    }
    let (mut packet, _) = receiver.try_recv().unwrap();
    assert_eq!(
        PacketHeader::decode(&mut packet).unwrap().wnd_size,
        DEFAULT_RECV_WND - 192 * 1024
    );
    assert!(
        receiver.try_recv().is_err(),
        "small reads must not each create an ACK packet"
    );
}
