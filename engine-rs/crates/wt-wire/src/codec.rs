use bytes::{Buf, BytesMut};
use std::io::{self, Error, ErrorKind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::Notify;

use crate::message::Message;

pub const DEFAULT_MAX_MESSAGE_LENGTH: usize = 256 * 1024; // 256 KiB
pub const WRITE_BUFFER_HIGH_WATER_LEN: usize = 256 * 1024;

/// Decode a message from a byte buffer. If a complete message is present,
/// it is removed from `src` and returned. If incomplete, returns `Ok(None)`.
pub fn decode_message(src: &mut BytesMut, max_len: usize) -> io::Result<Option<Message>> {
    if src.len() < 4 {
        return Ok(None);
    }

    let length = u32::from_be_bytes([src[0], src[1], src[2], src[3]]) as usize;
    if length > max_len {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("message too long: length {length} > max {max_len}"),
        ));
    }

    if src.len() < 4 + length {
        // Reserve space if needed
        src.reserve(4 + length - src.len());
        return Ok(None);
    }

    // Advance past length prefix
    src.advance(4);
    let payload = src.split_to(length).freeze();
    let msg = Message::decode_payload(payload)?;
    Ok(Some(msg))
}

/// A framed message reader over an `AsyncRead` stream.
pub struct FramedReader<R> {
    reader: R,
    buffer: BytesMut,
    max_message_length: usize,
}

impl<R: AsyncRead + Unpin> FramedReader<R> {
    pub fn new(reader: R) -> Self {
        Self::with_max_length(reader, DEFAULT_MAX_MESSAGE_LENGTH)
    }

    pub fn with_max_length(reader: R, max_len: usize) -> Self {
        Self {
            reader,
            buffer: BytesMut::with_capacity(32 * 1024),
            max_message_length: max_len,
        }
    }

    /// Read the next message from the stream.
    /// Returns `None` on clean EOF at message boundary.
    pub async fn read_msg(&mut self) -> io::Result<Option<Message>> {
        loop {
            if let Some(msg) = decode_message(&mut self.buffer, self.max_message_length)? {
                return Ok(Some(msg));
            }

            let n = self.reader.read_buf(&mut self.buffer).await?;
            if n == 0 {
                if self.buffer.is_empty() {
                    return Ok(None);
                } else {
                    return Err(Error::new(
                        ErrorKind::UnexpectedEof,
                        format!(
                            "incomplete message of {} bytes remaining at EOF",
                            self.buffer.len()
                        ),
                    ));
                }
            }
        }
    }
}

/// A framed message writer over an `AsyncWrite` stream.
pub struct FramedWriter<W> {
    writer: W,
    buffer: BytesMut,
}

impl<W: AsyncWrite + Unpin> FramedWriter<W> {
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            buffer: BytesMut::with_capacity(32 * 1024),
        }
    }

    pub fn write_msg(&mut self, msg: &Message) {
        msg.encode(&mut self.buffer);
    }

    pub fn buffer_len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_buffer_full(&self) -> bool {
        self.buffer.len() >= WRITE_BUFFER_HIGH_WATER_LEN
    }

    pub async fn flush(&mut self) -> io::Result<()> {
        while !self.buffer.is_empty() {
            let n = self.writer.write(&self.buffer).await?;
            if n == 0 {
                return Err(Error::new(
                    ErrorKind::WriteZero,
                    "failed to write buffered message data",
                ));
            }
            self.buffer.advance(n);
        }
        self.writer.flush().await
    }
}

/// Stats recorded by the message writer.
#[derive(Debug, Default, Clone)]
pub struct MsgWriterStats {
    pub total_write_duration: Duration,
    pub total_bytes_written: u64,
    pub total_data_bytes_written: u64,
    pub data_upload_rate: f64,
}

/// A peer connection message writer that implements Go's `peerConnMsgWriter`
/// semantics with write buffer swapping, wakeup notification, and metrics.
pub struct PeerConnMsgWriter {
    write_buffer: BytesMut,
    piece_data_bytes: usize,
    wakeup: Arc<Notify>,
    closed: Arc<AtomicBool>,
    stats: parking_lot::Mutex<MsgWriterStats>,
}

impl PeerConnMsgWriter {
    pub fn new(wakeup: Arc<Notify>, closed: Arc<AtomicBool>) -> Self {
        Self {
            write_buffer: BytesMut::with_capacity(64 * 1024),
            piece_data_bytes: 0,
            wakeup,
            closed,
            stats: parking_lot::Mutex::new(MsgWriterStats::default()),
        }
    }

    pub fn write_msg(&mut self, msg: &Message) -> bool {
        if let Message::Piece { piece, .. } = msg {
            self.piece_data_bytes += piece.len();
        }
        msg.encode(&mut self.write_buffer);
        self.wakeup.notify_waiters();
        !self.is_buffer_full()
    }

    pub fn is_buffer_full(&self) -> bool {
        self.write_buffer.len() >= WRITE_BUFFER_HIGH_WATER_LEN
    }

    pub fn buffer_len(&self) -> usize {
        self.write_buffer.len()
    }

    pub fn stats(&self) -> MsgWriterStats {
        self.stats.lock().clone()
    }

    /// Run the message writer loop over the underlying stream.
    /// Emulates Go's `run(keepAliveTimeout)`.
    pub async fn run<W, F, K>(
        &mut self,
        mut writer: W,
        keep_alive_timeout: Duration,
        mut fill_write_buffer: F,
        mut keep_alive: K,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
        F: FnMut(&mut Self),
        K: FnMut() -> bool,
    {
        let mut last_write = Instant::now();
        let mut front_buf = BytesMut::with_capacity(64 * 1024);

        while !self.closed.load(Ordering::Relaxed) {
            // BACKPORT FIX (WEBTOR_PATCH.md / #1070):
            // Register wakeup notification BEFORE calling fill_write_buffer,
            // so any wakeup triggered during fill_write_buffer is observed
            // rather than dropped.
            let wakeup = self.wakeup.clone();
            let notified = wakeup.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            fill_write_buffer(self);

            let need_keepalive = self.write_buffer.is_empty()
                && last_write.elapsed() >= keep_alive_timeout
                && keep_alive();

            if need_keepalive {
                Message::Keepalive.encode(&mut self.write_buffer);
            }

            if self.write_buffer.is_empty() {
                let sleep_duration = keep_alive_timeout.saturating_sub(last_write.elapsed());
                tokio::select! {
                    _ = notified => {}
                    _ = tokio::time::sleep(sleep_duration) => {}
                }
                continue;
            }

            // Flip buffers
            std::mem::swap(&mut front_buf, &mut self.write_buffer);
            let piece_bytes = self.piece_data_bytes;
            self.piece_data_bytes = 0;

            let start_write = Instant::now();
            let starting_len = front_buf.len() as u64;

            while !front_buf.is_empty() {
                let n = writer.write(&front_buf).await?;
                if n == 0 {
                    return Err(Error::new(ErrorKind::WriteZero, "zero bytes written"));
                }
                front_buf.advance(n);
            }
            writer.flush().await?;

            let write_dur = start_write.elapsed();
            {
                let mut stats = self.stats.lock();
                stats.total_write_duration += write_dur;
                stats.total_bytes_written += starting_len;
                stats.total_data_bytes_written += piece_bytes as u64;
                let secs = write_dur.as_secs_f64();
                if secs > 0.0 {
                    stats.data_upload_rate = piece_bytes as f64 / secs;
                }
            }

            last_write = Instant::now();
        }

        Ok(())
    }
}
