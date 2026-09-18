//! `transport.IrohConfig`, `BindIroh` and the dial phases (`transport/iroh.go:22-406`) on Rust iroh 1.2.0,
//! `relayModeOf` (`cmd/dstore/main.go:123-137`) and the go-iroh v0.2.0 default relay map
//! (`relay/relay.go:23-30`).
//!
//! Mapping decisions (PORTING.md §4.7, §5.12; transport §4.5-§4.6, §7):
//! - The endpoint is built from `presets::Minimal` with the portmapper disabled, and no address lookup is
//!   attached to it. Discovery (the go-iroh mDNS resolver port, plus number0 DNS with relays) runs only in
//!   `discover_dial`, as Go's `discoverDial` is the only path that looks up an id.
//! - A dial phase is one `Endpoint::connect` over every candidate of the phase: Rust iroh sends the
//!   handshake on all known paths of a remote and selects the first that answers, which gives Go's "live
//!   beats dead" race. `connect` returns only after the handshake, so Go's `awaitHandshake` is inherent.
//!   Every `connect` is capped at min(ctx deadline, `CONNECT_PHASE_CAP`) (DD-4).
//! - A phase's error is `dial <addr>: <err>` for each candidate, joined in candidate order, with the one
//!   inner error of that connect (DD-4). A relay candidate on an endpoint without relays is never dialled
//!   (go-iroh `dialTargets`), and its line reads `iroh: no reachable address for endpoint`.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::sync::Arc;
use std::time::Duration;

use dstore_gocompat::ctx::Ctx;
use dstore_gocompat::slog::{Attr, Logger};
use dstore_transport::addr::{self, GoRelayUrl, GoTransportAddr};
use dstore_transport::{Conn, Endpoint, NodeId, TransportError};
use futures::StreamExt;
use futures::stream::BoxStream;
use iroh::address_lookup::{AddressLookup, DnsAddressLookup};
use iroh::endpoint::{
    ConnectError, ConnectWithOptsError, ConnectingError, IdleTimeout, PortmapperConfig,
    QuicTransportConfig, VarInt, presets,
};
use iroh::{EndpointAddr, EndpointId, RelayMap, RelayMode, RelayUrl, TransportAddr};

use crate::conn::IrohConn;
use crate::ifaces::interface_ips;
use crate::mdns::{Announcement, MdnsResolver};
use crate::{
    CLOSE_TIMEOUT, CONNECT_PHASE_CAP, DIRECT_TIMEOUT, GO_DEFAULT_RELAYS, KEEP_ALIVE,
    MAX_IDLE_TIMEOUT, MAX_INCOMING_BIDI_STREAMS, MDNS_LOOKUP_TIMEOUT, NOQ_INITIAL_RTT, ONLINE_WAIT,
    SEND_WINDOW, STREAM_RECEIVE_WINDOW,
};

/// The origin of number0's DNS discovery records (`TXT _iroh.<z32 id>.dns.iroh.link.`), given explicitly so
/// `IROH_FORCE_STAGING_RELAYS` cannot switch it (PORTING.md §5.12).
const DNS_ORIGIN: &str = "dns.iroh.link.";
/// go-iroh `ErrEndpointClosed`.
const ENDPOINT_CLOSED: &str = "iroh: endpoint closed";
/// go-iroh `acceptIncoming` on an endpoint without ALPNs.
const NO_ALPNS: &str = "iroh: no ALPNs configured; nothing to accept";
/// `bind_iroh` with `announce`: the mDNS responder and the pkarr publisher are node-side.
const ANNOUNCE_UNSUPPORTED: &str = "announce is node-side and not implemented";
/// The mDNS discovery warning (`transport/iroh.go:158`).
const MDNS_UNAVAILABLE: &str = "transport: mdns discovery unavailable";

/// The relay configuration of an enabled relay mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelayChoice {
    Default,
    Custom(GoRelayUrl),
}

/// cmd/dstore `relayModeOf`: no_relay → None; url → Custom (a parse error is returned verbatim); else
/// Default.
///
/// `--no-relay` wins over `--relay`. The error is `netaddr.ParseRelayURL`'s, e.g.
/// `failed to parse relay URL: parse ":bad": missing protocol scheme`.
pub fn relay_mode_of(url: &str, no_relay: bool) -> Result<Option<RelayChoice>, String> {
    if no_relay {
        return Ok(None);
    }
    if !url.is_empty() {
        let ru = addr::parse_relay_url(url)?;
        return Ok(Some(RelayChoice::Custom(ru)));
    }
    Ok(Some(RelayChoice::Default))
}

/// `transport.IrohConfig`.
pub struct IrohConfig {
    pub secret_key: iroh::SecretKey,
    /// Client: empty.
    pub alpns: Vec<String>,
    /// None = relays disabled.
    pub relay: Option<RelayChoice>,
    /// Node-side.
    pub advertise: Option<Vec<std::net::SocketAddr>>,
    /// Tests / node.
    pub bind_addr: Option<std::net::SocketAddr>,
    pub loopback: bool,
    /// None → `DIRECT_TIMEOUT`; zero too, as Go's zero `DirectTimeout`.
    pub direct_timeout: Option<Duration>,
    pub discover: bool,
    /// Must be false in v1.
    pub announce: bool,
    /// None → `Logger::default_logger()`.
    pub logger: Option<Logger>,
}

/// `transport.IrohEndpoint`.
pub struct IrohEndpoint {
    ep: iroh::Endpoint,
    id: NodeId,
    relays_enabled: bool,
    /// `IrohConfig::alpns` was non-empty: incoming connections can be accepted.
    accepts: bool,
    direct_timeout: Duration,
    /// The direct addresses, then the relay addresses known at bind (Go `e.addrs`).
    addrs: Vec<String>,
    relays: RelayStrings,
    /// Go `e.lookup`: present when `discover`.
    discovery: Option<Discovery>,
}

