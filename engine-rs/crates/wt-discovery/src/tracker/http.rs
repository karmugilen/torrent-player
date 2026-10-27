//! HTTP(S) tracker announce (BEP 3) with compact + dictionary peers, v4/v6.
//!
//! When [`HttpClientOpts::dialer`] is set, TCP is established through that hook
//! (Go `DialContext` / `trackerSockets` semantics) and HTTP/1.1 is spoken over
//! the stream (with rustls for HTTPS). Otherwise reqwest's default dial is used.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use url::Url;
use wt_meta::bencode::{self, Value};

use super::{
    AnnounceEvent, AnnounceOpt, AnnounceRequest, AnnounceResponse, Peer, Result, ScrapeItem,
    ScrapeResponse, TrackerError,
};
use crate::tracker::client::DialerHook;

/// Options for building an HTTP tracker client.
#[derive(Clone, Default)]
pub struct HttpClientOpts {
    /// Custom TCP dialer so the engine can route / track sockets.
    pub dialer: Option<DialerHook>,
    /// Override SNI / TLS server name (rarely needed).
    pub server_name: Option<String>,
    pub timeout: Option<Duration>,
}

/// HTTP(S) announce client for one tracker URL.
#[derive(Clone)]
pub struct HttpClient {
    url: Url,
    /// Used when no custom dialer is configured.
    http: Option<reqwest::Client>,
    dialer: Option<DialerHook>,
    server_name: Option<String>,
    timeout: Duration,
}

impl HttpClient {
    pub fn new(url: Url, opts: HttpClientOpts) -> Result<Self> {
        let timeout = opts
            .timeout
            .unwrap_or(super::DEFAULT_TRACKER_ANNOUNCE_TIMEOUT);
        let http = if opts.dialer.is_some() {
            None
        } else {
            Some(
                reqwest::Client::builder()
                    .timeout(timeout)
                    .user_agent("")
                    .redirect(reqwest::redirect::Policy::limited(5))
                    .use_rustls_tls()
                    .build()
                    .map_err(|e| TrackerError::Other(e.to_string()))?,
            )
        };
        Ok(Self {
            url,
            http,
            dialer: opts.dialer,
            server_name: opts.server_name,
            timeout,
        })
    }

    pub async fn announce(
        &self,
        req: AnnounceRequest,
        opt: AnnounceOpt,
    ) -> Result<AnnounceResponse> {
        let mut url = self.url.clone();
        set_announce_params(&mut url, &req, &opt);

        let ua = if opt.user_agent.is_empty() {
            DEFAULT_HTTP_USER_AGENT
        } else {
            opt.user_agent.as_str()
        };

        if self.dialer.is_some() {
            let (status, body) = self
                .exchange_dialed(&url, ua, opt.host_header.as_str())
                .await?;
            if status != 200 {
                return Err(TrackerError::HttpStatus {
                    status,
                    body: String::from_utf8_lossy(&body).into_owned(),
                });
            }
            return parse_announce_body(&body);
        }

        let http = self
            .http
            .as_ref()
            .ok_or_else(|| TrackerError::Other("http client missing".into()))?;
        let mut builder = http.get(url);
        if !ua.is_empty() {
            builder = builder.header(reqwest::header::USER_AGENT, ua);
        }
        if !opt.host_header.is_empty() {
            builder = builder.header(reqwest::header::HOST, &opt.host_header);
        }

        let resp = builder
            .send()
            .await
            .map_err(|e| map_reqwest(e, self.timeout))?;
        let status = resp.status().as_u16();
        let body = resp
            .bytes()
            .await
            .map_err(|e| TrackerError::Other(e.to_string()))?;
        if status != 200 {
            return Err(TrackerError::HttpStatus {
                status,
                body: String::from_utf8_lossy(&body).into_owned(),
            });
        }
        parse_announce_body(&body)
    }

