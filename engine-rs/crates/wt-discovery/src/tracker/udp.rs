//! UDP tracker client (BEP 15) with BEP 41 URL path options and Go-matching timeouts.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use rand::Rng;
use tokio::net::UdpSocket;
use tokio::time::timeout;

use super::{
    AnnounceOpt, AnnounceRequest, AnnounceResponse, Peer, Result, ScrapeItem, ScrapeResponse,
    TrackerError,
};
use crate::tracker::client::ListenPacketHook;

/// Default announce timeout used by Go `DefaultTrackerAnnounceTimeout`.
pub const DEFAULT_TRACKER_ANNOUNCE_TIMEOUT: Duration = Duration::from_secs(15);

const CONNECT_REQUEST_CONNECTION_ID: u64 = 0x0417_2710_1980;
const ACTION_CONNECT: i32 = 0;
const ACTION_ANNOUNCE: i32 = 1;
const ACTION_SCRAPE: i32 = 2;
const ACTION_ERROR: i32 = 3;

const OPTION_TYPE_URL_DATA: u8 = 2;

/// Go `udp.timeout`: 15s * 2^n, capped at n=8 → 3840s.
pub fn udp_timeout(contiguous_timeouts: usize) -> Duration {
    let n = contiguous_timeouts.min(8);
    let mut d = Duration::from_secs(15);
    for _ in 0..n {
        d *= 2;
    }
    d
}

#[derive(Clone, Default)]
pub struct UdpClientOpts {
    /// Override network: "udp", "udp4", "udp6".
    pub network: String,
    pub listen_packet: Option<ListenPacketHook>,
    pub timeout: Option<Duration>,
}

struct ConnState {
    conn_id: u64,
    issued: Option<Instant>,
}

/// UDP tracker client bound to one host (possibly with BEP 41 path).
pub struct UdpClient {
    host: String,
    /// Request URI path+query for BEP 41 (e.g. `/announce?pass=x`).
    request_uri: String,
    sock: Arc<UdpSocket>,
    remote: Mutex<Option<SocketAddr>>,
    state: Mutex<ConnState>,
    overall_timeout: Duration,
}

impl UdpClient {
    pub async fn new(host: &str, request_uri: &str, opts: UdpClientOpts) -> Result<Self> {
        let sock = if let Some(hook) = &opts.listen_packet {
            hook("udp", "0.0.0.0:0")
                .await
                .map_err(|e| TrackerError::Other(e.to_string()))?
        } else {
            let network = if opts.network.is_empty() {
                "udp"
            } else {
                opts.network.as_str()
            };
            let bind = match network {
                "udp4" => "0.0.0.0:0",
                "udp6" => "[::]:0",
                _ => "0.0.0.0:0",
            };
            UdpSocket::bind(bind).await.map_err(TrackerError::Io)?
        };
        Ok(Self {
            host: host.to_string(),
            request_uri: request_uri.to_string(),
            sock: Arc::new(sock),
            remote: Mutex::new(None),
            state: Mutex::new(ConnState {
                conn_id: 0,
                issued: None,
            }),
            overall_timeout: opts.timeout.unwrap_or(DEFAULT_TRACKER_ANNOUNCE_TIMEOUT),
        })
    }

    async fn resolve(&self) -> Result<SocketAddr> {
        if let Some(addr) = *self.remote.lock() {
            return Ok(addr);
        }
        // `self.host` is already `host:port` (or `[v6]:port`) from the URL parser.
        let host = self.host.clone();
        // host already includes port from URL.
        let mut iter = tokio::net::lookup_host(&host)
            .await
            .map_err(|e| TrackerError::Other(format!("resolve {host}: {e}")))?;
        let addr = iter
            .next()
            .ok_or_else(|| TrackerError::Other(format!("no addresses for {host}")))?;
        *self.remote.lock() = Some(addr);
        Ok(addr)
    }

    fn should_reconnect(&self) -> bool {
        let st = self.state.lock();
        match st.issued {
            None => true,
            Some(t) => t.elapsed() >= Duration::from_secs(60),
        }
    }