/// `transport.BindIroh`.
///
/// Steps, in Go's order where Rust allows:
/// 1. `announce` is refused (node-side, not implemented in v1).
/// 2. Bind from `presets::Minimal`: secret key, ALPNs, the QUIC transport config (keepalive 5 s, idle 60 s,
///    1024 bidi streams, initial RTT 333 ms, stream window 16 MiB, send window 64 MiB), portmapper off,
///    the relay mode, and `bind_addr` as the only IP socket when given. Errors → `transport: bind: …`.
///    The ctx does not bound the bind: go-iroh `Bind` ignores its ctx, so an ended ctx still binds.
/// 3. Direct addresses: `advertise` verbatim (an empty list advertises none), else `127.0.0.1:<port>` with
///    `loopback`, else the `interface_ips()` with the bound port (falling back to `127.0.0.1:<port>`). Each
///    valid one becomes an external address (unmapped, as go-iroh pins it), and every one is advertised as
///    `ip:<addr>`.
/// 4. With `discover`: the mDNS resolver starts (a failure logs WARN `transport: mdns discovery
///    unavailable` error=<text>, and a resolver without listener stays registered, so lookups still send
///    their query and wait out their 3 s), and with relays number0 DNS uses the endpoint's resolver. Go
///    starts both before binding; DNS needs the bound endpoint's resolver here.
/// 5. With relays: wait up to 10 s (bounded by ctx) for a home relay, then advertise `relay:<url>` for each
///    home relay URL.
pub async fn bind_iroh(ctx: &Ctx, cfg: IrohConfig) -> Result<Arc<IrohEndpoint>, TransportError> {
    if cfg.announce {
        return Err(TransportError::Bind(ANNOUNCE_UNSUPPORTED.to_string()));
    }
    let logger = cfg.logger.unwrap_or_else(Logger::default_logger);
    let (relay_mode, relays) = relay_setup(cfg.relay.as_ref());
    let accepts = !cfg.alpns.is_empty();
    let mut builder = iroh::Endpoint::builder(presets::Minimal)
        .secret_key(cfg.secret_key)
        .alpns(cfg.alpns.iter().map(|a| a.as_bytes().to_vec()).collect())
        .transport_config(transport_config())
        .portmapper_config(PortmapperConfig::Disabled)
        .relay_mode(relay_mode);
    if let Some(a) = cfg.bind_addr {
        builder = builder
            .clear_ip_transports()
            .bind_addr(a)
            .map_err(bind_error)?;
    }
    let ep = builder.bind().await.map_err(bind_error)?;

    let port = bound_port(&ep);
    let direct = match cfg.advertise {
        Some(advertise) => advertise,
        None if cfg.loopback => vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))],
        None => local_addr_ports(port),
    };
    let mut addrs = Vec::with_capacity(direct.len());
    for ap in &direct {
        if let Some(external) = external_candidate(ap) {
            ep.add_external_addr(external).await;
        }
        addrs.push(go_ip_addr_string(ap));
    }

    let discovery = if cfg.discover {
        Some(Discovery::start(&ep, cfg.relay.is_some(), &logger).await)
    } else {
        None
    };

    if cfg.relay.is_some() {
        let octx = ctx.with_timeout(ONLINE_WAIT);
        let _ = octx.run(ep.online()).await;
        let home = ep.addr();
        for u in home.relay_urls() {
            addrs.push(format!("relay:{}", relays.go_string(u)));
        }
    }

    Ok(Arc::new(IrohEndpoint {
        id: NodeId(*ep.id().as_bytes()),
        ep,
        relays_enabled: cfg.relay.is_some(),
        accepts,
        direct_timeout: direct_timeout_of(cfg.direct_timeout),
        addrs,
        relays,
        discovery,
    }))
}

/// dial = direct phase, relay+direct phase, discover_dial (transport §4.5).
#[async_trait::async_trait]
impl Endpoint for IrohEndpoint {
    fn id(&self) -> NodeId {
        self.id
    }

    /// Go `Dial`:
    /// 1. The id must be a valid curve point: `data is not a valid public key`.
    /// 2. `parse_addrs(addrs)`, split into relay addresses and the rest (IP and custom), both in order.
    /// 3. With direct candidates: one connect under `min(ctx, direct_timeout)`. Success returns; a failure
    ///    without relay candidates goes to discovery with that error, and with relay candidates it is
    ///    discarded.
    /// 4. With relay candidates: one connect over the relays, then the direct candidates, under ctx. A
    ///    failure goes to discovery with that error.
    /// 5. Without any candidate: discovery alone.
    async fn dial(
        &self,
        ctx: &Ctx,
        id: NodeId,
        addrs: Vec<String>,
        alpn: &str,
    ) -> Result<Arc<dyn Conn>, TransportError> {
        let eid = EndpointId::from_bytes(id.as_bytes()).map_err(|_| TransportError::InvalidKey)?;
        let (relays, direct): (Vec<GoTransportAddr>, Vec<GoTransportAddr>) =
            addr::parse_addrs(&addrs)
                .into_iter()
                .partition(GoTransportAddr::is_relay);
        if !direct.is_empty() {
            let dctx = ctx.with_timeout(self.direct_timeout);
            match self.connect_phase(&dctx, eid, &direct, alpn).await {
                Ok(c) => return Ok(c),
                Err(e) if relays.is_empty() => {
                    return self.discover_dial(ctx, eid, alpn, Some(e)).await;
                }
                Err(_) => {}
            }
        }
        if !relays.is_empty() {
            let all: Vec<GoTransportAddr> = relays.into_iter().chain(direct).collect();
            return match self.connect_phase(ctx, eid, &all, alpn).await {
                Ok(c) => Ok(c),
                Err(e) => self.discover_dial(ctx, eid, alpn, Some(e)).await,
            };
        }
        self.discover_dial(ctx, eid, alpn, None).await
    }

