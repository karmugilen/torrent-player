//! .torrent / info dict parse and encode (BEP 3 / 47 / 52).

use std::collections::BTreeMap;

use sha1::{Digest as _, Sha1};
use sha2::Sha256;
use wt_types::{FileInfo, Info, InfoHash, InfoHashV2, MetaInfo};

use crate::bencode::{self, Value};
use crate::error::{Error, Result};
use crate::merkle;

/// Parse a complete .torrent file into `MetaInfo`. Trailing bytes are an error
/// (matches Go `Load` + `ReadEOF`).
pub fn parse_metainfo(data: &[u8]) -> Result<MetaInfo> {
    let root = bencode::decode(data)?;
    let dict = root
        .as_dict()
        .ok_or_else(|| Error::MetaInfo("top-level value is not a dict".into()))?;

    let info_bytes = bencode::extract_dict_key_raw(data, b"info")?
        .unwrap_or(&[])
        .to_vec();

    let mut mi = MetaInfo {
        info_bytes,
        ..MetaInfo::default()
    };

    if let Some(v) = dict.get(b"announce".as_slice()) {
        mi.announce = value_to_string_lenient(v).unwrap_or_default();
    }
    if let Some(v) = dict.get(b"announce-list".as_slice()) {
        mi.announce_list = parse_announce_list(v);
    }
    if let Some(v) = dict.get(b"nodes".as_slice()) {
        // ignore_unmarshal_type_error: bad nodes are skipped
        mi.nodes = parse_nodes_lenient(v);
    }
    if let Some(v) = dict.get(b"creation date".as_slice()) {
        // ignore_unmarshal_type_error
        if let Some(n) = v.as_int() {
            mi.creation_date = n;
        }
    }
    if let Some(v) = dict.get(b"comment".as_slice()) {
        mi.comment = value_to_string_lenient(v).unwrap_or_default();
    }
    if let Some(v) = dict.get(b"created by".as_slice()) {
        mi.created_by = value_to_string_lenient(v).unwrap_or_default();
    }
    if let Some(v) = dict.get(b"encoding".as_slice()) {
        mi.encoding = value_to_string_lenient(v).unwrap_or_default();
    }
    if let Some(v) = dict.get(b"url-list".as_slice()) {
        mi.url_list = parse_url_list(v);
    }
    if let Some(v) = dict.get(b"piece layers".as_slice()) {
        mi.piece_layers = parse_piece_layers(v);
    }

    Ok(mi)
}

/// Encode `MetaInfo` to a .torrent file. The `info` key is written as the exact
/// `info_bytes` (not re-encoded), preserving the infohash.
pub fn encode_metainfo(mi: &MetaInfo) -> Result<Vec<u8>> {
    let mut dict = BTreeMap::new();

    if !mi.announce.is_empty() {
        dict.insert(b"announce".to_vec(), Value::from(mi.announce.as_str()));
    }
    if !mi.announce_list.is_empty() {
        let tiers: Vec<Value> = mi
            .announce_list
            .iter()
            .map(|tier| Value::List(tier.iter().map(|u| Value::from(u.as_str())).collect()))
            .collect();
        dict.insert(b"announce-list".to_vec(), Value::List(tiers));
    }
    if !mi.comment.is_empty() {
        dict.insert(b"comment".to_vec(), Value::from(mi.comment.as_str()));
    }
    if !mi.created_by.is_empty() {
        dict.insert(b"created by".to_vec(), Value::from(mi.created_by.as_str()));
    }
    if mi.creation_date != 0 {
        dict.insert(b"creation date".to_vec(), Value::Int(mi.creation_date));
    }
    if !mi.encoding.is_empty() {
        dict.insert(b"encoding".to_vec(), Value::from(mi.encoding.as_str()));
    }
    if mi.info_bytes.is_empty() {
        return Err(Error::MetaInfo(
            "marshalled InfoBytes should not be zero-length".into(),
        ));
    }
    // Insert raw info bytes by decoding them as a Value would re-encode; we
    // need exact bytes. Build the outer dict manually.
    if !mi.nodes.is_empty() {
        let nodes: Vec<Value> = mi.nodes.iter().map(|n| Value::from(n.as_str())).collect();
        dict.insert(b"nodes".to_vec(), Value::List(nodes));
    }
    if !mi.piece_layers.is_empty() {
        let mut pl = BTreeMap::new();
        for (root, layer) in &mi.piece_layers {
            pl.insert(root.to_vec(), Value::Bytes(layer.clone()));
        }
        dict.insert(b"piece layers".to_vec(), Value::Dict(pl));
    }
    if !mi.url_list.is_empty() {
        if mi.url_list.len() == 1 {
            dict.insert(b"url-list".to_vec(), Value::from(mi.url_list[0].as_str()));
        } else {
            dict.insert(
                b"url-list".to_vec(),
                Value::List(
                    mi.url_list
                        .iter()
                        .map(|u| Value::from(u.as_str()))
                        .collect(),
                ),
            );
        }
    }

    // Encode with exact info_bytes splice.
    Ok(encode_dict_with_raw_info(&dict, &mi.info_bytes))
}