    /// Optional scrape (BEP 48 / common HTTP scrape URL rewrite).
    pub async fn scrape(&self, info_hashes: &[[u8; 20]]) -> Result<ScrapeResponse> {
        let mut url = self.url.clone();
        // Conventional announce -> scrape path rewrite.
        if let Some(seg) = url.path_segments().and_then(|mut s| s.next_back()) {
            if seg == "announce" {
                let mut path = url.path().to_string();
                if let Some(stripped) = path.strip_suffix("announce") {
                    path = format!("{stripped}scrape");
                    url.set_path(&path);
                }
            }
        }
        let mut q = String::new();
        for (i, ih) in info_hashes.iter().enumerate() {
            if i > 0 {
                q.push('&');
            }
            q.push_str("info_hash=");
            q.push_str(&percent_encode_bytes(ih));
        }
        if url.query().is_some() {
            let existing = url.query().unwrap_or("").to_string();
            url.set_query(Some(&format!("{existing}&{q}")));
        } else {
            url.set_query(Some(&q));
        }

        if self.dialer.is_some() {
            let (status, body) = self
                .exchange_dialed(&url, DEFAULT_HTTP_USER_AGENT, "")
                .await?;
            if status != 200 {
                return Err(TrackerError::HttpStatus {
                    status,
                    body: String::from_utf8_lossy(&body).into_owned(),
                });
            }
            return parse_scrape_body(&body, info_hashes.len());
        }

        let http = self
            .http
            .as_ref()
            .ok_or_else(|| TrackerError::Other("http client missing".into()))?;
        let resp = http
            .get(url)
            .send()
            .await
            .map_err(|e| map_reqwest(e, self.timeout))?;
        let status = resp.status().as_u16();
        let body = resp
            .bytes()
            .await
            .map_err(|e| TrackerError::Other(e.to_string()))?;
        if status != 200 {
            return Err(TrackerError::HttpStatus {
                status,
                body: String::from_utf8_lossy(&body).into_owned(),
            });
        }
        parse_scrape_body(&body, info_hashes.len())
    }

    /// Dial via [`DialerHook`], optionally wrap TLS, speak HTTP/1.1.
    async fn exchange_dialed(
        &self,
        url: &Url,
        user_agent: &str,
        host_header: &str,
    ) -> Result<(u16, Vec<u8>)> {
        let dialer = self
            .dialer
            .as_ref()
            .ok_or_else(|| TrackerError::Other("dialer missing".into()))?;
        let host = url
            .host_str()
            .ok_or_else(|| TrackerError::Other("url missing host".into()))?;
        let https = url.scheme() == "https";
        let port = url.port().unwrap_or(if https { 443 } else { 80 });
        let addr = if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        };

        let tcp = tokio::time::timeout(self.timeout, dialer("tcp".into(), addr))
            .await
            .map_err(|_| TrackerError::Timeout)?
            .map_err(TrackerError::Io)?;

        let host_hdr = if host_header.is_empty() {
            if (https && port == 443) || (!https && port == 80) {
                host.to_string()
            } else if host.contains(':') && !host.starts_with('[') {
                format!("[{host}]:{port}")
            } else {
                format!("{host}:{port}")
            }
        } else {
            host_header.to_string()
        };

        let path = {
            let p = url.path();
            let path = if p.is_empty() { "/" } else { p };
            match url.query() {
                Some(q) => format!("{path}?{q}"),
                None => path.to_string(),
            }
        };
        let request = build_http_get(&path, &host_hdr, user_agent);

        let fut = async {
            if https {
                let server_name = self.server_name.clone().unwrap_or_else(|| host.to_string());
                let tls = tls_wrap(tcp, &server_name).await?;
                http1_exchange(tls, &request).await
            } else {
                http1_exchange(tcp, &request).await
            }
        };
        tokio::time::timeout(self.timeout, fut)
            .await
            .map_err(|_| TrackerError::Timeout)?
    }
}

fn build_http_get(path: &str, host: &str, user_agent: &str) -> Vec<u8> {
    let mut req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    if !user_agent.is_empty() {
        req.push_str("User-Agent: ");
        req.push_str(user_agent);
        req.push_str("\r\n");
    }
    req.push_str("\r\n");
    req.into_bytes()
}