    /// Go `Accept` (go-iroh `accept`): the next incoming connection whose handshake completes. A connection
    /// that dies during its handshake is skipped. A closed endpoint gives `iroh: endpoint closed`, one
    /// without ALPNs `iroh: no ALPNs configured; nothing to accept`, and a ctx end the ctx error.
    async fn accept(&self, ctx: &Ctx) -> Result<Arc<dyn Conn>, TransportError> {
        loop {
            if self.ep.is_closed() {
                return Err(TransportError::Quic(ENDPOINT_CLOSED.to_string()));
            }
            if !self.accepts {
                return Err(TransportError::Quic(NO_ALPNS.to_string()));
            }
            let Some(incoming) = ctx.run(self.ep.accept()).await? else {
                return Err(TransportError::Quic(ENDPOINT_CLOSED.to_string()));
            };
            let accepting = incoming
                .accept()
                .map_err(|e| TransportError::Quic(e.to_string()))?;
            match ctx.run(accepting).await? {
                Ok(c) => return Ok(Arc::new(IrohConn::new(c))),
                Err(ConnectingError::ConnectionError { .. }) => continue,
                Err(e) => return Err(TransportError::Quic(e.to_string())),
            }
        }
    }

    /// Go `Addrs`: the addresses of bind, plus `relay:<url>` for every home relay URL not yet listed (relay
    /// URLs may appear after bind).
    fn addrs(&self) -> Vec<String> {
        let mut out = self.addrs.clone();
        let home = self.ep.addr();
        for u in home.relay_urls() {
            let s = format!("relay:{}", self.relays.go_string(u));
            if !out.contains(&s) {
                out.push(s);
            }
        }
        out
    }

    /// Go `Close`: stops discovery, then closes the endpoint (every connection gets CONNECTION_CLOSE 0),
    /// waiting at most `CLOSE_TIMEOUT`.
    async fn close(&self) {
        self.close_bounded(CLOSE_TIMEOUT).await;
    }
}

impl IrohEndpoint {
    /// Go `IrohEndpoint.Raw`.
    pub fn raw(&self) -> &iroh::Endpoint {
        &self.ep
    }

    /// `Endpoint::close()` bounded to `d`.
    ///
    /// Stops the mDNS listener, then awaits iroh's close, which sends CONNECTION_CLOSE on every connection
    /// and waits for the peers to acknowledge (DD-11).
    pub async fn close_bounded(&self, d: Duration) {
        if let Some(discovery) = &self.discovery {
            discovery.mdns.close();
        }
        let _ = tokio::time::timeout(d, self.ep.close()).await;
    }

    /// One dial phase over `cands` (Go `raceConnect`): the convertible candidates in one connect.
    ///
    /// go-iroh `Connect` per candidate:
    /// - a closed endpoint or the own id fails every candidate before anything else, the ctx included;
    /// - a relay candidate without a relay transport has no dial target (`dialTargets`) and fails with
    ///   `iroh: no reachable address for endpoint` at once, so it is left out of the connect (iroh would
    ///   blackhole its datagrams until the phase cap) and its line carries that text;
    /// - the other candidate lines share the connect's error (DD-4). None converts → `iroh: no reachable
    ///   address for endpoint`.
    ///
    /// With discovery, go-iroh looks a relay-only candidate up inside its `Connect`; here the lookup runs in
    /// `discover_dial` after the phase, with the same outcome.
    async fn connect_phase(
        &self,
        ctx: &Ctx,
        eid: EndpointId,
        cands: &[GoTransportAddr],
        alpn: &str,
    ) -> Result<Arc<dyn Conn>, TransportError> {
        if let Some(e) = self.connect_precheck(eid) {
            return Err(phase_error(cands, |_| e.clone()));
        }
        let untargeted = |a: &GoTransportAddr| a.is_relay() && !self.relays_enabled;
        let targets: Vec<TransportAddr> = cands
            .iter()
            .filter(|a| !untargeted(a))
            .filter_map(to_iroh_addr)
            .collect();
        let e = if targets.is_empty() {
            TransportError::NoAddress
        } else {
            match self
                .connect(ctx, EndpointAddr::from_parts(eid, targets), alpn)
                .await
            {
                Ok(c) => return Ok(c),
                Err(e) => e,
            }
        };
        Err(phase_error(cands, |a| {
            if untargeted(a) {
                TransportError::NoAddress
            } else {
                e.clone()
            }
        }))
    }

    /// One `Endpoint::connect`, awaited to its completed handshake, capped at min(ctx deadline,
    /// `CONNECT_PHASE_CAP`). A ctx end is the ctx error (`context deadline exceeded`, `context canceled`).
    async fn connect(
        &self,
        ctx: &Ctx,
        addr: EndpointAddr,
        alpn: &str,
    ) -> Result<Arc<dyn Conn>, TransportError> {
        let cctx = ctx.with_timeout(CONNECT_PHASE_CAP);
        let conn = cctx
            .run(self.ep.connect(addr, alpn.as_bytes()))
            .await?
            .map_err(connect_error)?;
        Ok(Arc::new(IrohConn::new(conn)))
    }