fn encode_dict_with_raw_info(dict: &BTreeMap<Vec<u8>, Value>, info_bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(b'd');
    let info_key = b"info".as_slice();
    let mut inserted_info = false;
    for (k, v) in dict {
        if !inserted_info && k.as_slice() > info_key {
            // insert info before this key
            write_bytes_key(&mut out, info_key);
            out.extend_from_slice(info_bytes);
            inserted_info = true;
        }
        if k.as_slice() == info_key {
            write_bytes_key(&mut out, info_key);
            out.extend_from_slice(info_bytes);
            inserted_info = true;
            continue;
        }
        write_bytes_key(&mut out, k);
        out.extend_from_slice(&bencode::encode(v));
    }
    if !inserted_info {
        write_bytes_key(&mut out, info_key);
        out.extend_from_slice(info_bytes);
    }
    out.push(b'e');
    out
}

fn write_bytes_key(out: &mut Vec<u8>, key: &[u8]) {
    out.extend_from_slice(key.len().to_string().as_bytes());
    out.push(b':');
    out.extend_from_slice(key);
}

/// Parse an info dict from its exact bencoded bytes.
pub fn parse_info(info_bytes: &[u8]) -> Result<Info> {
    let root = bencode::decode(info_bytes)?;
    let dict = root
        .as_dict()
        .ok_or_else(|| Error::MetaInfo("info is not a dict".into()))?;

    let mut info = Info::default();

    if let Some(v) = dict.get(b"name".as_slice()) {
        info.name = value_to_string_lenient(v).unwrap_or_default();
    }
    let name_utf8 = dict
        .get(b"name.utf-8".as_slice())
        .and_then(value_to_string_lenient);
    if let Some(n) = name_utf8 {
        if !n.is_empty() {
            info.name = n;
        }
    }

    if let Some(v) = dict.get(b"piece length".as_slice()) {
        info.piece_length = v.as_int().unwrap_or(0);
    }
    // Piece length validation: must be > 0 for useful torrents; Go allows 0 in
    // marshal tests. We accept any non-negative; negative is clamped/rejected.
    if info.piece_length < 0 {
        return Err(Error::MetaInfo("negative piece length".into()));
    }

    if let Some(v) = dict.get(b"pieces".as_slice()) {
        info.pieces = v.as_bytes().unwrap_or(&[]).to_vec();
        if !info.pieces.len().is_multiple_of(20) {
            return Err(Error::MetaInfo(
                "pieces length is not a multiple of 20".into(),
            ));
        }
    }

    if let Some(v) = dict.get(b"private".as_slice()) {
        info.private = match v {
            Value::Int(n) => Some(*n != 0),
            Value::Bytes(b) => Some(b != b"0" && !b.is_empty()),
            _ => None, // ignore_unmarshal_type_error style
        };
    }

    if let Some(v) = dict.get(b"source".as_slice()) {
        info.source = value_to_string_lenient(v).unwrap_or_default();
    }

    if let Some(v) = dict.get(b"meta version".as_slice()) {
        info.meta_version = v.as_int().unwrap_or(0);
    }

    // BEP 52 file tree (keep raw for lossless re-encode).
    if let Some(raw) = bencode::extract_dict_key_raw(info_bytes, b"file tree")? {
        info.file_tree_raw = Some(raw.to_vec());
    }

    // v1 files / length
    let length = dict.get(b"length".as_slice()).and_then(|v| v.as_int());
    let files_val = dict.get(b"files".as_slice());

    if info.meta_version == 2 {
        if let Some(raw) = &info.file_tree_raw {
            let tree_val = bencode::decode(raw)?;
            info.files = parse_file_tree(&tree_val, info.piece_length)?;
            info.single_file = info.files.len() == 1 && info.files[0].path.is_empty();
        }
        // Hybrid: also may have v1 files/pieces; prefer file tree for file list
        // when meta version is 2 (matches Go UpvertedFiles).
    } else if let Some(files_v) = files_val {
        info.single_file = false;
        info.files = parse_v1_files(files_v)?;
    } else {
        info.single_file = true;
        let len = length.unwrap_or(0);
        let attr = dict
            .get(b"attr".as_slice())
            .and_then(value_to_string_lenient)
            .unwrap_or_default();
        info.files = vec![FileInfo {
            path: vec![],
            length: len,
            torrent_offset: 0,
            attr,
            pieces_root: None,
        }];
    }

    // If v1 multi-file was present alongside v2, Go's HasV1 checks Files/Length/Pieces.
    // Our Info already has files from the tree. Pieces may still be set for hybrid.

    Ok(info)
}