async fn tls_wrap(
    tcp: tokio::net::TcpStream,
    server_name: &str,
) -> Result<tokio_rustls::client::TlsStream<tokio::net::TcpStream>> {
    crate::ensure_rustls_provider();
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let cfg = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    // Match Go tracker TLS: allow self-signed / private trackers.
    let mut cfg = cfg;
    cfg.dangerous()
        .set_certificate_verifier(Arc::new(NoVerifier));
    let connector = tokio_rustls::TlsConnector::from(Arc::new(cfg));
    let name = rustls::pki_types::ServerName::try_from(server_name.to_string())
        .map_err(|e| TrackerError::Other(format!("bad server name: {e}")))?;
    connector
        .connect(name, tcp)
        .await
        .map_err(|e| TrackerError::Other(format!("tls: {e}")))
}

#[derive(Debug)]
struct NoVerifier;

impl rustls::client::danger::ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

async fn http1_exchange<S>(mut stream: S, request: &[u8]) -> Result<(u16, Vec<u8>)>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    stream.write_all(request).await.map_err(TrackerError::Io)?;
    stream.flush().await.map_err(TrackerError::Io)?;

    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 2048];
    loop {
        let n = stream.read(&mut tmp).await.map_err(TrackerError::Io)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            // Keep reading until EOF (Connection: close) or Content-Length satisfied.
            if let Some(cl) = content_length(&buf) {
                if let Some(header_end) = find_header_end(&buf) {
                    if buf.len() >= header_end + cl {
                        break;
                    }
                }
            }
        }
    }
    parse_http_response(&buf)
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

fn content_length(buf: &[u8]) -> Option<usize> {
    let text = std::str::from_utf8(buf).ok()?;
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("content-length:") {
            return rest.trim().parse().ok();
        }
        if line.is_empty() {
            break;
        }
    }
    None
}

fn parse_http_response(buf: &[u8]) -> Result<(u16, Vec<u8>)> {
    let header_end = find_header_end(buf).ok_or_else(|| {
        TrackerError::Other(format!("incomplete http response ({} bytes)", buf.len()))
    })?;
    let header =
        std::str::from_utf8(&buf[..header_end]).map_err(|e| TrackerError::Other(e.to_string()))?;
    let status_line = header.lines().next().unwrap_or("");
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| TrackerError::Other(format!("bad status line: {status_line}")))?;
    Ok((status, buf[header_end..].to_vec()))
}

pub const DEFAULT_HTTP_USER_AGENT: &str = "wt-discovery/0.1";

fn map_reqwest(e: reqwest::Error, _timeout: Duration) -> TrackerError {
    if e.is_timeout() {
        TrackerError::Timeout
    } else {
        TrackerError::Other(e.to_string())
    }
}

fn set_announce_params(url: &mut Url, ar: &AnnounceRequest, opts: &AnnounceOpt) {
    let left = if ar.left < 0 { i64::MAX } else { ar.left };
    // info_hash and peer_id are raw bytes in the query string (BEP 3).
    let mut pairs: Vec<(String, String)> = vec![
        ("key".into(), ar.key.to_string()),
        ("info_hash".into(), raw_query_bytes(&ar.info_hash.0)),
        ("peer_id".into(), raw_query_bytes(&ar.peer_id)),
        ("port".into(), ar.port.to_string()),
        ("uploaded".into(), ar.uploaded.to_string()),
        ("downloaded".into(), ar.downloaded.to_string()),
        ("left".into(), left.to_string()),
        ("compact".into(), "1".into()),
        ("supportcrypto".into(), "1".into()),
    ];
    if ar.event != AnnounceEvent::None {
        pairs.push(("event".into(), ar.event.as_str().to_string()));
    }

    let mut push_ip = |version_key: &str, ip: Option<IpAddr>| {
        if let Some(ip) = ip {
            let s = ip.to_string();
            pairs.push((version_key.into(), s.clone()));
            pairs.push(("ip".into(), s));
        }
    };
    push_ip("ipv4", opts.client_ip4.filter(|a| a.is_ipv4()));
    push_ip("ipv6", opts.client_ip6.filter(|a| a.is_ipv6()));

    // Build query manually so we can percent-encode like Go (space as %20).
    let mut qstr = String::new();
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            qstr.push('&');
        }
        qstr.push_str(&percent_encode_str(k));
        qstr.push('=');
        // Values that are already percent-encoded (info_hash/peer_id) pass through.
        if k == "info_hash" || k == "peer_id" {
            qstr.push_str(v);
        } else {
            qstr.push_str(&percent_encode_str(v).replace('+', "%20"));
        }
    }
    if let Some(existing) = url.query() {
        url.set_query(Some(&format!("{existing}&{qstr}")));
    } else {
        url.set_query(Some(&qstr));
    }
}