    /// Go `discoverDial`: without discovery an earlier phase error stands. Otherwise the id is looked up and
    /// the first usable answer dialled; a failure is joined to the earlier error as `discovery: …`.
    async fn discover_dial(
        &self,
        ctx: &Ctx,
        eid: EndpointId,
        alpn: &str,
        prev: Option<TransportError>,
    ) -> Result<Arc<dyn Conn>, TransportError> {
        let Some(discovery) = &self.discovery else {
            // Go: `Connect` with a bare id and no lookup services.
            return Err(prev.unwrap_or_else(|| {
                self.connect_precheck(eid)
                    .unwrap_or(TransportError::NoAddress)
            }));
        };
        let result = self.discover_connect(ctx, discovery, eid, alpn).await;
        match (result, prev) {
            (Ok(c), _) => Ok(c),
            (Err(e), Some(p)) => Err(TransportError::Joined(vec![
                p,
                TransportError::Discovery(Box::new(e)),
            ])),
            (Err(e), None) => Err(e),
        }
    }

    /// go-iroh `Connect` over a bare id (`connectEarly`): closed endpoint, self, lookup (`lookupAddr`), the
    /// dial targets of the answer (relay URLs only with a relay transport), then the connect.
    async fn discover_connect(
        &self,
        ctx: &Ctx,
        discovery: &Discovery,
        eid: EndpointId,
        alpn: &str,
    ) -> Result<Arc<dyn Conn>, TransportError> {
        if let Some(e) = self.connect_precheck(eid) {
            return Err(e);
        }
        // go-iroh `lookupAddr` never looks up the zero id.
        if eid.as_bytes() == &[0u8; 32] {
            return Err(TransportError::NoAddress);
        }
        let Some(mut found) = discovery.first_usable(ctx, eid).await else {
            return Err(TransportError::NoAddress);
        };
        if !self.relays_enabled {
            found.addrs.retain(|a| !a.is_relay());
        }
        if found.addrs.is_empty() {
            return Err(TransportError::NoAddress);
        }
        self.connect(ctx, found, alpn).await
    }

    /// The checks go-iroh `connectEarly` makes before looking an id up.
    fn connect_precheck(&self, eid: EndpointId) -> Option<TransportError> {
        if self.ep.is_closed() {
            return Some(TransportError::Quic(ENDPOINT_CLOSED.to_string()));
        }
        if eid == self.ep.id() {
            return Some(TransportError::SelfConnect);
        }
        None
    }
}

/// Go `raceConnect`'s failure: `dial <addr>: <err>` for every candidate, in candidate order (DD-4), with
/// the inner error `inner` gives that candidate.
fn phase_error(
    cands: &[GoTransportAddr],
    inner: impl Fn(&GoTransportAddr) -> TransportError,
) -> TransportError {
    TransportError::Joined(
        cands
            .iter()
            .map(|a| TransportError::DialAddr {
                addr: a.to_string(),
                source: Box::new(inner(a)),
            })
            .collect(),
    )
}

/// `IrohConfig::direct_timeout`: None, or zero as Go's zero `DirectTimeout`, → `DIRECT_TIMEOUT`.
fn direct_timeout_of(d: Option<Duration>) -> Duration {
    match d {
        Some(d) if !d.is_zero() => d,
        _ => DIRECT_TIMEOUT,
    }
}

/// iroh connect errors with go-iroh's texts where both have one; the rest keep iroh's text (DD-4).
fn connect_error(e: ConnectError) -> TransportError {
    match &e {
        ConnectError::Connect {
            source: ConnectWithOptsError::SelfConnect { .. },
            ..
        } => TransportError::SelfConnect,
        ConnectError::Connect {
            source: ConnectWithOptsError::NoAddress { .. },
            ..
        } => TransportError::NoAddress,
        ConnectError::Connect {
            source: ConnectWithOptsError::EndpointClosed { .. },
            ..
        } => TransportError::Quic(ENDPOINT_CLOSED.to_string()),
        _ => TransportError::Quic(e.to_string()),
    }
}

fn bind_error(e: impl std::fmt::Display) -> TransportError {
    TransportError::Bind(e.to_string())
}

/// `iroh.QUICTransportConfig{KeepAlivePeriod: 5s, MaxIdleTimeout: 60s, MaxIncomingStreams: 1024}`, plus the
/// explicit initial RTT (the RTT sentinel) and the flow-control windows of transport §4.8. iroh's own
/// multipath settings (path keepalive, path idle, path count) stay.
fn transport_config() -> QuicTransportConfig {
    QuicTransportConfig::builder()
        .keep_alive_interval(KEEP_ALIVE)
        .max_idle_timeout(IdleTimeout::try_from(MAX_IDLE_TIMEOUT).ok())
        .max_concurrent_bidi_streams(VarInt::from_u32(MAX_INCOMING_BIDI_STREAMS))
        .initial_rtt(NOQ_INITIAL_RTT)
        .stream_receive_window(VarInt::from_u32(STREAM_RECEIVE_WINDOW))
        .send_window(SEND_WINDOW)
        .build()
}

/// The Go strings of the configured relay URLs, by their iroh form. Go advertises a home relay in the form
/// `netaddr.ParseRelayURL` gave it, which keeps default ports that `url::Url` drops.
#[derive(Debug, Default)]
struct RelayStrings(Vec<(RelayUrl, String)>);

impl RelayStrings {
    fn go_string(&self, u: &RelayUrl) -> String {
        self.0
            .iter()
            .find(|(k, _)| k == u)
            .map_or_else(|| go_relay_string(u), |(_, s)| s.clone())
    }
}