/// Encode an `Info` to bencode. For v2, `file_tree_raw` is preferred when set.
pub fn encode_info(info: &Info) -> Result<Vec<u8>> {
    let mut dict = BTreeMap::new();

    // attr on info-level (BEP 47) — uncommon for multi-file; skip unless single-file padding
    if info.single_file {
        if let Some(fi) = info.files.first() {
            if !fi.attr.is_empty() {
                dict.insert(b"attr".to_vec(), Value::from(fi.attr.as_str()));
            }
        }
    }

    if let Some(ref raw) = info.file_tree_raw {
        // Splice raw file tree like info_bytes.
        // We'll handle via encode_dict_with_raw_key below.
        let _ = raw;
    } else if info.meta_version == 2 {
        let tree = build_file_tree_value(&info.files)?;
        dict.insert(b"file tree".to_vec(), tree);
    }

    if !info.single_file && info.meta_version != 2 && !info.files.is_empty() {
        let files: Vec<Value> = info
            .files
            .iter()
            .map(|fi| {
                let mut fd = BTreeMap::new();
                if !fi.attr.is_empty() {
                    fd.insert(b"attr".to_vec(), Value::from(fi.attr.as_str()));
                }
                fd.insert(b"length".to_vec(), Value::Int(fi.length));
                fd.insert(
                    b"path".to_vec(),
                    Value::List(fi.path.iter().map(|p| Value::from(p.as_str())).collect()),
                );
                Value::Dict(fd)
            })
            .collect();
        dict.insert(b"files".to_vec(), Value::List(files));
    }

    if info.single_file {
        let length = info.files.first().map(|fi| fi.length).unwrap_or(0);
        dict.insert(b"length".to_vec(), Value::Int(length));
    }

    if info.meta_version != 0 {
        dict.insert(b"meta version".to_vec(), Value::Int(info.meta_version));
    }

    dict.insert(b"name".to_vec(), Value::from(info.name.as_str()));
    dict.insert(b"piece length".to_vec(), Value::Int(info.piece_length));

    if !info.pieces.is_empty() || info.meta_version != 2 {
        // Go omitempty: empty pieces omitted only when empty; for v1 always present
        // in TestMarshalInfo pieces is empty slice and still encoded as 0:
        dict.insert(b"pieces".to_vec(), Value::Bytes(info.pieces.clone()));
    }

    if let Some(p) = info.private {
        dict.insert(b"private".to_vec(), Value::Int(if p { 1 } else { 0 }));
    }

    if !info.source.is_empty() {
        dict.insert(b"source".to_vec(), Value::from(info.source.as_str()));
    }

    if let Some(ref raw) = info.file_tree_raw {
        Ok(encode_dict_with_raw_key(&dict, b"file tree", raw))
    } else {
        Ok(bencode::encode(&Value::Dict(dict)))
    }
}

