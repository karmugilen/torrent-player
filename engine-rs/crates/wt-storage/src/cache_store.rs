use hex;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use wt_types::{PeerInfo, StorageError, StorageResult};

pub const PEER_HINT_TTL: Duration = Duration::from_secs(45 * 60);
pub const PEER_HINT_MAX_PER_TORRENT: usize = 32;
pub const PEER_HINT_MAX_TORRENTS: usize = 64;
pub const DHT_CONTACT_TTL: Duration = Duration::from_secs(24 * 3600);
pub const DHT_CONTACT_MAX_PER_FAMILY: usize = 32;
pub const RESUME_WINDOW_TTL: Duration = Duration::from_secs(2 * 3600);
pub const RESUME_WINDOW_MAX_BYTES: i64 = 4 << 20;
pub const RESUME_WINDOW_TOTAL_MAX: i64 = 16 << 20;
pub const RESUME_WINDOW_VERSION: i32 = 2;
pub const PEER_HINT_MAX_LOAD_BYTES: i64 = 1 << 20;
pub const DHT_CONTACT_MAX_LOAD_BYTES: i64 = 512 << 10;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CacheCategoryView {
    pub id: String,
    pub bytes: i64,
    #[serde(rename = "reclaimableBytes")]
    pub reclaimable_bytes: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CacheStatusResponse {
    pub categories: Vec<CacheCategoryView>,
    #[serde(rename = "memoryBytes")]
    pub memory_bytes: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerHint {
    pub addr: String,
    pub source: String,
    pub transport: String,
    #[serde(rename = "lastSuccess")]
    pub last_success: String,
    pub expires: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct PeerHintFile {
    pub torrents: HashMap<String, Vec<PeerHint>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DhtContact {
    pub addr: String,
    pub family: String,
    #[serde(rename = "lastGood")]
    pub last_good: String,
    pub expires: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct DhtContactFile {
    pub contacts: Vec<DhtContact>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ResumeWindowMeta {
    pub version: i32,
    #[serde(rename = "infoHash")]
    pub info_hash: String,
    #[serde(rename = "fileIndex")]
    pub file_index: usize,
    #[serde(rename = "fileName")]
    pub file_name: String,
    #[serde(rename = "fileLength")]
    pub file_length: i64,
    #[serde(rename = "pieceLength")]
    pub piece_length: i64,
    pub offset: i64,
    pub length: i64,
    pub checksum: String,
    pub generation: u64,
    #[serde(rename = "savedAt")]
    pub saved_at: String,
    pub expires: String,
}

pub fn peer_hints_path(base: &Path) -> PathBuf {
    base.join(".cache").join("peer-hints.json")
}

pub fn dht_contacts_path(base: &Path) -> PathBuf {
    base.join(".cache").join("dht-contacts.json")
}

pub fn resume_window_dir(base: &Path, info_hash: &str) -> PathBuf {
    base.join(".cache").join("resume").join(info_hash)
}

pub fn dir_bytes(path: &Path) -> i64 {
    let mut total = 0i64;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                total += dir_bytes(&p);
            } else if let Ok(meta) = entry.metadata() {
                total += meta.len() as i64;
            }
        }
    }
    total
}

pub fn file_size(path: &Path) -> i64 {
    fs::metadata(path).map(|m| m.len() as i64).unwrap_or(0)
}

pub fn resume_checksum(data: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

pub fn format_time(t: SystemTime) -> String {
    let dur = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    format!("{}.{:09}Z", dur.as_secs(), dur.subsec_nanos())
}

pub fn parse_time(s: &str) -> Option<SystemTime> {
    if s.is_empty() || s == "0001-01-01T00:00:00Z" || s == "0" {
        return Some(UNIX_EPOCH);
    }
    // Attempt standard formats or simple secs.nanos
    if let Some((secs_str, nsecs_str)) = s.trim_end_matches('Z').split_once('.') {
        let secs: u64 = secs_str.parse().ok()?;
        let nsecs: u32 = nsecs_str.parse().ok()?;
        Some(UNIX_EPOCH + Duration::new(secs, nsecs))
    } else if let Ok(secs) = s.trim_end_matches('Z').parse::<u64>() {
        Some(UNIX_EPOCH + Duration::from_secs(secs))
    } else {
        // Fallback: accept as now + 1hr if future or expired
        Some(SystemTime::now())
    }
}

pub fn is_valid_peer_addr(addr_str: &str) -> Option<SocketAddr> {
    let addr: SocketAddr = addr_str.parse().ok()?;
    let ip = addr.ip();
    if addr.port() == 0 || ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() {
        return None;
    }
    Some(addr)
}

pub fn is_hex_info_hash(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}

// ---------------------------------------------------------------- peer hints store

#[derive(Default, Debug)]
pub struct PeerHintStore {
    hints: HashMap<String, Vec<PeerHint>>,
    gen: u64,
}

impl PeerHintStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn remember_peer_hint(
        &mut self,
        base_dir: &Path,
        info_hash: &str,
        peer: &PeerInfo,
        private: bool,
    ) {
        if private {
            return;
        }
        let ip = peer.addr.ip();
        if peer.addr.port() == 0 || ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() {
            return;
        }
        let transport = "tcp".to_string(); // Default to tcp
        let now = SystemTime::now();
        let expires = now + PEER_HINT_TTL;
        let hint = PeerHint {
            addr: peer.addr.to_string(),
            source: peer.source.code().to_string(),
            transport,
            last_success: format_time(now),
            expires: format_time(expires),
        };

        let list = self.hints.entry(info_hash.to_string()).or_default();
        if let Some(existing) = list
            .iter_mut()
            .find(|h| h.addr == hint.addr && h.transport == hint.transport)
        {
            *existing = hint;
        } else {
            list.push(hint);
        }

        // Keep newest first
        list.sort_by(|a, b| b.last_success.cmp(&a.last_success));
        if list.len() > PEER_HINT_MAX_PER_TORRENT {
            list.truncate(PEER_HINT_MAX_PER_TORRENT);
        }

        while self.hints.len() > PEER_HINT_MAX_TORRENTS {
            // Find oldest torrent
            let mut oldest_hash: Option<String> = None;
            let mut oldest_time = String::new();
            for (h, hs) in &self.hints {
                if h == info_hash || hs.is_empty() {
                    continue;
                }
                if oldest_hash.is_none() || hs[0].last_success < oldest_time {
                    oldest_hash = Some(h.clone());
                    oldest_time = hs[0].last_success.clone();
                }
            }
            if let Some(h) = oldest_hash {
                self.hints.remove(&h);
            } else {
                break;
            }
        }

        self.persist(base_dir);
    }

    pub fn forget_peer_hints(&mut self, base_dir: &Path, info_hash: &str) {
        if self.hints.remove(info_hash).is_some() {
            self.persist(base_dir);
        }
    }

    pub fn get_hints(&self, info_hash: &str) -> Vec<PeerHint> {
        self.hints.get(info_hash).cloned().unwrap_or_default()
    }

    pub fn hints_mut(&mut self) -> &mut HashMap<String, Vec<PeerHint>> {
        &mut self.hints
    }

    pub fn persist(&mut self, base_dir: &Path) {
        self.gen += 1;
        let path = peer_hints_path(base_dir);
        let _ = fs::create_dir_all(path.parent().unwrap());
        let file_payload = PeerHintFile {
            torrents: self.hints.clone(),
        };
        let data = match serde_json::to_vec(&file_payload) {
            Ok(d) => d,
            Err(_) => return,
        };
        let tmp = path.with_extension("tmp");
        if fs::write(&tmp, data).is_ok() {
            let _ = fs::rename(tmp, path);
        }
    }

    pub fn load(&mut self, base_dir: &Path) {
        let path = peer_hints_path(base_dir);
        let data = match fs::read(&path) {
            Ok(d) => d,
            Err(_) => return,
        };
        if data.len() as i64 > PEER_HINT_MAX_LOAD_BYTES {
            return;
        }
        let file_payload: PeerHintFile = match serde_json::from_slice(&data) {
            Ok(p) => p,
            Err(_) => return,
        };

        let now = SystemTime::now();
        let mut out = HashMap::new();
        let mut count = 0;
        for (hash, hints) in file_payload.torrents {
            if !is_hex_info_hash(&hash) || count >= PEER_HINT_MAX_TORRENTS {
                continue;
            }
            let mut kept = Vec::new();
            for h in hints {
                if is_valid_peer_addr(&h.addr).is_none() {
                    continue;
                }
                if let Some(exp) = parse_time(&h.expires) {
                    if exp > now && kept.len() < PEER_HINT_MAX_PER_TORRENT {
                        kept.push(h);
                    }
                }
            }
            if !kept.is_empty() {
                out.insert(hash, kept);
                count += 1;
            }
        }
        self.hints = out;
    }
}

// ---------------------------------------------------------------- dht contacts store

#[derive(Default, Debug)]
pub struct DhtContactStore {
    contacts: Vec<DhtContact>,
    gen: u64,
}

impl DhtContactStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn remember_dht_contact(
        &mut self,
        base_dir: &Path,
        addr_str: &str,
        family_opt: Option<&str>,
    ) {
        let addr = match is_valid_peer_addr(addr_str) {
            Some(a) => a,
            None => return,
        };
        let family = match family_opt {
            Some(f) if !f.is_empty() => f.to_string(),
            _ => match addr.ip() {
                IpAddr::V4(_) => "ipv4".to_string(),
                IpAddr::V6(_) => "ipv6".to_string(),
            },
        };
        let now = SystemTime::now();
        let contact = DhtContact {
            addr: addr.to_string(),
            family,
            last_good: String::new(),
            expires: format_time(now + DHT_CONTACT_TTL),
        };

        if let Some(existing) = self.contacts.iter_mut().find(|c| c.addr == contact.addr) {
            if contact.last_good.is_empty() && !existing.last_good.is_empty() {
                // keep existing last_good
            } else {
                existing.last_good = contact.last_good.clone();
            }
            existing.expires = contact.expires.clone();
        } else {
            self.contacts.push(contact);
        }

        let mut v4: Vec<DhtContact> = self
            .contacts
            .iter()
            .filter(|c| c.family != "ipv6")
            .cloned()
            .collect();
        let mut v6: Vec<DhtContact> = self
            .contacts
            .iter()
            .filter(|c| c.family == "ipv6")
            .cloned()
            .collect();

        v4.sort_by(|a, b| b.last_good.cmp(&a.last_good));
        v6.sort_by(|a, b| b.last_good.cmp(&a.last_good));

        if v4.len() > DHT_CONTACT_MAX_PER_FAMILY {
            v4.truncate(DHT_CONTACT_MAX_PER_FAMILY);
        }
        if v6.len() > DHT_CONTACT_MAX_PER_FAMILY {
            v6.truncate(DHT_CONTACT_MAX_PER_FAMILY);
        }

        v4.extend(v6);
        self.contacts = v4;
        self.persist(base_dir);
    }

    pub fn contacts(&self) -> &[DhtContact] {
        &self.contacts
    }

    pub fn contacts_mut(&mut self) -> &mut Vec<DhtContact> {
        &mut self.contacts
    }

    pub fn persist(&mut self, base_dir: &Path) {
        self.gen += 1;
        let path = dht_contacts_path(base_dir);
        let _ = fs::create_dir_all(path.parent().unwrap());
        let file_payload = DhtContactFile {
            contacts: self.contacts.clone(),
        };
        let data = match serde_json::to_vec(&file_payload) {
            Ok(d) => d,
            Err(_) => return,
        };
        let tmp = path.with_extension("tmp");
        if fs::write(&tmp, data).is_ok() {
            let _ = fs::rename(tmp, path);
        }
    }

    pub fn load(&mut self, base_dir: &Path) {
        let path = dht_contacts_path(base_dir);
        let data = match fs::read(&path) {
            Ok(d) => d,
            Err(_) => return,
        };
        if data.len() as i64 > DHT_CONTACT_MAX_LOAD_BYTES {
            return;
        }
        let file_payload: DhtContactFile = match serde_json::from_slice(&data) {
            Ok(p) => p,
            Err(_) => return,
        };
        let now = SystemTime::now();
        let mut kept = Vec::new();
        for mut c in file_payload.contacts {
            let addr = match is_valid_peer_addr(&c.addr) {
                Some(a) => a,
                None => continue,
            };
            let family = match addr.ip() {
                IpAddr::V4(_) => "ipv4",
                IpAddr::V6(_) => "ipv6",
            };
            let count = kept
                .iter()
                .filter(|x: &&DhtContact| x.family == family)
                .count();
            if count >= DHT_CONTACT_MAX_PER_FAMILY {
                continue;
            }
            c.family = family.to_string();
            if let Some(exp) = parse_time(&c.expires) {
                if exp > now {
                    kept.push(c);
                }
            }
        }
        self.contacts = kept;
    }
}

// ---------------------------------------------------------------- resume window

pub fn save_resume_window(
    base_dir: &Path,
    meta: &ResumeWindowMeta,
    data: &[u8],
) -> StorageResult<()> {
    if meta.length <= 0 || data.len() as i64 != meta.length || meta.length > RESUME_WINDOW_MAX_BYTES
    {
        return Err(StorageError::Other("invalid resume window payload".into()));
    }
    let dir = resume_window_dir(base_dir, &meta.info_hash);
    fs::create_dir_all(&dir)?;

    let raw_meta = serde_json::to_vec(meta)
        .map_err(|e| StorageError::Other(format!("serialize resume meta: {}", e)))?;
    let final_path = dir.join("window.bin");
    let tmp_path = dir.join(format!(".window-tmp-{}", std::process::id()));

    let mut f = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&tmp_path)?;
    let hdr = (raw_meta.len() as u32).to_be_bytes();
    f.write_all(&hdr)?;
    f.write_all(&raw_meta)?;
    f.write_all(data)?;
    f.sync_all()?;
    drop(f);

    fs::rename(tmp_path, final_path)?;
    let _ = fs::remove_file(dir.join("window.json"));
    Ok(())
}

pub fn load_resume_window(
    base_dir: &Path,
    info_hash: &str,
) -> StorageResult<(ResumeWindowMeta, Vec<u8>)> {
    let path = resume_window_dir(base_dir, info_hash).join("window.bin");
    let mut f = File::open(&path)?;
    let mut hdr = [0u8; 4];
    f.read_exact(&mut hdr)?;
    let meta_len = u32::from_be_bytes(hdr) as usize;
    if meta_len == 0 || meta_len > (64 << 10) {
        return Err(StorageError::Other("invalid resume window header".into()));
    }
    let mut raw_meta = vec![0u8; meta_len];
    f.read_exact(&mut raw_meta)?;

    let meta: ResumeWindowMeta = serde_json::from_slice(&raw_meta)
        .map_err(|e| StorageError::Other(format!("invalid resume window json: {}", e)))?;
    if meta.version != RESUME_WINDOW_VERSION
        || meta.info_hash != info_hash
        || meta.length <= 0
        || meta.length > RESUME_WINDOW_MAX_BYTES
        || meta.checksum.len() != 40
    {
        return Err(StorageError::Other("invalid resume window meta".into()));
    }
    if let Some(exp) = parse_time(&meta.expires) {
        if exp <= SystemTime::now() {
            return Err(StorageError::Other("resume window expired".into()));
        }
    }

    let mut data = Vec::with_capacity(meta.length as usize);
    f.read_to_end(&mut data)?;
    if data.len() as i64 != meta.length || resume_checksum(&data) != meta.checksum {
        return Err(StorageError::Other(
            "checksum mismatch in resume window".into(),
        ));
    }
    Ok((meta, data))
}

pub fn delete_resume_window(base_dir: &Path, info_hash: &str) -> StorageResult<()> {
    let dir = resume_window_dir(base_dir, info_hash);
    let _ = fs::remove_dir_all(dir);
    Ok(())
}

pub fn clear_cache_categories(base_dir: &Path, categories: &[String]) -> StorageResult<()> {
    if categories.is_empty() {
        return Err(StorageError::Other("no categories".into()));
    }
    for c in categories {
        match c.as_str() {
            "startup" => {
                let p = base_dir.join(".startup");
                let _ = fs::remove_dir_all(p);
            }
            "resume" => {
                let p = base_dir.join(".cache").join("resume");
                let _ = fs::remove_dir_all(p);
            }
            "peers" => {
                let p = peer_hints_path(base_dir);
                let _ = fs::remove_file(p);
            }
            "dht" => {
                let p = dht_contacts_path(base_dir);
                let _ = fs::remove_file(p);
            }
            other => {
                return Err(StorageError::Other(format!(
                    "invalid cache category \"{}\"",
                    other
                )));
            }
        }
    }
    Ok(())
}
