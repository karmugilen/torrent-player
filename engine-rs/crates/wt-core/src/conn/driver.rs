//! Connection I/O driver executing reader and writer event loops.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::BytesMut;
use tokio::io::{AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::{mpsc, Notify};
use wt_wire::codec::FramedReader;
use wt_wire::message::Message;

use crate::conn::handle::{ConnHandle, ConnHandler};
use crate::ratelimit::Limiter;
use crate::PeerStream;

/// Default keepalive ping interval (Go client: 120s).
pub const DEFAULT_KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(120);

/// Default read inactivity deadline (Go client: 150s).
pub const DEFAULT_READ_DEADLINE: Duration = Duration::from_secs(150);

/// Spawns the driver tasks (reader and writer) for a live connection.
///
/// Returns the join handles of the reader and writer tasks.
#[allow(clippy::too_many_arguments)]
pub fn spawn_connection_driver<H, S>(
    stream: S,
    mut handler: H,
    handle: ConnHandle,
    mut write_rx: mpsc::Receiver<Message>,
    write_waker: Arc<Notify>,
    download_limiter: Option<Limiter>,
    upload_limiter: Option<Limiter>,
    keepalive_timeout: Duration,
    read_deadline: Duration,
) -> (tokio::task::JoinHandle<()>, tokio::task::JoinHandle<()>)
where
    H: ConnHandler,
    S: PeerStream,
{
    let (read_half, write_half) = tokio::io::split(stream);

    // Spawn reader task
    let r_handle = handle.clone();
    let r_dl = download_limiter;
    let reader_task = tokio::spawn(async move {
        run_reader_loop(
            read_half,
            &mut handler,
            &r_handle,
            r_dl.as_ref(),
            read_deadline,
        )
        .await;
    });

    // Spawn writer task
    let w_handle = handle.clone();
    let w_ul = upload_limiter;
    let writer_task = tokio::spawn(async move {
        run_writer_loop(
            write_half,
            &w_handle,
            &mut write_rx,
            write_waker,
            w_ul.as_ref(),
            keepalive_timeout,
        )
        .await;
    });

    (reader_task, writer_task)
}

async fn run_reader_loop<H>(
    read_half: ReadHalf<impl PeerStream>,
    handler: &mut H,
    handle: &ConnHandle,
    download_limiter: Option<&Limiter>,
    read_deadline: Duration,
) where
    H: ConnHandler,
{
    let mut reader = FramedReader::new(read_half);
    let stats = handle.stats();

    while !handle.is_closed() {
        let read_fut = tokio::time::timeout(read_deadline, reader.read_msg());
        let msg = match read_fut.await {
            Ok(Ok(Some(msg))) => msg,
            Ok(Ok(None)) => {
                handle.close("connection closed by peer (EOF)");
                break;
            }
            Ok(Err(err)) => {
                handle.close(format!("frame decode error: {err}"));
                break;
            }
            Err(_) => {
                handle.close("read deadline exceeded (keepalive timeout)");
                break;
            }
        };

        match msg {
            Message::Piece {
                ref piece,
                index: _,
                begin: _,
            } => {
                let len = piece.len();
                stats.bytes_read.fetch_add(len as i64, Ordering::Relaxed);
                stats
                    .bytes_read_data
                    .fetch_add(len as i64, Ordering::Relaxed);
                stats.chunks_read.fetch_add(1, Ordering::Relaxed);
                if let Some(limiter) = download_limiter {
                    if let Some(delay) = limiter.reserve_delay(len) {
                        if !delay.is_zero() {
                            tokio::time::sleep(delay).await;
                        }
                    }
                }
            }
            Message::Keepalive => {
                stats.bytes_read.fetch_add(4, Ordering::Relaxed);
                continue;
            }
            _ => {
                stats.bytes_read.fetch_add(4, Ordering::Relaxed);
            }
        }

        if let Err(err) = handler.on_message(handle, msg) {
            handle.close(format!("handler error: {err}"));
            break;
        }
    }

    let reason = handle
        .close_reason()
        .unwrap_or_else(|| "connection closed".to_string());
    handler.on_closed(handle, &reason);
}

async fn run_writer_loop(
    mut write_half: WriteHalf<impl PeerStream>,
    handle: &ConnHandle,
    write_rx: &mut mpsc::Receiver<Message>,
    write_waker: Arc<Notify>,
    upload_limiter: Option<&Limiter>,
    keepalive_timeout: Duration,
) {
    let mut write_buf = BytesMut::with_capacity(64 * 1024);
    let mut last_write = Instant::now();
    let stats = handle.stats();

    while !handle.is_closed() {
        // BACKPORT FIX (WEBTOR_PATCH.md / #1070):
        // Register wakeup notification BEFORE checking channel/queue,
        // so wakeups occurring concurrently are not missed.
        let wakeup = write_waker.clone();
        let notified = wakeup.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        let mut piece_bytes = 0usize;
        let mut chunks = 0i64;

        // Drain pending messages into write_buf
        while let Ok(msg) = write_rx.try_recv() {
            if let Message::Piece { ref piece, .. } = msg {
                piece_bytes += piece.len();
                chunks += 1;
            }
            msg.encode(&mut write_buf);
            if write_buf.len() >= 64 * 1024 {
                break;
            }
        }

        let need_keepalive = write_buf.is_empty() && last_write.elapsed() >= keepalive_timeout;

        if need_keepalive {
            Message::Keepalive.encode(&mut write_buf);
        }

        if write_buf.is_empty() {
            let sleep_dur = keepalive_timeout.saturating_sub(last_write.elapsed());
            tokio::select! {
                _ = notified => {}
                _ = tokio::time::sleep(sleep_dur) => {}
            }
            continue;
        }

        // Apply upload rate limiting if piece data is written
        if piece_bytes > 0 {
            if let Some(limiter) = upload_limiter {
                if let Some(delay) = limiter.reserve_delay(piece_bytes) {
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }

        let bytes_to_write = write_buf.len();
        if let Err(err) = write_half.write_all(&write_buf).await {
            handle.close(format!("write error: {err}"));
            break;
        }
        if let Err(err) = write_half.flush().await {
            handle.close(format!("flush error: {err}"));
            break;
        }

        write_buf.clear();
        last_write = Instant::now();

        stats
            .bytes_written
            .fetch_add(bytes_to_write as i64, Ordering::Relaxed);
        if piece_bytes > 0 {
            stats
                .bytes_written_data
                .fetch_add(piece_bytes as i64, Ordering::Relaxed);
            stats.chunks_written.fetch_add(chunks, Ordering::Relaxed);
        }
    }
}