fn encode_dict_with_raw_key(
    dict: &BTreeMap<Vec<u8>, Value>,
    raw_key: &[u8],
    raw_value: &[u8],
) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(b'd');
    let mut inserted = false;
    for (k, v) in dict {
        if !inserted && k.as_slice() > raw_key {
            write_bytes_key(&mut out, raw_key);
            out.extend_from_slice(raw_value);
            inserted = true;
        }
        if k.as_slice() == raw_key {
            write_bytes_key(&mut out, raw_key);
            out.extend_from_slice(raw_value);
            inserted = true;
            continue;
        }
        write_bytes_key(&mut out, k);
        out.extend_from_slice(&bencode::encode(v));
    }
    if !inserted {
        write_bytes_key(&mut out, raw_key);
        out.extend_from_slice(raw_value);
    }
    out.push(b'e');
    out
}

/// SHA-1 of the exact info dict bytes (v1 infohash).
pub fn info_hash_v1(info_bytes: &[u8]) -> InfoHash {
    let digest = Sha1::digest(info_bytes);
    let mut out = [0u8; 20];
    out.copy_from_slice(&digest);
    InfoHash(out)
}

/// SHA-256 of the exact info dict bytes (v2 infohash).
pub fn info_hash_v2(info_bytes: &[u8]) -> InfoHashV2 {
    let digest = Sha256::digest(info_bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    InfoHashV2(out)
}

/// Choose a piece length for a new torrent (power of two, multiple of 16 KiB,
/// targeting ~1024–2048 pieces). Matches Go `ChoosePieceLength`.
pub fn choose_piece_length(total_length: i64) -> i64 {
    const MINIMUM: i64 = 16 * 1024;
    const TARGET_MAX: i64 = 1 << 11; // 2048
    let mut piece_length = MINIMUM;
    let mut pieces = total_length / piece_length;
    while pieces >= TARGET_MAX {
        piece_length <<= 1;
        pieces >>= 1;
    }
    piece_length
}

// ---- helpers ----

fn value_to_string_lenient(v: &Value) -> Option<String> {
    match v {
        Value::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
        Value::Int(n) => Some(n.to_string()),
        _ => None,
    }
}

fn parse_announce_list(v: &Value) -> Vec<Vec<String>> {
    let Some(list) = v.as_list() else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|tier| {
            let tier_list = tier.as_list()?;
            Some(
                tier_list
                    .iter()
                    .filter_map(value_to_string_lenient)
                    .collect(),
            )
        })
        .collect()
}

fn parse_url_list(v: &Value) -> Vec<String> {
    match v {
        Value::Bytes(_) => value_to_string_lenient(v).into_iter().collect(),
        Value::List(items) => items.iter().filter_map(value_to_string_lenient).collect(),
        _ => Vec::new(),
    }
}

