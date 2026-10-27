//! Deterministic torrent fixtures built with wt-meta (SHA-1 piece hashes).

use rand::{RngCore, SeedableRng};
use sha1::{Digest, Sha1};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use wt_meta::bencode::{self, Value};
use wt_meta::{encode_info, encode_metainfo, info_hash_v1, FileInfo, Info, MetaInfo};
use wt_types::InfoHash;

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct Fixture {
    pub name: String,
    pub piece_length: i64,
    pub info_hash: InfoHash,
    pub info_bytes: Vec<u8>,
    pub torrent_bytes: Vec<u8>,
    pub files: Vec<FixtureFile>,
    pub total_length: i64,
    pub num_pieces: u32,
}

#[derive(Clone, Debug)]
pub struct FixtureFile {
    pub path: Vec<String>,
    pub length: i64,
    pub data_path: PathBuf,
}

#[derive(Clone, Copy, Debug)]
pub struct Scale {
    pub video_bytes: u64,
    pub seek_bytes: u64,
    pub piece_length: i64,
}

impl Scale {
    pub fn from_env() -> Self {
        match std::env::var("WT_PARITY_SCALE").as_deref() {
            // Keeps fixtures plus one active leecher below the worker's
            // 256 MiB disk budget, while seeks exceed the 16 MiB RAM cache.
            Ok("bounded") => Self {
                video_bytes: 64 << 20,
                seek_bytes: 32 << 20,
                piece_length: 256 << 10,
            },
            Ok("small") => Self {
                video_bytes: 8 << 20,
                seek_bytes: 4 << 20,
                piece_length: 256 << 10,
            },
            Ok("tiny") => Self {
                video_bytes: 1 << 20,
                seek_bytes: 1 << 20,
                piece_length: 256 << 10,
            },
            _ => Self {
                // Task default: 256 MiB video-like, 256 KiB pieces (seek_search style).
                video_bytes: 256 << 20,
                seek_bytes: 32 << 20,
                piece_length: 256 << 10,
            },
        }
    }
}

pub fn ensure_fixtures(root: &Path, scale: Scale) -> Result<(Fixture, Fixture), String> {
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let video = build_single_file(
        root,
        "parity-video.bin",
        scale.video_bytes,
        scale.piece_length,
        0xC0FFEE01,
    )?;
    let multifile = build_multi_file(root, scale.piece_length, 0xC0FFEE02)?;
    // Seek fixture reuses a medium single-file torrent.
    let _seek = build_single_file(
        root,
        "parity-seek.bin",
        scale.seek_bytes,
        scale.piece_length,
        0xC0FFEE03,
    )?;
    Ok((video, multifile))
}

pub fn load_seek_fixture(root: &Path, scale: Scale) -> Result<Fixture, String> {
    build_single_file(
        root,
        "parity-seek.bin",
        scale.seek_bytes,
        scale.piece_length,
        0xC0FFEE03,
    )
}

fn build_single_file(
    root: &Path,
    name: &str,
    length: u64,
    piece_length: i64,
    seed: u64,
) -> Result<Fixture, String> {
    let dir = root.join(name.trim_end_matches(".bin"));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let data_path = dir.join(name);
    let torrent_path = dir.join(format!("{name}.torrent"));
    if data_path.exists() && torrent_path.exists() {
        return load_existing(&data_path, &torrent_path, name);
    }
    write_deterministic_file(&data_path, length, seed)?;
    let data = fs::read(&data_path).map_err(|e| e.to_string())?;
    let pieces = hash_pieces(&data, piece_length as usize);
    let info = Info {
        name: name.to_string(),
        piece_length,
        pieces,
        files: vec![FileInfo {
            path: vec![],
            length: length as i64,
            torrent_offset: 0,
            attr: String::new(),
            pieces_root: None,
        }],
        single_file: true,
        private: Some(true),
        source: "wt-parity".into(),
        meta_version: 0,
        file_tree_raw: None,
    };
    finish_fixture(
        dir,
        name,
        info,
        vec![FixtureFile {
            path: vec![],
            length: length as i64,
            data_path,
        }],
    )
}

