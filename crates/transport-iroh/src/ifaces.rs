//! `transport/ifaces.go`: the interface addresses a node advertises.
//!
//! Go's `net.Interfaces()` lists interfaces in index order and each interface's addresses in OS order.
//! `getifaddrs` returns the same entries (plus link-layer ones, which carry no IP); a stable sort by
//! interface index gives Go's order on Linux (netlink dumps IPv4 before IPv6) and on the BSDs (the
//! `NET_RT_IFLIST` sysctl both use).

use std::collections::HashMap;
use std::net::IpAddr;

use nix::ifaddrs::getifaddrs;
use nix::net::if_::{InterfaceFlags, if_nametoindex};
use nix::sys::socket::SockaddrStorage;

/// Interface name prefixes skipped as bridges and tunnels.
pub const BRIDGE_PREFIXES: [&str; 10] = [
    "docker", "br-", "cni", "flannel", "veth", "virbr", "lxc", "utun", "awdl", "llw",
];

/// `interfaceIPs`: the `transport/ifaces.go` filters (node-side advertise).
///
/// The machine's dialable unicast addresses: interfaces that are up and not bridges (case-sensitive
/// prefixes), each address unmapped, without duplicates, loopback, link-local unicast, link-local
/// multicast or unspecified addresses. A failure to list the interfaces gives no addresses.
pub fn interface_ips() -> Vec<IpAddr> {
    let Ok(list) = getifaddrs() else {
        return Vec::new();
    };
    let mut indexes: HashMap<String, u32> = HashMap::new();
    let mut addrs = Vec::new();
    for a in list {
        let Some(ip) = a.address.as_ref().and_then(sockaddr_ip) else {
            continue;
        };
        let index = *indexes
            .entry(a.interface_name.clone())
            .or_insert_with(|| if_nametoindex(a.interface_name.as_str()).unwrap_or(u32::MAX));
        addrs.push(IfaceAddr {
            index,
            up: a.flags.contains(InterfaceFlags::IFF_UP),
            name: a.interface_name,
            ip,
        });
    }
    addrs.sort_by_key(|a| a.index);
    filter_ips(&addrs)
}

/// One interface address as `net.Interfaces()` + `ifc.Addrs()` present it.
#[derive(Clone, Debug)]
struct IfaceAddr {
    name: String,
    index: u32,
    up: bool,
    ip: IpAddr,
}

/// The IP of an `AF_INET`/`AF_INET6` address (Go's `*net.IPNet` entries).
fn sockaddr_ip(s: &SockaddrStorage) -> Option<IpAddr> {
    if let Some(v4) = s.as_sockaddr_in() {
        return Some(IpAddr::V4(v4.ip()));
    }
    s.as_sockaddr_in6().map(|v6| IpAddr::V6(v6.ip()))
}

/// `isBridgeName`.
fn is_bridge_name(name: &str) -> bool {
    BRIDGE_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// The loop of `interfaceIPs` over the listed addresses, in order.
fn filter_ips(addrs: &[IfaceAddr]) -> Vec<IpAddr> {
    let mut out: Vec<IpAddr> = Vec::new();
    for a in addrs {
        if !a.up || is_bridge_name(&a.name) {
            continue;
        }
        let ip = unmap(a.ip);
        if out.contains(&ip)
            || ip.is_loopback()
            || is_link_local_unicast(ip)
            || is_link_local_multicast(ip)
            || ip.is_unspecified()
        {
            continue;
        }
        out.push(ip);
    }
    out
}

/// `netip.Addr.Unmap`: `::ffff:a.b.c.d` → `a.b.c.d`.
fn unmap(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        IpAddr::V4(_) => ip,
    }
}

/// `netip.Addr.IsLinkLocalUnicast`: 169.254.0.0/16, fe80::/10.
fn is_link_local_unicast(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.octets()[..2] == [169, 254],
        IpAddr::V6(v6) => v6.segments()[0] & 0xffc0 == 0xfe80,
    }
}

