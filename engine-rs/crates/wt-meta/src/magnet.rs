//! Magnet URI parse and format (v1, v2, hybrid).

use std::collections::BTreeMap;

use data_encoding::BASE32;
use wt_types::{InfoHash, InfoHashV2, Magnet};

use crate::error::{Error, Result};

const BTIH_PREFIX: &str = "urn:btih:";
const BTMH_PREFIX: &str = "urn:btmh:";

/// Multihash code for SHA2-256.
const MH_SHA2_256: u8 = 0x12;

/// Parse a magnet URI (v1, v2, or hybrid). Supports `xt=urn:btih:` (hex or
/// base32) and `xt=urn:btmh:` (multihash hex), plus `tr`, `dn`, `ws`/`xs`/`as`,
/// and `x.pe` (stored in `params`).
pub fn parse_magnet(uri: &str) -> Result<Magnet> {
    let (scheme, query) = split_magnet(uri)?;
    if !scheme.eq_ignore_ascii_case("magnet") {
        return Err(Error::Magnet(format!("unexpected scheme {scheme:?}")));
    }

    let mut magnet = Magnet::default();
    let mut params: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for (key, value) in parse_query(query) {
        match key.as_str() {
            "xt" => {
                if let Some(encoded) = value.strip_prefix(BTIH_PREFIX) {
                    if magnet.info_hash.is_some() {
                        return Err(Error::Magnet(
                            "more than one v1 infohash found in magnet link".into(),
                        ));
                    }
                    magnet.info_hash = Some(parse_encoded_v1_infohash(encoded)?);
                } else if let Some(encoded) = value.strip_prefix(BTMH_PREFIX) {
                    if magnet.info_hash_v2.is_some() {
                        return Err(Error::Magnet(
                            "more than one v2 infohash found in magnet link".into(),
                        ));
                    }
                    magnet.info_hash_v2 = Some(parse_v2_infohash(encoded)?);
                } else {
                    params.entry("xt".into()).or_default().push(value);
                }
            }
            "dn" => {
                if magnet.display_name.is_empty() {
                    magnet.display_name = value;
                } else {
                    params.entry("dn".into()).or_default().push(value);
                }
            }
            "tr" => magnet.trackers.push(value),
            "ws" | "xs" | "as" => magnet.web_seeds.push(value),
            other => {
                params.entry(other.to_string()).or_default().push(value);
            }
        }
    }

    if magnet.info_hash.is_none() && magnet.info_hash_v2.is_none() {
        return Err(Error::Magnet("missing infohash".into()));
    }

    for (k, vs) in params {
        for v in vs {
            magnet.params.push((k.clone(), v));
        }
    }

    Ok(magnet)
}

/// Format a magnet URI. Matches Go `MagnetV2.String()`: unescaped `xt=urn:btih:` /
/// `xt=urn:btmh:` first, then remaining query params sorted by key (Go
/// `url.Values.Encode`).
pub fn magnet_uri(m: &Magnet) -> String {
    let mut parts: Vec<String> = Vec::new();

    if let Some(ih) = &m.info_hash {
        parts.push(format!("xt={BTIH_PREFIX}{}", ih.to_hex()));
    }
    if let Some(ih2) = &m.info_hash_v2 {
        parts.push(format!(
            "xt={BTMH_PREFIX}{}",
            infohash_v2_multihash_hex(ih2)
        ));
    }

    // Collect remaining as multi-map for sorted encode (like url.Values).
    let mut values: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for tr in &m.trackers {
        values.entry("tr".into()).or_default().push(tr.clone());
    }
    if !m.display_name.is_empty() {
        values
            .entry("dn".into())
            .or_default()
            .push(m.display_name.clone());
    }
    for ws in &m.web_seeds {
        values.entry("ws".into()).or_default().push(ws.clone());
    }
    for (k, v) in &m.params {
        values.entry(k.clone()).or_default().push(v.clone());
    }

    let encoded = encode_query(&values);
    if !encoded.is_empty() {
        parts.push(encoded);
    }

    format!("magnet:?{}", parts.join("&"))
}

