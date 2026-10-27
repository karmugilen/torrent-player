use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use wt_types::{StorageError, StorageResult};

pub const CHECKPOINT_MAGIC: u32 = 0x57435031; // "WCP1"
pub const CHECKPOINT_VERSION: u32 = 1;
pub const MAX_CHECKPOINT_PIECES: usize = 512 * 1024;
pub const MAX_CHECKPOINT_FILES: usize = 65_536;
pub const MAX_CHECKPOINT_PATH: usize = 4_096;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FileIdentity {
    pub external: bool,
    pub dev: u64,
    pub ino: u64,
    pub length: i64,
    pub mod_nano: i64,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PieceCheckpoint {
    pub info_hash: [u8; 20],
    pub piece_length: i64,
    pub num_pieces: usize,
    pub files: Vec<FileIdentity>,
    pub complete: Vec<bool>,
}

pub fn checkpoint_path(base_dir: &Path, info_hash_hex: &str) -> PathBuf {
    base_dir
        .join(".checkpoints")
        .join(format!("{}.chk", info_hash_hex))
}

pub fn identity_from_file(
    file: &File,
    external: bool,
    length: i64,
    path: String,
) -> (FileIdentity, bool) {
    let metadata = match file.metadata() {
        Ok(m) => m,
        Err(_) => {
            return (
                FileIdentity {
                    external,
                    length,
                    path,
                    ..Default::default()
                },
                false,
            );
        }
    };
    identity_from_metadata(&metadata, external, length, path)
}

pub fn identity_from_metadata(
    meta: &fs::Metadata,
    external: bool,
    length: i64,
    path: String,
) -> (FileIdentity, bool) {
    let dev = meta.dev();
    let ino = meta.ino();
    let mtime_sec = meta.mtime();
    let mtime_nsec = meta.mtime_nsec();
    let mod_nano = mtime_sec
        .checked_mul(1_000_000_000)
        .and_then(|ns| ns.checked_add(mtime_nsec))
        .unwrap_or(0);

    let id = FileIdentity {
        external,
        dev,
        ino,
        length,
        mod_nano,
        path,
    };
    (id, true)
}

pub fn file_stable(saved: &FileIdentity, current: &FileIdentity) -> bool {
    if saved.dev == 0 && saved.ino == 0 {
        return false;
    }
    if saved.external != current.external
        || saved.length != current.length
        || saved.dev != current.dev
        || saved.ino != current.ino
    {
        return false;
    }
    if !saved.external && saved.path != current.path {
        return false;
    }
    true
}

pub fn file_identity_equal(a: &FileIdentity, b: &FileIdentity) -> bool {
    file_stable(a, b) && a.mod_nano == b.mod_nano
}

pub fn identities_match(saved: &[FileIdentity], current: &[FileIdentity]) -> bool {
    if saved.len() != current.len() {
        return false;
    }
    for i in 0..saved.len() {
        if !file_identity_equal(&saved[i], &current[i]) {
            return false;
        }
    }
    true
}

impl PieceCheckpoint {
    pub fn bitfield_bytes(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.num_pieces.div_ceil(8)];
        for (i, &ok) in self.complete.iter().enumerate() {
            if ok {
                out[i / 8] |= 1 << (i % 8);
            }
        }
        out
    }

    pub fn decode_bitfield(bits: &[u8], n: usize) -> Vec<bool> {
        let mut out = vec![false; n];
        for i in 0..n {
            if (bits[i / 8] & (1 << (i % 8))) != 0 {
                out[i] = true;
            }
        }
        out
    }
}