/// Encode bytes for a query value the way BitTorrent expects (each byte %XX or safe).
fn raw_query_bytes(b: &[u8]) -> String {
    percent_encode_bytes(b)
}

fn percent_encode_bytes(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len() * 3);
    for &c in b {
        match c {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(c as char);
            }
            _ => {
                out.push('%');
                out.push(hex_digit(c >> 4));
                out.push(hex_digit(c & 0xf));
            }
        }
    }
    out
}

fn percent_encode_str(s: &str) -> String {
    percent_encode_bytes(s.as_bytes())
}

fn hex_digit(n: u8) -> char {
    char::from(b"0123456789ABCDEF"[n as usize])
}

pub(crate) fn parse_announce_body(body: &[u8]) -> Result<AnnounceResponse> {
    let (val, _) = bencode::decode_prefix(body).map_err(|e| TrackerError::Decode(e.to_string()))?;
    if let Some(reason) = val
        .dict_get_str("failure reason")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        return Err(TrackerError::FailureReason(reason.to_string()));
    }

    let mut resp = AnnounceResponse {
        interval: val
            .dict_get_str("interval")
            .and_then(|v| v.as_int())
            .unwrap_or(0) as i32,
        min_interval: val
            .dict_get_str("min interval")
            .and_then(|v| v.as_int())
            .map(|i| i as i32),
        leechers: val
            .dict_get_str("incomplete")
            .and_then(|v| v.as_int())
            .unwrap_or(0) as i32,
        seeders: val
            .dict_get_str("complete")
            .and_then(|v| v.as_int())
            .unwrap_or(0) as i32,
        peers: Vec::new(),
        warning: val
            .dict_get_str("warning message")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        tracker_id: val
            .dict_get_str("tracker id")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    };

    if let Some(peers_v) = val.dict_get_str("peers") {
        resp.peers.extend(parse_peers(peers_v, false)?);
    }
    if let Some(peers6_v) = val.dict_get_str("peers6") {
        resp.peers.extend(parse_peers(peers6_v, true)?);
    }
    Ok(resp)
}

fn parse_peers(v: &Value, ipv6: bool) -> Result<Vec<Peer>> {
    match v {
        Value::Bytes(b) => {
            if ipv6 {
                parse_compact_v6(b)
            } else {
                parse_compact_v4(b)
            }
        }
        Value::List(list) => {
            let mut out = Vec::with_capacity(list.len());
            for item in list {
                let dict = item
                    .as_dict()
                    .ok_or_else(|| TrackerError::Decode("peer not dict".into()))?;
                let ip_s = dict
                    .get(b"ip".as_slice())
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| TrackerError::Decode("peer missing ip".into()))?;
                let port = dict
                    .get(b"port".as_slice())
                    .and_then(|x| x.as_int())
                    .ok_or_else(|| TrackerError::Decode("peer missing port".into()))?
                    as u16;
                let ip: IpAddr = ip_s
                    .parse()
                    .map_err(|e: std::net::AddrParseError| TrackerError::Decode(e.to_string()))?;
                let id = dict.get(b"peer id".as_slice()).and_then(|x| {
                    x.as_bytes().and_then(|b| {
                        if b.len() == 20 {
                            let mut id = [0u8; 20];
                            id.copy_from_slice(b);
                            Some(id)
                        } else {
                            None
                        }
                    })
                });
                out.push(Peer {
                    addr: SocketAddr::new(ip, port),
                    id,
                });
            }
            Ok(out)
        }
        _ => Err(TrackerError::Decode("unsupported peers type".into())),
    }
}