/// `netip.Addr.IsLinkLocalMulticast`: 224.0.0.0/24, and IPv6 `ff02::/16` under Go's `0xff0f` mask.
fn is_link_local_multicast(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.octets()[..3] == [224, 0, 0],
        IpAddr::V6(v6) => v6.segments()[0] & 0xff0f == 0xff02,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(name: &str, index: u32, up: bool, ip: &str) -> IfaceAddr {
        IfaceAddr {
            name: name.to_string(),
            index,
            up,
            ip: ip.parse().expect("test ip"),
        }
    }

    fn ips(list: &[&str]) -> Vec<IpAddr> {
        list.iter().map(|s| s.parse().expect("test ip")).collect()
    }

    #[test]
    fn keeps_unicast_addresses_of_up_interfaces_in_order() {
        let list = [
            a("lo0", 1, true, "127.0.0.1"),
            a("lo0", 1, true, "::1"),
            a("en0", 4, true, "192.168.1.20"),
            a("en0", 4, true, "fe80::1c2b:3aff:fe00:1"),
            a("en0", 4, true, "2001:db8::20"),
            a("en1", 5, false, "10.0.0.9"),
            a("eth1", 6, true, "10.1.2.3"),
        ];
        assert_eq!(
            filter_ips(&list),
            ips(&["192.168.1.20", "2001:db8::20", "10.1.2.3"])
        );
    }

    #[test]
    fn skips_bridge_prefixes_case_sensitively() {
        let mut list: Vec<IfaceAddr> = BRIDGE_PREFIXES
            .iter()
            .enumerate()
            .map(|(i, p)| {
                a(
                    &format!("{p}0"),
                    i as u32 + 1,
                    true,
                    &format!("10.9.0.{}", i + 1),
                )
            })
            .collect();
        list.push(a("Docker0", 20, true, "10.8.0.1"));
        list.push(a("eth-docker", 21, true, "10.8.0.2"));
        list.push(a("br0", 22, true, "10.8.0.3"));
        assert_eq!(
            filter_ips(&list),
            ips(&["10.8.0.1", "10.8.0.2", "10.8.0.3"])
        );
    }

    #[test]
    fn unmaps_and_deduplicates() {
        let list = [
            a("en0", 4, true, "::ffff:10.0.0.1"),
            a("en0", 4, true, "10.0.0.1"),
            a("en1", 5, true, "10.0.0.1"),
            a("en1", 5, true, "::ffff:127.0.0.1"),
        ];
        assert_eq!(filter_ips(&list), ips(&["10.0.0.1"]));
    }

    #[test]
    fn skips_link_local_multicast_and_unspecified() {
        let list = [
            a("en0", 4, true, "169.254.10.1"),
            a("en0", 4, true, "169.253.10.1"),
            a("en0", 4, true, "224.0.0.251"),
            a("en0", 4, true, "224.0.1.1"),
            a("en0", 4, true, "0.0.0.0"),
            a("en0", 4, true, "::"),
            a("en0", 4, true, "febf::1"),
            a("en0", 4, true, "fec0::1"),
            a("en0", 4, true, "ff02::fb"),
            a("en0", 4, true, "ff12::1"),
            a("en0", 4, true, "ff05::2"),
        ];
        assert_eq!(
            filter_ips(&list),
            ips(&["169.253.10.1", "224.0.1.1", "fec0::1", "ff05::2"])
        );
    }

    #[test]
    fn netip_predicates() {
        assert_eq!(
            unmap("::ffff:1.2.3.4".parse().expect("ip")),
            ips(&["1.2.3.4"])[0]
        );
        assert_eq!(
            unmap("::1.2.3.4".parse().expect("ip")),
            ips(&["::1.2.3.4"])[0]
        );
        assert!(is_link_local_unicast("fe80::1".parse().expect("ip")));
        assert!(is_link_local_unicast("febf:ffff::1".parse().expect("ip")));
        assert!(!is_link_local_unicast("fe7f::1".parse().expect("ip")));
        assert!(is_link_local_multicast("224.0.0.0".parse().expect("ip")));
        assert!(!is_link_local_multicast("225.0.0.1".parse().expect("ip")));
        assert!(is_link_local_multicast("ff32::1".parse().expect("ip")));
        assert!(!is_link_local_multicast("ff01::1".parse().expect("ip")));
        assert!(is_bridge_name("utun3"));
        assert!(!is_bridge_name("en0"));
    }
}