/// The iroh relay mode of a relay choice (PORTING.md §5.12):
/// - None → disabled;
/// - Default → go-iroh's canary hosts;
/// - Custom → that URL, or an empty map when `url::Url` rejects what Go accepted (DD-13): relays then count
///   as enabled, bind waits for a home relay that never comes, and no relay path is dialled.
///
/// Every relay gets QUIC port `RELAY_QUIC_PORT` through `RelayConfig::from(RelayUrl)`.
fn relay_setup(choice: Option<&RelayChoice>) -> (RelayMode, RelayStrings) {
    match choice {
        None => (RelayMode::Disabled, RelayStrings::default()),
        Some(RelayChoice::Default) => {
            let strings: Vec<(RelayUrl, String)> = GO_DEFAULT_RELAYS
                .iter()
                .filter_map(|s| {
                    let u = s.parse::<RelayUrl>().ok()?;
                    let g = addr::parse_relay_url(s).ok()?;
                    Some((u, g.0))
                })
                .collect();
            let map: RelayMap = strings.iter().map(|(u, _)| u.clone()).collect();
            (RelayMode::Custom(map), RelayStrings(strings))
        }
        Some(RelayChoice::Custom(g)) => match g.0.parse::<RelayUrl>() {
            Ok(u) => (
                RelayMode::Custom(RelayMap::from(u.clone())),
                RelayStrings(vec![(u, g.0.clone())]),
            ),
            Err(_) => (
                RelayMode::Custom(RelayMap::empty()),
                RelayStrings::default(),
            ),
        },
    }
}

/// Go `ep.LocalAddr().Port()`: the port of the IPv4 socket (the one `127.0.0.1` reaches), else of the first.
fn bound_port(ep: &iroh::Endpoint) -> u16 {
    let socks = ep.bound_sockets();
    socks
        .iter()
        .find(|a| a.is_ipv4())
        .or_else(|| socks.first())
        .map_or(0, SocketAddr::port)
}

/// `localAddrPorts`: every `interface_ips()` address with `port`, or `127.0.0.1:<port>`.
fn local_addr_ports(port: u16) -> Vec<SocketAddr> {
    let out: Vec<SocketAddr> = interface_ips()
        .into_iter()
        .map(|ip| SocketAddr::new(ip, port))
        .collect();
    if out.is_empty() {
        return vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))];
    }
    out
}

/// go-iroh `canonicalNATTraversalCandidate`, which `AddExternalAddr` pins: unspecified and zero-port
/// addresses are ignored (Rust iroh would store them), and an IPv4-mapped address is unmapped.
fn external_candidate(a: &SocketAddr) -> Option<SocketAddr> {
    if a.ip().is_unspecified() || a.port() == 0 {
        return None;
    }
    let ip = match a.ip() {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    };
    Some(match a {
        SocketAddr::V6(v6) if ip.is_ipv6() => SocketAddr::V6(*v6),
        _ => SocketAddr::new(ip, a.port()),
    })
}

/// `netaddr.IPAddr{Addr: ap}.String()`. A scope id is rendered as its interface name when it has one.
fn go_ip_addr_string(a: &SocketAddr) -> String {
    let (ip, zone) = match a {
        SocketAddr::V4(v4) => (IpAddr::V4(*v4.ip()), None),
        SocketAddr::V6(v6) => (IpAddr::V6(*v6.ip()), zone_name(v6.scope_id())),
    };
    GoTransportAddr::Ip {
        ip,
        zone,
        port: a.port(),
    }
    .to_string()
}

fn zone_name(scope_id: u32) -> Option<String> {
    if scope_id == 0 {
        return None;
    }
    let name = nix::net::if_::if_indextoname(scope_id)
        .ok()
        .and_then(|n| n.into_string().ok());
    Some(name.unwrap_or_else(|| scope_id.to_string()))
}

/// A fresh ephemeral identity.
pub fn generate_secret_key() -> iroh::SecretKey {
    iroh::SecretKey::generate()
}

/// GoTransportAddr → iroh candidate; None when it cannot be converted (unparseable relay URL, unknown
/// zone name).
///
/// - `Relay`: the Go string through `url::Url` (e.g. `relay:example.com` does not convert).
/// - `Ip`: a zone becomes the scope id of the interface of that name, else Go's numeric fallback (the
///   leading decimal digits). An IPv4-mapped IPv6 address is dialled as IPv4, as Go's dual-stack socket
///   does; iroh keeps separate IPv4 and IPv6 sockets.
/// - `Custom`: `CustomAddr::from_parts(id, data)`.
pub fn to_iroh_addr(a: &GoTransportAddr) -> Option<iroh::TransportAddr> {
    match a {
        GoTransportAddr::Relay(u) => u.0.parse::<RelayUrl>().ok().map(TransportAddr::Relay),
        GoTransportAddr::Ip { ip, zone, port } => {
            let sa = match ip {
                IpAddr::V4(v4) => SocketAddr::V4(SocketAddrV4::new(*v4, *port)),
                IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                    Some(v4) => SocketAddr::V4(SocketAddrV4::new(v4, *port)),
                    None => {
                        let scope = match zone {
                            None => 0,
                            Some(z) => zone_index(z, name_to_index)?,
                        };
                        SocketAddr::V6(SocketAddrV6::new(*v6, *port, 0, scope))
                    }
                },
            };
            Some(TransportAddr::Ip(sa))
        }
        GoTransportAddr::Custom { id, data } => Some(TransportAddr::Custom(
            iroh_base::CustomAddr::from_parts(*id, data),
        )),
    }
}

fn name_to_index(name: &str) -> Option<u32> {
    nix::net::if_::if_nametoindex(name).ok().filter(|&i| i != 0)
}

/// Go `ipv6ZoneCache.index`: the interface of that name, else `dtoi(zone)` (the leading decimal digits,
/// saturating at 0xFFFFFF). No interface and no digits → None.
fn zone_index(zone: &str, by_name: impl Fn(&str) -> Option<u32>) -> Option<u32> {
    if let Some(i) = by_name(zone) {
        return Some(i);
    }
    let digits = zone.bytes().take_while(u8::is_ascii_digit);
    let mut n: u32 = 0;
    let mut any = false;
    for d in digits {
        any = true;
        n = n.saturating_mul(10).saturating_add(u32::from(d - b'0'));
        if n >= 0xFF_FFFF {
            return Some(0xFF_FFFF);
        }
    }
    any.then_some(n)
}