    async fn ensure_connected(&self) -> Result<u64> {
        if !self.should_reconnect() {
            return Ok(self.state.lock().conn_id);
        }
        let tid = rand::thread_rng().gen::<i32>();
        let mut req = Vec::with_capacity(16);
        req.extend_from_slice(&CONNECT_REQUEST_CONNECTION_ID.to_be_bytes());
        req.extend_from_slice(&ACTION_CONNECT.to_be_bytes());
        req.extend_from_slice(&tid.to_be_bytes());

        let body = self.round_trip(tid, &req, 0).await?;
        if body.len() < 8 {
            return Err(TrackerError::UdpProtocol("short connect response".into()));
        }
        let conn_id = u64::from_be_bytes(body[0..8].try_into().unwrap());
        let mut st = self.state.lock();
        st.conn_id = conn_id;
        st.issued = Some(Instant::now());
        Ok(conn_id)
    }

    async fn round_trip(
        &self,
        expect_tid: i32,
        packet: &[u8],
        contiguous_timeouts: usize,
    ) -> Result<Vec<u8>> {
        let addr = self.resolve().await?;
        let deadline = Instant::now() + self.overall_timeout;
        let mut attempt = contiguous_timeouts;

        loop {
            if Instant::now() >= deadline {
                return Err(TrackerError::Timeout);
            }
            self.sock
                .send_to(packet, addr)
                .await
                .map_err(TrackerError::Io)?;

            let wait = udp_timeout(attempt).min(deadline.saturating_duration_since(Instant::now()));
            let mut buf = [0u8; 2048];
            match timeout(wait, self.sock.recv_from(&mut buf)).await {
                Ok(Ok((n, from))) => {
                    if from != addr {
                        continue;
                    }
                    if n < 8 {
                        return Err(TrackerError::UdpProtocol("short response".into()));
                    }
                    let action = i32::from_be_bytes(buf[0..4].try_into().unwrap());
                    let tid = i32::from_be_bytes(buf[4..8].try_into().unwrap());
                    if tid != expect_tid {
                        continue;
                    }
                    if action == ACTION_ERROR {
                        let msg = String::from_utf8_lossy(&buf[8..n]).into_owned();
                        return Err(TrackerError::UdpProtocol(msg));
                    }
                    return Ok(buf[8..n].to_vec());
                }
                Ok(Err(e)) => return Err(TrackerError::Io(e)),
                Err(_) => {
                    attempt += 1;
                    if Instant::now() >= deadline {
                        return Err(TrackerError::Timeout);
                    }
                    // Retry with doubled timeout like Go.
                    continue;
                }
            }
        }
    }

    pub async fn announce(
        &self,
        mut req: AnnounceRequest,
        opt: AnnounceOpt,
    ) -> Result<AnnounceResponse> {
        if req.ip_address == 0 {
            if let Some(IpAddr::V4(v4)) = opt.client_ip4 {
                req.ip_address = u32::from_be_bytes(v4.octets());
            }
        }
        let conn_id = self.ensure_connected().await?;
        let tid = rand::thread_rng().gen::<i32>();
        let mut packet = Vec::with_capacity(16 + 82 + 64);
        packet.extend_from_slice(&conn_id.to_be_bytes());
        packet.extend_from_slice(&ACTION_ANNOUNCE.to_be_bytes());
        packet.extend_from_slice(&tid.to_be_bytes());
        packet.extend_from_slice(&marshal_announce_request(&req));
        packet.extend_from_slice(&encode_bep41_options(&self.request_uri));

        let body = self.round_trip(tid, &packet, 0).await?;
        if body.len() < 12 {
            return Err(TrackerError::UdpProtocol("short announce response".into()));
        }
        let interval = i32::from_be_bytes(body[0..4].try_into().unwrap());
        let leechers = i32::from_be_bytes(body[4..8].try_into().unwrap());
        let seeders = i32::from_be_bytes(body[8..12].try_into().unwrap());
        let peers_bytes = &body[12..];

        let remote = self.resolve().await?;
        let peers = if remote.is_ipv6() {
            parse_compact_peers6(peers_bytes)?
        } else {
            parse_compact_peers4(peers_bytes)?
        };

        Ok(AnnounceResponse {
            interval,
            min_interval: None,
            leechers,
            seeders,
            peers,
            warning: None,
            tracker_id: None,
        })
    }

