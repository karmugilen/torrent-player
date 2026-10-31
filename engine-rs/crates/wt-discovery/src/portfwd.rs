//! Router port mapping so peers can connect to us: UPnP IGD (Go anacrolix
//! `forwardPort`, TCP and UDP for the listen port) with NAT-PMP (RFC 6886)
//! as a fallback for routers without UPnP.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::time::{timeout, Instant};

const SSDP_ADDR: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(239, 255, 255, 250), 1900);
/// Go `upnp.Discover(0, 2*time.Second)`.
const DISCOVER_FOR: Duration = Duration::from_secs(2);
const HTTP_TIMEOUT: Duration = Duration::from_secs(3);
const NATPMP_PORT: u16 = 5351;
/// NAT-PMP lease; mappings are renewed well before it ends.
const NATPMP_LIFETIME: u32 = 7200;
const SEARCH_TARGETS: [&str; 4] = [
    "urn:schemas-upnp-org:device:InternetGatewayDevice:1",
    "urn:schemas-upnp-org:device:InternetGatewayDevice:2",
    "urn:schemas-upnp-org:service:WANIPConnection:1",
    "urn:schemas-upnp-org:service:WANPPPConnection:1",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    Tcp,
    Udp,
}

impl Protocol {
    fn name(self) -> &'static str {
        match self {
            Protocol::Tcp => "TCP",
            Protocol::Udp => "UDP",
        }
    }
}

/// A mapping we added, kept so it can be removed on close.
#[derive(Clone, Debug)]
pub enum PortMapping {
    Upnp { control_url: String, service_type: String, protocol: Protocol, external_port: u16 },
    NatPmp { gateway: Ipv4Addr, protocol: Protocol, internal_port: u16 },
}

/// Map `port` (TCP and UDP) on the local router. Returns the mappings that
/// succeeded; empty when no router answered (mobile data, no UPnP/NAT-PMP).
pub async fn map_port(port: u16, description: &str) -> Vec<PortMapping> {
    let mut out = Vec::new();
    for gateway in discover_upnp().await {
        for protocol in [Protocol::Tcp, Protocol::Udp] {
            if upnp_add(&gateway, protocol, port, description).await.is_ok() {
                out.push(PortMapping::Upnp {
                    control_url: gateway.control_url.clone(),
                    service_type: gateway.service_type.clone(),
                    protocol,
                    external_port: port,
                });
            }
        }
    }
    if !out.is_empty() {
        return out;
    }
    if let Some(gateway) = default_gateway().await {
        for protocol in [Protocol::Tcp, Protocol::Udp] {
            if natpmp_map(gateway, protocol, port, NATPMP_LIFETIME).await.is_ok() {
                out.push(PortMapping::NatPmp { gateway, protocol, internal_port: port });
            }
        }
    }
    out
}

/// Remove mappings added by [`map_port`] (Go `clearPortMappings`).
pub async fn unmap(mappings: &[PortMapping]) {
    for m in mappings {
        match m {
            PortMapping::Upnp { control_url, service_type, protocol, external_port } => {
                let args = format!(
                    "<NewRemoteHost></NewRemoteHost><NewExternalPort>{external_port}</NewExternalPort>\
                     <NewProtocol>{}</NewProtocol>",
                    protocol.name()
                );
                let _ = soap(control_url, service_type, "DeletePortMapping", &args).await;
            }
            PortMapping::NatPmp { gateway, protocol, internal_port } => {
                let _ = natpmp_map(*gateway, *protocol, *internal_port, 0).await;
            }
        }
    }
}

#[derive(Clone, Debug)]
struct UpnpGateway {
    control_url: String,
    service_type: String,
    local_ip: IpAddr,
}

