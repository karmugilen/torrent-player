use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::duplex;
use tokio::sync::Notify;
use wt_wire::codec::PeerConnMsgWriter;
use wt_wire::message::Message;

// Regression test port for WEBTOR_PATCH.md / #1070:
// A broadcast/wakeup that lands while fillWriteBuffer is running must NOT be lost.
// The writer installs its wakeup channel/listener before filling, so that a concurrent
// broadcast is observed by the subsequent wait.
#[tokio::test]
async fn test_msg_writer_broadcast_during_fill_wakes_writer() {
    let wakeup = Arc::new(Notify::new());
    let closed = Arc::new(AtomicBool::new(false));
    let fills = Arc::new(AtomicI32::new(0));

    let mut writer = PeerConnMsgWriter::new(wakeup.clone(), closed.clone());
    let (pipe_in, _pipe_out) = duplex(1024);

    let fills_clone = fills.clone();
    let wakeup_clone = wakeup.clone();

    let writer_task = tokio::spawn(async move {
        writer
            .run(
                pipe_in,
                Duration::from_secs(3600),
                |w| {
                    let prev = fills_clone.fetch_add(1, Ordering::SeqCst);
                    if prev == 0 {
                        // Concurrent wakeup during the first fill!
                        wakeup_clone.notify_waiters();
                    } else if prev == 1 {
                        // Second fill succeeded because writer woke up from the first notify!
                        // Write a message so the writer has something to send
                        w.write_msg(&Message::Keepalive);
                    }
                },
                || false,
            )
            .await
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while fills.load(Ordering::SeqCst) < 2 {
        if tokio::time::Instant::now() > deadline {
            closed.store(true, Ordering::SeqCst);
            wakeup.notify_waiters();
            panic!("writer never woke after a broadcast during fill: wakeup lost");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    closed.store(true, Ordering::SeqCst);
    wakeup.notify_waiters();
    let _ = tokio::time::timeout(Duration::from_secs(5), writer_task)
        .await
        .expect("writer task did not terminate within timeout");
}
