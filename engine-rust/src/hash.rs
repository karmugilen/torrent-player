//! Extent hasher and verified pread. Holes are SHA-1'd as zeros without a disk
//! read when SEEK_DATA/SEEK_HOLE work. Callers own fds and buffers.

use sha1::{Digest, Sha1};
use std::io::{Error, ErrorKind};
use std::mem::size_of;
use std::os::unix::io::RawFd;
use std::slice;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FileSpan {
    pub fd: i32,
    pub _pad: i32,
    pub file_offset: i64,
    pub length: i64,
}

const ZERO_CHUNK: [u8; 64 * 1024] = [0; 64 * 1024];

fn write_zeros(hasher: &mut Sha1, mut remaining: u64) {
    while remaining > 0 {
        let n = remaining.min(ZERO_CHUNK.len() as u64) as usize;
        hasher.update(&ZERO_CHUNK[..n]);
        remaining -= n as u64;
    }
}

fn pread_all(fd: RawFd, mut offset: i64, buf: &mut [u8]) -> Result<(), Error> {
    let mut done = 0;
    while done < buf.len() {
        let n = unsafe {
            libc::pread(
                fd,
                buf[done..].as_mut_ptr() as *mut libc::c_void,
                buf.len() - done,
                offset,
            )
        };
        if n < 0 {
            let err = Error::last_os_error();
            if err.kind() == ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if n == 0 {
            return Err(Error::new(ErrorKind::UnexpectedEof, "short pread"));
        }
        done += n as usize;
        offset += n as i64;
    }
    Ok(())
}

fn lseek(fd: RawFd, offset: i64, whence: libc::c_int) -> Result<i64, Error> {
    let pos = unsafe { libc::lseek(fd, offset, whence) };
    if pos < 0 {
        Err(Error::last_os_error())
    } else {
        Ok(pos)
    }
}

/// SEEK_DATA/SEEK_HOLE. Ok(None) means the rest of the span is a hole (ENXIO).
/// Err with InvalidInput means the filesystem does not expose sparse extents.
fn seek_kind(fd: RawFd, offset: i64, whence: libc::c_int) -> Result<Option<i64>, Error> {
    match lseek(fd, offset, whence) {
        Ok(pos) => Ok(Some(pos)),
        Err(err) => {
            if let Some(errno) = err.raw_os_error() {
                if errno == libc::ENXIO {
                    return Ok(None);
                }
                if errno == libc::EINVAL || errno == libc::ENOTSUP {
                    return Err(Error::from(ErrorKind::Unsupported));
                }
            }
            Err(err)
        }
    }
}

fn hash_span_dense(hasher: &mut Sha1, span: FileSpan, saw_data: &mut bool) -> Result<(), Error> {
    let mut buf = [0u8; 64 * 1024];
    let mut pos = 0i64;
    while pos < span.length {
        let n = ((span.length - pos) as usize).min(buf.len());
        pread_all(span.fd, span.file_offset + pos, &mut buf[..n])?;
        if buf[..n].iter().any(|&b| b != 0) {
            *saw_data = true;
        }
        hasher.update(&buf[..n]);
        pos += n as i64;
    }
    Ok(())
}

fn hash_span_sparse(hasher: &mut Sha1, span: FileSpan, saw_data: &mut bool) -> Result<(), Error> {
    let end = span.file_offset + span.length;
    let mut abs = span.file_offset;
    while abs < end {
        let data_pos = match seek_kind(span.fd, abs, libc::SEEK_DATA)? {
            None => {
                write_zeros(hasher, (end - abs) as u64);
                return Ok(());
            }
            Some(pos) => {
                if pos >= end {
                    write_zeros(hasher, (end - abs) as u64);
                    return Ok(());
                }
                pos
            }
        };
        if data_pos > abs {
            write_zeros(hasher, (data_pos - abs) as u64);
            abs = data_pos;
        }
        let hole_pos = match seek_kind(span.fd, abs, libc::SEEK_HOLE)? {
            None => end,
            Some(pos) => pos.min(end),
        };
        if hole_pos <= abs {
            return hash_span_dense(hasher, FileSpan { file_offset: abs, length: end - abs, ..span }, saw_data);
        }
        *saw_data = true;
        let mut remaining = hole_pos - abs;
        let mut buf = [0u8; 64 * 1024];
        while remaining > 0 {
            let n = (remaining as usize).min(buf.len());
            pread_all(span.fd, abs, &mut buf[..n])?;
            hasher.update(&buf[..n]);
            abs += n as i64;
            remaining -= n as i64;
        }
    }
    Ok(())
}

fn hash_extents(spans: &[FileSpan]) -> Result<([u8; 20], bool), Error> {
    let mut hasher = Sha1::new();
    let mut saw_data = false;
    for &span in spans {
        if span.fd < 0 || span.length < 0 || span.file_offset < 0 {
            return Err(Error::new(ErrorKind::InvalidInput, "invalid span"));
        }
        if span.length == 0 {
            continue;
        }
        match hash_span_sparse(&mut hasher, span, &mut saw_data) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::Unsupported => {
                hash_span_dense(&mut hasher, span, &mut saw_data)?;
            }
            Err(err) => return Err(err),
        }
    }
    let digest = hasher.finalize();
    let mut out = [0u8; 20];
    out.copy_from_slice(&digest);
    Ok((out, !saw_data))
}