/// The Go form of a relay URL as nodes publish it ("relay:" prefix added by the caller).
///
/// The URL's string normalised as `netaddr.ParseRelayURL` does (lower-case host, `/` for an empty path of a
/// special scheme), which leaves `url::Url` strings as they are.
pub fn go_relay_string(u: &iroh::RelayUrl) -> String {
    match addr::parse_relay_url(u.as_str()) {
        Ok(g) => g.0,
        Err(_) => u.to_string(),
    }
}

/// Go `e.lookup` as dstore registers it: the mDNS resolver, and number0 DNS with relays.
struct Discovery {
    /// The go-iroh resolver. When its listener could not start, a resolver without listener stands in:
    /// Go's resolver stays registered, so a lookup still multicasts its query and waits out the timeout.
    mdns: Arc<MdnsResolver>,
    dns: Option<DnsAddressLookup>,
}

impl Discovery {
    /// `setupDiscovery` for a client (`Discover` without `Announce`).
    async fn start(ep: &iroh::Endpoint, relays: bool, logger: &Logger) -> Discovery {
        let mdns = match MdnsResolver::start(logger.clone()).await {
            Ok(r) => r,
            Err(err) => {
                logger.warn(MDNS_UNAVAILABLE, vec![Attr::any("error", err)]);
                MdnsResolver::without_listener(logger.clone())
            }
        };
        let dns = if relays {
            ep.dns_resolver().ok().map(|r| {
                DnsAddressLookup::builder(DNS_ORIGIN.to_string())
                    .dns_resolver(r.clone())
                    .build()
            })
        } else {
            None
        };
        Discovery { mdns, dns }
    }

    /// go-iroh `lookupAddr`: every resolver runs concurrently, errors are skipped, and the first answer for
    /// `eid` with a non-empty address set wins. None when every resolver ends or ctx ends first.
    async fn first_usable(&self, ctx: &Ctx, eid: EndpointId) -> Option<EndpointAddr> {
        let mut lookups: Vec<BoxStream<'_, Option<EndpointAddr>>> = Vec::with_capacity(2);
        lookups.push(futures::stream::once(self.resolve_mdns(ctx, eid)).boxed());
        if let Some(stream) = self.dns.as_ref().and_then(|d| d.resolve(eid)) {
            lookups.push(
                stream
                    .map(|item| item.ok().map(|found| found.into_endpoint_addr()))
                    .boxed(),
            );
        }
        let mut merged = futures::stream::select_all(lookups);
        let first = async {
            while let Some(found) = merged.next().await {
                if let Some(a) = found
                    && a.id == eid
                    && !a.addrs.is_empty()
                {
                    return Some(a);
                }
            }
            None
        };
        ctx.run(first).await.ok().flatten()
    }

    /// The mDNS item: cache or query, polled up to `MDNS_LOOKUP_TIMEOUT`.
    async fn resolve_mdns(&self, ctx: &Ctx, eid: EndpointId) -> Option<EndpointAddr> {
        self.mdns
            .resolve(ctx, *eid.as_bytes(), MDNS_LOOKUP_TIMEOUT)
            .await
            .and_then(|a| announcement_addr(&a))
    }
}