fn build_multi_file(root: &Path, piece_length: i64, seed: u64) -> Result<Fixture, String> {
    let name = "parity-multi";
    let dir = root.join(name);
    fs::create_dir_all(dir.join("a")).map_err(|e| e.to_string())?;
    fs::create_dir_all(dir.join("b")).map_err(|e| e.to_string())?;
    let files_spec = [
        ("a/one.bin", 512 * 1024u64),
        ("a/two.bin", 1024 * 1024),
        ("b/three.bin", 768 * 1024),
    ];
    let torrent_path = dir.join(format!("{name}.torrent"));
    let mut fixture_files = Vec::new();
    let mut concat = Vec::new();
    let mut offset = 0i64;
    for (i, (rel, len)) in files_spec.iter().enumerate() {
        let path = dir.join(rel);
        if !path.exists() {
            write_deterministic_file(&path, *len, seed + i as u64)?;
        }
        let data = fs::read(&path).map_err(|e| e.to_string())?;
        concat.extend_from_slice(&data);
        let components: Vec<String> = rel.split('/').map(|s| s.to_string()).collect();
        fixture_files.push(FixtureFile {
            path: components.clone(),
            length: *len as i64,
            data_path: path,
        });
        offset += *len as i64;
    }
    let _ = offset;
    if torrent_path.exists() {
        return load_existing(&fixture_files[0].data_path, &torrent_path, name);
    }
    let pieces = hash_pieces(&concat, piece_length as usize);
    let mut files = Vec::new();
    let mut off = 0i64;
    for ff in &fixture_files {
        files.push(FileInfo {
            path: ff.path.clone(),
            length: ff.length,
            torrent_offset: off,
            attr: String::new(),
            pieces_root: None,
        });
        off += ff.length;
    }
    let info = Info {
        name: name.to_string(),
        piece_length,
        pieces,
        files,
        single_file: false,
        private: Some(true),
        source: "wt-parity".into(),
        meta_version: 0,
        file_tree_raw: None,
    };
    finish_fixture(dir, name, info, fixture_files)
}

fn finish_fixture(
    dir: PathBuf,
    name: &str,
    info: Info,
    files: Vec<FixtureFile>,
) -> Result<Fixture, String> {
    let info_bytes = encode_info(&info).map_err(|e| e.to_string())?;
    let ih = info_hash_v1(&info_bytes);
    let mi = MetaInfo {
        info_bytes: info_bytes.clone(),
        announce: String::new(),
        announce_list: vec![],
        nodes: vec![],
        creation_date: 0,
        comment: "wt-parity fixture".into(),
        created_by: "wt-parity".into(),
        encoding: String::new(),
        url_list: vec![],
        piece_layers: vec![],
    };
    let torrent_bytes = encode_metainfo(&mi).map_err(|e| e.to_string())?;
    let torrent_path = dir.join(format!("{name}.torrent"));
    fs::write(&torrent_path, &torrent_bytes).map_err(|e| e.to_string())?;
    // Also write a sidecar JSON for quick reload.
    let meta = serde_json::json!({
        "name": name,
        "infoHash": ih.to_hex(),
        "pieceLength": info.piece_length,
        "totalLength": info.total_length(),
        "numPieces": info.num_pieces(),
    });
    fs::write(dir.join("fixture.json"), meta.to_string()).ok();
    Ok(Fixture {
        name: name.to_string(),
        piece_length: info.piece_length,
        info_hash: ih,
        info_bytes,
        torrent_bytes,
        total_length: info.total_length(),
        num_pieces: info.num_pieces(),
        files,
    })
}

fn load_existing(data_path: &Path, torrent_path: &Path, name: &str) -> Result<Fixture, String> {
    let torrent_bytes = fs::read(torrent_path).map_err(|e| e.to_string())?;
    let mi = wt_meta::parse_metainfo(&torrent_bytes).map_err(|e| e.to_string())?;
    let info = wt_meta::parse_info(&mi.info_bytes).map_err(|e| e.to_string())?;
    let ih = info_hash_v1(&mi.info_bytes);
    let mut files = Vec::new();
    for fi in &info.files {
        let rel = if fi.path.is_empty() {
            PathBuf::from(&info.name)
        } else {
            fi.path.iter().collect()
        };
        // For single-file the data_path IS the file.
        let dp = if info.single_file {
            data_path.to_path_buf()
        } else {
            data_path
                .parent()
                .and_then(|p| p.parent())
                .unwrap_or(Path::new("."))
                .join(name)
                .join(&rel)
        };
        files.push(FixtureFile {
            path: fi.path.clone(),
            length: fi.length,
            data_path: dp,
        });
    }
    Ok(Fixture {
        name: name.to_string(),
        piece_length: info.piece_length,
        info_hash: ih,
        info_bytes: mi.info_bytes,
        torrent_bytes,
        total_length: info.total_length(),
        num_pieces: info.num_pieces(),
        files,
    })
}

fn write_deterministic_file(path: &Path, length: u64, seed: u64) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let mut f = fs::File::create(path).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut left = length;
    while left > 0 {
        let n = (left as usize).min(buf.len());
        rng.fill_bytes(&mut buf[..n]);
        // Embed a simple "video-like" header pattern every megabyte.
        if left == length {
            buf[..4].copy_from_slice(b"PARI");
        }
        f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        left -= n as u64;
    }
    Ok(())
}