fn parse_compact_v4(b: &[u8]) -> Result<Vec<Peer>> {
    if !b.len().is_multiple_of(6) {
        return Err(TrackerError::Decode(format!(
            "compact peers length {} not multiple of 6",
            b.len()
        )));
    }
    let mut out = Vec::with_capacity(b.len() / 6);
    for chunk in b.as_chunks::<6>().0 {
        let ip = Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
        let port = u16::from_be_bytes([chunk[4], chunk[5]]);
        out.push(Peer::new(SocketAddr::new(IpAddr::V4(ip), port)));
    }
    Ok(out)
}

fn parse_compact_v6(b: &[u8]) -> Result<Vec<Peer>> {
    if !b.len().is_multiple_of(18) {
        return Err(TrackerError::Decode(format!(
            "compact peers6 length {} not multiple of 18",
            b.len()
        )));
    }
    let mut out = Vec::with_capacity(b.len() / 18);
    for chunk in b.as_chunks::<18>().0 {
        let mut octets = [0u8; 16];
        octets.copy_from_slice(&chunk[..16]);
        let ip = Ipv6Addr::from(octets);
        let port = u16::from_be_bytes([chunk[16], chunk[17]]);
        out.push(Peer::new(SocketAddr::new(IpAddr::V6(ip), port)));
    }
    Ok(out)
}

fn parse_scrape_body(body: &[u8], expect: usize) -> Result<ScrapeResponse> {
    let (val, _) = bencode::decode_prefix(body).map_err(|e| TrackerError::Decode(e.to_string()))?;
    let files = val
        .dict_get_str("files")
        .and_then(|v| v.as_dict())
        .ok_or_else(|| TrackerError::Decode("scrape missing files".into()))?;
    let mut out = Vec::with_capacity(expect);
    for v in files.values() {
        out.push(ScrapeItem {
            complete: v
                .dict_get_str("complete")
                .and_then(|x| x.as_int())
                .unwrap_or(0) as i32,
            downloaded: v
                .dict_get_str("downloaded")
                .and_then(|x| x.as_int())
                .unwrap_or(0) as i32,
            incomplete: v
                .dict_get_str("incomplete")
                .and_then(|x| x.as_int())
                .unwrap_or(0) as i32,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_compact_peers() {
        let mut body = Vec::new();
        body.extend_from_slice(b"d8:intervali900e5:peers6:");
        body.extend_from_slice(&[1, 2, 3, 4, 0x1a, 0xe1]);
        body.push(b'e');
        let resp = parse_announce_body(&body).unwrap();
        assert_eq!(resp.interval, 900);
        assert_eq!(resp.peers.len(), 1);
        assert_eq!(resp.peers[0].addr.to_string(), "1.2.3.4:6881");
    }

    #[test]
    fn parse_failure_reason() {
        let body = b"d14:failure reason15:torrent missinge";
        let err = parse_announce_body(body).unwrap_err();
        match err {
            TrackerError::FailureReason(s) => assert_eq!(s, "torrent missing"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parse_dict_peers() {
        let body =
            b"d8:intervali60e5:peersld2:ip7:1.2.3.47:peer id20:abcdefghijklmnopqrst4:porti6881eeee";
        let resp = parse_announce_body(body).unwrap();
        assert_eq!(resp.peers.len(), 1);
        assert_eq!(resp.peers[0].addr.port(), 6881);
        assert!(resp.peers[0].id.is_some());
    }
}
