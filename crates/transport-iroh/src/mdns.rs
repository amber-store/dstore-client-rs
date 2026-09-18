//! A port of the go-iroh v0.2.0 `iroh/mdns` resolver (`mdns.go`, `dnsmsg.go`, `reuse_unix.go`): queries and
//! announcement parsing compatible with go-iroh nodes (records from any section).
//!
//! Only the resolver side is ported (PORTING.md §4.7, transport §3.3, §4.6, Addenda 2): a client listens,
//! caches every announcement it hears and multicasts queries. The responder and publisher are node-side
//! and out of v1, so a Rust client answers no query, as a passive go-iroh `Discovery` does.
//!
//! Go normative sources: `iroh/mdns/mdns.go` (`Start`, `readLoop`, `listenIPv4MDNS`, `listenIPv6MDNS`,
//! `listenMDNS`, `joinGroup`, `Resolve`, `handlePacket`, `query`, `writeMulticast`, `endpointLabel`,
//! `parseEndpointLabel`, `serviceName`, `instanceName`, `infoFromAnnouncement`), `iroh/mdns/dnsmsg.go`
//! (`buildQuery`, `parseAnnouncement`, `parseDNS`, `readRR`, `readName`, `parseTXT`), `dns/endpointinfo.go`
//! (`AddIPAddrs` de-duplication, `NewUserData`), `key/key.go` (`ParseEndpointID`), and x/net v0.56.0
//! `ipv4`/`ipv6` `JoinGroup` (`MCAST_JOIN_GROUP` by interface index).

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::os::fd::{AsRawFd, RawFd};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use dstore_gocompat::ctx::Ctx;
use dstore_gocompat::errno::io_error_text;
use dstore_gocompat::slog::{Attr, Logger};
use nix::libc;
use nix::net::if_::InterfaceFlags;
use socket2::{Domain, Socket, Type};
use tokio::net::UdpSocket;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

/// go-iroh `DefaultServiceName`.
pub const SERVICE_NAME: &str = "irohv1";

/// `serviceName(SERVICE_NAME)`: the DNS-SD service domain.
const SERVICE_DOMAIN: &str = "_irohv1._udp.local";
/// `"." + serviceName(SERVICE_NAME)`: the suffix of an instance name.
const INSTANCE_SUFFIX: &[u8] = b"._irohv1._udp.local";

const MDNS_PORT: u16 = 5353;
const IPV4_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const IPV6_GROUP: Ipv6Addr = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 0xfb);

/// go-iroh `defaultLookupTimeout`, used when a non-positive timeout is given (`WithLookupTimeout`).
const DEFAULT_LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);
/// `Resolve` polls the cache on a 25 ms ticker.
const POLL_INTERVAL: Duration = Duration::from_millis(25);
/// `readLoop`'s buffer: longer datagrams are truncated to it.
const READ_BUFFER: usize = 1500;

const DNS_TYPE_A: u16 = 1;
const DNS_TYPE_PTR: u16 = 12;
const DNS_TYPE_TXT: u16 = 16;
const DNS_TYPE_AAAA: u16 = 28;
const DNS_TYPE_SRV: u16 = 33;
const DNS_CLASS_IN: u16 = 1;

/// `readName` follows at most 32 steps, labels and compression pointers together.
const MAX_NAME_STEPS: usize = 32;
/// go-iroh `dns.UserDataMaxLength`.
const USER_DATA_MAX_LENGTH: usize = 245;

/// RFC 4648 base32, lowercase, no padding.
pub fn endpoint_label(id: &[u8; 32]) -> String {
    data_encoding::BASE32_NOPAD.encode(id).to_ascii_lowercase()
}

/// `instanceName(SERVICE_NAME, id)`.
fn instance_name(id: &[u8; 32]) -> String {
    format!("{}.{SERVICE_DOMAIN}", endpoint_label(id))
}

/// Two PTR questions, no compression.
///
/// `buildQuery(serviceName("irohv1"), instanceName("irohv1", id))`: a 12-byte header that is all zero except
/// QDCOUNT = 2, then the service and the instance name, each asked as PTR/IN.
pub fn build_query(id: &[u8; 32]) -> Vec<u8> {
    let instance = instance_name(id);
    let mut b = Vec::with_capacity(12 + SERVICE_DOMAIN.len() + instance.len() + 12);
    b.extend_from_slice(&[0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0]);
    for name in [SERVICE_DOMAIN, instance.as_str()] {
        append_name(&mut b, name);
        b.extend_from_slice(&DNS_TYPE_PTR.to_be_bytes());
        b.extend_from_slice(&DNS_CLASS_IN.to_be_bytes());
    }
    b
}

/// `dnsBuilder.name` for the fixed names of a query, whose labels are 4 to 52 bytes long.
fn append_name(b: &mut Vec<u8>, name: &str) {
    for label in name.split('.') {
        // Every label here is non-empty and at most 63 bytes, so dnsBuilder.name never fails for it.
        b.push(label.len() as u8);
        b.extend_from_slice(label.as_bytes());
    }
    b.push(0);
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
///
/// go-iroh `parseAnnouncement(packet, "irohv1")` followed by `infoFromAnnouncement`:
/// - every question is skipped, and the answer, authority and additional records are read together; one
///   malformed record refuses the whole packet, trailing bytes are ignored, and the header flags are not
///   checked;
/// - an instance comes from a PTR under the service (compared ignoring ASCII case), an SRV or a TXT record;
///   it must end in `._irohv1._udp.local` exactly, its label must parse as an endpoint id, its SRV (the last
///   one) must carry a non-zero port, and its SRV target must own at least one A/AAAA record (host names
///   match exactly);
/// - `addrs` are the target's A/AAAA addresses with the SRV port, in record order and de-duplicated;
///   IPv4-mapped IPv6 addresses stay IPv6;
/// - TXT keys are case-sensitive, the last TXT record of the instance replaces earlier ones, and the last
///   `relay=`/`user-data=` entry wins; `relay` is normalised by `ParseRelayURL` and dropped when it does
///   not parse, and user data longer than 245 bytes is dropped.
///
/// Differences that only malformed network input can observe:
/// - An A/AAAA record with a wrong rdata length gives Go an invalid address. That address still counts
///   for "at least one address" and is kept in go-iroh's address set. Here it is dropped, so such an
///   announcement can come back with fewer addresses, even none.
/// - Relay and user-data bytes that are not UTF-8 are converted lossily (U+FFFD), after the 245-byte check
///   (PORTING.md DD-8).
/// - When several instances qualify, the first one seen wins; Go iterates a map (DD-10).
pub fn parse_announcement(packet: &[u8]) -> Option<Announcement> {
    let records = parse_dns(packet)?;
    let mut hosts: HashMap<&[u8], Vec<Option<IpAddr>>> = HashMap::new();
    let mut txt: HashMap<&[u8], &TxtAttrs> = HashMap::new();
    let mut srv: HashMap<&[u8], (u16, &[u8])> = HashMap::new();
    let mut instances: Vec<&[u8]> = Vec::new();
    for rr in &records {
        let name = rr.name.as_slice();
        match &rr.rdata {
            RData::Ptr(ptr) => {
                // strings.EqualFold against an ASCII name without 'k' or 's' is an ASCII case-insensitive
                // comparison: no other rune folds to these letters.
                if name.eq_ignore_ascii_case(SERVICE_DOMAIN.as_bytes()) {
                    note_instance(&mut instances, ptr);
                }
            }
            RData::Srv { port, target } => {
                srv.insert(name, (*port, target.as_slice()));
                note_instance(&mut instances, name);
            }
            RData::Txt(attrs) => {
                txt.insert(name, attrs);
                note_instance(&mut instances, name);
            }
            RData::Ip(ip) => hosts.entry(name).or_default().push(*ip),
            RData::Other => {}
        }
    }
    for inst in instances {
        // Go checks HasSuffix(ToLower(inst), instService) and then trims with the case-sensitive TrimSuffix.
        // An instance whose suffix differs only in case keeps its whole name as the label, which never parses
        // as an endpoint id ('.' and '_' are neither hex nor base32), so both steps reduce to an exact suffix.
        let Some(label) = inst.strip_suffix(INSTANCE_SUFFIX) else {
            continue;
        };
        let Some(id) = parse_endpoint_label(label) else {
            continue;
        };
        let Some(&(port, target)) = srv.get(inst) else {
            continue;
        };
        if port == 0 {
            continue;
        }
        let ips = match hosts.get(target) {
            Some(ips) if !ips.is_empty() => ips,
            _ => continue,
        };
        // dns.EndpointData.AddIPAddrs: order kept, duplicates skipped (netip.AddrPort comparison).
        let mut addrs: Vec<SocketAddr> = Vec::new();
        for ip in ips.iter().flatten() {
            let addr = SocketAddr::new(*ip, port);
            if !addrs.contains(&addr) {
                addrs.push(addr);
            }
        }
        let (relay, user_data) = match txt.get(inst) {
            Some(attrs) => (relay_url(&attrs.relay), user_data(&attrs.user_data)),
            None => (None, None),
        };
        return Some(Announcement {
            id,
            addrs,
            relay,
            user_data,
        });
    }
    None
}

/// Adds `name` to the instance list unless it is already there (first-seen order).
fn note_instance<'a>(instances: &mut Vec<&'a [u8]>, name: &'a [u8]) {
    if !instances.contains(&name) {
        instances.push(name);
    }
}