/// go-iroh `infoFromAnnouncement`: the announced IP addresses, and the relay URL when it parses.
fn announcement_addr(a: &Announcement) -> Option<EndpointAddr> {
    let id = EndpointId::from_bytes(&a.id).ok()?;
    let mut addrs: Vec<TransportAddr> = a.addrs.iter().map(|s| TransportAddr::Ip(*s)).collect();
    if let Some(relay) = a.relay.as_deref().and_then(|r| r.parse::<RelayUrl>().ok()) {
        addrs.push(TransportAddr::Relay(relay));
    }
    Some(EndpointAddr::from_parts(id, addrs))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::net::Ipv6Addr;

    use dstore_testkit::golden;
    use serde::Deserialize;

    fn relay(s: &str) -> GoTransportAddr {
        GoTransportAddr::Relay(GoRelayUrl(s.to_string()))
    }

    fn ip(s: &str) -> GoTransportAddr {
        addr::parse_transport_addr(s).expect("test address")
    }

    // cmd/dstore relayModeOf.
    #[test]
    fn relay_mode_of_matches_go() {
        assert_eq!(relay_mode_of("", true), Ok(None));
        assert_eq!(relay_mode_of("https://example.com", true), Ok(None));
        assert_eq!(relay_mode_of(":bad", true), Ok(None));
        assert_eq!(relay_mode_of("", false), Ok(Some(RelayChoice::Default)));
        assert_eq!(
            relay_mode_of("https://Example.COM", false),
            Ok(Some(RelayChoice::Custom(GoRelayUrl(
                "https://example.com/".to_string()
            ))))
        );
        assert_eq!(
            relay_mode_of("relay.example.com", false),
            Ok(Some(RelayChoice::Custom(GoRelayUrl(
                "relay.example.com".to_string()
            ))))
        );
        assert_eq!(
            relay_mode_of(":bad", false),
            Err("failed to parse relay URL: parse \":bad\": missing protocol scheme".to_string())
        );
    }

    fn map_of(mode: &RelayMode) -> RelayMap {
        match mode {
            RelayMode::Custom(m) => m.clone(),
            other => panic!("want a custom relay map, got {other:?}"),
        }
    }

    /// (Go url string, QUIC port) of every relay in the map, in URL order.
    fn configs(mode: &RelayMode, strings: &RelayStrings) -> Vec<(String, Option<u16>)> {
        let relays: Vec<Arc<iroh::RelayConfig>> = map_of(mode).relays();
        let mut out: Vec<(String, Option<u16>)> = relays
            .iter()
            .map(|c| (strings.go_string(&c.url), c.quic.as_ref().map(|q| q.port)))
            .collect();
        out.sort();
        out
    }

    #[test]
    fn relay_setup_modes() {
        let (mode, strings) = relay_setup(None);
        assert_eq!(mode, RelayMode::Disabled);
        assert!(strings.0.is_empty());

        let (mode, strings) = relay_setup(Some(&RelayChoice::Default));
        assert_eq!(map_of(&mode).len(), GO_DEFAULT_RELAYS.len());
        assert_eq!(strings.0.len(), GO_DEFAULT_RELAYS.len());

        // Go keeps the default port a url::Url drops.
        let (mode, strings) = relay_setup(Some(&RelayChoice::Custom(GoRelayUrl(
            "https://example.com:443/".to_string(),
        ))));
        assert_eq!(
            configs(&mode, &strings),
            vec![(
                "https://example.com:443/".to_string(),
                Some(crate::RELAY_QUIC_PORT)
            )]
        );

        // DD-13: a URL Go accepts and url::Url rejects gives an empty map, with relays still enabled.
        let (mode, strings) = relay_setup(Some(&RelayChoice::Custom(GoRelayUrl(
            "relay.example.com".to_string(),
        ))));
        assert!(map_of(&mode).is_empty());
        assert!(strings.0.is_empty());
    }

    #[derive(Debug, Deserialize)]
    struct RelayConfigJson {
        url: String,
        quic_port: Option<u16>,
    }

    #[derive(Debug, Deserialize)]
    struct RelayFile {
        default_quic_port: u16,
        default_map: Vec<RelayConfigJson>,
        default_relay_addrs: Vec<String>,
        custom_url_map: Vec<RelayConfigJson>,
    }

    // transport/relay_urls.json: the maps bind_iroh configures give Go's relay.DefaultMap() and
    // ModeCustomURLs configs (URL-sorted, QUIC port 7842).
    #[test]
    fn relay_maps_match_go_vectors() {
        let f: RelayFile = golden::load_json("transport/relay_urls.json");
        assert_eq!(f.default_quic_port, crate::RELAY_QUIC_PORT);

        let (mode, strings) = relay_setup(Some(&RelayChoice::Default));
        let want: Vec<(String, Option<u16>)> = f
            .default_map
            .iter()
            .map(|c| (c.url.clone(), c.quic_port))
            .collect();
        assert_eq!(configs(&mode, &strings), want);
        let addrs: Vec<String> = configs(&mode, &strings)
            .iter()
            .map(|(u, _)| format!("relay:{u}"))
            .collect();
        assert_eq!(addrs, f.default_relay_addrs);

        let mut custom = Vec::new();
        for input in ["https://relay.example./", "http://Relay.Example:8080"] {
            let choice = relay_mode_of(input, false)
                .expect("custom relay parses")
                .expect("relays enabled");
            let (mode, strings) = relay_setup(Some(&choice));
            custom.extend(configs(&mode, &strings));
        }
        custom.sort();
        let want: Vec<(String, Option<u16>)> = f
            .custom_url_map
            .iter()
            .map(|c| (c.url.clone(), c.quic_port))
            .collect();
        assert_eq!(custom, want);
    }

    #[test]
    fn dns_origin_is_number0_production() {
        assert_eq!(
            DNS_ORIGIN,
            iroh::address_lookup::N0_DNS_ENDPOINT_ORIGIN_PROD
        );
    }

    #[test]
    fn transport_config_sets_go_values_and_the_rtt_sentinel() {
        let dbg = format!("{:?}", transport_config());
        for want in [
            "max_concurrent_bidi_streams: 1024",
            "initial_rtt: 333ms",
            "stream_receive_window: 16777216",
            "send_window: 67108864",
            "keep_alive_interval: Some(5s)",
            "max_idle_timeout: Some(60000)",
        ] {
            assert!(dbg.contains(want), "{want} not in {dbg}");
        }
    }

    #[test]
    fn to_iroh_addr_converts_candidates() {
        assert_eq!(
            to_iroh_addr(&ip("ip:127.0.0.1:9")),
            Some(TransportAddr::Ip("127.0.0.1:9".parse().expect("addr")))
        );
        assert_eq!(
            to_iroh_addr(&ip("ip:[::1]:9")),
            Some(TransportAddr::Ip("[::1]:9".parse().expect("addr")))
        );
        // A 4-in-6 address is dialled over IPv4.
        assert_eq!(
            to_iroh_addr(&ip("ip:[::ffff:1.2.3.4]:5")),
            Some(TransportAddr::Ip("1.2.3.4:5".parse().expect("addr")))
        );
        assert_eq!(
            to_iroh_addr(&relay("https://use1-1.relay.n0.iroh-canary.iroh.link./")),
            Some(TransportAddr::Relay(
                "https://use1-1.relay.n0.iroh-canary.iroh.link./"
                    .parse()
                    .expect("url")
            ))
        );
        assert_eq!(to_iroh_addr(&relay("example.com")), None);
        assert_eq!(to_iroh_addr(&relay("")), None);
        assert_eq!(
            to_iroh_addr(&ip("custom:1_abcd")),
            Some(TransportAddr::Custom(iroh_base::CustomAddr::from_parts(
                1,
                &[0xab, 0xcd]
            )))
        );
    }

    // Go ipv6ZoneCache.index: the name first, then dtoi's leading digits.
    #[test]
    fn zone_index_matches_go() {
        let by_name = |name: &str| (name == "en0").then_some(4);
        assert_eq!(zone_index("en0", by_name), Some(4));
        assert_eq!(zone_index("7", by_name), Some(7));
        assert_eq!(zone_index("12abc", by_name), Some(12));
        assert_eq!(zone_index("0", by_name), Some(0));
        assert_eq!(zone_index("99999999", by_name), Some(0xFF_FFFF));
        assert_eq!(zone_index("eth9", by_name), None);
        assert_eq!(zone_index("", by_name), None);
    }

    #[test]
    fn go_relay_string_normalises_like_go() {
        let u: RelayUrl = "https://use1-1.relay.n0.iroh-canary.iroh.link."
            .parse()
            .expect("url");
        assert_eq!(
            go_relay_string(&u),
            "https://use1-1.relay.n0.iroh-canary.iroh.link./"
        );
        let u: RelayUrl = "https://Example.COM:8443".parse().expect("url");
        assert_eq!(go_relay_string(&u), "https://example.com:8443/");
    }

    #[test]
    fn ip_addr_strings_are_go_forms() {
        assert_eq!(
            go_ip_addr_string(&"192.168.1.2:4242".parse().expect("addr")),
            "ip:192.168.1.2:4242"
        );
        assert_eq!(
            go_ip_addr_string(&"[2001:db8::1]:4242".parse().expect("addr")),
            "ip:[2001:db8::1]:4242"
        );
        assert_eq!(
            go_ip_addr_string(&"[::ffff:10.0.0.1]:1".parse().expect("addr")),
            "ip:[::ffff:10.0.0.1]:1"
        );
    }

    // go-iroh canonicalNATTraversalCandidate.
    #[test]
    fn external_candidates_skip_unspecified_and_port_zero_and_unmap() {
        let cand = |s: &str| external_candidate(&s.parse().expect("addr"));
        assert_eq!(
            cand("127.0.0.1:5"),
            Some("127.0.0.1:5".parse().expect("addr"))
        );
        assert_eq!(
            cand("[2001:db8::1]:5"),
            Some("[2001:db8::1]:5".parse().expect("addr"))
        );
        assert_eq!(
            cand("[::ffff:10.0.0.1]:8"),
            Some("10.0.0.1:8".parse().expect("addr"))
        );
        assert_eq!(cand("0.0.0.0:5"), None);
        assert_eq!(cand("[::]:5"), None);
        assert_eq!(cand("10.0.0.1:0"), None);
        let scoped = SocketAddr::V6(SocketAddrV6::new("fe80::1".parse().expect("ip"), 7, 0, 3));
        assert_eq!(external_candidate(&scoped), Some(scoped));
    }

    // transport.IrohConfig.DirectTimeout: the zero value means 2 s.
    #[test]
    fn direct_timeout_defaults_like_go() {
        assert_eq!(direct_timeout_of(None), DIRECT_TIMEOUT);
        assert_eq!(direct_timeout_of(Some(Duration::ZERO)), DIRECT_TIMEOUT);
        assert_eq!(
            direct_timeout_of(Some(Duration::from_millis(500))),
            Duration::from_millis(500)
        );
    }

    // transport §5.7: one inner error per candidate line, in candidate order.
    #[test]
    fn phase_errors_join_candidates_in_order() {
        let cands = [relay("example.com"), ip("ip:127.0.0.1:9"), ip("1_abcd")];
        let deadline = TransportError::Ctx(dstore_gocompat::ctx::CtxError::DeadlineExceeded);
        let e = phase_error(&cands, |_| deadline.clone());
        assert_eq!(
            e.to_string(),
            "dial relay:example.com: context deadline exceeded\n\
             dial ip:127.0.0.1:9: context deadline exceeded\n\
             dial 1_abcd: context deadline exceeded"
        );
        let e = phase_error(&cands, |a| {
            if a.is_relay() {
                TransportError::NoAddress
            } else {
                deadline.clone()
            }
        });
        assert_eq!(
            e.to_string(),
            "dial relay:example.com: iroh: no reachable address for endpoint\n\
             dial ip:127.0.0.1:9: context deadline exceeded\n\
             dial 1_abcd: context deadline exceeded"
        );
        let e = phase_error(&cands[1..2], |_| TransportError::SelfConnect);
        assert_eq!(
            e.to_string(),
            "dial ip:127.0.0.1:9: iroh: cannot connect to self"
        );
        let joined = TransportError::Joined(vec![
            e,
            TransportError::Discovery(Box::new(TransportError::NoAddress)),
        ]);
        assert_eq!(
            joined.to_string(),
            "dial ip:127.0.0.1:9: iroh: cannot connect to self\n\
             discovery: iroh: no reachable address for endpoint"
        );
    }

    #[test]
    fn announcements_become_endpoint_addrs() {
        let key = iroh::SecretKey::from_bytes(&[7; 32]).public();
        let a = Announcement {
            id: *key.as_bytes(),
            addrs: vec![
                "192.168.1.2:4242".parse().expect("addr"),
                SocketAddr::from((Ipv6Addr::LOCALHOST, 4242)),
            ],
            relay: Some("https://use1-1.relay.n0.iroh-canary.iroh.link./".to_string()),
            user_data: None,
        };
        let got = announcement_addr(&a).expect("valid id");
        assert_eq!(got.id, key);
        assert_eq!(got.addrs.len(), 3);
        assert_eq!(got.relay_urls().count(), 1);

        let unparseable = Announcement {
            relay: Some("not a url".to_string()),
            ..a.clone()
        };
        assert_eq!(
            announcement_addr(&unparseable).map(|e| e.addrs.len()),
            Some(2)
        );

        let mut bad = a;
        bad.id = [2; 32];
        bad.id[1..].fill(0);
        assert!(announcement_addr(&bad).is_none());
    }

    #[test]
    fn generated_keys_are_fresh() {
        assert_ne!(
            generate_secret_key().public(),
            generate_secret_key().public()
        );
    }
}
