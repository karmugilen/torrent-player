//! BEP 40 peer priority (CRC32C over masked IPs / ports).
//!
//! Port of `engine-go/patches/torrent/bep40.go`.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crc32c::crc32c;

/// Compute BEP 40 peer priority for two endpoints.
///
/// Returns 0 when the addresses are incomparable (mixed v4/v6). Matches Go's
/// `bep40PriorityIgnoreError`.
pub fn priority(a: SocketAddr, b: SocketAddr) -> u32 {
    priority_checked(a, b).unwrap_or_default()
}

/// Same as [`priority`] but returns an error for incomparable address families.
pub fn priority_checked(a: SocketAddr, b: SocketAddr) -> Result<u32, Bep40Error> {
    let bs = priority_bytes(a, b)?;
    let mid = bs.len() / 2;
    let (left, right) = (&bs[..mid], &bs[mid..]);
    let ordered = if left > right {
        [right, left].concat()
    } else {
        bs
    };
    Ok(crc32c(&ordered))
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Bep40Error {
    #[error("incomparable IPs")]
    Incomparable,
}

fn priority_bytes(a: SocketAddr, b: SocketAddr) -> Result<Vec<u8>, Bep40Error> {
    if a.ip() == b.ip() {
        let mut ret = [0u8; 4];
        ret[0..2].copy_from_slice(&a.port().to_be_bytes());
        ret[2..4].copy_from_slice(&b.port().to_be_bytes());
        return Ok(ret.to_vec());
    }

    match (a.ip(), b.ip()) {
        (IpAddr::V4(a4), IpAddr::V4(b4)) => {
            let m = ipv4_mask(a4, b4);
            let mut out = Vec::with_capacity(8);
            out.extend_from_slice(&mask_v4(a4, m));
            out.extend_from_slice(&mask_v4(b4, m));
            Ok(out)
        }
        (IpAddr::V6(a6), IpAddr::V6(b6)) => {
            // Treat v4-mapped as v4 when both are mapped.
            if let (Some(a4), Some(b4)) = (a6.to_ipv4_mapped(), b6.to_ipv4_mapped()) {
                return priority_bytes(
                    SocketAddr::new(IpAddr::V4(a4), a.port()),
                    SocketAddr::new(IpAddr::V4(b4), b.port()),
                );
            }
            let m = ipv6_mask(a6, b6)?;
            let mut out = Vec::with_capacity(32);
            out.extend_from_slice(&mask_v6(a6, &m));
            out.extend_from_slice(&mask_v6(b6, &m));
            Ok(out)
        }
        _ => Err(Bep40Error::Incomparable),
    }
}

fn same_subnet_v4(ones: u8, a: Ipv4Addr, b: Ipv4Addr) -> bool {
    let mask = ipv4_cidr_mask(ones);
    mask_v4(a, mask) == mask_v4(b, mask)
}

fn ipv4_cidr_mask(ones: u8) -> [u8; 4] {
    let bits: u32 = if ones >= 32 {
        u32::MAX
    } else if ones == 0 {
        0
    } else {
        (!0u32) << (32 - ones)
    };
    bits.to_be_bytes()
}

fn ipv4_mask(a: Ipv4Addr, b: Ipv4Addr) -> [u8; 4] {
    if !same_subnet_v4(16, a, b) {
        return [0xff, 0xff, 0x55, 0x55];
    }
    if !same_subnet_v4(24, a, b) {
        return [0xff, 0xff, 0xff, 0x55];
    }
    [0xff, 0xff, 0xff, 0xff]
}

fn mask_v4(ip: Ipv4Addr, mask: [u8; 4]) -> [u8; 4] {
    let o = ip.octets();
    [
        o[0] & mask[0],
        o[1] & mask[1],
        o[2] & mask[2],
        o[3] & mask[3],
    ]
}

fn same_subnet_v6(ones: u16, a: Ipv6Addr, b: Ipv6Addr) -> bool {
    let mask = ipv6_cidr_mask(ones);
    mask_v6(a, &mask) == mask_v6(b, &mask)
}

fn ipv6_cidr_mask(ones: u16) -> [u8; 16] {
    let mut out = [0u8; 16];
    let full = (ones / 8) as usize;
    let rem = ones % 8;
    for b in out.iter_mut().take(full.min(16)) {
        *b = 0xff;
    }
    if full < 16 && rem > 0 {
        out[full] = (!0u8) << (8 - rem);
    }
    out
}

/// Go `ipv6Mask`: walk prefix length from 6..=16 octets until the subnet differs.
fn ipv6_mask(a: Ipv6Addr, b: Ipv6Addr) -> Result<[u8; 16], Bep40Error> {
    for i in 6..=16 {
        if !same_subnet_v6(i * 8, a, b) {
            return Ok(mask_prefix_then_55(i as usize, 16));
        }
    }
    // Identical addresses are handled earlier; reaching here means equal through /128.
    Err(Bep40Error::Incomparable)
}

fn mask_prefix_then_55(prefix_bytes: usize, total: usize) -> [u8; 16] {
    let mut ret = [0x55u8; 16];
    debug_assert!(total == 16);
    for b in ret.iter_mut().take(prefix_bytes.min(16)) {
        *b = 0xff;
    }
    ret
}

fn mask_v6(ip: Ipv6Addr, mask: &[u8; 16]) -> [u8; 16] {
    let o = ip.octets();
    let mut out = [0u8; 16];
    for i in 0..16 {
        out[i] = o[i] & mask[i];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddrV4};

    fn v4(a: u8, b: u8, c: u8, d: u8) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(a, b, c, d), 0))
    }

    #[test]
    fn bep40_priority_vectors() {
        // From Go bep40_test.go
        assert_eq!(
            priority(v4(123, 213, 32, 10), v4(98, 76, 54, 32)),
            0xec2d7224
        );
        assert_eq!(
            priority(v4(98, 76, 54, 32), v4(123, 213, 32, 10)),
            0xec2d7224
        );
        assert_eq!(
            priority(v4(123, 213, 32, 10), v4(123, 213, 32, 234)),
            0x99568189
        );
        assert_eq!(
            priority(v4(206, 248, 98, 111), v4(142, 147, 89, 224)),
            0x2b41d456
        );
    }

    #[test]
    fn same_ip_returns_ports() {
        let a = v4(123, 213, 32, 234);
        let bytes = priority_bytes(a, a).unwrap();
        assert_eq!(bytes, [0, 0, 0, 0]);
    }
}
