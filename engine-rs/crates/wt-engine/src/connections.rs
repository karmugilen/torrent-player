//! Live peer connection views and GeoIP country resolution.
//!
//! Port of `engine-go/connections.go`.

use parking_lot::{Mutex, RwLock};
use serde::Deserialize;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::types::PeerConnectionView;

#[derive(Deserialize)]
struct MaxMindCountryRecord<'a> {
    #[serde(borrow)]
    country: Option<MaxMindCountry<'a>>,
}

#[derive(Deserialize)]
struct MaxMindCountry<'a> {
    #[serde(borrow)]
    iso_code: Option<&'a str>,
}

pub struct GeoIpResolver {
    dir: PathBuf,
    reader: RwLock<Option<maxminddb::Reader<Vec<u8>>>>,
    /// Last load attempt. Android copies the database after the engine is
    /// ready, so a missing file is retried rather than cached forever.
    last_attempt: Mutex<Option<Instant>>,
}

const GEOIP_RETRY: Duration = Duration::from_secs(30);

impl GeoIpResolver {
    /// Loading reads the whole database; defer it to the first lookup so
    /// engine startup does not wait on that file I/O.
    pub fn new(download_dir: impl AsRef<Path>) -> Self {
        Self {
            dir: download_dir.as_ref().to_path_buf(),
            reader: RwLock::new(None),
            last_attempt: Mutex::new(None),
        }
    }

    fn ensure_loaded(&self) {
        if self.reader.read().is_some() {
            return;
        }
        let mut last = self.last_attempt.lock();
        if self.reader.read().is_some() || last.is_some_and(|t| t.elapsed() < GEOIP_RETRY) {
            return;
        }
        *last = Some(Instant::now());
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return; };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue; };
            if name.starts_with("geoip-country") && name.ends_with(".mmdb") {
                if let Ok(r) = maxminddb::Reader::open_readfile(&path) {
                    *self.reader.write() = Some(r);
                    return;
                }
            }
        }
    }

    pub fn lookup_country(&self, address: &str) -> Option<String> {
        let (host, _) = if let Ok(sock) = address.parse::<SocketAddr>() {
            (sock.ip().to_string(), sock.port())
        } else if let Some((h, _)) = address.rsplit_once(':') {
            (
                h.trim_start_matches('[').trim_end_matches(']').to_string(),
                0,
            )
        } else {
            return None;
        };

        let ip = host.parse::<IpAddr>().ok()?;
        match ip {
            IpAddr::V4(v4) => {
                let octets = v4.octets();
                if v4.is_private()
                    || v4.is_loopback()
                    || v4.is_link_local()
                    || v4.is_broadcast()
                    || v4.is_documentation()
                    || octets[0] == 0
                {
                    return None;
                }
            }
            IpAddr::V6(v6) => {
                if v6.is_loopback() || v6.is_multicast() {
                    return None;
                }
            }
        }

        self.ensure_loaded();
        let lock = self.reader.read();
        let reader = lock.as_ref()?;
        let record: MaxMindCountryRecord = reader.lookup(ip).ok()?;
        record
            .country
            .and_then(|c| c.iso_code.map(|s| s.to_string()))
    }
}

pub fn sort_peer_connections(peers: &mut [PeerConnectionView]) {
    peers.sort_by(|a, b| {
        if a.address == b.address {
            a.transport.cmp(&b.transport)
        } else {
            a.address.cmp(&b.address)
        }
    });
}
