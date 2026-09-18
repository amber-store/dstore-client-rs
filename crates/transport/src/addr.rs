//! Go address strings: go-iroh `netaddr` (`endpointaddr.go`, `relayurl.go`) parse and format rules,
//! `netip.ParseAddrPort`, and dstore `transport.ParseAddrs`.

use std::net::IpAddr;

/// A relay URL in its normalised Go `url.String()` form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoRelayUrl(pub String);

/// go-iroh `netaddr.TransportAddr`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GoTransportAddr {
    Relay(GoRelayUrl),
    /// An IPv4-mapped IPv6 address is kept as IPv6.
    Ip {
        ip: IpAddr,
        zone: Option<String>,
        port: u16,
    },
    Custom {
        id: u64,
        data: Vec<u8>,
    },
}

/// "relay:<url>", "ip:1.2.3.4:5", "ip:[fe80::1%en0]:7", "<id:x>_<hex>" (no "custom:").
impl std::fmt::Display for GoTransportAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        todo!()
    }
}

impl GoTransportAddr {
    pub fn is_relay(&self) -> bool {
        todo!()
    }
}

/// `netaddr.ParseRelayURL`: "failed to parse relay URL: parse \"…\": …".
pub fn parse_relay_url(s: &str) -> Result<GoRelayUrl, String> {
    todo!()
}

/// `netip.ParseAddrPort` with Go's texts.
pub fn parse_addr_port(s: &str) -> Result<(IpAddr, Option<String>, u16), String> {
    todo!()
}

/// `netaddr.ParseTransportAddr` with Go's texts.
pub fn parse_transport_addr(s: &str) -> Result<GoTransportAddr, String> {
    todo!()
}

/// Go `transport.ParseAddrs`: bare ip:port fallback, silent skip.
pub fn parse_addrs(addrs: &[String]) -> Vec<GoTransportAddr> {
    todo!()
}