/// `infoFromAnnouncement`'s relay: `netaddr.ParseRelayURL` of a non-empty value, dropped on error.
fn relay_url(raw: &[u8]) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    dstore_transport::addr::parse_relay_url(&String::from_utf8_lossy(raw))
        .ok()
        .map(|u| u.0)
}

/// `infoFromAnnouncement`'s user data: `dns.NewUserData` of a non-empty value, dropped when too long.
fn user_data(raw: &[u8]) -> Option<String> {
    if raw.is_empty() || raw.len() > USER_DATA_MAX_LENGTH {
        return None;
    }
    Some(String::from_utf8_lossy(raw).into_owned())
}

/// `parseEndpointLabel` = `key.ParseEndpointID`: 64 bytes are hex, anything else is `strings.ToUpper` then
/// RFC 4648 base32 without padding (Go's decoder), which must give 32 bytes; the bytes must be a valid
/// curve point. The z-base-32 form is not accepted.
fn parse_endpoint_label(label: &[u8]) -> Option<[u8; 32]> {
    let bytes = if label.len() == 64 {
        dstore_gocompat::hex::decode_string(label).ok()?
    } else {
        let upper = dstore_gocompat::strings::to_upper(label);
        dstore_gocompat::base32::decode_nopad(dstore_gocompat::base32::STD_ALPHABET, &upper).ok()?
    };
    let id = <[u8; 32]>::try_from(bytes).ok()?;
    iroh_base::PublicKey::from_bytes(&id).ok()?;
    Some(id)
}

/// A resource record reduced to what `parseAnnouncement` reads.
struct Record {
    name: Vec<u8>,
    rdata: RData,
}

enum RData {
    Ptr(Vec<u8>),
    Srv {
        port: u16,
        target: Vec<u8>,
    },
    Txt(TxtAttrs),
    /// A or AAAA; `None` when the rdata length is wrong (Go's zero `netip.Addr`).
    Ip(Option<IpAddr>),
    Other,
}

/// The TXT keys `parseAnnouncement` reads; empty = absent (Go's map lookup gives "").
#[derive(Default)]
struct TxtAttrs {
    relay: Vec<u8>,
    user_data: Vec<u8>,
}

fn be16(b: &[u8], off: usize) -> Option<u16> {
    let pair = b.get(off..off.checked_add(2)?)?;
    <[u8; 2]>::try_from(pair).ok().map(u16::from_be_bytes)
}

/// `parseDNS`: skips the questions, then reads ANCOUNT + NSCOUNT + ARCOUNT records.
fn parse_dns(packet: &[u8]) -> Option<Vec<Record>> {
    if packet.len() < 12 {
        return None;
    }
    let qd = be16(packet, 4)?;
    let an = be16(packet, 6)?;
    let ns = be16(packet, 8)?;
    let ar = be16(packet, 10)?;
    let mut off = 12usize;
    for _ in 0..qd {
        let (_, next) = read_name(packet, off)?;
        if next + 4 > packet.len() {
            return None;
        }
        off = next + 4;
    }
    let total = usize::from(an) + usize::from(ns) + usize::from(ar);
    let mut records = Vec::new();
    for _ in 0..total {
        let (rr, next) = read_rr(packet, off)?;
        off = next;
        records.push(rr);
    }
    Some(records)
}

/// `readRR`. PTR and SRV target names are read from the whole packet: they may use compression and
/// run past the rdata.
fn read_rr(packet: &[u8], off: usize) -> Option<(Record, usize)> {
    let (name, off) = read_name(packet, off)?;
    if off + 10 > packet.len() {
        return None;
    }
    let typ = be16(packet, off)?;
    let n = usize::from(be16(packet, off + 8)?);
    let start = off + 10;
    let data = packet.get(start..start + n)?;
    let rdata = match typ {
        DNS_TYPE_PTR => RData::Ptr(read_name(packet, start)?.0),
        DNS_TYPE_SRV => {
            // A short SRV rdata ("mdns: short srv") refuses the whole packet.
            let port = be16(data, 4)?;
            RData::Srv {
                port,
                target: read_name(packet, start + 6)?.0,
            }
        }
        DNS_TYPE_TXT => RData::Txt(parse_txt(data)),
        DNS_TYPE_A => RData::Ip(
            <[u8; 4]>::try_from(data)
                .ok()
                .map(|b| IpAddr::V4(Ipv4Addr::from(b))),
        ),
        DNS_TYPE_AAAA => RData::Ip(
            <[u8; 16]>::try_from(data)
                .ok()
                .map(|b| IpAddr::V6(Ipv6Addr::from(b))),
        ),
        _ => RData::Other,
    };
    Some((Record { name, rdata }, start + n))
}

/// `readName`: labels joined with "." (raw bytes), compression pointers anywhere in the packet (forward
/// ones too), at most 32 steps. Returns the name and the offset after it in the record.
fn read_name(packet: &[u8], mut off: usize) -> Option<(Vec<u8>, usize)> {
    let mut name: Vec<u8> = Vec::new();
    let mut labels = 0usize;
    let mut next = off;
    let mut jumped = false;
    for _ in 0..MAX_NAME_STEPS {
        let l = *packet.get(off)?;
        if l & 0xc0 == 0xc0 {
            let low = *packet.get(off + 1)?;
            let ptr = (usize::from(l & 0x3f) << 8) | usize::from(low);
            if !jumped {
                next = off + 2;
                jumped = true;
            }
            off = ptr;
            continue;
        }
        if l & 0xc0 != 0 {
            // "mdns: unsupported name label"
            return None;
        }
        off += 1;
        if l == 0 {
            if !jumped {
                next = off;
            }
            return Some((name, next));
        }
        let end = off + usize::from(l);
        let label = packet.get(off..end)?;
        if labels > 0 {
            name.push(b'.');
        }
        name.extend_from_slice(label);
        labels += 1;
        off = end;
    }
    // "mdns: compression loop"
    None
}

/// `parseTXT`: length-prefixed strings until the data ends or a length overruns it; each `k=v` (cut at
/// the first '=') sets key k, the last occurrence winning; strings without '=' are skipped.
fn parse_txt(mut data: &[u8]) -> TxtAttrs {
    let mut attrs = TxtAttrs::default();
    while let Some((&n, rest)) = data.split_first() {
        let Some((s, tail)) = rest.split_at_checked(usize::from(n)) else {
            break;
        };
        if let Some(eq) = s.iter().position(|&b| b == b'=') {
            let (key, value) = s.split_at(eq);
            let value = value.get(1..).unwrap_or_default();
            match key {
                b"relay" => attrs.relay = value.to_vec(),
                b"user-data" => attrs.user_data = value.to_vec(),
                _ => {}
            }
        }
        data = tail;
    }
    attrs
}

/// The mDNS resolver: sockets on every up multicast interface and an announcement cache.
///
/// Dropping it (or calling [`MdnsResolver::close`]) stops the listener, as cancelling the ctx given to
/// go-iroh's `Start` does.
pub struct MdnsResolver {
    shared: Arc<Shared>,
}

/// State shared with the read loops, which hold it without keeping the resolver alive.
struct Shared {
    logger: Logger,
    cache: Mutex<HashMap<[u8; 32], Announcement>>,
    tx: Mutex<Tx>,
    bg: CancellationToken,
}

