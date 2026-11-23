//! Platform file helpers shared by storage I/O.
//!
//! Unix (Linux/Android/macOS) keeps the existing fd/`FileExt` paths. Windows
//! uses handle-based positioned I/O and path-only attach (no external fds).

use std::fs::File;
use std::io;

/// External descriptor type for [`crate::DocumentTorrentStorage::attach_descriptors`].
/// Matches Android JNI `int` fds. Desktop Windows rejects `Some(fd)`.
pub type RawFd = i32;

/// Stable id for pending fsync tracking (Unix fd or Windows HANDLE as u64).
pub type SyncKey = u64;

pub fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.read_exact_at(buf, offset)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut done = 0;
        while done < buf.len() {
            let n = file.seek_read(&mut buf[done..], offset + done as u64)?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "failed to fill whole buffer",
                ));
            }
            done += n;
        }
        Ok(())
    }
}

pub fn write_all_at(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.write_all_at(buf, offset)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut buf = buf;
        let mut offset = offset;
        while !buf.is_empty() {
            let n = file.seek_write(buf, offset)?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "failed to write whole buffer",
                ));
            }
            buf = &buf[n..];
            offset += n as u64;
        }
        Ok(())
    }
}

pub fn file_id(file: &File) -> SyncKey {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        file.as_raw_fd() as SyncKey
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        file.as_raw_handle() as usize as SyncKey
    }
}

/// Duplicate an external descriptor into an owned [`File`].
///
/// On Windows external descriptors are unsupported (path-based attach only).
pub fn file_from_external_fd(fd: RawFd) -> io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::io::{FromRawFd, RawFd as UnixRawFd};
        if fd < 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid fd"));
        }
        let dup_fd = unsafe { libc::dup(fd as UnixRawFd) };
        if dup_fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { File::from_raw_fd(dup_fd) })
    }
    #[cfg(windows)]
    {
        let _ = fd;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "external file descriptors are not supported on this platform",
        ))
    }
}

pub fn same_file(a: Option<&File>, b: &File) -> bool {
    let Some(a) = a else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (a.metadata(), b.metadata()) {
            (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
            _ => false,
        }
    }
    #[cfg(windows)]
    {
        file_id(a) == file_id(b)
    }
}

pub fn identity_fields(meta: &std::fs::Metadata) -> (u64, u64, i64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mod_nano = meta
            .mtime()
            .checked_mul(1_000_000_000)
            .and_then(|ns| ns.checked_add(meta.mtime_nsec()))
            .unwrap_or(0);
        (meta.dev(), meta.ino(), mod_nano)
    }
    #[cfg(windows)]
    {
        let mod_nano = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        // Windows has no stable ino/dev in std; length+mtime+path still gate
        // restore via file_stable's zero-dev/ino early-out when unset.
        (0, 0, mod_nano)
    }
}