fn infohash_v2_multihash_hex(ih: &InfoHashV2) -> String {
    // multihash: <varint code=0x12><varint len=0x20><digest>
    let mut buf = Vec::with_capacity(34);
    buf.push(MH_SHA2_256);
    buf.push(32);
    buf.extend_from_slice(&ih.0);
    hex::encode(buf)
}

fn parse_encoded_v1_infohash(encoded: &str) -> Result<InfoHash> {
    let mut out = [0u8; 20];
    match encoded.len() {
        40 => {
            hex::decode_to_slice(encoded, &mut out)
                .map_err(|e| Error::Magnet(format!("error decoding xt hex: {e}")))?;
        }
        32 => {
            let decoded = BASE32
                .decode(encoded.as_bytes())
                .map_err(|e| Error::Magnet(format!("error decoding xt base32: {e}")))?;
            if decoded.len() != 20 {
                return Err(Error::Magnet(format!(
                    "base32 infohash has length {}",
                    decoded.len()
                )));
            }
            out.copy_from_slice(&decoded);
        }
        n => {
            return Err(Error::Magnet(format!(
                "unhandled xt parameter encoding (encoded length {n})"
            )));
        }
    }
    Ok(InfoHash(out))
}

fn parse_v2_infohash(encoded: &str) -> Result<InfoHashV2> {
    let b = hex::decode(encoded)
        .map_err(|e| Error::Magnet(format!("error decoding v2 multihash: {e}")))?;
    // Expect 0x12 0x20 <32 bytes>
    if b.len() < 2 {
        return Err(Error::Magnet("bad multihash".into()));
    }
    let code = b[0];
    let len = b[1] as usize;
    if code != MH_SHA2_256 || len != 32 || b.len() != 2 + 32 {
        return Err(Error::Magnet("bad multihash".into()));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&b[2..]);
    Ok(InfoHashV2(out))
}

fn split_magnet(uri: &str) -> Result<(&str, &str)> {
    let Some((scheme, rest)) = uri.split_once(':') else {
        return Err(Error::Magnet("error parsing uri: missing scheme".into()));
    };
    let query = rest.strip_prefix('?').unwrap_or(rest);
    Ok((scheme, query))
}