/// Where queries go (`writeMulticast`).
enum Tx {
    /// `Start` is running: its sockets.
    Listening {
        v4: Arc<UdpSocket>,
        v6: Option<Arc<UdpSocket>>,
    },
    /// `Start` is not running (closed, or its IPv4 read loop failed): a fresh socket per family.
    Stopped,
    /// Unit tests: queries are recorded, no socket is opened.
    #[cfg(test)]
    Recorded(Vec<Vec<u8>>),
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl MdnsResolver {
    /// Joins 224.0.0.251:5353 (required) and [ff02::fb]:5353 (best effort) with
    /// SO_REUSEADDR+SO_REUSEPORT on every up multicast interface; failure → Err(text), and the caller logs
    /// WARN "transport: mdns discovery unavailable" error=<text>.
    ///
    /// go-iroh `Start` up to its read loops:
    /// - an IPv4 failure returns Go's text, e.g.
    ///   `mdns: listen udp4: listen udp4 0.0.0.0:5353: bind: address already in use` or
    ///   `mdns: join ipv4 multicast: setsockopt: no such device`;
    /// - an IPv6 failure logs DEBUG `mdns: not listening on ipv6` err=<text> and continues over IPv4.
    ///
    /// The read loops then run in background tasks. When the IPv4 read loop fails, Go's `Start` returns
    /// `mdns: read: …`, which dstore logs; this port logs the same WARN line through `logger` and stops
    /// the listener.
    pub async fn start(logger: Logger) -> Result<Arc<MdnsResolver>, String> {
        let v4 = listen_mdns_group(Family::V4)?;
        let v6 = match listen_mdns_group(Family::V6) {
            Ok(s) => Some(Arc::new(s)),
            Err(err) => {
                logger.debug("mdns: not listening on ipv6", vec![Attr::any("err", err)]);
                None
            }
        };
        let v4 = Arc::new(v4);
        let shared = Arc::new(Shared {
            logger,
            cache: Mutex::new(HashMap::new()),
            tx: Mutex::new(Tx::Listening {
                v4: Arc::clone(&v4),
                v6: v6.clone(),
            }),
            bg: CancellationToken::new(),
        });
        if let Some(v6) = v6 {
            tokio::spawn(read_loop(Arc::clone(&shared), v6, Family::V6));
        }
        tokio::spawn(read_loop(Arc::clone(&shared), v4, Family::V4));
        Ok(Arc::new(MdnsResolver { shared }))
    }

    /// A resolver that never listens: go-iroh's registered `Discovery` whose `Start` failed.
    ///
    /// dstore registers the Discovery before `Start` runs and keeps it when `Start` fails, so a lookup by id
    /// still multicasts its query, from a fresh socket per family (`writeMulticast` without `Start`), and
    /// waits out the lookup timeout, since nothing fills the cache. An endpoint whose [`MdnsResolver::start`]
    /// failed logs the error and registers this resolver in its place.
    ///
    /// Not in PORTING.md §4.7; added so that failure path keeps Go's query and timing without a stand-in.
    pub fn without_listener(logger: Logger) -> Arc<MdnsResolver> {
        let shared = Arc::new(Shared {
            logger,
            cache: Mutex::new(HashMap::new()),
            tx: Mutex::new(Tx::Stopped),
            bg: CancellationToken::new(),
        });
        shared.bg.cancel();
        Arc::new(MdnsResolver { shared })
    }

    /// Cache hit → at once; else send the query and poll the cache every 25 ms until `timeout` or ctx
    /// end.
    ///
    /// go-iroh `Resolve`: the cache is checked before the ctx, and a miss sends the query even when ctx
    /// has already ended. A timeout of zero means go-iroh's default of 10 s (`WithLookupTimeout` ignores
    /// non-positive values). The ctx error item Go yields is `None` here; dstore's lookup skips it.
    pub async fn resolve(
        &self,
        ctx: &Ctx,
        id: [u8; 32],
        timeout: Duration,
    ) -> Option<Announcement> {
        let shared = &self.shared;
        if let Some(found) = shared.item(&id) {
            return Some(found);
        }
        shared.query(&id);
        let timer = tokio::time::sleep(lookup_timeout(timeout));
        tokio::pin!(timer);
        let done = ctx.done();
        tokio::pin!(done);
        let mut tick =
            tokio::time::interval_at(tokio::time::Instant::now() + POLL_INTERVAL, POLL_INTERVAL);
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                () = &mut done => return None,
                () = &mut timer => return None,
                _ = tick.tick() => {
                    if let Some(found) = shared.item(&id) {
                        return Some(found);
                    }
                }
            }
        }
    }

    /// Stops the listener: the read loops end and the sockets close, as when go-iroh's `Start` ctx ends.
    /// The cache stays; a later `resolve` still answers from it, and a miss multicasts its query from a
    /// fresh socket per family (`writeMulticast` without `Start`) and waits out the timeout.
    ///
    /// Not in PORTING.md §4.7; added so an endpoint can stop discovery on close without dropping its
    /// `Arc`.
    pub fn close(&self) {
        self.shared.stop();
    }
}

impl Drop for MdnsResolver {
    fn drop(&mut self) {
        self.shared.stop();
    }
}

/// Seams for the unit tests of the endpoint's discovery wiring (`endpoint.rs`); no socket is opened.
#[cfg(test)]
impl MdnsResolver {
    /// A resolver without sockets: `cached` answers lookups, and queries are recorded, not sent.
    pub(crate) fn with_cache(logger: Logger, cached: Vec<Announcement>) -> Arc<MdnsResolver> {
        let cache = cached.into_iter().map(|a| (a.id, a)).collect();
        Arc::new(MdnsResolver {
            shared: Arc::new(Shared {
                logger,
                cache: Mutex::new(cache),
                tx: Mutex::new(Tx::Recorded(Vec::new())),
                bg: CancellationToken::new(),
            }),
        })
    }

    /// The listener still runs: not closed, not dropped, and not a resolver without listener.
    pub(crate) fn is_listening(&self) -> bool {
        !self.shared.bg.is_cancelled()
    }
}

/// A non-positive lookup timeout falls back to go-iroh's default.
fn lookup_timeout(timeout: Duration) -> Duration {
    if timeout.is_zero() {
        DEFAULT_LOOKUP_TIMEOUT
    } else {
        timeout
    }
}

impl Shared {
    /// `handlePacket`: an announcement replaces the cached entry of its id. Anything else would go to
    /// `answerQuery`, which answers nothing for a passive Discovery.
    fn handle_packet(&self, packet: &[u8]) {
        if let Some(found) = parse_announcement(packet) {
            lock(&self.cache).insert(found.id, found);
        }
    }

    /// `item`.
    fn item(&self, id: &[u8; 32]) -> Option<Announcement> {
        lock(&self.cache).get(id).cloned()
    }

    /// `query`: fire and forget, write errors ignored.
    fn query(&self, id: &[u8; 32]) {
        let packet = build_query(id);
        let mut tx = lock(&self.tx);
        match &mut *tx {
            Tx::Listening { v4, v6 } => {
                let v4 = Arc::clone(v4);
                let v6 = v6.clone();
                tokio::spawn(async move {
                    let _ = v4.send_to(&packet, Family::V4.multicast_dst()).await;
                    if let Some(v6) = v6 {
                        let _ = v6.send_to(&packet, Family::V6.multicast_dst()).await;
                    }
                });
            }
            Tx::Stopped => {
                tokio::spawn(async move {
                    write_once(Family::V4, &packet).await;
                    write_once(Family::V6, &packet).await;
                });
            }
            #[cfg(test)]
            Tx::Recorded(sent) => sent.push(packet),
        }
    }

    /// Ends the read loops and drops the listener sockets.
    fn stop(&self) {
        self.bg.cancel();
        let mut tx = lock(&self.tx);
        if matches!(*tx, Tx::Listening { .. }) {
            *tx = Tx::Stopped;
        }
    }
}

/// `writeOnce`: multicasts `packet` from a new socket of family `f`; every error is ignored.
async fn write_once(f: Family, packet: &[u8]) {
    let Ok(sock) = UdpSocket::bind(f.any_port()).await else {
        return;
    };
    let _ = sock.send_to(packet, f.multicast_dst()).await;
}

/// `readLoop`: caches announcements until the resolver stops or a read fails.
async fn read_loop(shared: Arc<Shared>, sock: Arc<UdpSocket>, f: Family) {
    let mut buf = vec![0u8; READ_BUFFER];
    loop {
        let res = tokio::select! {
            () = shared.bg.cancelled() => return,
            res = sock.recv_from(&mut buf) => res,
        };
        match res {
            Ok((n, _)) => shared.handle_packet(buf.get(..n).unwrap_or_default()),
            Err(e) => {
                if shared.bg.is_cancelled() {
                    return;
                }
                // Go ignores the IPv6 loop's error. The IPv4 loop's error ends Start, which closes both
                // sockets, and dstore logs it.
                if f == Family::V4 {
                    shared.logger.warn(
                        "transport: mdns discovery unavailable",
                        vec![Attr::any("error", read_error_text(f, &e))],
                    );
                    shared.stop();
                }
                return;
            }
        }
    }
}

/// An address family of the listener.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    V4,
    V6,
}

