//! [`ReaderBackend`] adapter over `wt_core::reader::Reader`.
//!
//! Kept in a sibling module so playback orchestration stays free of wire/conn
//! details. agy-4's `PlaybackHost::open_file_reader` typically returns
//! `Box::new(CoreReaderBackend::from_file(&file))`.

use std::io;
use std::sync::Arc;

use wt_core::reader::Reader;
use wt_core::{File, ReadaheadContext, ReadaheadFn};

use crate::playback::{ReadaheadCallback, ReaderBackend, PLAYBACK_URGENT_BYTES};

/// Blocking wt-core reader with readahead / responsive / cancel hooks.
pub struct CoreReaderBackend {
    inner: Reader,
    stream: Option<(Arc<wt_storage::DocumentTorrentStorage>, wt_core::Torrent)>,
}

impl CoreReaderBackend {
    pub fn from_file(file: &File) -> Self {
        let mut inner = file.new_reader();
        inner.set_readahead(PLAYBACK_URGENT_BYTES);
        Self {
            inner,
            stream: None,
        }
    }

    pub fn from_reader(inner: Reader) -> Self {
        Self {
            inner,
            stream: None,
        }
    }

    pub fn from_file_with_storage(
        file: &File,
        storage: Arc<wt_storage::DocumentTorrentStorage>,
    ) -> Self {
        let mut backend = Self::from_file(file);
        if storage.stream_mode() {
            storage.set_startup_file(file.index());
            backend.stream = Some((
                storage,
                wt_core::Torrent {
                    inner: file.torrent.clone(),
                },
            ));
        }
        backend
    }

    pub fn into_inner(self) -> Reader {
        self.inner
    }
}

impl ReaderBackend for CoreReaderBackend {
    fn read_at(&mut self, buf: &mut [u8], offset: i64) -> io::Result<usize> {
        use std::io::{Read, Seek, SeekFrom};
        self.inner.seek(SeekFrom::Start(offset.max(0) as u64))?;
        if let Some((storage, torrent)) = &self.stream {
            use wt_core::reader::{PieceRange, ReaderHost};
            let abs = self.inner.file_offset + offset;
            let piece_length = torrent
                .info()
                .ok_or_else(|| io::Error::other("metadata unavailable"))?
                .piece_length;
            let piece = (abs / piece_length) as u32;
            torrent.update_piece_completion(piece);
            // Publish the existing reader's demand without asking its verified
            // read path to wait. Storage can already have a delivered body block.
            torrent.inner.set_reader_positions(
                self.inner.id(),
                PieceRange {
                    begin: piece,
                    end: ((abs + self.inner.readahead.max(1) + piece_length - 1) / piece_length)
                        .min(torrent.num_pieces() as i64) as u32,
                },
            );
            loop {
                if self.inner.cancel.as_ref().is_some_and(|c| c.is_cancelled())
                    || torrent.inner.is_closed()
                {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "playback closed or superseded",
                    ));
                }
                let count = buf.len().min((piece_length - abs % piece_length) as usize);
                let (n, ready, result) = storage.read_verified_stream(&mut buf[..count], abs);
                result.map_err(|e| io::Error::other(e.to_string()))?;
                if n > 0 {
                    return Ok(n);
                }
                if let Some(ready) = ready {
                    let generation = ready.generation();
                    ready.wait_timeout(generation, std::time::Duration::from_millis(25));
                } else {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "stream storage returned no bytes",
                    ));
                }
            }
        }
        self.inner.read(buf)
    }

    fn set_readahead(&mut self, bytes: i64) {
        self.inner.set_readahead(bytes);
    }

    fn set_readahead_fn(&mut self, f: Option<ReadaheadCallback>) {
        let mapped: Option<ReadaheadFn> = f.map(|cb| {
            Arc::new(move |ctx: &ReadaheadContext| {
                (cb)(ctx.current_pos, ctx.contiguous_read_start_pos)
            }) as ReadaheadFn
        });
        self.inner.set_readahead_fn(mapped);
    }

    fn set_responsive(&mut self, on: bool) {
        self.inner.set_webtor_responsive(on);
    }

    fn set_cancel(&mut self, cancel: wt_core::CancelToken) {
        self.inner.set_cancel(cancel);
    }

    fn cancel(&mut self) {
        if let Some(cancel) = &self.inner.cancel {
            cancel.cancel();
        }
    }

    fn close(&mut self) {
        self.inner.close();
    }
}
