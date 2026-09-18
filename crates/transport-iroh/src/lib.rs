//! dstore `transport/iroh.go` and `transport/ifaces.go` on Rust iroh 1.2.0, `relayModeOf`
//! (`cmd/dstore/main.go:123-137`), the go-iroh v0.2.0 default relay map, the lookup semantics of go-iroh
//! `iroh/endpoint.go`, and a port of the go-iroh `iroh/mdns` resolver.
//!
//! Spec: PORTING.md §4.7, §5.12; port-notes/transport.md §2.3, §2.5-§2.8, §2.10, §3.2-§3.4, §4.5-§4.8.
#![deny(unsafe_op_in_unsafe_fn)]

mod conn;
mod endpoint;
pub mod ifaces;
pub mod mdns;

pub use conn::*;
pub use endpoint::*;

/// The iroh release this transport is built on (`=1.2.0`). The public API names its types
/// ([`IrohConfig::secret_key`], [`IrohEndpoint::raw`], [`to_iroh_addr`]), so callers use this re-export to
/// get the same version.
pub use iroh;

/// go-iroh `relay.DefaultMap()` hosts (`relay/relay.go:23-30`).
pub const GO_DEFAULT_RELAYS: [&str; 4] = [
    "https://use1-1.relay.n0.iroh-canary.iroh.link.",
    "https://usw1-1.relay.n0.iroh-canary.iroh.link.",
    "https://euc1-1.relay.n0.iroh-canary.iroh.link.",
    "https://aps1-1.relay.n0.iroh-canary.iroh.link.",
];
pub const RELAY_QUIC_PORT: u16 = 7842;
pub const KEEP_ALIVE: std::time::Duration = std::time::Duration::from_secs(5);
pub const MAX_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
pub const MAX_INCOMING_BIDI_STREAMS: u32 = 1024;
/// noq's initial RTT, set explicitly: a path whose RTT equals it counts as not measured (DD-5).
pub const NOQ_INITIAL_RTT: std::time::Duration = std::time::Duration::from_millis(333);
pub const STREAM_RECEIVE_WINDOW: u32 = 16 << 20;
pub const SEND_WINDOW: u64 = 64 << 20;
pub const DIRECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
pub const ONLINE_WAIT: std::time::Duration = std::time::Duration::from_secs(10);
pub const MDNS_LOOKUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
pub const CONNECT_PHASE_CAP: std::time::Duration = std::time::Duration::from_secs(10);
pub const CLOSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
