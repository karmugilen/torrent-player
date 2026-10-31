//! Local Service Discovery (BEP 14): find peers with the same torrent on the
//! local network through multicast announces.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use tokio::net::UdpSocket;

pub const LSD_GROUP: Ipv4Addr = Ipv4Addr::new(239, 192, 152, 143);
pub const LSD_PORT: u16 = 6771;
/// BEP 14: announce each torrent at most once a minute; we use 5 minutes.
pub const ANNOUNCE_EVERY: std::time::Duration = std::time::Duration::from_secs(300);
/// Info hashes per announce; keeps a datagram well under 1400 bytes.
pub const HASHES_PER_ANNOUNCE: usize = 20;

/// A socket joined to the LSD multicast group. Other BitTorrent clients on
/// this device may hold the port too, so it is shared.
pub fn bind() -> std::io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, LSD_PORT)).into())?;
    socket.join_multicast_v4(&LSD_GROUP, &Ipv4Addr::UNSPECIFIED)?;
    socket.set_multicast_loop_v4(false)?;
    UdpSocket::from_std(socket.into())
}

/// One announce for up to [`HASHES_PER_ANNOUNCE`] hex info hashes.
pub fn announce(port: u16, cookie: &str, info_hashes: &[String]) -> String {
    let mut msg = format!("BT-SEARCH * HTTP/1.1\r\nHost: {LSD_GROUP}:{LSD_PORT}\r\nPort: {port}\r\n");
    for hash in info_hashes {
        msg.push_str("Infohash: ");
        msg.push_str(hash);
        msg.push_str("\r\n");
    }
    msg.push_str("cookie: ");
    msg.push_str(cookie);
    msg.push_str("\r\n\r\n\r\n");
    msg
}

/// An announce from another client: its port and the info hashes it has.
#[derive(Debug, PartialEq, Eq)]
pub struct Announce {
    pub port: u16,
    pub cookie: Option<String>,
    pub info_hashes: Vec<[u8; 20]>,
}

pub fn parse(datagram: &[u8]) -> Option<Announce> {
    let text = std::str::from_utf8(datagram).ok()?;
    let mut lines = text.split("\r\n");
    if !lines.next()?.starts_with("BT-SEARCH * HTTP/1.1") {
        return None;
    }
    let mut port = None;
    let mut cookie = None;
    let mut info_hashes = Vec::new();
    for line in lines {
        let Some((key, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        if key.eq_ignore_ascii_case("port") {
            port = value.parse::<u16>().ok().filter(|&p| p != 0);
        } else if key.eq_ignore_ascii_case("cookie") {
            cookie = Some(value.to_string());
        } else if key.eq_ignore_ascii_case("infohash") {
            let mut hash = [0u8; 20];
            if value.len() == 40 && hex::decode_to_slice(value, &mut hash).is_ok() {
                info_hashes.push(hash);
            }
        }
    }
    Some(Announce { port: port?, cookie, info_hashes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announce_round_trips() {
        let hash = "0123456789abcdef0123456789abcdef01234567".to_string();
        let msg = announce(6881, "c00k1e", std::slice::from_ref(&hash));
        let parsed = parse(msg.as_bytes()).unwrap();
        assert_eq!(parsed.port, 6881);
        assert_eq!(parsed.cookie.as_deref(), Some("c00k1e"));
        let mut expected = [0u8; 20];
        hex::decode_to_slice(&hash, &mut expected).unwrap();
        assert_eq!(parsed.info_hashes, vec![expected]);
        assert!(parse(b"M-SEARCH * HTTP/1.1\r\n\r\n").is_none());
    }
}
