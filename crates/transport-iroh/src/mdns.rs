//! A port of the go-iroh v0.2.0 `iroh/mdns` resolver (`mdns.go`, `dnsmsg.go`): queries and announcement
//! parsing compatible with go-iroh nodes (records from any section).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dstore_gocompat::ctx::Ctx;
use dstore_gocompat::slog::Logger;
use tokio_util::sync::CancellationToken;

pub const SERVICE_NAME: &str = "irohv1";

/// RFC 4648 base32, lowercase, no padding.
pub fn endpoint_label(id: &[u8; 32]) -> String {
    todo!()
}

/// Two PTR questions, no compression.
pub fn build_query(id: &[u8; 32]) -> Vec<u8> {
    todo!()
}

/// An endpoint announcement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Announcement {
    pub id: [u8; 32],
    pub addrs: Vec<SocketAddr>,
    pub relay: Option<String>,
    pub user_data: Option<String>,
}

/// Records from any section; compression pointers ≤ 32.
pub fn parse_announcement(packet: &[u8]) -> Option<Announcement> {
    todo!()
}

/// The mDNS resolver: sockets on every up multicast interface and an announcement cache.
pub struct MdnsResolver {
    logger: Logger,
    cache: Mutex<HashMap<[u8; 32], Announcement>>,
    sockets: Vec<Arc<tokio::net::UdpSocket>>,
    bg: CancellationToken,
}

impl MdnsResolver {
    /// Joins 224.0.0.251:5353 (required) and [ff02::fb]:5353 (best effort) with
    /// SO_REUSEADDR+SO_REUSEPORT on every up multicast interface; failure → Err(text), and the caller logs
    /// WARN "transport: mdns discovery unavailable" error=<text>.
    pub async fn start(logger: Logger) -> Result<Arc<MdnsResolver>, String> {
        todo!()
    }

    /// Cache hit → at once; else send the query and poll the cache every 25 ms until `timeout` or ctx
    /// end.
    pub async fn resolve(
        &self,
        ctx: &Ctx,
        id: [u8; 32],
        timeout: Duration,
    ) -> Option<Announcement> {
        todo!()
    }
}
