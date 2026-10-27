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
}

impl CoreReaderBackend {
    pub fn from_file(file: &File) -> Self {
        let mut inner = file.new_reader();
        inner.set_readahead(PLAYBACK_URGENT_BYTES);
        Self { inner }
    }

    pub fn from_reader(inner: Reader) -> Self {
        Self { inner }
    }

    pub fn into_inner(self) -> Reader {
        self.inner
    }
}

impl ReaderBackend for CoreReaderBackend {
    fn read_at(&mut self, buf: &mut [u8], offset: i64) -> io::Result<usize> {
        use std::io::{Read, Seek, SeekFrom};
        self.inner.seek(SeekFrom::Start(offset.max(0) as u64))?;
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

    fn cancel(&mut self) {
        // CancelToken wiring is host-owned; closing the reader unblocks waits.
    }

    fn close(&mut self) {
        // Reader drop joins; explicit close not required by wt-core API.
    }
}