impl Family {
    /// The Go network name.
    fn network(self) -> &'static str {
        match self {
            Family::V4 => "udp4",
            Family::V6 => "udp6",
        }
    }

    /// The name in `mdns: join <name> multicast`.
    fn ip_name(self) -> &'static str {
        match self {
            Family::V4 => "ipv4",
            Family::V6 => "ipv6",
        }
    }

    fn domain(self) -> Domain {
        match self {
            Family::V4 => Domain::IPV4,
            Family::V6 => Domain::IPV6,
        }
    }

    /// The listen address, `0.0.0.0:5353` or `[::]:5353`.
    fn listen_addr(self) -> SocketAddr {
        match self {
            Family::V4 => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), MDNS_PORT),
            Family::V6 => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), MDNS_PORT),
        }
    }

    /// A wildcard address with an ephemeral port (`net.ListenUDP(network, nil)`).
    fn any_port(self) -> SocketAddr {
        SocketAddr::new(self.listen_addr().ip(), 0)
    }

    /// `ipv4Multicast` / `ipv6Multicast`.
    fn multicast_dst(self) -> SocketAddr {
        match self {
            Family::V4 => SocketAddr::new(IpAddr::V4(IPV4_GROUP), MDNS_PORT),
            Family::V6 => SocketAddr::new(IpAddr::V6(IPV6_GROUP), MDNS_PORT),
        }
    }

    fn ip_level(self) -> libc::c_int {
        match self {
            Family::V4 => libc::IPPROTO_IP,
            Family::V6 => libc::IPPROTO_IPV6,
        }
    }

    /// The group address as the `gr_group` of a `struct group_req` (port 0; `sin_len` set on Darwin, as
    /// x/net's `groupReq.setGroup` does).
    fn group_storage(self) -> libc::sockaddr_storage {
        // SAFETY: an all-zero `sockaddr_storage` is a valid value.
        let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        match self {
            Family::V4 => {
                // SAFETY: an all-zero `sockaddr_in` is a valid value.
                let mut sin: libc::sockaddr_in = unsafe { std::mem::zeroed() };
                #[cfg(target_vendor = "apple")]
                {
                    sin.sin_len = std::mem::size_of::<libc::sockaddr_in>() as u8;
                }
                sin.sin_family = libc::AF_INET as libc::sa_family_t;
                sin.sin_addr.s_addr = u32::from_ne_bytes(IPV4_GROUP.octets());
                // SAFETY: `sockaddr_storage` is larger than `sockaddr_in` and aligned for every socket
                // address type, so the write stays inside `storage`.
                unsafe { std::ptr::write((&raw mut storage).cast::<libc::sockaddr_in>(), sin) };
            }
            Family::V6 => {
                // SAFETY: an all-zero `sockaddr_in6` is a valid value.
                let mut sin6: libc::sockaddr_in6 = unsafe { std::mem::zeroed() };
                #[cfg(target_vendor = "apple")]
                {
                    sin6.sin6_len = std::mem::size_of::<libc::sockaddr_in6>() as u8;
                }
                sin6.sin6_family = libc::AF_INET6 as libc::sa_family_t;
                sin6.sin6_addr.s6_addr = IPV6_GROUP.octets();
                // SAFETY: `sockaddr_storage` is larger than `sockaddr_in6` and aligned for every socket
                // address type, so the write stays inside `storage`.
                unsafe { std::ptr::write((&raw mut storage).cast::<libc::sockaddr_in6>(), sin6) };
            }
        }
        storage
    }
}

/// Go's `net.OpError` text of a failed `ListenPacket`, wrapped by `listenMDNS`.
fn listen_error_text(f: Family, inner: &str) -> String {
    format!(
        "mdns: listen {net}: listen {net} {addr}: {inner}",
        net = f.network(),
        addr = f.listen_addr()
    )
}

/// `os.NewSyscallError(name, err)`.
fn syscall_error_text(name: &str, e: &io::Error) -> String {
    format!("{name}: {}", io_error_text(e))
}

/// `listenIPv4MDNS`'s / `listenIPv6MDNS`'s join failure.
fn join_error_text(f: Family, e: &io::Error) -> String {
    format!(
        "mdns: join {} multicast: {}",
        f.ip_name(),
        syscall_error_text("setsockopt", e)
    )
}

/// `readLoop`'s failure: `mdns: read: read udp4 0.0.0.0:5353: recvfrom: <errno>`.
fn read_error_text(f: Family, e: &io::Error) -> String {
    format!(
        "mdns: read: read {} {}: {}",
        f.network(),
        f.listen_addr(),
        syscall_error_text("recvfrom", e)
    )
}

/// `listenIPv4MDNS` / `listenIPv6MDNS`: listen, then join the group.
fn listen_mdns_group(f: Family) -> Result<UdpSocket, String> {
    let sock = listen_mdns(f)?;
    join_group(sock.as_raw_fd(), f).map_err(|e| join_error_text(f, &e))?;
    Ok(sock)
}

/// `listenMDNS`: `net.ListenConfig{Control: reusePortControl}.ListenPacket(network, host:5353)`, in the
/// order Go's `net` package runs: socket, default options (IPV6_V6ONLY for udp6, SO_BROADCAST), the
/// control function, bind.
fn listen_mdns(f: Family) -> Result<UdpSocket, String> {
    let fail = |inner: String| listen_error_text(f, &inner);
    let sock = Socket::new(f.domain(), Type::DGRAM, None)
        .map_err(|e| fail(syscall_error_text("socket", &e)))?;
    if f == Family::V6 {
        // setDefaultSockopts ignores this error.
        let _ = sock.set_only_v6(true);
    }
    sock.set_broadcast(true)
        .map_err(|e| fail(syscall_error_text("setsockopt", &e)))?;
    // reusePortControl sets both options and returns the first failure as a bare errno.
    let reuse_addr = sock.set_reuse_address(true);
    let reuse_port = sock.set_reuse_port(true);
    reuse_addr
        .and(reuse_port)
        .map_err(|e| fail(io_error_text(&e)))?;
    sock.bind(&f.listen_addr().into())
        .map_err(|e| fail(syscall_error_text("bind", &e)))?;
    sock.set_nonblocking(true)
        .map_err(|e| fail(syscall_error_text("setnonblock", &e)))?;
    UdpSocket::from_std(std::net::UdpSocket::from(sock)).map_err(|e| fail(io_error_text(&e)))
}

/// `joinGroup`: joins on every up multicast interface, and falls back to the interface the routing
/// table picks (index 0) when none accepts the join; only the fallback's error is returned.
fn join_group(fd: RawFd, f: Family) -> io::Result<()> {
    let mut joined = false;
    for index in multicast_interface_indexes() {
        if join_mcast_group(fd, f, index).is_ok() {
            joined = true;
        }
    }
    if joined {
        return Ok(());
    }
    join_mcast_group(fd, f, 0)
}

/// `net.Interfaces()` filtered to `FlagUp` and `FlagMulticast`, in index order; a listing error gives
/// none (Go ignores it).
fn multicast_interface_indexes() -> Vec<u32> {
    let Ok(addrs) = nix::ifaddrs::getifaddrs() else {
        return Vec::new();
    };
    select_multicast_interfaces(addrs.map(|a| (a.interface_name, a.flags)), |name: &str| {
        nix::net::if_::if_nametoindex(name)
            .ok()
            .filter(|&index| index != 0)
    })
}

/// The indexes of the up, multicast-capable interfaces among `getifaddrs` entries (one entry per
/// address, so an interface appears several times), sorted and unique.
fn select_multicast_interfaces(
    entries: impl IntoIterator<Item = (String, InterfaceFlags)>,
    index_of: impl Fn(&str) -> Option<u32>,
) -> Vec<u32> {
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for (name, flags) in entries {
        if seen.contains(&name) {
            continue;
        }
        if flags.contains(InterfaceFlags::IFF_UP)
            && flags.contains(InterfaceFlags::IFF_MULTICAST)
            && let Some(index) = index_of(&name)
        {
            out.push(index);
        }
        seen.push(name);
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// `struct group_req` and `MCAST_JOIN_GROUP`, which x/net's `JoinGroup` uses on Darwin and Linux.
#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
mod sys {
    use nix::libc;

    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub(super) const MCAST_JOIN_GROUP: libc::c_int = libc::MCAST_JOIN_GROUP;
    /// `<netinet/in.h>`; libc does not export it for Apple targets.
    #[cfg(target_vendor = "apple")]
    pub(super) const MCAST_JOIN_GROUP: libc::c_int = 80;

    /// Darwin packs `struct group_req` to 4 bytes (132 bytes); Linux uses the natural layout.
    #[cfg_attr(target_vendor = "apple", repr(C, packed(4)))]
    #[cfg_attr(not(target_vendor = "apple"), repr(C))]
    pub(super) struct GroupReq {
        pub(super) gr_interface: u32,
        pub(super) gr_group: libc::sockaddr_storage,
    }

    // x/net: sizeofGroupReq = 0x84 on darwin, 0x88 on 64-bit linux.
    #[cfg(target_vendor = "apple")]
    const _: () = assert!(std::mem::size_of::<GroupReq>() == 0x84);
    #[cfg(all(
        any(target_os = "linux", target_os = "android"),
        target_pointer_width = "64"
    ))]
    const _: () = assert!(std::mem::size_of::<GroupReq>() == 0x88);
}