    pub async fn scrape(&self, info_hashes: &[[u8; 20]]) -> Result<ScrapeResponse> {
        let conn_id = self.ensure_connected().await?;
        let tid = rand::thread_rng().gen::<i32>();
        let mut packet = Vec::with_capacity(16 + 20 * info_hashes.len());
        packet.extend_from_slice(&conn_id.to_be_bytes());
        packet.extend_from_slice(&ACTION_SCRAPE.to_be_bytes());
        packet.extend_from_slice(&tid.to_be_bytes());
        for ih in info_hashes {
            packet.extend_from_slice(ih);
        }
        let body = self.round_trip(tid, &packet, 0).await?;
        if body.len() % 12 != 0 {
            return Err(TrackerError::UdpProtocol("bad scrape body length".into()));
        }
        let mut out = Vec::with_capacity(body.len() / 12);
        for chunk in body.as_chunks::<12>().0 {
            out.push(ScrapeItem {
                complete: i32::from_be_bytes(chunk[0..4].try_into().unwrap()),
                downloaded: i32::from_be_bytes(chunk[4..8].try_into().unwrap()),
                incomplete: i32::from_be_bytes(chunk[8..12].try_into().unwrap()),
            });
        }
        Ok(out)
    }
}

fn marshal_announce_request(r: &AnnounceRequest) -> [u8; 82] {
    let mut buf = [0u8; 82];
    buf[0..20].copy_from_slice(&r.info_hash.0);
    buf[20..40].copy_from_slice(&r.peer_id);
    buf[40..48].copy_from_slice(&r.downloaded.to_be_bytes());
    buf[48..56].copy_from_slice(&r.left.to_be_bytes());
    buf[56..64].copy_from_slice(&r.uploaded.to_be_bytes());
    buf[64..68].copy_from_slice(&(r.event as i32).to_be_bytes());
    buf[68..72].copy_from_slice(&r.ip_address.to_be_bytes());
    buf[72..76].copy_from_slice(&r.key.to_be_bytes());
    buf[76..80].copy_from_slice(&r.num_want.to_be_bytes());
    buf[80..82].copy_from_slice(&r.port.to_be_bytes());
    buf
}

fn encode_bep41_options(request_uri: &str) -> Vec<u8> {
    let mut ret = Vec::new();
    let mut rest = request_uri.as_bytes();
    while !rest.is_empty() {
        let l = rest.len().min(255);
        ret.push(OPTION_TYPE_URL_DATA);
        ret.push(l as u8);
        ret.extend_from_slice(&rest[..l]);
        rest = &rest[l..];
    }
    ret
}

fn parse_compact_peers4(b: &[u8]) -> Result<Vec<Peer>> {
    if !b.len().is_multiple_of(6) {
        return Err(TrackerError::UdpProtocol(format!(
            "peers len {} not multiple of 6",
            b.len()
        )));
    }
    let mut out = Vec::with_capacity(b.len() / 6);
    for chunk in b.as_chunks::<6>().0 {
        let ip = std::net::Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
        let port = u16::from_be_bytes([chunk[4], chunk[5]]);
        out.push(Peer::new(SocketAddr::new(std::net::IpAddr::V4(ip), port)));
    }
    Ok(out)
}

fn parse_compact_peers6(b: &[u8]) -> Result<Vec<Peer>> {
    if !b.len().is_multiple_of(18) {
        return Err(TrackerError::UdpProtocol(format!(
            "peers6 len {} not multiple of 18",
            b.len()
        )));
    }
    let mut out = Vec::with_capacity(b.len() / 18);
    for chunk in b.as_chunks::<18>().0 {
        let mut o = [0u8; 16];
        o.copy_from_slice(&chunk[..16]);
        let ip = std::net::Ipv6Addr::from(o);
        let port = u16::from_be_bytes([chunk[16], chunk[17]]);
        out.push(Peer::new(SocketAddr::new(std::net::IpAddr::V6(ip), port)));
    }
    Ok(out)
}

use std::net::IpAddr;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_backoff_matches_go() {
        assert_eq!(udp_timeout(0), Duration::from_secs(15));
        assert_eq!(udp_timeout(1), Duration::from_secs(30));
        assert_eq!(udp_timeout(2), Duration::from_secs(60));
        assert_eq!(udp_timeout(8), Duration::from_secs(3840));
        assert_eq!(udp_timeout(9), Duration::from_secs(3840));
    }

    #[test]
    fn announce_request_wire_size() {
        let r = AnnounceRequest::default();
        assert_eq!(marshal_announce_request(&r).len(), 82);
    }

    #[test]
    fn bep41_chunks() {
        let long = "x".repeat(300);
        let enc = encode_bep41_options(&long);
        // two URLData options: 255 + 45
        assert_eq!(enc[0], OPTION_TYPE_URL_DATA);
        assert_eq!(enc[1], 255);
        assert_eq!(enc[2 + 255], OPTION_TYPE_URL_DATA);
        assert_eq!(enc[2 + 255 + 1], 45);
    }
}