/// Parse application/x-www-form-urlencoded query into (key, value) pairs in order.
fn parse_query(query: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if query.is_empty() {
        return out;
    }
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = match pair.split_once('=') {
            Some((k, v)) => (k, v),
            None => (pair, ""),
        };
        out.push((percent_decode(k), percent_decode(v)));
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let h = &s[i + 1..i + 3];
                if let Ok(b) = u8::from_str_radix(h, 16) {
                    out.push(b);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Encode like Go `url.Values.Encode`: keys sorted, values in insertion order
/// per key, spaces as `+`.
fn encode_query(values: &BTreeMap<String, Vec<String>>) -> String {
    let mut parts = Vec::new();
    for (k, vs) in values {
        for v in vs {
            parts.push(format!("{}={}", query_escape(k), query_escape(v)));
        }
    }
    parts.join("&")
}

fn query_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push(char::from(b"0123456789ABCDEF"[(b >> 4) as usize]));
                out.push(char::from(b"0123456789ABCDEF"[(b & 0xf) as usize]));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_btih() {
        let uri = "magnet:?xt=urn:btih:51340689c960f0778a4387aef9b4b52fd08390cd&dn=Shit+Movie+%281985%29+1337p+-+Eru&tr=http%3A%2F%2Fhttp.was.great%21&tr=udp%3A%2F%2Fanti.piracy.honeypot%3A6969";
        let m = parse_magnet(uri).unwrap();
        assert_eq!(
            m.info_hash.unwrap().to_hex(),
            "51340689c960f0778a4387aef9b4b52fd08390cd"
        );
        assert_eq!(m.display_name, "Shit Movie (1985) 1337p - Eru");
        assert_eq!(
            m.trackers,
            vec!["http://http.was.great!", "udp://anti.piracy.honeypot:6969"]
        );
    }

    #[test]
    fn parse_base32_btih() {
        let uri = "magnet:?xt=urn:btih:ZOCMZQIPFFW7OLLMIC5HUB6BPCSDEOQU";
        let m = parse_magnet(uri).unwrap();
        assert!(m.info_hash.is_some());
    }

    #[test]
    fn roundtrip_magnet_string() {
        let m = Magnet {
            info_hash: InfoHash::from_hex("51340689c960f0778a4387aef9b4b52fd08390cd"),
            display_name: "Shit Movie (1985) 1337p - Eru".into(),
            trackers: vec![
                "http://http.was.great!".into(),
                "udp://anti.piracy.honeypot:6969".into(),
            ],
            ..Magnet::default()
        };
        let s = magnet_uri(&m);
        let m2 = parse_magnet(&s).unwrap();
        assert_eq!(m.info_hash, m2.info_hash);
        assert_eq!(m.display_name, m2.display_name);
        assert_eq!(m.trackers, m2.trackers);
    }

    #[test]
    fn parse_v2_only() {
        let uri = "magnet:?xt=urn:btmh:1220caf1e1c30e81cb361b9ee167c4aa64228a7fa4fa9f6105232b28ad099f3a302e&dn=bittorrent-v2-test";
        let m = parse_magnet(uri).unwrap();
        assert!(m.info_hash.is_none());
        assert_eq!(
            m.info_hash_v2.unwrap().to_hex(),
            "caf1e1c30e81cb361b9ee167c4aa64228a7fa4fa9f6105232b28ad099f3a302e"
        );
        assert_eq!(m.display_name, "bittorrent-v2-test");
    }

    #[test]
    fn parse_hybrid() {
        let uri = "magnet:?xt=urn:btih:631a31dd0a46257d5078c0dee4e66e26f73e42ac&xt=urn:btmh:1220d8dd32ac93357c368556af3ac1d95c9d76bd0dff6fa9833ecdac3d53134efabb&dn=bittorrent-v1-v2-hybrid-test";
        let m = parse_magnet(uri).unwrap();
        assert_eq!(
            m.info_hash.unwrap().to_hex(),
            "631a31dd0a46257d5078c0dee4e66e26f73e42ac"
        );
        assert_eq!(
            m.info_hash_v2.unwrap().to_hex(),
            "d8dd32ac93357c368556af3ac1d95c9d76bd0dff6fa9833ecdac3d53134efabb"
        );
    }

    #[test]
    fn reject_empty_and_sha1() {
        assert!(parse_magnet("").is_err());
        assert!(parse_magnet("magnet:?xt=urn:sha1:YNCKHTQCWBTRNJIV4WNAE52SJUQCZO5C").is_err());
        assert!(parse_magnet("magnet:?xt=urn:btih:this hash is really broken").is_err());
    }

    #[test]
    fn url_list_magnet_format() {
        // Expected Go output from TestMetainfoWithListURLList
        let m = Magnet {
            info_hash: InfoHash::from_hex("d4b197dff199aad447a9a352e31528adbbd97922"),
            trackers: vec![
                "http://bt1.archive.org:6969/announce".into(),
                "http://bt2.archive.org:6969/announce".into(),
            ],
            web_seeds: vec![
                "https://archive.org/download/".into(),
                "http://ia601600.us.archive.org/26/items/".into(),
                "http://ia801600.us.archive.org/26/items/".into(),
            ],
            ..Magnet::default()
        };
        let s = magnet_uri(&m);
        assert_eq!(
            s,
            "magnet:?xt=urn:btih:d4b197dff199aad447a9a352e31528adbbd97922&tr=http%3A%2F%2Fbt1.archive.org%3A6969%2Fannounce&tr=http%3A%2F%2Fbt2.archive.org%3A6969%2Fannounce&ws=https%3A%2F%2Farchive.org%2Fdownload%2F&ws=http%3A%2F%2Fia601600.us.archive.org%2F26%2Fitems%2F&ws=http%3A%2F%2Fia801600.us.archive.org%2F26%2Fitems%2F"
        );
    }
}