async fn discover_upnp() -> Vec<UpnpGateway> {
    let Ok(socket) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await else {
        return Vec::new();
    };
    for st in SEARCH_TARGETS {
        let msg = format!(
            "M-SEARCH * HTTP/1.1\r\nHOST: {SSDP_ADDR}\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: {st}\r\n\r\n"
        );
        let _ = socket.send_to(msg.as_bytes(), SSDP_ADDR).await;
    }
    let mut locations: Vec<String> = Vec::new();
    let deadline = Instant::now() + DISCOVER_FOR;
    let mut buf = [0u8; 2048];
    while let Ok(Ok((n, _))) = tokio::time::timeout_at(deadline, socket.recv_from(&mut buf)).await {
        let text = String::from_utf8_lossy(&buf[..n]);
        if let Some(location) = header(&text, "location") {
            if !locations.iter().any(|l| l == location) {
                locations.push(location.to_string());
            }
        }
    }
    let mut gateways: Vec<UpnpGateway> = Vec::new();
    for location in locations {
        let Some(gateway) = describe(&location).await else { continue };
        if !gateways.iter().any(|g| g.control_url == gateway.control_url) {
            gateways.push(gateway);
        }
    }
    gateways
}

/// Case-insensitive HTTP-style header value from an SSDP response.
fn header<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    text.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

/// Fetch a device description and find its WAN connection service.
async fn describe(location: &str) -> Option<UpnpGateway> {
    let url = url::Url::parse(location).ok()?;
    let client = http_client()?;
    let xml = timeout(HTTP_TIMEOUT, async { client.get(url.clone()).send().await?.text().await })
        .await
        .ok()?
        .ok()?;
    let (service_type, control) = wan_service(&xml)?;
    let base = tag(&xml, "URLBase")
        .and_then(|b| url::Url::parse(b.trim()).ok())
        .unwrap_or_else(|| url.clone());
    let control_url = base.join(control.trim()).ok()?.to_string();
    let local_ip = local_ip_towards(url.host_str()?, url.port_or_known_default()?).await?;
    Some(UpnpGateway { control_url, service_type, local_ip })
}

/// The first `<service>` whose type is a WAN IP or PPP connection.
fn wan_service(xml: &str) -> Option<(String, String)> {
    let mut rest = xml;
    while let Some(start) = rest.find("<service>") {
        let after = &rest[start + "<service>".len()..];
        let end = after.find("</service>")?;
        let block = &after[..end];
        if let (Some(st), Some(control)) = (tag(block, "serviceType"), tag(block, "controlURL")) {
            let st = st.trim();
            if st.contains("WANIPConnection") || st.contains("WANPPPConnection") {
                return Some((st.to_string(), control.to_string()));
            }
        }
        rest = &after[end..];
    }
    None
}

fn tag<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(&xml[start..end])
}

async fn local_ip_towards(host: &str, port: u16) -> Option<IpAddr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await.ok()?;
    socket.connect((host, port)).await.ok()?;
    Some(socket.local_addr().ok()?.ip())
}

fn http_client() -> Option<reqwest::Client> {
    reqwest::Client::builder().timeout(HTTP_TIMEOUT).no_proxy().build().ok()
}

async fn upnp_add(gateway: &UpnpGateway, protocol: Protocol, port: u16, description: &str) -> Result<(), ()> {
    // Go maps permanently (lease 0); some routers only accept a finite lease.
    for lease in [0u32, 3600] {
        let args = format!(
            "<NewRemoteHost></NewRemoteHost><NewExternalPort>{port}</NewExternalPort>\
             <NewProtocol>{}</NewProtocol><NewInternalPort>{port}</NewInternalPort>\
             <NewInternalClient>{}</NewInternalClient><NewEnabled>1</NewEnabled>\
             <NewPortMappingDescription>{}</NewPortMappingDescription>\
             <NewLeaseDuration>{lease}</NewLeaseDuration>",
            protocol.name(),
            gateway.local_ip,
            xml_escape(description)
        );
        if soap(&gateway.control_url, &gateway.service_type, "AddPortMapping", &args).await.is_ok() {
            return Ok(());
        }
    }
    Err(())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

async fn soap(control_url: &str, service_type: &str, action: &str, args: &str) -> Result<(), ()> {
    let body = format!(
        "<?xml version=\"1.0\"?>\
         <s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
         s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body>\
         <u:{action} xmlns:u=\"{service_type}\">{args}</u:{action}></s:Body></s:Envelope>"
    );
    let client = http_client().ok_or(())?;
    let response = client
        .post(control_url)
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .header("SOAPAction", format!("\"{service_type}#{action}\""))
        .body(body)
        .send()
        .await
        .map_err(|_| ())?;
    if response.status().is_success() { Ok(()) } else { Err(()) }
}

/// The default IPv4 gateway: the kernel route table where readable, else
/// the usual `.1` address of the local /24.
async fn default_gateway() -> Option<Ipv4Addr> {
    if let Ok(routes) = tokio::fs::read_to_string("/proc/net/route").await {
        if let Some(gw) = parse_default_route(&routes) {
            return Some(gw);
        }
    }
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await.ok()?;
    // Connecting a UDP socket sends nothing; it only picks the route.
    socket.connect(SocketAddr::from(([192, 0, 2, 1], 9))).await.ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if ip.is_private() => {
            let [a, b, c, _] = ip.octets();
            Some(Ipv4Addr::new(a, b, c, 1))
        }
        _ => None,
    }
}

