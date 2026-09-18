//! `transport/ifaces.go`: the interface addresses a node advertises.

/// Interface name prefixes skipped as bridges and tunnels.
pub const BRIDGE_PREFIXES: [&str; 10] = [
    "docker", "br-", "cni", "flannel", "veth", "virbr", "lxc", "utun", "awdl", "llw",
];

/// `interfaceIPs`: the `transport/ifaces.go` filters (node-side advertise).
pub fn interface_ips() -> Vec<std::net::IpAddr> {
    todo!()
}
