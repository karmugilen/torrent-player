use std::fs::File;
use std::io::{Error, ErrorKind};
use std::os::unix::io::AsRawFd;
use std::os::unix::io::RawFd;

/// Finds the next data offset at or after `offset`.
/// Returns `Ok(Some(pos))` if data found.
/// Returns `Ok(None)` if no more data (only holes until EOF, i.e. ENXIO).
/// Returns `Err(Unsupported)` if the filesystem doesn't support sparse extents (EINVAL/ENOTSUP).
pub fn seek_data(fd: RawFd, offset: i64) -> Result<Option<i64>, Error> {
    let pos = unsafe { libc::lseek(fd, offset, libc::SEEK_DATA) };
    if pos >= 0 {
        return Ok(Some(pos));
    }
    let err = Error::last_os_error();
    if let Some(code) = err.raw_os_error() {
        if code == libc::ENXIO {
            return Ok(None);
        }
        if code == libc::EINVAL || code == libc::ENOTSUP || code == libc::EOPNOTSUPP {
            return Err(Error::from(ErrorKind::Unsupported));
        }
    }
    Err(err)
}

/// Finds the next hole offset at or after `offset`.
/// Returns `Ok(Some(pos))` if hole found.
/// Returns `Ok(None)` if no more hole (ENXIO).
/// Returns `Err(Unsupported)` if the filesystem doesn't support sparse extents.
pub fn seek_hole(fd: RawFd, offset: i64) -> Result<Option<i64>, Error> {
    let pos = unsafe { libc::lseek(fd, offset, libc::SEEK_HOLE) };
    if pos >= 0 {
        return Ok(Some(pos));
    }
    let err = Error::last_os_error();
    if let Some(code) = err.raw_os_error() {
        if code == libc::ENXIO {
            return Ok(None);
        }
        if code == libc::EINVAL || code == libc::ENOTSUP || code == libc::EOPNOTSUPP {
            return Err(Error::from(ErrorKind::Unsupported));
        }
    }
    Err(err)
}

/// Discards file data in `[offset, offset + length)` while keeping the logical size.
/// Returns `Ok(true)` if punched successfully.
/// Returns `Ok(false)` if filesystem does not support hole punching.
pub fn punch_hole(fd: RawFd, offset: i64, length: i64) -> Result<bool, Error> {
    if length <= 0 {
        return Ok(true);
    }
    let res = unsafe {
        libc::fallocate(
            fd,
            libc::FALLOC_FL_PUNCH_HOLE | libc::FALLOC_FL_KEEP_SIZE,
            offset,
            length,
        )
    };
    if res == 0 {
        return Ok(true);
    }
    let err = Error::last_os_error();
    if let Some(code) = err.raw_os_error() {
        if code == libc::EOPNOTSUPP || code == libc::ENOTSUP || code == libc::EINVAL {
            return Ok(false);
        }
    }
    Err(err)
}

/// Returns the allocated/occupied disk bytes of the file.
pub fn file_occupied_bytes(file: &File) -> Result<i64, Error> {
    let fd = file.as_raw_fd();
    unsafe {
        let mut st: libc::stat = std::mem::zeroed();
        if libc::fstat(fd, &mut st) != 0 {
            return Err(Error::last_os_error());
        }
        Ok((st.st_blocks as i64) * 512)
    }
}