fn parse_default_route(table: &str) -> Option<Ipv4Addr> {
    table.lines().skip(1).find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 3 || fields[1] != "00000000" {
            return None;
        }
        let gw = u32::from_str_radix(fields[2], 16).ok()?;
        (gw != 0).then(|| Ipv4Addr::from(gw.to_le_bytes()))
    })
}

/// RFC 6886 mapping request; lifetime 0 removes the mapping.
async fn natpmp_map(gateway: Ipv4Addr, protocol: Protocol, port: u16, lifetime: u32) -> Result<(), ()> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await.map_err(|_| ())?;
    socket.connect((gateway, NATPMP_PORT)).await.map_err(|_| ())?;
    let opcode = match protocol {
        Protocol::Udp => 1u8,
        Protocol::Tcp => 2u8,
    };
    let mut req = [0u8; 12];
    req[1] = opcode;
    req[4..6].copy_from_slice(&port.to_be_bytes());
    req[6..8].copy_from_slice(&(if lifetime == 0 { 0 } else { port }).to_be_bytes());
    req[8..12].copy_from_slice(&lifetime.to_be_bytes());
    let mut wait = Duration::from_millis(250);
    let mut buf = [0u8; 16];
    for _ in 0..3 {
        socket.send(&req).await.map_err(|_| ())?;
        if let Ok(Ok(n)) = timeout(wait, socket.recv(&mut buf)).await {
            if n >= 16 && buf[1] == 128 + opcode {
                return if u16::from_be_bytes([buf[2], buf[3]]) == 0 { Ok(()) } else { Err(()) };
            }
        }
        wait *= 2;
    }
    Err(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_wan_service_and_ssdp_location() {
        let xml = "<root><URLBase>http://192.168.1.1:5000/</URLBase><device><serviceList>\
            <service><serviceType>urn:schemas-upnp-org:service:Layer3Forwarding:1</serviceType>\
            <controlURL>/l3f</controlURL></service>\
            <service><serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType>\
            <controlURL>/ctl/IPConn</controlURL></service></serviceList></device></root>";
        let (st, control) = wan_service(xml).unwrap();
        assert_eq!(st, "urn:schemas-upnp-org:service:WANIPConnection:1");
        assert_eq!(control, "/ctl/IPConn");
        assert_eq!(tag(xml, "URLBase"), Some("http://192.168.1.1:5000/"));
        let reply = "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=120\r\nLocation: http://192.168.1.1:5000/rootDesc.xml\r\n\r\n";
        assert_eq!(header(reply, "location"), Some("http://192.168.1.1:5000/rootDesc.xml"));
    }

    #[test]
    fn reads_the_default_gateway_from_the_route_table() {
        let table = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\n\
            wlan0\t0001A8C0\t00000000\t0001\t0\t0\t0\t00FFFFFF\n\
            wlan0\t00000000\t0101A8C0\t0003\t0\t0\t0\t00000000\n";
        assert_eq!(parse_default_route(table), Some(Ipv4Addr::new(192, 168, 1, 1)));
    }
}
