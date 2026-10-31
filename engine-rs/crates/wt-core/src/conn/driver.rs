//! Connection I/O driver executing reader and writer event loops.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::BytesMut;
use tokio::io::{AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::{mpsc, Notify};
use wt_wire::codec::FramedReader;
use wt_wire::handshake::PeerExtensionBits;
use wt_wire::message::Message;

use crate::conn::handle::{ConnHandle, ConnHandler};
use crate::ratelimit::Limiter;
use crate::PeerStream;

/// Default keepalive ping interval (Go client: 120s).
pub const DEFAULT_KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(120);

/// Default read inactivity deadline (Go client: 150s).
pub const DEFAULT_READ_DEADLINE: Duration = Duration::from_secs(150);

/// The writer sends up to this many bytes per socket write.
const WRITE_BATCH_BYTES: usize = 64 * 1024;

/// Peer requests served between yields: one writer batch of 16 KiB blocks.
const REQUESTS_PER_YIELD: usize = WRITE_BATCH_BYTES / (16 * 1024);

/// `wake_interval` reads peer and torrent state; not worth doing per message.
const WAKE_INTERVAL_REFRESH: Duration = Duration::from_millis(50);

/// Least time between wakes taken ahead of the socket: enough for a seek or a
/// cancel, not a full on_wake per message on a busy peer.
const WAKE_AHEAD_GAP: Duration = Duration::from_millis(5);

/// Spawns the driver tasks (reader and writer) for a live connection.
///
/// Returns the join handles of the reader and writer tasks.
#[allow(clippy::too_many_arguments)]
pub fn spawn_connection_driver<H, S>(
    stream: S,
    mut handler: H,
    handle: ConnHandle,
    mut write_rx: mpsc::UnboundedReceiver<Message>,
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

/// Reserve `n` bytes in burst-sized parts. A limit below one block made the
/// whole reservation unsatisfiable and was silently skipped; Go's limited
/// reader waits in burst-sized parts instead.
async fn wait_for_tokens(limiter: &Limiter, n: usize) {
    let mut left = n;
    while left > 0 {
        let part = left.min(limiter.burst().max(1));
        match limiter.reserve_delay(part) {
            Some(delay) if !delay.is_zero() => tokio::time::sleep(delay).await,
            Some(_) => {}
            None => return,
        }
        left -= part;
    }
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
    let mut deadline = tokio::time::Instant::now() + read_deadline;
    let mut requests_since_yield = 0usize;
    let mut interval = None;
    let mut interval_at: Option<Instant> = None;
    let mut last_wake = Instant::now();

    while !handle.is_closed() {
        if interval_at.is_none_or(|at| at.elapsed() >= WAKE_INTERVAL_REFRESH) {
            interval = handler
                .wake_interval()
                .map(|d| d.max(Duration::from_millis(1)));
            interval_at = Some(Instant::now());
        }
        // The timer below restarts with every message, so a peer that keeps
        // sending would otherwise never reach it.
        if interval.is_some_and(|d| last_wake.elapsed() >= d) {
            last_wake = Instant::now();
            handle.take_wake_pending();
            handler.on_wake(handle);
        } else if last_wake.elapsed() >= WAKE_AHEAD_GAP && handle.take_wake_pending() {
            last_wake = Instant::now();
            handler.on_wake(handle);
        }
        // Prefer reading peer messages over wake timers. With a large pipeline the
        // outstanding-aware wake fires every 100ms; without `biased`, select can
        // starve Piece reads and stall the transfer for tens of seconds.
        let filled = tokio::select! {
            biased;
            result = reader.fill_frame() => result,
            _ = handle.read_waker().notified() => {
                if !handle.is_closed() {
                    handle.take_wake_pending();
                    last_wake = Instant::now();
                    handler.on_wake(handle);
                }
                continue;
            }
            _ = async { match interval { Some(d) => tokio::time::sleep(d).await, None => std::future::pending().await } } => {
                last_wake = Instant::now();
                handler.on_wake(handle);
                continue;
            }
            _ = tokio::time::sleep_until(deadline) => {
                handle.close("read deadline exceeded (keepalive timeout)");
                break;
            }
        };
        match filled {
            Ok(true) => {}
            Ok(false) => {
                handle.close("connection closed by peer (EOF)");
                break;
            }
            Err(err) => {
                handle.close(format!("frame decode error: {err}"));
                break;
            }
        }
        deadline = tokio::time::Instant::now() + read_deadline;

        if let Some(len) = reader.buffered_piece().map(|(_, _, data)| data.len()) {
            stats.bytes_read.fetch_add(len as i64, Ordering::Relaxed);
            stats.bytes_read_data.fetch_add(len as i64, Ordering::Relaxed);
            stats.chunks_read.fetch_add(1, Ordering::Relaxed);
            if let Some(limiter) = download_limiter {
                wait_for_tokens(limiter, len).await;
            }
            let result = match reader.buffered_piece() {
                Some((index, begin, data)) => handler.on_piece(handle, index, begin, data),
                None => Ok(()),
            };
            reader.consume_frame();
            if let Err(err) = result {
                handle.close(format!("handler error: {err}"));
                break;
            }
            continue;
        }

        let msg = match reader.take_buffered() {
            Ok(Some(msg)) => msg,
            Ok(None) => continue,
            Err(err) => {
                handle.close(format!("frame decode error: {err}"));
                break;
            }
        };

        match msg {
            Message::Keepalive => {
                stats.bytes_read.fetch_add(4, Ordering::Relaxed);
                continue;
            }
            _ => {
                stats.bytes_read.fetch_add(4, Ordering::Relaxed);
            }
        }

        let served = matches!(msg, Message::Request { .. });
        if let Err(err) = handler.on_message(handle, msg) {
            handle.close(format!("handler error: {err}"));
            break;
        }
        // A request burst arrives in one socket read, so serving it never
        // waits on the socket. The writer this task wakes sits in this
        // worker's LIFO slot, which other workers cannot steal, so it sent
        // nothing until the whole burst was read from storage. Yield after
        // each writer batch so reads from storage and socket writes overlap.
        if served {
            requests_since_yield += 1;
            if requests_since_yield >= REQUESTS_PER_YIELD {
                requests_since_yield = 0;
                tokio::task::yield_now().await;
            }
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
    write_rx: &mut mpsc::UnboundedReceiver<Message>,
    write_waker: Arc<Notify>,
    upload_limiter: Option<&Limiter>,
    keepalive_timeout: Duration,
) {
    let mut write_buf = BytesMut::with_capacity(WRITE_BATCH_BYTES);
    let mut last_write = Instant::now();
    let stats = handle.stats();
    // Piece bytes in write_buf. A partial batch goes round the loop again to
    // collect more messages, so these must not reset until it is written:
    // they did, and uploads went uncounted and past the upload limit.
    let mut piece_bytes = 0usize;
    let mut chunks = 0i64;

    while !handle.is_closed() {
        // BACKPORT FIX (WEBTOR_PATCH.md / #1070):
        // Register wakeup notification BEFORE checking channel/queue,
        // so wakeups occurring concurrently are not missed.
        let wakeup = write_waker.clone();
        let notified = wakeup.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        // Drain pending messages into write_buf
        let mut drained = 0usize;
        while let Ok(msg) = write_rx.try_recv() {
            drained += 1;
            if let Message::Piece { index, begin, ref piece } = msg {
                // Go onPeerSentCancel: an unsent cancelled block is not
                // uploaded; Fast peers get a Reject instead (BEP 6).
                if handle.take_peer_cancel(index, begin, piece.len() as u32) {
                    if PeerExtensionBits(handle.peer_extension_bits()).supports_fast() {
                        Message::Reject { index, begin, length: piece.len() as u32 }
                            .encode(&mut write_buf);
                    }
                    continue;
                }
                piece_bytes += piece.len();
                chunks += 1;
            }
            msg.encode(&mut write_buf);
            if write_buf.len() >= WRITE_BATCH_BYTES {
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

        // The producer task notifies once per message. Flushing as soon as the
        // channel is empty splits a request burst into thousands of tiny
        // writes (measured ~2 requests per write). Hold a partial batch while
        // messages keep arriving, and write it once a pass drains nothing.
        // The hold yields rather than sleeps: tokio timers tick in whole
        // milliseconds, so the 100 µs sleep here held every partial batch,
        // including every batch of requests, for 1-2 ms.
        if write_buf.len() < WRITE_BATCH_BYTES && drained > 0 {
            tokio::task::yield_now().await;
            continue;
        }

        // Apply upload rate limiting if piece data is written
        if piece_bytes > 0 {
            if let Some(limiter) = upload_limiter {
                wait_for_tokens(limiter, piece_bytes).await;
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
        piece_bytes = 0;
        chunks = 0;
    }
}