pub fn write_piece_checkpoint(path: &Path, cp: &PieceCheckpoint) -> StorageResult<()> {
    if cp.num_pieces > MAX_CHECKPOINT_PIECES || cp.complete.len() != cp.num_pieces {
        return Err(StorageError::Other("invalid checkpoint".into()));
    }
    if cp.files.len() > MAX_CHECKPOINT_FILES {
        return Err(StorageError::Other("invalid checkpoint file count".into()));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("chk.tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&tmp)?;

    let mut buf = Vec::with_capacity(40 + cp.files.len() * 40 + cp.num_pieces.div_ceil(8));
    buf.extend_from_slice(&CHECKPOINT_MAGIC.to_be_bytes());
    buf.extend_from_slice(&CHECKPOINT_VERSION.to_be_bytes());
    buf.extend_from_slice(&cp.info_hash);
    buf.extend_from_slice(&(cp.piece_length as u64).to_be_bytes());
    buf.extend_from_slice(&(cp.num_pieces as u32).to_be_bytes());
    buf.extend_from_slice(&(cp.files.len() as u32).to_be_bytes());

    for f in &cp.files {
        buf.push(if f.external { 1 } else { 0 });
        buf.extend_from_slice(&f.dev.to_be_bytes());
        buf.extend_from_slice(&f.ino.to_be_bytes());
        buf.extend_from_slice(&(f.length as u64).to_be_bytes());
        buf.extend_from_slice(&(f.mod_nano as u64).to_be_bytes());
        let path_bytes = f.path.as_bytes();
        buf.extend_from_slice(&(path_bytes.len() as u32).to_be_bytes());
        buf.extend_from_slice(path_bytes);
    }
    buf.extend_from_slice(&cp.bitfield_bytes());

    file.write_all(&buf)?;
    file.sync_all()?;
    drop(file);

    fs::rename(&tmp, path)?;
    Ok(())
}

pub fn read_piece_checkpoint(path: &Path) -> StorageResult<PieceCheckpoint> {
    let mut file = File::open(path)?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)?;

    if data.len() < 40 {
        return Err(StorageError::Other("checkpoint too short".into()));
    }
    let magic = u32::from_be_bytes(data[0..4].try_into().unwrap());
    if magic != CHECKPOINT_MAGIC {
        return Err(StorageError::Other("bad checkpoint magic".into()));
    }
    let version = u32::from_be_bytes(data[4..8].try_into().unwrap());
    if version != CHECKPOINT_VERSION {
        return Err(StorageError::Other("unsupported checkpoint version".into()));
    }
    let mut info_hash = [0u8; 20];
    info_hash.copy_from_slice(&data[8..28]);
    let piece_length = u64::from_be_bytes(data[28..36].try_into().unwrap()) as i64;
    let num_pieces = u32::from_be_bytes(data[36..40].try_into().unwrap()) as usize;

    if num_pieces > MAX_CHECKPOINT_PIECES || piece_length <= 0 {
        return Err(StorageError::Other("invalid checkpoint header".into()));
    }

    let mut off = 40;
    if off + 4 > data.len() {
        return Err(StorageError::Other("truncated checkpoint files".into()));
    }
    let n_files = u32::from_be_bytes(data[off..off + 4].try_into().unwrap()) as usize;
    off += 4;
    if n_files > MAX_CHECKPOINT_FILES {
        return Err(StorageError::Other("invalid checkpoint file count".into()));
    }

    let bit_bytes = num_pieces.div_ceil(8);
    let mut files = Vec::with_capacity(n_files);
    for _ in 0..n_files {
        let need = off + 1 + 8 + 8 + 8 + 8 + 4;
        if need > data.len() {
            return Err(StorageError::Other("truncated file identity".into()));
        }
        let external = data[off] == 1;
        off += 1;
        let dev = u64::from_be_bytes(data[off..off + 8].try_into().unwrap());
        off += 8;
        let ino = u64::from_be_bytes(data[off..off + 8].try_into().unwrap());
        off += 8;
        let length = u64::from_be_bytes(data[off..off + 8].try_into().unwrap()) as i64;
        off += 8;
        let mod_nano = u64::from_be_bytes(data[off..off + 8].try_into().unwrap()) as i64;
        off += 8;
        let path_len = u32::from_be_bytes(data[off..off + 4].try_into().unwrap()) as usize;
        off += 4;
        if path_len > MAX_CHECKPOINT_PATH || off + path_len > data.len() {
            return Err(StorageError::Other("truncated file path".into()));
        }
        let path = String::from_utf8_lossy(&data[off..off + path_len]).into_owned();
        off += path_len;
        files.push(FileIdentity {
            external,
            dev,
            ino,
            length,
            mod_nano,
            path,
        });
    }

    if off + bit_bytes > data.len() {
        return Err(StorageError::Other("truncated bitfield".into()));
    }
    let complete = PieceCheckpoint::decode_bitfield(&data[off..off + bit_bytes], num_pieces);

    Ok(PieceCheckpoint {
        info_hash,
        piece_length,
        num_pieces,
        files,
        complete,
    })
}

pub fn delete_piece_checkpoint(base_dir: &Path, info_hash_hex: &str) -> StorageResult<()> {
    let p = checkpoint_path(base_dir, info_hash_hex);
    match fs::remove_file(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(StorageError::Io(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checkpoint_round_trip() {
        let dir = std::env::temp_dir().join(format!("chk-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test.chk");

        let cp = PieceCheckpoint {
            info_hash: [42u8; 20],
            piece_length: 65536,
            num_pieces: 10,
            files: vec![
                FileIdentity {
                    external: true,
                    dev: 100,
                    ino: 200,
                    length: 12345,
                    mod_nano: 999999,
                    path: "file1.mp4".into(),
                },
                FileIdentity {
                    external: false,
                    dev: 100,
                    ino: 201,
                    length: 54321,
                    mod_nano: 888888,
                    path: "file2.mkv".into(),
                },
            ],
            complete: vec![
                true, false, true, true, false, false, false, true, false, true,
            ],
        };

        write_piece_checkpoint(&path, &cp).unwrap();
        let loaded = read_piece_checkpoint(&path).unwrap();
        assert_eq!(cp, loaded);

        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir_all(&dir);
    }
}