fn pread_extents(spans: &[FileSpan], buf: &mut [u8]) -> Result<usize, Error> {
    let mut done = 0;
    for &span in spans {
        if span.fd < 0 || span.length < 0 || span.file_offset < 0 {
            return Err(Error::new(ErrorKind::InvalidInput, "invalid span"));
        }
        if span.length == 0 {
            continue;
        }
        let n = span.length as usize;
        if done + n > buf.len() {
            return Err(Error::new(ErrorKind::InvalidInput, "buffer too small"));
        }
        pread_all(span.fd, span.file_offset, &mut buf[done..done + n])?;
        done += n;
    }
    Ok(done)
}

/// # Safety
/// `spans` must be valid for `count`. `out_hash` must point at 20 writable bytes.
#[no_mangle]
pub unsafe extern "C" fn webtor_hash_extents(spans: *const FileSpan, count: usize, out_hash: *mut u8) -> i32 {
    if out_hash.is_null() || (count != 0 && spans.is_null()) || count > isize::MAX as usize / size_of::<FileSpan>() {
        return -1;
    }
    let spans = if count == 0 { &[] } else { slice::from_raw_parts(spans, count) };
    match hash_extents(spans) {
        Ok((digest, all_holes)) => {
            slice::from_raw_parts_mut(out_hash, 20).copy_from_slice(&digest);
            if all_holes {
                1
            } else {
                0
            }
        }
        Err(_) => -1,
    }
}

/// # Safety
/// `spans` and `buf` must be valid for their lengths.
#[no_mangle]
pub unsafe extern "C" fn webtor_pread_extents(
    spans: *const FileSpan,
    count: usize,
    buf: *mut u8,
    len: usize,
) -> i32 {
    if (count != 0 && spans.is_null()) || (len != 0 && buf.is_null()) || count > isize::MAX as usize / size_of::<FileSpan>() {
        return -1;
    }
    let spans = if count == 0 { &[] } else { slice::from_raw_parts(spans, count) };
    let buf = if len == 0 { &mut [] } else { slice::from_raw_parts_mut(buf, len) };
    match pread_extents(spans, buf) {
        Ok(n) if n <= i32::MAX as usize => n as i32,
        _ => -1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha1::{Digest, Sha1};
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::os::unix::io::AsRawFd;

    fn sha1_of(bytes: &[u8]) -> [u8; 20] {
        let mut h = Sha1::new();
        h.update(bytes);
        let d = h.finalize();
        let mut out = [0u8; 20];
        out.copy_from_slice(&d);
        out
    }

    #[test]
    fn dense_span_matches_sha1() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("webtor-hash-{}.dat", std::process::id()));
        let mut f = OpenOptions::new().create(true).write(true).read(true).truncate(true).open(&path).unwrap();
        let data = vec![7u8; 8000];
        f.write_all(&data).unwrap();
        f.flush().unwrap();
        let span = FileSpan { fd: f.as_raw_fd(), _pad: 0, file_offset: 0, length: data.len() as i64 };
        let (got, all_holes) = hash_extents(&[span]).unwrap();
        assert!(!all_holes);
        assert_eq!(got, sha1_of(&data));
        let mut buf = vec![0u8; data.len()];
        assert_eq!(pread_extents(&[span], &mut buf).unwrap(), data.len());
        assert_eq!(buf, data);
        drop(f);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn all_hole_span_matches_zero_hash() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("webtor-hole-{}.dat", std::process::id()));
        let f = OpenOptions::new().create(true).write(true).read(true).truncate(true).open(&path).unwrap();
        f.set_len(16 * 1024).unwrap();
        let span = FileSpan { fd: f.as_raw_fd(), _pad: 0, file_offset: 0, length: 16 * 1024 };
        let (got, all_holes) = hash_extents(&[span]).unwrap();
        assert_eq!(got, sha1_of(&[0u8; 16 * 1024]));
        // tmpfs may allocate; all_holes is best-effort.
        let _ = all_holes;
        drop(f);
        let _ = std::fs::remove_file(path);
    }
}