/// `setsockopt(fd, IPPROTO_IP|IPPROTO_IPV6, MCAST_JOIN_GROUP, group_req{index, group})`.
#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn join_mcast_group(fd: RawFd, f: Family, index: u32) -> io::Result<()> {
    let req = sys::GroupReq {
        gr_interface: index,
        gr_group: f.group_storage(),
    };
    let len = libc::socklen_t::try_from(std::mem::size_of::<sys::GroupReq>())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: `fd` is an open socket for the duration of the call; `req` is a fully initialised
    // `struct group_req` with the platform's layout and `len` is its size, so the kernel reads only memory
    // that belongs to `req`.
    let rc = unsafe {
        libc::setsockopt(
            fd,
            f.ip_level(),
            sys::MCAST_JOIN_GROUP,
            (&raw const req).cast(),
            len,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Other Unix targets are out of scope (PORTING.md §5.13).
#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
fn join_mcast_group(_fd: RawFd, _f: Family, _index: u32) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

#[cfg(test)]
mod tests {
    use super::*;

    use dstore_gocompat::slog::{Handler, Level, Record as SlogRecord};

    /// Seed `01 02 … 20` of transport §5.6.
    const ID0_HEX: &str = "79b5562e8fe654f94078b112e8a98ba7901f853ae695bed7e0e3910bad049664";
    const LABEL0: &str = "pg2vmlup4zkpsqdywejorkmlu6ib7bj242k35v7a4oiqxlieszsa";

    fn unhex(s: &str) -> Vec<u8> {
        match dstore_gocompat::hex::decode_string(s.as_bytes()) {
            Ok(b) => b,
            Err(e) => panic!("bad hex {s:?}: {e}"),
        }
    }

    fn id0() -> [u8; 32] {
        match <[u8; 32]>::try_from(unhex(ID0_HEX)) {
            Ok(id) => id,
            Err(_) => panic!("id0 is not 32 bytes"),
        }
    }

    /// The id of a valid key other than id0.
    fn id1() -> [u8; 32] {
        *iroh_base::SecretKey::from_bytes(&[7u8; 32])
            .public()
            .as_bytes()
    }

    struct Discard;
    impl Handler for Discard {
        fn enabled(&self, _level: Level) -> bool {
            false
        }
        fn handle(&self, _handler_attrs: &[Attr], _r: &SlogRecord) {}
    }

    fn discard_logger() -> Logger {
        Logger::new(Arc::new(Discard))
    }

    impl MdnsResolver {
        /// A resolver without sockets that records its queries.
        fn recording() -> MdnsResolver {
            MdnsResolver {
                shared: Arc::new(Shared {
                    logger: discard_logger(),
                    cache: Mutex::new(HashMap::new()),
                    tx: Mutex::new(Tx::Recorded(Vec::new())),
                    bg: CancellationToken::new(),
                }),
            }
        }

        fn sent(&self) -> Vec<Vec<u8>> {
            match &*lock(&self.shared.tx) {
                Tx::Recorded(sent) => sent.clone(),
                _ => Vec::new(),
            }
        }
    }

    /// `serviceName` of go-iroh.
    fn go_service_name(service: &str) -> String {
        let service = service.trim_matches('.');
        if service.starts_with('_') {
            format!("{service}._udp.local")
        } else {
            format!("_{service}._udp.local")
        }
    }

    fn push_name(b: &mut Vec<u8>, name: &str) {
        for label in name.trim_end_matches('.').split('.') {
            b.push(label.len() as u8);
            b.extend_from_slice(label.as_bytes());
        }
        b.push(0);
    }

    fn push_rr(b: &mut Vec<u8>, name: &str, typ: u16, rdata: &[u8]) {
        push_name(b, name);
        b.extend_from_slice(&typ.to_be_bytes());
        b.extend_from_slice(&DNS_CLASS_IN.to_be_bytes());
        b.extend_from_slice(&120u32.to_be_bytes());
        b.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        b.extend_from_slice(rdata);
    }

    fn name_bytes(name: &str) -> Vec<u8> {
        let mut b = Vec::new();
        push_name(&mut b, name);
        b
    }

    fn header(flags: u16, qd: u16, an: u16, ns: u16, ar: u16) -> Vec<u8> {
        let mut b = vec![0, 0];
        for v in [flags, qd, an, ns, ar] {
            b.extend_from_slice(&v.to_be_bytes());
        }
        b
    }

    /// A port of go-iroh `buildAnnouncement` (node-side, so only needed as parser input here).
    fn build_announcement(
        service: &str,
        id: &[u8; 32],
        port: u16,
        ips: &[IpAddr],
        relay: &str,
        user_data: &str,
    ) -> Vec<u8> {
        let svc = go_service_name(service);
        let label = endpoint_label(id);
        let inst = format!("{label}.{svc}");
        let host = format!("{label}.local");
        let mut txt: Vec<String> = Vec::new();
        if !relay.is_empty() {
            txt.push(format!("relay={relay}"));
        }
        if !user_data.is_empty() {
            txt.push(format!("user-data={user_data}"));
        }
        let mut b = header(0x8400, 0, 3 + ips.len() as u16, 0, 0);
        push_rr(&mut b, &svc, DNS_TYPE_PTR, &name_bytes(&inst));
        let mut srv = vec![0, 0, 0, 0];
        srv.extend_from_slice(&port.to_be_bytes());
        srv.extend_from_slice(&name_bytes(&host));
        push_rr(&mut b, &inst, DNS_TYPE_SRV, &srv);
        let mut t = Vec::new();
        for v in &txt {
            if v.len() > 255 {
                continue;
            }
            t.push(v.len() as u8);
            t.extend_from_slice(v.as_bytes());
        }
        push_rr(&mut b, &inst, DNS_TYPE_TXT, &t);
        for ip in ips {
            let v4 = match ip {
                IpAddr::V4(v4) => Some(*v4),
                IpAddr::V6(v6) => v6.to_ipv4_mapped(),
            };
            match (v4, ip) {
                (Some(v4), _) => push_rr(&mut b, &host, DNS_TYPE_A, &v4.octets()),
                (None, IpAddr::V6(v6)) => push_rr(&mut b, &host, DNS_TYPE_AAAA, &v6.octets()),
                (None, IpAddr::V4(_)) => {}
            }
        }
        b
    }

    fn ip(s: &str) -> IpAddr {
        match s.parse() {
            Ok(ip) => ip,
            Err(e) => panic!("bad ip {s:?}: {e}"),
        }
    }

    fn sa(s: &str) -> SocketAddr {
        match s.parse() {
            Ok(a) => a,
            Err(e) => panic!("bad socket address {s:?}: {e}"),
        }
    }

    #[test]
    fn service_names_match_go() {
        assert_eq!(SERVICE_DOMAIN, go_service_name(SERVICE_NAME));
        assert_eq!(
            INSTANCE_SUFFIX,
            format!(".{}", go_service_name(SERVICE_NAME)).as_bytes()
        );
        assert_eq!(go_service_name("_other."), "_other._udp.local");
    }

    #[test]
    fn endpoint_label_of_seed_01_to_20() {
        assert_eq!(endpoint_label(&id0()), LABEL0);
        assert_eq!(
            instance_name(&id0()),
            format!("{LABEL0}._irohv1._udp.local")
        );
    }

    /// go-iroh `TestEndpointLabelMatchesRustMDNS` plus the `ParseEndpointID` edges.
    #[test]
    fn endpoint_label_round_trips_and_parse_edges() {
        for id in [id0(), id1()] {
            assert_eq!(
                parse_endpoint_label(endpoint_label(&id).as_bytes()),
                Some(id)
            );
        }
        // Case-insensitive base32.
        assert_eq!(
            parse_endpoint_label(LABEL0.to_uppercase().as_bytes()),
            Some(id0())
        );
        // strings.ToUpper maps U+0131 (dotless i) to 'I', so Go accepts it in place of an 'i'.
        let dotless = LABEL0.replacen('i', "\u{131}", 1);
        assert_eq!(dotless.len(), 53);
        assert_eq!(parse_endpoint_label(dotless.as_bytes()), Some(id0()));
        // 64 bytes are hex, in either case.
        assert_eq!(parse_endpoint_label(ID0_HEX.as_bytes()), Some(id0()));
        assert_eq!(
            parse_endpoint_label(ID0_HEX.to_uppercase().as_bytes()),
            Some(id0())
        );
        // z-base-32 is refused.
        assert_eq!(
            parse_endpoint_label(b"xg4icmwxh3kx1odasrjqtkcmw6eb9bj4h4k57i9yhqeozmer131y"),
            None
        );
        // Wrong lengths and bad symbols.
        assert_eq!(parse_endpoint_label(&LABEL0.as_bytes()[..51]), None);
        assert_eq!(
            parse_endpoint_label(format!("{LABEL0}aaaaaaaa").as_bytes()),
            None
        );
        assert_eq!(parse_endpoint_label(b""), None);
        assert_eq!(
            parse_endpoint_label(format!("{}!", &LABEL0[..51]).as_bytes()),
            None
        );
        // Not a curve point (transport §5.5: first byte 2, the rest zero).
        let mut bad = [0u8; 32];
        bad[0] = 2;
        assert_eq!(parse_endpoint_label(endpoint_label(&bad).as_bytes()), None);
    }

    #[test]
    fn query_layout() {
        let q = build_query(&id0());
        assert_eq!(&q[..12], &[0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0]);
        let Some((service, off)) = read_name(&q, 12) else {
            panic!("service name does not parse");
        };
        assert_eq!(service, SERVICE_DOMAIN.as_bytes());
        assert_eq!(&q[off..off + 4], &[0, 12, 0, 1]);
        let Some((instance, off)) = read_name(&q, off + 4) else {
            panic!("instance name does not parse");
        };
        assert_eq!(instance, instance_name(&id0()).as_bytes());
        assert_eq!(&q[off..], &[0, 12, 0, 1]);
        // No compression: the instance repeats the service labels.
        assert!(!q.iter().any(|&b| b & 0xc0 == 0xc0));
    }

    #[test]
    fn read_name_steps_and_pointers() {
        // 31 labels and the terminator take 32 steps: accepted.
        let mut p = Vec::new();
        for _ in 0..31 {
            p.extend_from_slice(&[1, b'a']);
        }
        p.push(0);
        let Some((name, next)) = read_name(&p, 0) else {
            panic!("31 labels refused");
        };
        assert_eq!(name.len(), 31 * 2 - 1);
        assert_eq!(next, p.len());
        // 32 labels need 33 steps: refused ("compression loop").
        let mut p = Vec::new();
        for _ in 0..32 {
            p.extend_from_slice(&[1, b'a']);
        }
        p.push(0);
        assert!(read_name(&p, 0).is_none());

        // A pointer ends the name in the record: next = pointer + 2. Forward pointers are followed.
        let p = [0xc0, 4, 0xff, 0xff, 3, b'f', b'o', b'o', 0];
        assert_eq!(read_name(&p, 0), Some((b"foo".to_vec(), 2)));
        // Labels, then a pointer back to them.
        let p = [3, b'b', b'a', b'r', 0, 1, b'x', 0xc0, 0];
        assert_eq!(read_name(&p, 5), Some((b"x.bar".to_vec(), 9)));
        // A label may contain '.' bytes; they are joined as they are.
        let p = [3, b'a', b'.', b'b', 0];
        assert_eq!(read_name(&p, 0), Some((b"a.b".to_vec(), 5)));
        // Root name.
        assert_eq!(read_name(&[0], 0), Some((Vec::new(), 1)));
        // Loops, reserved label types, truncation.
        assert!(read_name(&[0xc0, 0], 0).is_none());
        assert!(read_name(&[0x40, 0], 0).is_none());
        assert!(read_name(&[0x80, 0], 0).is_none());
        assert!(read_name(&[0xc0], 0).is_none());
        assert!(read_name(&[3, b'a', b'b'], 0).is_none());
        assert!(read_name(&[1, b'a'], 0).is_none());
        assert!(read_name(&[], 0).is_none());
        assert!(read_name(&[0xc0, 9], 0).is_none());
    }

    #[test]
    fn txt_attributes() {
        let mut data = Vec::new();
        for s in [
            "relay=https://a.example./",
            "noequals",
            "user-data=a=b",
            "RELAY=upper",
            "relay=https://b.example./",
        ] {
            data.push(s.len() as u8);
            data.extend_from_slice(s.as_bytes());
        }
        let attrs = parse_txt(&data);
        assert_eq!(attrs.relay, b"https://b.example./");
        assert_eq!(attrs.user_data, b"a=b");

        // An overrunning length stops the scan and keeps what came before.
        let attrs = parse_txt(&[
            11, b'u', b's', b'e', b'r', b'-', b'd', b'a', b't', b'a', b'=', b'x', 40, b'u',
        ]);
        assert_eq!(attrs.user_data, b"x");
        assert!(attrs.relay.is_empty());
        // Empty strings and values.
        let attrs = parse_txt(&[0, 6, b'r', b'e', b'l', b'a', b'y', b'=']);
        assert!(attrs.relay.is_empty());
        assert!(parse_txt(&[]).user_data.is_empty());
    }

    /// go-iroh `TestAnnouncementRoundTrip` without its relay; `announcement_round_trip_with_relay` adds it.
    #[test]
    fn announcement_round_trip() {
        let id = id1();
        let packet = build_announcement(
            SERVICE_NAME,
            &id,
            7777,
            &[ip("192.0.2.1"), ip("2001:db8::1")],
            "",
            "lan",
        );
        let Some(got) = parse_announcement(&packet) else {
            panic!("parseAnnouncement failed");
        };
        assert_eq!(
            got,
            Announcement {
                id,
                addrs: vec![sa("192.0.2.1:7777"), sa("[2001:db8::1]:7777")],
                relay: None,
                user_data: Some("lan".to_string()),
            }
        );
    }

    #[test]
    fn announcement_round_trip_with_relay() {
        let id = id1();
        let packet = build_announcement(
            SERVICE_NAME,
            &id,
            7777,
            &[ip("192.0.2.1"), ip("2001:db8::1")],
            "https://relay.example/",
            "lan",
        );
        let Some(got) = parse_announcement(&packet) else {
            panic!("parseAnnouncement failed");
        };
        assert_eq!(got.relay.as_deref(), Some("https://relay.example/"));
        assert_eq!(got.user_data.as_deref(), Some("lan"));
        // An unparseable relay is dropped, the rest is kept.
        let packet = build_announcement(SERVICE_NAME, &id, 7777, &[ip("192.0.2.1")], "%zz", "");
        let Some(got) = parse_announcement(&packet) else {
            panic!("parseAnnouncement failed");
        };
        assert_eq!(got.relay, None);
        assert_eq!(got.addrs, vec![sa("192.0.2.1:7777")]);
    }

    /// go-iroh `TestServiceNameIsolation`.
    #[test]
    fn service_name_isolation() {
        let packet = build_announcement("other", &id1(), 7777, &[ip("192.0.2.1")], "", "");
        assert_eq!(parse_announcement(&packet), None);
    }

    #[test]
    fn parse_details() {
        let id = id0();
        let label = endpoint_label(&id);
        let inst = format!("{label}._irohv1._udp.local");
        let host = format!("{label}.local");
        let srv = |port: u16, target: &str| {
            let mut r = vec![0, 0, 0, 0];
            r.extend_from_slice(&port.to_be_bytes());
            r.extend_from_slice(&name_bytes(target));
            r
        };

        // A wrong A rdata length: Go keeps an invalid address and reports an announcement; the address is
        // dropped here.
        let mut p = header(0x8400, 0, 3, 0, 0);
        push_rr(&mut p, &inst, DNS_TYPE_SRV, &srv(4242, &host));
        push_rr(&mut p, &host, DNS_TYPE_A, &[10, 0, 0]);
        push_rr(&mut p, &host, DNS_TYPE_A, &[10, 0, 0, 1]);
        let Some(got) = parse_announcement(&p) else {
            panic!("announcement refused");
        };
        assert_eq!(got.addrs, vec![sa("10.0.0.1:4242")]);
        let mut p = header(0x8400, 0, 2, 0, 0);
        push_rr(&mut p, &inst, DNS_TYPE_SRV, &srv(4242, &host));
        push_rr(&mut p, &host, DNS_TYPE_AAAA, &[0; 15]);
        let Some(got) = parse_announcement(&p) else {
            panic!("announcement refused");
        };
        assert!(got.addrs.is_empty());

        // The last SRV of an instance wins; A and IPv4-mapped AAAA are distinct addresses.
        let mut p = header(0x8400, 0, 4, 0, 0);
        push_rr(&mut p, &inst, DNS_TYPE_SRV, &srv(1111, "other.local"));
        push_rr(&mut p, &inst, DNS_TYPE_SRV, &srv(4242, &host));
        push_rr(&mut p, &host, DNS_TYPE_A, &[1, 2, 3, 4]);
        let mapped = Ipv4Addr::new(1, 2, 3, 4).to_ipv6_mapped();
        push_rr(&mut p, &host, DNS_TYPE_AAAA, &mapped.octets());
        let Some(got) = parse_announcement(&p) else {
            panic!("announcement refused");
        };
        assert_eq!(
            got.addrs,
            vec![sa("1.2.3.4:4242"), sa("[::ffff:1.2.3.4]:4242")]
        );

        // A later TXT record replaces the whole earlier one.
        let txt = |s: &str| {
            let mut r = vec![s.len() as u8];
            r.extend_from_slice(s.as_bytes());
            r
        };
        let mut p = header(0x8400, 0, 4, 0, 0);
        push_rr(&mut p, &inst, DNS_TYPE_TXT, &txt("user-data=first"));
        push_rr(&mut p, &inst, DNS_TYPE_TXT, &txt("other=x"));
        push_rr(&mut p, &inst, DNS_TYPE_SRV, &srv(4242, &host));
        push_rr(&mut p, &host, DNS_TYPE_A, &[1, 2, 3, 4]);
        let Some(got) = parse_announcement(&p) else {
            panic!("announcement refused");
        };
        assert_eq!(got.user_data, None);

        // User data of 245 bytes is kept, 246 bytes dropped.
        for (n, want) in [(245usize, true), (246, false)] {
            let mut p = header(0x8400, 0, 3, 0, 0);
            push_rr(
                &mut p,
                &inst,
                DNS_TYPE_TXT,
                &txt(&format!("user-data={}", "u".repeat(n))),
            );
            push_rr(&mut p, &inst, DNS_TYPE_SRV, &srv(4242, &host));
            push_rr(&mut p, &host, DNS_TYPE_A, &[1, 2, 3, 4]);
            let Some(got) = parse_announcement(&p) else {
                panic!("announcement refused");
            };
            assert_eq!(got.user_data.is_some(), want, "user data of {n} bytes");
        }

        // A PTR under the service names an instance, but without SRV there is nothing to announce.
        let mut p = header(0x8400, 0, 1, 0, 0);
        push_rr(
            &mut p,
            "_IROHV1._udp.local",
            DNS_TYPE_PTR,
            &name_bytes(&inst),
        );
        assert_eq!(parse_announcement(&p), None);

        // Unknown record types are skipped; a truncated record refuses the packet.
        let mut p = header(0x8400, 0, 3, 0, 0);
        push_rr(&mut p, &inst, 99, &[1, 2, 3]);
        push_rr(&mut p, &inst, DNS_TYPE_SRV, &srv(4242, &host));
        push_rr(&mut p, &host, DNS_TYPE_A, &[1, 2, 3, 4]);
        assert!(parse_announcement(&p).is_some());
        assert_eq!(parse_announcement(&p[..p.len() - 1]), None);
        // A question with a short type/class refuses the packet.
        let mut p = header(0, 1, 0, 0, 0);
        push_name(&mut p, SERVICE_DOMAIN);
        p.extend_from_slice(&[0, 12, 0]);
        assert_eq!(parse_announcement(&p), None);
    }

    #[test]
    fn select_interfaces() {
        let up_mc = InterfaceFlags::IFF_UP | InterfaceFlags::IFF_MULTICAST;
        let entries = vec![
            ("en0".to_string(), up_mc | InterfaceFlags::IFF_BROADCAST),
            (
                "lo0".to_string(),
                InterfaceFlags::IFF_UP
                    | InterfaceFlags::IFF_LOOPBACK
                    | InterfaceFlags::IFF_MULTICAST,
            ),
            ("en0".to_string(), InterfaceFlags::empty()),
            (
                "gif0".to_string(),
                InterfaceFlags::IFF_POINTOPOINT | InterfaceFlags::IFF_MULTICAST,
            ),
            ("stf0".to_string(), InterfaceFlags::IFF_UP),
            ("gone".to_string(), up_mc),
            ("en5".to_string(), up_mc),
        ];
        let index_of = |name: &str| match name {
            "lo0" => Some(1),
            "gif0" => Some(2),
            "stf0" => Some(3),
            "en0" => Some(11),
            "en5" => Some(7),
            _ => None,
        };
        assert_eq!(
            select_multicast_interfaces(entries, index_of),
            vec![1, 7, 11]
        );
        assert!(select_multicast_interfaces(Vec::new(), index_of).is_empty());
    }

    #[test]
    fn error_texts() {
        let e = io::Error::from_raw_os_error(libc::EADDRINUSE);
        assert_eq!(
            listen_error_text(Family::V4, &syscall_error_text("bind", &e)),
            "mdns: listen udp4: listen udp4 0.0.0.0:5353: bind: address already in use"
        );
        let e = io::Error::from_raw_os_error(libc::EACCES);
        assert_eq!(
            listen_error_text(Family::V6, &io_error_text(&e)),
            "mdns: listen udp6: listen udp6 [::]:5353: permission denied"
        );
        let e = io::Error::from_raw_os_error(libc::ENODEV);
        assert_eq!(
            join_error_text(Family::V4, &e),
            format!(
                "mdns: join ipv4 multicast: setsockopt: {}",
                io_error_text(&e)
            )
        );
        assert_eq!(
            join_error_text(Family::V6, &e),
            format!(
                "mdns: join ipv6 multicast: setsockopt: {}",
                io_error_text(&e)
            )
        );
        let e = io::Error::from_raw_os_error(libc::EBADF);
        assert_eq!(
            read_error_text(Family::V4, &e),
            "mdns: read: read udp4 0.0.0.0:5353: recvfrom: bad file descriptor"
        );
    }

    #[test]
    fn families() {
        assert_eq!(Family::V4.multicast_dst(), sa("224.0.0.251:5353"));
        assert_eq!(Family::V6.multicast_dst(), sa("[ff02::fb]:5353"));
        assert_eq!(Family::V4.any_port(), sa("0.0.0.0:0"));
        assert_eq!(Family::V6.any_port(), sa("[::]:0"));
        let v4 = Family::V4.group_storage();
        assert_eq!(i32::from(v4.ss_family), libc::AF_INET);
        let v6 = Family::V6.group_storage();
        assert_eq!(i32::from(v6.ss_family), libc::AF_INET6);
        #[cfg(target_vendor = "apple")]
        {
            assert_eq!(v4.ss_len, 16);
            assert_eq!(v6.ss_len, 28);
        }
    }

    #[test]
    fn zero_timeout_means_go_default() {
        assert_eq!(lookup_timeout(Duration::ZERO), Duration::from_secs(10));
        assert_eq!(
            lookup_timeout(Duration::from_secs(3)),
            Duration::from_secs(3)
        );
    }

    /// go-iroh `TestDiscoveryResolveFromPacket`: a cached announcement resolves at once, without a query.
    #[tokio::test(start_paused = true)]
    async fn resolve_from_packet() {
        let r = MdnsResolver::recording();
        let id = id1();
        let packet = build_announcement(SERVICE_NAME, &id, 7777, &[ip("192.0.2.1")], "", "");
        r.shared.handle_packet(&packet);
        let start = tokio::time::Instant::now();
        let got = r
            .resolve(&Ctx::background(), id, Duration::from_millis(20))
            .await;
        assert_eq!(got.map(|a| a.id), Some(id));
        assert_eq!(start.elapsed(), Duration::ZERO);
        assert!(r.sent().is_empty());
        // The cache answers even when the ctx has ended.
        let ctx = Ctx::background().with_cancel();
        ctx.cancel();
        assert!(r.resolve(&ctx, id, Duration::from_secs(5)).await.is_some());
        assert!(r.sent().is_empty());
    }

    /// The caching half of go-iroh `TestHandlePacketAnswersQueries`.
    #[test]
    fn handle_packet_caches_announcements_only() {
        let r = MdnsResolver::recording();
        r.shared.handle_packet(&build_query(&id0()));
        assert!(lock(&r.shared.cache).is_empty());
        let peer = id1();
        let packet = build_announcement(SERVICE_NAME, &peer, 7777, &[ip("192.0.2.9")], "", "");
        r.shared.handle_packet(&packet);
        assert!(r.shared.item(&peer).is_some());
        // A newer announcement replaces the entry.
        let packet = build_announcement(SERVICE_NAME, &peer, 4433, &[ip("192.0.2.10")], "", "x");
        r.shared.handle_packet(&packet);
        assert_eq!(
            r.shared.item(&peer),
            Some(Announcement {
                id: peer,
                addrs: vec![sa("192.0.2.10:4433")],
                relay: None,
                user_data: Some("x".to_string()),
            })
        );
    }

    /// A miss sends one query and waits out the timeout: 80 ms, dstore's 3 s, and go-iroh's 10 s default for a
    /// zero timeout (`WithLookupTimeout` ignores non-positive values).
    #[tokio::test(start_paused = true)]
    async fn resolve_miss_queries_and_times_out() {
        let r = MdnsResolver::recording();
        for (timeout, want) in [
            (Duration::from_millis(80), Duration::from_millis(80)),
            (crate::MDNS_LOOKUP_TIMEOUT, Duration::from_secs(3)),
            (Duration::ZERO, Duration::from_secs(10)),
        ] {
            let start = tokio::time::Instant::now();
            assert_eq!(r.resolve(&Ctx::background(), id0(), timeout).await, None);
            assert_eq!(start.elapsed(), want, "timeout {timeout:?}");
        }
        assert_eq!(r.sent(), vec![build_query(&id0()); 3]);
    }

    /// `Resolve` polls the cache on a 25 ms ticker whose first tick comes after 25 ms: an announcement heard at
    /// 60 ms is found by the 75 ms poll.
    #[tokio::test(start_paused = true)]
    async fn resolve_polls_the_cache_every_25ms() {
        let r = Arc::new(MdnsResolver::recording());
        let id = id1();
        let packet = build_announcement(SERVICE_NAME, &id, 7777, &[ip("192.0.2.1")], "", "");
        let feeder = {
            let r = Arc::clone(&r);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(30)).await;
                // An announcement of another id first, which must not end the lookup.
                let other = build_announcement(SERVICE_NAME, &id0(), 1, &[ip("192.0.2.2")], "", "");
                r.shared.handle_packet(&other);
                tokio::time::sleep(Duration::from_millis(30)).await;
                r.shared.handle_packet(&packet);
            })
        };
        let start = tokio::time::Instant::now();
        let got = r
            .resolve(&Ctx::background(), id, Duration::from_secs(5))
            .await;
        assert_eq!(got.map(|a| a.addrs), Some(vec![sa("192.0.2.1:7777")]));
        assert_eq!(start.elapsed(), Duration::from_millis(75));
        assert_eq!(r.sent().len(), 1);
        if let Err(e) = feeder.await {
            panic!("feeder: {e}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn resolve_ends_with_ctx() {
        let r = MdnsResolver::recording();
        // A deadline between two polls.
        let ctx = Ctx::background().with_timeout(Duration::from_millis(60));
        let start = tokio::time::Instant::now();
        assert_eq!(r.resolve(&ctx, id0(), Duration::from_secs(5)).await, None);
        assert_eq!(start.elapsed(), Duration::from_millis(60));

        // A cancel.
        let ctx = Ctx::background().with_cancel();
        let canceller = {
            let ctx = ctx.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(40)).await;
                ctx.cancel();
            })
        };
        let start = tokio::time::Instant::now();
        assert_eq!(r.resolve(&ctx, id0(), Duration::from_secs(5)).await, None);
        assert_eq!(start.elapsed(), Duration::from_millis(40));
        if let Err(e) = canceller.await {
            panic!("canceller: {e}");
        }

        // An ended ctx still sends the query, then returns at once.
        let ctx = Ctx::background().with_cancel();
        ctx.cancel();
        let start = tokio::time::Instant::now();
        assert_eq!(r.resolve(&ctx, id1(), Duration::from_secs(5)).await, None);
        assert_eq!(start.elapsed(), Duration::ZERO);
        assert_eq!(
            r.sent(),
            vec![
                build_query(&id0()),
                build_query(&id0()),
                build_query(&id1())
            ]
        );
    }

    #[test]
    fn close_and_drop_stop_the_listener() {
        let r = MdnsResolver::recording();
        let token = r.shared.bg.clone();
        r.close();
        assert!(token.is_cancelled());
        // A recording resolver keeps recording.
        assert!(matches!(*lock(&r.shared.tx), Tx::Recorded(_)));
        let r = MdnsResolver::recording();
        let token = r.shared.bg.clone();
        drop(r);
        assert!(token.is_cancelled());
    }

    /// dstore keeps a Discovery whose `Start` failed: nothing listens, and queries go out from fresh sockets.
    #[test]
    fn without_listener_never_listens() {
        let r = MdnsResolver::without_listener(discard_logger());
        assert!(r.shared.bg.is_cancelled());
        assert!(matches!(*lock(&r.shared.tx), Tx::Stopped));
        assert!(lock(&r.shared.cache).is_empty());
        r.close();
        assert!(matches!(*lock(&r.shared.tx), Tx::Stopped));
    }

    /// A go-iroh-layout packet with one instance label: SRV `<label>._irohv1._udp.local` → `host.local:4242`,
    /// then one A record per rdata (AAAA from 15 bytes on), as the review's Go probe built it.
    fn probe_packet(label: &[u8], rdatas: &[&[u8]]) -> Vec<u8> {
        let mut p = header(0x8400, 0, 1 + rdatas.len() as u16, 0, 0);
        p.push(label.len() as u8);
        p.extend_from_slice(label);
        p.extend_from_slice(&name_bytes(SERVICE_DOMAIN));
        p.extend_from_slice(&DNS_TYPE_SRV.to_be_bytes());
        p.extend_from_slice(&DNS_CLASS_IN.to_be_bytes());
        p.extend_from_slice(&120u32.to_be_bytes());
        let mut srv = vec![0, 0, 0, 0];
        srv.extend_from_slice(&4242u16.to_be_bytes());
        srv.extend_from_slice(&name_bytes("host.local"));
        p.extend_from_slice(&(srv.len() as u16).to_be_bytes());
        p.extend_from_slice(&srv);
        for rdata in rdatas {
            let typ = if rdata.len() >= 15 {
                DNS_TYPE_AAAA
            } else {
                DNS_TYPE_A
            };
            push_rr(&mut p, "host.local", typ, rdata);
        }
        p
    }

    /// `parseEndpointLabel` edges inside a packet. The expected outcomes are what go-iroh v0.2.0
    /// `parseAnnouncement` returned for the same packets (a scratch `go:linkname` probe run during the review).
    #[test]
    fn label_edges_match_go() {
        let a: &[u8] = &[10, 0, 0, 1];
        // The probe's packet for the plain label, byte for byte.
        assert_eq!(
            dstore_gocompat::hex::encode(&probe_packet(LABEL0.as_bytes(), &[a])),
            "00008400000000020000000034706732766d6c7570347a6b707371647977656a6f726b6d6c7536696237626a3234326b3335763761\
             346f6971786c6965737a7361075f69726f687631045f756470056c6f63616c000021000100000078001200000000109204686f7374\
             056c6f63616c0004686f7374056c6f63616c00000100010000007800040a000001"
        );
        let label = LABEL0.as_bytes();
        let cases: Vec<(&str, Vec<u8>, bool)> = vec![
            ("plain", label.to_vec(), true),
            // PORTING §9 C4: Go's base32 decoder ignores the 4 trailing bits of the last symbol.
            ("trailing_bits_b", [&label[..51], &b"b"[..]].concat(), true),
            ("trailing_bits_p", [&label[..51], &b"p"[..]].concat(), true),
            // The decoder strips '\r' and '\n' anywhere...
            (
                "newline_inside",
                [&label[..10], &b"\n"[..], &label[10..]].concat(),
                true,
            ),
            (
                "crlf_inside",
                [&label[..5], &b"\r\n"[..], &label[5..]].concat(),
                true,
            ),
            // ...but a 64-byte label is decoded as hex.
            (
                "newlines_to_64_bytes",
                [label, &[b'\n'; 12][..]].concat(),
                false,
            ),
            // strings.ToUpper maps U+017F (long s) to 'S' and leaves U+212A (Kelvin sign) as it is.
            (
                "long_s",
                LABEL0.replacen('s', "\u{17f}", 1).into_bytes(),
                true,
            ),
            (
                "kelvin",
                LABEL0.replacen('k', "\u{212a}", 1).into_bytes(),
                false,
            ),
            // An invalid UTF-8 byte becomes U+FFFD before decoding.
            (
                "invalid_utf8_tail",
                [&label[..51], &[0xff][..]].concat(),
                false,
            ),
        ];
        for (name, label, want) in cases {
            let got = parse_announcement(&probe_packet(&label, &[a]));
            if want {
                assert_eq!(
                    got.map(|g| (g.id, g.addrs)),
                    Some((id0(), vec![sa("10.0.0.1:4242")])),
                    "{name}"
                );
            } else {
                assert_eq!(got, None, "{name}");
            }
        }
        // Wrong A/AAAA rdata lengths. Go returned [invalid AddrPort], [invalid AddrPort 10.0.0.1:4242] and
        // [invalid AddrPort]; this port drops the invalid address (see parse_announcement).
        let short: &[u8] = &[10, 0, 0];
        let aaaa15: &[u8] = &[0; 15];
        let addrs =
            |rdatas: &[&[u8]]| parse_announcement(&probe_packet(label, rdatas)).map(|g| g.addrs);
        assert_eq!(addrs(&[short]), Some(Vec::new()));
        assert_eq!(addrs(&[short, a]), Some(vec![sa("10.0.0.1:4242")]));
        assert_eq!(addrs(&[aaaa15]), Some(Vec::new()));
    }
}
