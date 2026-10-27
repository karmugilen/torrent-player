//! Live peer connection views and GeoIP country resolution.
//!
//! Port of `engine-go/connections.go`.

use parking_lot::RwLock;
use serde::Deserialize;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;

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
    reader: RwLock<Option<maxminddb::Reader<Vec<u8>>>>,
}

impl GeoIpResolver {
    pub fn new(download_dir: impl AsRef<Path>) -> Self {
        let dir = download_dir.as_ref();
        let mut reader = None;
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with("geoip-country") && name.ends_with(".mmdb") {
                        if let Ok(r) = maxminddb::Reader::open_readfile(&path) {
                            reader = Some(r);
                            break;
                        }
                    }
                }
            }
        }
        Self {
            reader: RwLock::new(reader),
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