fn hash_pieces(data: &[u8], piece_length: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < data.len() {
        let end = (start + piece_length).min(data.len());
        let digest = Sha1::digest(&data[start..end]);
        out.extend_from_slice(&digest);
        start = end;
    }
    out
}

/// Prepare a seeder download dir: write engine-owned `.boundary` parts and a
/// matching `.checkpoints` file so configure restores completions.
pub fn prepare_seeder_store(dir: &Path, fixture: &Fixture) -> Result<(), String> {
    let hash = fixture.info_hash.to_hex();
    let boundary = dir.join(".boundary").join(&hash);
    fs::create_dir_all(&boundary).map_err(|e| e.to_string())?;
    let mut identities = Vec::new();
    for (i, ff) in fixture.files.iter().enumerate() {
        let part = boundary.join(format!("{i}.part"));
        fs::hard_link(&ff.data_path, &part)
            .map_err(|e| format!("hard link {}: {e}", ff.data_path.display()))?;
        // Ensure exact length.
        let meta = fs::metadata(&part).map_err(|e| e.to_string())?;
        if meta.len() != ff.length as u64 {
            return Err(format!("part length {} != {}", meta.len(), ff.length));
        }
        identities.push(file_identity(&part, ff, false)?);
    }
    write_checkpoint(dir, fixture, &identities)?;
    Ok(())
}

#[derive(Clone, Debug)]
struct FileIdentity {
    external: bool,
    dev: u64,
    ino: u64,
    length: i64,
    mod_nano: i64,
    path: String,
}

fn file_identity(part: &Path, ff: &FixtureFile, external: bool) -> Result<FileIdentity, String> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::metadata(part).map_err(|e| e.to_string())?;
    let path = if ff.path.is_empty() {
        String::new()
    } else {
        ff.path.join("/")
    };
    Ok(FileIdentity {
        external,
        dev: meta.dev(),
        ino: meta.ino(),
        length: ff.length,
        mod_nano: meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0),
        path,
    })
}

fn write_checkpoint(
    dir: &Path,
    fixture: &Fixture,
    identities: &[FileIdentity],
) -> Result<(), String> {
    let chk_dir = dir.join(".checkpoints");
    fs::create_dir_all(&chk_dir).map_err(|e| e.to_string())?;
    let path = chk_dir.join(format!("{}.chk", fixture.info_hash.to_hex()));
    let mut buf = Vec::new();
    const MAGIC: u32 = 0x5743_5031;
    buf.extend_from_slice(&MAGIC.to_be_bytes());
    buf.extend_from_slice(&1u32.to_be_bytes());
    buf.extend_from_slice(&fixture.info_hash.0);
    buf.extend_from_slice(&(fixture.piece_length as u64).to_be_bytes());
    buf.extend_from_slice(&fixture.num_pieces.to_be_bytes());
    buf.extend_from_slice(&(identities.len() as u32).to_be_bytes());
    for id in identities {
        buf.push(u8::from(id.external));
        buf.extend_from_slice(&id.dev.to_be_bytes());
        buf.extend_from_slice(&id.ino.to_be_bytes());
        buf.extend_from_slice(&(id.length as u64).to_be_bytes());
        buf.extend_from_slice(&(id.mod_nano as u64).to_be_bytes());
        let pb = id.path.as_bytes();
        buf.extend_from_slice(&(pb.len() as u32).to_be_bytes());
        buf.extend_from_slice(pb);
    }
    let nbytes = (fixture.num_pieces as usize).div_ceil(8);
    let mut bits = vec![0u8; nbytes];
    for i in 0..fixture.num_pieces as usize {
        bits[i / 8] |= 1 << (i % 8);
    }
    buf.extend_from_slice(&bits);
    let tmp = path.with_extension("chk.tmp");
    fs::write(&tmp, &buf).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}

/// Build a private magnet with explicit `x.pe` peers (no public trackers).
pub fn magnet_with_peers(info_hash: &InfoHash, display: &str, peers: &[(String, u16)]) -> String {
    let mut m = wt_types::Magnet {
        info_hash: Some(*info_hash),
        display_name: display.to_string(),
        ..Default::default()
    };
    for (host, port) in peers {
        m.params.push(("x.pe".into(), format!("{host}:{port}")));
    }
    wt_meta::magnet_uri(&m)
}

/// Encode torrent as base64 for /add torrentData.
pub fn torrent_b64(fixture: &Fixture) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(&fixture.torrent_bytes)
}

/// Debug helper: dump info dict keys.
#[allow(dead_code)]
pub fn info_keys(info_bytes: &[u8]) -> Vec<String> {
    let v = bencode::decode(info_bytes).unwrap_or(Value::Dict(BTreeMap::new()));
    v.as_dict()
        .map(|d| {
            d.keys()
                .map(|k| String::from_utf8_lossy(k).into_owned())
                .collect()
        })
        .unwrap_or_default()
}