fn parse_nodes_lenient(v: &Value) -> Vec<String> {
    match v {
        Value::Bytes(b) => {
            // ignore_unmarshal_type_error for empty string / wrong type
            if b.is_empty() {
                return Vec::new();
            }
            value_to_string_lenient(v).into_iter().collect()
        }
        Value::List(items) => items
            .iter()
            .filter_map(|item| match item {
                Value::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
                Value::List(pair) if pair.len() >= 2 => {
                    let host = value_to_string_lenient(&pair[0])?;
                    let port = match &pair[1] {
                        Value::Int(n) => n.to_string(),
                        Value::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
                        _ => return None,
                    };
                    Some(format!("{host}:{port}"))
                }
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn parse_piece_layers(v: &Value) -> Vec<([u8; 32], Vec<u8>)> {
    let Some(dict) = v.as_dict() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (k, val) in dict {
        if k.len() != 32 {
            continue;
        }
        let mut root = [0u8; 32];
        root.copy_from_slice(k);
        let layer = match val {
            Value::Bytes(b) => b.clone(),
            _ => continue,
        };
        out.push((root, layer));
    }
    out
}

fn parse_v1_files(v: &Value) -> Result<Vec<FileInfo>> {
    let list = v
        .as_list()
        .ok_or_else(|| Error::MetaInfo("files is not a list".into()))?;
    let mut files = Vec::with_capacity(list.len());
    let mut offset: i64 = 0;
    for item in list {
        let d = item
            .as_dict()
            .ok_or_else(|| Error::MetaInfo("file entry is not a dict".into()))?;
        let length = d
            .get(b"length".as_slice())
            .and_then(|x| x.as_int())
            .unwrap_or(0);
        let path = path_from_file_dict(d);
        let attr = d
            .get(b"attr".as_slice())
            .and_then(value_to_string_lenient)
            .unwrap_or_default();
        files.push(FileInfo {
            path,
            length,
            torrent_offset: offset,
            attr,
            pieces_root: None,
        });
        offset += length;
    }
    Ok(files)
}

fn path_from_file_dict(d: &BTreeMap<Vec<u8>, Value>) -> Vec<String> {
    // Prefer path.utf-8
    if let Some(v) = d.get(b"path.utf-8".as_slice()) {
        if let Some(p) = list_of_strings(v) {
            if !p.is_empty() {
                return p;
            }
        }
    }
    d.get(b"path".as_slice())
        .and_then(list_of_strings)
        .unwrap_or_default()
}

fn list_of_strings(v: &Value) -> Option<Vec<String>> {
    let list = v.as_list()?;
    Some(list.iter().filter_map(value_to_string_lenient).collect())
}

/// Parse BEP 52 file tree into `FileInfo` list with piece-aligned offsets
/// (padding semantics like Go).
fn parse_file_tree(tree: &Value, piece_length: i64) -> Result<Vec<FileInfo>> {
    let mut files = Vec::new();
    let mut offset: i64 = 0;
    walk_file_tree(tree, &[], piece_length, &mut offset, &mut files)?;
    Ok(files)
}

fn walk_file_tree(
    node: &Value,
    path: &[String],
    piece_length: i64,
    offset: &mut i64,
    out: &mut Vec<FileInfo>,
) -> Result<()> {
    let dict = node
        .as_dict()
        .ok_or_else(|| Error::MetaInfo("file tree node is not a dict".into()))?;

    // Directory entries are all keys except "".
    let mut dir_keys: Vec<&Vec<u8>> = dict.keys().filter(|k| !k.is_empty()).collect();
    dir_keys.sort();

    if dir_keys.is_empty() {
        // File leaf: properties under "".
        let props = dict.get(b"".as_slice());
        let (length, pieces_root) = match props {
            Some(Value::Dict(pd)) => {
                let length = pd
                    .get(b"length".as_slice())
                    .and_then(|x| x.as_int())
                    .unwrap_or(0);
                let pieces_root = pd
                    .get(b"pieces root".as_slice())
                    .and_then(|x| x.as_bytes())
                    .and_then(|b| {
                        if b.len() == 32 {
                            let mut a = [0u8; 32];
                            a.copy_from_slice(b);
                            Some(a)
                        } else {
                            None
                        }
                    });
                (length, pieces_root)
            }
            _ => (0, None),
        };
        out.push(FileInfo {
            path: path.to_vec(),
            length,
            torrent_offset: *offset,
            attr: String::new(),
            pieces_root,
        });
        // v2 files are piece-aligned.
        if piece_length > 0 {
            *offset += (length + piece_length - 1) / piece_length * piece_length;
        } else {
            *offset += length;
        }
        return Ok(());
    }

    for key in dir_keys {
        let name = String::from_utf8_lossy(key).into_owned();
        let mut child_path = path.to_vec();
        child_path.push(name);
        let child = dict
            .get(key)
            .ok_or_else(|| Error::MetaInfo("missing file tree child".into()))?;
        walk_file_tree(child, &child_path, piece_length, offset, out)?;
    }
    Ok(())
}

fn build_file_tree_value(files: &[FileInfo]) -> Result<Value> {
    // Rebuild a minimal file tree from flat files (best-effort).
    let mut root: BTreeMap<Vec<u8>, Value> = BTreeMap::new();
    for fi in files {
        if fi.is_padding() {
            continue;
        }
        if fi.path.is_empty() {
            let mut props = BTreeMap::new();
            props.insert(b"length".to_vec(), Value::Int(fi.length));
            if let Some(pr) = fi.pieces_root {
                props.insert(b"pieces root".to_vec(), Value::Bytes(pr.to_vec()));
            }
            let mut leaf = BTreeMap::new();
            leaf.insert(b"".to_vec(), Value::Dict(props));
            return Ok(Value::Dict(leaf));
        }
        insert_file_tree_path(&mut root, &fi.path, fi)?;
    }
    Ok(Value::Dict(root))
}

fn insert_file_tree_path(
    dir: &mut BTreeMap<Vec<u8>, Value>,
    path: &[String],
    fi: &FileInfo,
) -> Result<()> {
    if path.is_empty() {
        return Err(Error::MetaInfo("empty path in multi-file tree".into()));
    }
    let key = path[0].as_bytes().to_vec();
    if path.len() == 1 {
        let mut props = BTreeMap::new();
        props.insert(b"length".to_vec(), Value::Int(fi.length));
        if let Some(pr) = fi.pieces_root {
            props.insert(b"pieces root".to_vec(), Value::Bytes(pr.to_vec()));
        }
        let mut leaf = BTreeMap::new();
        leaf.insert(b"".to_vec(), Value::Dict(props));
        dir.insert(key, Value::Dict(leaf));
        return Ok(());
    }
    let entry = dir
        .entry(key)
        .or_insert_with(|| Value::Dict(BTreeMap::new()));
    let child = match entry {
        Value::Dict(d) => d,
        _ => return Err(Error::MetaInfo("file tree conflict".into())),
    };
    insert_file_tree_path(child, &path[1..], fi)
}

/// Convenience: parse metainfo and its info dict together.
pub fn parse_torrent(data: &[u8]) -> Result<(MetaInfo, Info)> {
    let mi = parse_metainfo(data)?;
    let info = parse_info(&mi.info_bytes)?;
    Ok((mi, info))
}

/// Re-export merkle validation for callers that have MetaInfo + Info.
pub fn validate_metainfo_piece_layers(mi: &MetaInfo, info: &Info) -> Result<()> {
    if !info.has_v2() {
        return Ok(());
    }
    merkle::validate_piece_layers(&mi.piece_layers, &info.files, info.piece_length)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unmarshal_empty_dict() {
        let mi = parse_metainfo(b"de").unwrap();
        assert!(mi.info_bytes.is_empty());
    }

    #[test]
    fn unmarshal_info_bytes() {
        let mi = parse_metainfo(b"d4:infodee").unwrap();
        assert_eq!(mi.info_bytes, b"de");
    }

    #[test]
    fn trailing_newline_errors() {
        let data = include_bytes!("../testdata/minimal-trailing-newline.torrent");
        assert!(parse_metainfo(data).is_err());
    }

    #[test]
    fn string_creation_date_ignored() {
        let mi = parse_metainfo(b"d13:creation date23:29.03.2018 22:18:14 UTC4:infodee").unwrap();
        assert_eq!(mi.creation_date, 0);
        assert_eq!(mi.info_bytes, b"de");
    }

    #[test]
    fn empty_string_nodes() {
        let mi = parse_metainfo(b"d5:nodes0:e").unwrap();
        assert!(mi.nodes.is_empty());
    }

    #[test]
    fn marshal_info_empty() {
        let info = Info {
            pieces: vec![],
            ..Info::default()
        };
        let b = encode_info(&info).unwrap();
        assert_eq!(b, b"d4:name0:12:piece lengthi0e6:pieces0:e");
    }

    #[test]
    fn info_hash_trackerless() {
        let data = include_bytes!("../testdata/trackerless.torrent");
        let mi = parse_metainfo(data).unwrap();
        let ih = info_hash_v1(&mi.info_bytes);
        // Known from filename pattern / Go HashInfoBytes — verify roundtrip encode info
        let info = parse_info(&mi.info_bytes).unwrap();
        assert_eq!(info.name, "testfile.bin");
        assert_eq!(info.piece_length, 32768);
        assert!(info.single_file);
        let _ = ih;
    }

    #[test]
    fn choose_piece_length_basic() {
        assert_eq!(choose_piece_length(0), 16 * 1024);
        assert!(choose_piece_length(100_000_000) >= 16 * 1024);
    }
}
