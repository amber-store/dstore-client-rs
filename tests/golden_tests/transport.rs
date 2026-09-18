//! Golden tests of `dstore-transport` (owner transport): `transport/addrs.json`, `transport/relay_urls.json`,
//! `transport/pool_scripts.json`. Schemas: VECTORS.md "Family `transport`" and `tools/vectorgen/docs/vectorgen-view.md`.

use std::collections::HashMap;
use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use dstore_gocompat::time::{duration_round, duration_string};
use dstore_testkit::golden::{self, decimal_i64, decimal_u64};
use dstore_transport::addr::{self, GoTransportAddr};
use dstore_transport::mem::{MemEndpoint, Network};
use dstore_transport::{CallError, Conn, Ctx, Endpoint, NodeId, Pool, Stream, TransportError};
use dstore_wire::Msg;
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---- transport/addrs.json ----

#[derive(Debug, Deserialize)]
struct AddrJson {
    kind: String,
    string: String,
    relay_url: Option<String>,
    ip: Option<String>,
    zone: Option<String>,
    port: Option<u16>,
    custom_id: Option<String>,
    custom_data: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ParseAddrCase {
    input: String,
    ok: bool,
    addr: Option<AddrJson>,
    error: Option<String>,
    parse_addrs: Vec<AddrJson>,
}

#[derive(Debug, Deserialize)]
struct ParseAddrsCase {
    name: String,
    input: Vec<String>,
    output: Vec<AddrJson>,
}

#[derive(Debug, Deserialize)]
struct AddrPortCase {
    input: String,
    ok: bool,
    ip: Option<String>,
    zone: Option<String>,
    port: Option<u16>,
    string: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AddrsFile {
    parse_transport_addr: Vec<ParseAddrCase>,
    parse_addrs: Vec<ParseAddrsCase>,
    parse_addr_port: Vec<AddrPortCase>,
}

fn std_ip(s: &str) -> IpAddr {
    match s.parse() {
        Ok(a) => a,
        Err(e) => panic!("vector address {s:?} does not parse: {e}"),
    }
}

fn check_addr(what: &str, got: &GoTransportAddr, want: &AddrJson) {
    assert_eq!(got.to_string(), want.string, "{what}: String()");
    assert_eq!(got.is_relay(), want.kind == "relay", "{what}: kind");
    match (want.kind.as_str(), got) {
        ("relay", GoTransportAddr::Relay(u)) => {
            assert_eq!(Some(&u.0), want.relay_url.as_ref(), "{what}: relay_url");
        }
        ("ip", GoTransportAddr::Ip { ip, zone, port }) => {
            let want_ip = want.ip.as_deref().map(std_ip);
            assert_eq!(Some(*ip), want_ip, "{what}: ip");
            assert_eq!(zone, &want.zone, "{what}: zone");
            assert_eq!(Some(*port), want.port, "{what}: port");
        }
        ("custom", GoTransportAddr::Custom { id, data }) => {
            let want_id = want.custom_id.as_deref().map(|s| match s.parse::<u64>() {
                Ok(v) => v,
                Err(e) => panic!("{what}: custom_id {s:?}: {e}"),
            });
            assert_eq!(Some(*id), want_id, "{what}: custom_id");
            assert_eq!(
                Some(hex::encode(data)),
                want.custom_data,
                "{what}: custom_data"
            );
        }
        (kind, got) => panic!("{what}: want kind {kind}, got {got:?}"),
    }
}

fn check_addr_list(what: &str, got: &[GoTransportAddr], want: &[AddrJson]) {
    assert_eq!(got.len(), want.len(), "{what}: {got:?}");
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        check_addr(&format!("{what}[{i}]"), g, w);
    }
}

#[test]
fn parse_transport_addr_and_parse_addrs() {
    let f: AddrsFile = golden::load_json("transport/addrs.json");
    assert!(!f.parse_transport_addr.is_empty());
    for c in &f.parse_transport_addr {
        let what = format!("parse_transport_addr({:?})", c.input);
        match (addr::parse_transport_addr(&c.input), &c.addr) {
            (Ok(got), Some(want)) => {
                assert!(c.ok, "{what}");
                check_addr(&what, &got, want);
            }
            (Err(e), None) => {
                assert!(!c.ok, "{what}");
                assert_eq!(Some(e), c.error, "{what}");
            }
            (got, _) => panic!("{what}: got {got:?}, want ok={} error={:?}", c.ok, c.error),
        }
        let got = addr::parse_addrs(std::slice::from_ref(&c.input));
        check_addr_list(
            &format!("ParseAddrs([{:?}])", c.input),
            &got,
            &c.parse_addrs,
        );
    }
    for c in &f.parse_addrs {
        let got = addr::parse_addrs(&c.input);
        check_addr_list(&format!("parse_addrs {}", c.name), &got, &c.output);
    }
}

#[test]
fn parse_addr_port() {
    let f: AddrsFile = golden::load_json("transport/addrs.json");
    assert!(!f.parse_addr_port.is_empty());
    for c in &f.parse_addr_port {
        let what = format!("parse_addr_port({:?})", c.input);
        match addr::parse_addr_port(&c.input) {
            Ok((ip, zone, port)) => {
                assert!(c.ok, "{what}: got ok, want {:?}", c.error);
                assert_eq!(Some(ip), c.ip.as_deref().map(std_ip), "{what}: ip");
                assert_eq!(zone, c.zone, "{what}: zone");
                assert_eq!(Some(port), c.port, "{what}: port");
                let s = GoTransportAddr::Ip { ip, zone, port }.to_string();
                assert_eq!(
                    s.strip_prefix("ip:"),
                    c.string.as_deref(),
                    "{what}: AddrPort.String()"
                );
            }
            Err(e) => {
                assert!(!c.ok, "{what}: got {e:?}");
                assert_eq!(Some(e), c.error, "{what}");
            }
        }
    }
}

// ---- transport/relay_urls.json ----

#[derive(Debug, Deserialize)]
struct RelayUrlCase {
    input: String,
    ok: bool,
    string: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RelayConfigJson {
    url: String,
    quic_port: Option<u16>,
}

#[derive(Debug, Deserialize)]
struct RelayFile {
    parse_relay_url: Vec<RelayUrlCase>,
    default_quic_port: u16,
    default_map: Vec<RelayConfigJson>,
    default_relay_addrs: Vec<String>,
    custom_url_map: Vec<RelayConfigJson>,
}

fn relay_string(s: &str) -> String {
    match addr::parse_relay_url(s) {
        Ok(u) => u.0,
        Err(e) => panic!("parse_relay_url({s:?}): {e}"),
    }
}

#[test]
fn parse_relay_url() {
    let f: RelayFile = golden::load_json("transport/relay_urls.json");
    assert!(!f.parse_relay_url.is_empty());
    for c in &f.parse_relay_url {
        let what = format!("parse_relay_url({:?})", c.input);
        match addr::parse_relay_url(&c.input) {
            Ok(u) => {
                assert!(c.ok, "{what}: got {u:?}, want {:?}", c.error);
                assert_eq!(Some(u.0), c.string, "{what}");
            }
            Err(e) => {
                assert!(!c.ok, "{what}: got {e:?}");
                assert_eq!(Some(e), c.error, "{what}");
            }
        }
    }
}

// The Go strings of the default relay map and a `--relay` map (the maps themselves are transport-iroh's):
// every URL normalises through `parse_relay_url` and gets QUIC port 7842.
#[test]
fn relay_map_strings() {
    let f: RelayFile = golden::load_json("transport/relay_urls.json");
    assert_eq!(f.default_quic_port, dstore_transport_iroh::RELAY_QUIC_PORT);

    let mut defaults: Vec<String> = dstore_transport_iroh::GO_DEFAULT_RELAYS
        .iter()
        .map(|s| relay_string(s))
        .collect();
    defaults.sort(); // relay.Map.Configs() and URLs() sort by string.
    let map_urls: Vec<String> = f.default_map.iter().map(|c| c.url.clone()).collect();
    assert_eq!(defaults, map_urls);
    for c in &f.default_map {
        assert_eq!(
            c.quic_port,
            Some(dstore_transport_iroh::RELAY_QUIC_PORT),
            "{}",
            c.url
        );
    }
    let addrs: Vec<String> = defaults.iter().map(|u| format!("relay:{u}")).collect();
    assert_eq!(addrs, f.default_relay_addrs);
    for a in &f.default_relay_addrs {
        match addr::parse_transport_addr(a) {
            Ok(got) => {
                assert!(got.is_relay(), "{a}");
                assert_eq!(&got.to_string(), a);
            }
            Err(e) => panic!("{a}: {e}"),
        }
    }

    let mut custom: Vec<String> = ["https://relay.example./", "http://Relay.Example:8080"]
        .iter()
        .map(|s| relay_string(s))
        .collect();
    custom.sort();
    let map_urls: Vec<String> = f.custom_url_map.iter().map(|c| c.url.clone()).collect();
    assert_eq!(custom, map_urls);
    for c in &f.custom_url_map {
        assert_eq!(
            c.quic_port,
            Some(dstore_transport_iroh::RELAY_QUIC_PORT),
            "{}",
            c.url
        );
    }
}

// ---- transport/pool_scripts.json ----

#[derive(Debug, Deserialize)]
struct PoolFile {
    scripts: Vec<Script>,
    rtt_round: Vec<RttCase>,
    error_texts: Vec<ErrorText>,
}

#[derive(Debug, Deserialize)]
struct Script {
    name: String,
    #[allow(dead_code)]
    description: String,
    per_peer: i64,
    nodes: Vec<NodeJson>,
    steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
struct NodeJson {
    name: String,
    id: String,
    bound: bool,
    alpns: Vec<String>,
    server: Option<ServerJson>,
}

#[derive(Debug, Clone, Deserialize)]
struct ServerJson {
    mode: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    text: String,
    #[serde(default, deserialize_with = "decimal_i64")]
    retry_after_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct MsgJson {
    #[serde(rename = "type")]
    typ: i64,
    #[serde(deserialize_with = "decimal_u64")]
    epoch: u64,
    code: String,
    text: String,
    #[serde(deserialize_with = "decimal_i64")]
    retry_after_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct RemoteJson {
    code: String,
    text: String,
    #[serde(deserialize_with = "decimal_i64")]
    retry_after_ns: i64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct PathJson {
    direct: bool,
    #[serde(deserialize_with = "decimal_i64")]
    rtt_ns: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct Step {
    op: String,
    #[serde(default)]
    repeat: usize,
    #[serde(default)]
    peer: String,
    #[serde(default)]
    alpn: String,
    #[serde(default)]
    timeout_ms: u64,
    #[serde(default)]
    ms: u64,
    down: Option<bool>,
    a: Option<String>,
    b: Option<String>,
    cut: Option<bool>,
    #[serde(default)]
    text: String,
    request: Option<MsgJson>,
    stream: Option<usize>,
    server_conn: Option<usize>,
    server_stream: Option<usize>,
    #[serde(default)]
    data: String,
    #[serde(default)]
    buf_len: usize,
    ok: bool,
    error: Option<String>,
    conn: Option<usize>,
    n: Option<usize>,
    #[serde(default)]
    read: String,
    reply: Option<MsgJson>,
    remote: Option<RemoteJson>,
    path: Option<PathJson>,
    dial_attempts: Option<usize>,
    dials: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct RttCase {
    #[serde(deserialize_with = "decimal_i64")]
    ns: i64,
    #[serde(deserialize_with = "decimal_i64")]
    rounded_ns: i64,
    string: String,
}

#[derive(Debug, Deserialize)]
struct ErrorText {
    name: String,
    text: String,
}

/// The observable result of one step, in the vectors' terms.
#[derive(Debug, Default, PartialEq)]
struct Outcome {
    ok: bool,
    error: Option<String>,
    conn: Option<usize>,
    stream: Option<usize>,
    server_conn: Option<usize>,
    server_stream: Option<usize>,
    n: Option<usize>,
    read: Vec<u8>,
    reply: Option<MsgJson>,
    remote: Option<RemoteJson>,
    path: Option<PathJson>,
    dial_attempts: Option<usize>,
    dials: Option<usize>,
}

impl Outcome {
    fn result<T, E: std::fmt::Display>(r: &Result<T, E>) -> Outcome {
        match r {
            Ok(_) => Outcome {
                ok: true,
                ..Default::default()
            },
            Err(e) => Outcome {
                error: Some(e.to_string()),
                ..Default::default()
            },
        }
    }
}

/// The outcome a step's vector records. Stream and connection indexes are outputs only for the ops that
/// create them. In Rust a TErr reply is only a `CallError::Remote`, so Go's TErr `reply` is not expected.
fn expected(st: &Step) -> Outcome {
    let mut o = Outcome {
        ok: st.ok,
        error: st.error.clone(),
        conn: st.conn,
        n: st.n,
        read: golden::hex(&st.read),
        reply: st.reply.clone(),
        remote: st.remote.clone(),
        path: st.path.clone(),
        dial_attempts: st.dial_attempts,
        dials: st.dials,
        ..Default::default()
    };
    match st.op.as_str() {
        "open" => o.stream = st.stream,
        "server_accept" => o.server_conn = st.server_conn,
        "server_accept_stream" | "server_open_stream" => o.server_stream = st.server_stream,
        _ => {}
    }
    if o.remote.is_some() {
        o.reply = None;
    }
    o
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The generator's `tspScriptEndpoint`: wraps the client's `MemEndpoint`, counts dial attempts, numbers
/// successful dials and can fail the next dial with a given text before it reaches the network.
struct ScriptEndpoint {
    inner: Arc<MemEndpoint>,
    state: Mutex<ScriptState>,
}

#[derive(Default)]
struct ScriptState {
    attempts: usize,
    conns: Vec<Arc<dyn Conn>>,
    fail_next: String,
}

impl ScriptEndpoint {
    fn index(&self, c: &Arc<dyn Conn>) -> Option<usize> {
        let want = Arc::as_ptr(c) as *const ();
        lock(&self.state)
            .conns
            .iter()
            .position(|x| Arc::as_ptr(x) as *const () == want)
    }

    fn counts(&self) -> (usize, usize) {
        let st = lock(&self.state);
        (st.attempts, st.conns.len())
    }
}

// The expansion of `#[async_trait]` for the trait's async methods (the root package has no async-trait
// dependency).
impl Endpoint for ScriptEndpoint {
    fn id(&self) -> NodeId {
        self.inner.id()
    }

    fn dial<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        ctx: &'life1 Ctx,
        id: NodeId,
        addrs: Vec<String>,
        alpn: &'life2 str,
    ) -> BoxFuture<'async_trait, Result<Arc<dyn Conn>, TransportError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let fail = {
                let mut st = lock(&self.state);
                st.attempts += 1;
                std::mem::take(&mut st.fail_next)
            };
            if !fail.is_empty() {
                return Err(TransportError::Quic(fail));
            }
            let c = self.inner.dial(ctx, id, addrs, alpn).await?;
            lock(&self.state).conns.push(c.clone());
            Ok(c)
        })
    }

    fn accept<'life0, 'life1, 'async_trait>(
        &'life0 self,
        ctx: &'life1 Ctx,
    ) -> BoxFuture<'async_trait, Result<Arc<dyn Conn>, TransportError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { self.inner.accept(ctx).await })
    }

    fn addrs(&self) -> Vec<String> {
        self.inner.addrs()
    }

    fn close<'life0, 'async_trait>(&'life0 self) -> BoxFuture<'async_trait, ()>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { self.inner.close().await })
    }
}

/// `tspHandle`: read one frame, answer per mode, then `wire.CloseStream`.
async fn serve_stream(mut s: Stream, srv: ServerJson) {
    if let Ok(req) = dstore_wire::read_msg(&mut *s.recv).await {
        match srv.mode.as_str() {
            "pong" => {
                let reply = Msg {
                    typ: dstore_wire::T_PONG,
                    epoch: req.epoch.wrapping_add(1),
                    ..Default::default()
                };
                let _ = dstore_wire::write_msg(&mut *s.send, &reply).await;
            }
            "err" => {
                let mut m = dstore_wire::err_msg(&srv.code, &srv.text);
                m.retry_after = srv.retry_after_ms;
                let _ = dstore_wire::write_msg(&mut *s.send, &m).await;
            }
            _ => {}
        }
    }
    s.close_stream();
}

/// `tspServe`: an accept loop, one task per connection and per stream.
async fn serve(ctx: Ctx, ep: Arc<MemEndpoint>, srv: ServerJson) {
    while let Ok(c) = ep.accept(&ctx).await {
        let ctx = ctx.clone();
        let srv = srv.clone();
        tokio::spawn(async move {
            while let Ok(s) = c.accept_stream(&ctx).await {
                tokio::spawn(serve_stream(s, srv.clone()));
            }
        });
    }
}

const SERVER_TIMEOUT: Duration = Duration::from_secs(2);

struct Harness {
    net: Arc<Network>,
    ids: HashMap<String, NodeId>,
    eps: HashMap<String, Arc<MemEndpoint>>,
    ep: Arc<ScriptEndpoint>,
    pool: Pool,
    streams: Vec<Stream>,
    sconns: Vec<Arc<dyn Conn>>,
    sstreams: Vec<Stream>,
}

impl Harness {
    fn peer(&self, name: &str) -> NodeId {
        match self.ids.get(name) {
            Some(id) => *id,
            None => panic!("unknown node {name:?}"),
        }
    }

    fn endpoint(&self, name: &str) -> Arc<MemEndpoint> {
        match self.eps.get(name) {
            Some(ep) => ep.clone(),
            None => panic!("node {name:?} is not bound"),
        }
    }

    /// `stepCtx`: the step's timeout, else the fallback, else a plain cancellable ctx.
    fn step_ctx(st: &Step, fallback: Option<Duration>) -> Ctx {
        let bg = Ctx::background();
        if st.timeout_ms > 0 {
            bg.with_timeout(Duration::from_millis(st.timeout_ms))
        } else if let Some(d) = fallback {
            bg.with_timeout(d)
        } else {
            bg.with_cancel()
        }
    }

    fn counts(&self, o: &mut Outcome) {
        let (a, d) = self.ep.counts();
        o.dial_attempts = Some(a);
        o.dials = Some(d);
    }

    async fn run(&mut self, st: &Step) -> Outcome {
        match st.op.as_str() {
            "get" => {
                let id = self.peer(&st.peer);
                let ctx = Self::step_ctx(st, None);
                let r = self.pool.get(&ctx, id, &st.alpn).await;
                ctx.cancel();
                let mut o = Outcome::result(&r);
                if let Ok(c) = &r {
                    match self.ep.index(c) {
                        Some(i) => o.conn = Some(i),
                        None => panic!("get returned a connection the endpoint never dialed"),
                    }
                }
                self.counts(&mut o);
                o
            }
            "open" => {
                let id = self.peer(&st.peer);
                let ctx = Self::step_ctx(st, None);
                let r = self.pool.open(&ctx, id, &st.alpn).await;
                ctx.cancel();
                let mut o = Outcome::result(&r);
                if let Ok(s) = r {
                    o.stream = Some(self.streams.len());
                    self.streams.push(s);
                }
                self.counts(&mut o);
                o
            }
            "call" => {
                let id = self.peer(&st.peer);
                let Some(req) = &st.request else {
                    panic!("call without a request");
                };
                let msg = Msg {
                    typ: req.typ,
                    epoch: req.epoch,
                    ..Default::default()
                };
                let ctx = Self::step_ctx(st, None);
                let r = self.pool.call(&ctx, id, &st.alpn, &msg).await;
                ctx.cancel();
                let mut o = Outcome::result(&r);
                match &r {
                    Ok(reply) => {
                        o.reply = Some(MsgJson {
                            typ: reply.typ,
                            epoch: reply.epoch,
                            code: reply.code.clone(),
                            text: reply.text.clone(),
                            retry_after_ms: reply.retry_after,
                        });
                    }
                    Err(CallError::Remote(e)) => {
                        o.remote = Some(RemoteJson {
                            code: e.code.clone(),
                            text: e.text.clone(),
                            retry_after_ns: i64::try_from(e.retry_after.as_nanos())
                                .unwrap_or(i64::MAX),
                        });
                    }
                    Err(_) => {}
                }
                self.counts(&mut o);
                o
            }
            "drop" => {
                self.pool.drop_peer(self.peer(&st.peer), &st.alpn);
                Outcome {
                    ok: true,
                    ..Default::default()
                }
            }
            "path" => match self.pool.path(self.peer(&st.peer), &st.alpn) {
                Some(p) => Outcome {
                    ok: true,
                    path: Some(PathJson {
                        direct: p.direct,
                        rtt_ns: i64::try_from(p.rtt.as_nanos()).unwrap_or(i64::MAX),
                    }),
                    ..Default::default()
                },
                None => Outcome::default(),
            },
            "close" => {
                self.pool.close();
                Outcome {
                    ok: true,
                    ..Default::default()
                }
            }
            "set_down" => {
                let Some(down) = st.down else {
                    panic!("set_down needs down");
                };
                self.net.set_down(self.peer(&st.peer), down);
                Outcome {
                    ok: true,
                    ..Default::default()
                }
            }
            "partition" => {
                let (Some(a), Some(b), Some(cut)) = (&st.a, &st.b, st.cut) else {
                    panic!("partition needs a, b and cut");
                };
                self.net.partition(self.peer(a), self.peer(b), cut);
                Outcome {
                    ok: true,
                    ..Default::default()
                }
            }
            "fail_next_dial" => {
                lock(&self.ep.state).fail_next = st.text.clone();
                Outcome {
                    ok: true,
                    ..Default::default()
                }
            }
            "sleep" => {
                tokio::time::sleep(Duration::from_millis(st.ms)).await;
                Outcome {
                    ok: true,
                    ..Default::default()
                }
            }
            "close_endpoint" => {
                self.endpoint(&st.peer).close().await;
                Outcome {
                    ok: true,
                    ..Default::default()
                }
            }
            "stream_write" | "stream_read" | "stream_close_write" | "stream_cancel_read" => {
                let i = index(st.stream, self.streams.len(), "stream");
                let op = st.op.trim_start_matches("stream_");
                stream_op(st, &mut self.streams[i], op).await
            }
            "server_write" | "server_read" | "server_close_write" | "server_cancel_read" => {
                let i = index(st.server_stream, self.sstreams.len(), "server stream");
                let op = st.op.trim_start_matches("server_");
                stream_op(st, &mut self.sstreams[i], op).await
            }
            "server_accept" => {
                let ep = self.endpoint(&st.peer);
                let ctx = Self::step_ctx(st, Some(SERVER_TIMEOUT));
                let r = ep.accept(&ctx).await;
                ctx.cancel();
                let mut o = Outcome::result(&r);
                if let Ok(c) = r {
                    o.server_conn = Some(self.sconns.len());
                    self.sconns.push(c);
                }
                o
            }
            "server_accept_stream" | "server_open_stream" => {
                let i = index(st.server_conn, self.sconns.len(), "server conn");
                let c = self.sconns[i].clone();
                let ctx = Self::step_ctx(st, Some(SERVER_TIMEOUT));
                let r = if st.op == "server_accept_stream" {
                    c.accept_stream(&ctx).await
                } else {
                    c.open_stream(&ctx).await
                };
                ctx.cancel();
                let mut o = Outcome::result(&r);
                if let Ok(s) = r {
                    o.server_stream = Some(self.sstreams.len());
                    self.sstreams.push(s);
                }
                o
            }
            "server_wait_closed" => {
                let i = index(st.server_conn, self.sconns.len(), "server conn");
                let c = self.sconns[i].clone();
                match tokio::time::timeout(Duration::from_millis(st.ms), c.closed()).await {
                    Ok(()) => Outcome {
                        ok: true,
                        ..Default::default()
                    },
                    Err(_) => Outcome {
                        error: Some("not closed".to_string()),
                        ..Default::default()
                    },
                }
            }
            op => panic!("unknown op {op:?}"),
        }
    }
}

fn index(i: Option<usize>, n: usize, what: &str) -> usize {
    match i {
        Some(i) if i < n => i,
        _ => panic!("bad {what} index {i:?} of {n}"),
    }
}

/// `tspStreamOp`: one `Write`, one `Read` into a `buf_len` buffer (a read of 0 bytes is Go's `EOF`),
/// `CloseWrite` or `CancelRead(0)`.
async fn stream_op(st: &Step, s: &mut Stream, op: &str) -> Outcome {
    match op {
        "write" => {
            // Go's pipe `Write` writes everything or fails, reporting the bytes written before the failure.
            let data = golden::hex(&st.data);
            let mut n = 0;
            while let Some(rest) = data.get(n..).filter(|r| !r.is_empty()) {
                match s.send.write(rest).await {
                    Ok(0) => {
                        return Outcome {
                            error: Some("short write".to_string()),
                            n: Some(n),
                            ..Default::default()
                        };
                    }
                    Ok(k) => n += k,
                    Err(e) => {
                        return Outcome {
                            error: Some(dstore_gocompat::errno::io_error_text(&e)),
                            n: Some(n),
                            ..Default::default()
                        };
                    }
                }
            }
            Outcome {
                ok: true,
                n: Some(n),
                ..Default::default()
            }
        }
        "read" => {
            assert!(st.buf_len > 0, "read needs buf_len");
            let mut buf = vec![0u8; st.buf_len];
            match s.recv.read(&mut buf).await {
                Ok(0) => Outcome {
                    error: Some("EOF".to_string()),
                    n: Some(0),
                    ..Default::default()
                },
                Ok(n) => Outcome {
                    ok: true,
                    n: Some(n),
                    read: buf[..n].to_vec(),
                    ..Default::default()
                },
                Err(e) => Outcome {
                    error: Some(dstore_gocompat::errno::io_error_text(&e)),
                    n: Some(0),
                    ..Default::default()
                },
            }
        }
        "close_write" => {
            s.close_write();
            Outcome {
                ok: true,
                ..Default::default()
            }
        }
        "cancel_read" => {
            s.cancel_read(0);
            Outcome {
                ok: true,
                ..Default::default()
            }
        }
        op => panic!("unknown stream op {op:?}"),
    }
}

fn node_id(hex32: &str) -> NodeId {
    let b = golden::hex(hex32);
    match <[u8; 32]>::try_from(b.as_slice()) {
        Ok(a) => NodeId(a),
        Err(_) => panic!("node id {hex32:?} is not 32 bytes"),
    }
}

async fn run_script(sc: &Script) {
    let net = Network::new();
    let server_ctx = Ctx::background().with_cancel();
    let mut servers = Vec::new();
    let mut ids = HashMap::new();
    let mut eps = HashMap::new();
    for n in &sc.nodes {
        let id = node_id(&n.id);
        ids.insert(n.name.clone(), id);
        if !n.bound {
            continue;
        }
        let alpns: Vec<&str> = n.alpns.iter().map(String::as_str).collect();
        let ep = net.bind(id, &alpns);
        eps.insert(n.name.clone(), ep.clone());
        if let Some(srv) = &n.server
            && srv.mode != "manual"
            && srv.mode != "none"
        {
            servers.push(tokio::spawn(serve(server_ctx.clone(), ep, srv.clone())));
        }
    }
    let Some(client) = eps.get("client").cloned() else {
        panic!("script {} has no bound client", sc.name);
    };
    let ep = Arc::new(ScriptEndpoint {
        inner: client,
        state: Mutex::new(ScriptState::default()),
    });
    let dyn_ep: Arc<dyn Endpoint> = ep.clone();
    let pool = Pool::new(
        dyn_ep,
        Arc::new(|_| Vec::new()),
        usize::try_from(sc.per_peer).unwrap_or(0),
    );
    let mut h = Harness {
        net,
        ids,
        eps,
        ep,
        pool,
        streams: Vec::new(),
        sconns: Vec::new(),
        sstreams: Vec::new(),
    };
    for (i, st) in sc.steps.iter().enumerate() {
        let want = expected(st);
        let runs = st.repeat.max(1);
        let mut got = Outcome::default();
        for run in 0..runs {
            got = h.run(st).await;
            assert_eq!(
                (got.ok, &got.error),
                (want.ok, &want.error),
                "script {} step {i} ({}) run {run}",
                sc.name,
                st.op
            );
        }
        assert_eq!(got, want, "script {} step {i} ({})", sc.name, st.op);
        // Go's TErr reply agrees with the remote error Rust returns.
        if let (Some(reply), Some(remote)) = (&st.reply, &st.remote) {
            assert_eq!(reply.typ, dstore_wire::T_ERR);
            assert_eq!((&reply.code, &reply.text), (&remote.code, &remote.text));
            assert_eq!(reply.retry_after_ms * 1_000_000, remote.retry_after_ns);
        }
    }
    h.pool.close();
    server_ctx.cancel();
    for s in servers {
        s.abort();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pool_scripts() {
    let f: PoolFile = golden::load_json("transport/pool_scripts.json");
    let names: Vec<&str> = f.scripts.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "grow_to_per_peer_then_round_robin",
            "per_peer_zero_means_one",
            "failed_dial_window",
            "failed_dial_ignored_with_live_conn",
            "drop_close_and_path",
            "closed_conns_are_filtered",
            "mem_dial_errors",
            "call_reply",
            "call_remote_error",
            "call_ctx_deadline",
            "call_eof_does_not_drop",
            "open_stream_failure_drops",
            "mem_stream_pipes",
        ]
    );
    for sc in &f.scripts {
        run_script(sc).await;
    }
}

#[test]
fn rtt_round() {
    let f: PoolFile = golden::load_json("transport/pool_scripts.json");
    assert!(!f.rtt_round.is_empty());
    for c in &f.rtt_round {
        let r = duration_round(c.ns, 1_000_000);
        assert_eq!(r, c.rounded_ns, "Round({})", c.ns);
        assert_eq!(duration_string(r), c.string, "String({r})");
    }
}

fn parsed(s: &str) -> GoTransportAddr {
    match addr::parse_transport_addr(s) {
        Ok(a) => a,
        Err(e) => panic!("{s}: {e}"),
    }
}

fn dial(a: &str, inner: TransportError) -> TransportError {
    TransportError::DialAddr {
        addr: parsed(a).to_string(),
        source: Box::new(inner),
    }
}

fn text(s: &str) -> TransportError {
    TransportError::Quic(s.to_string())
}

#[test]
fn error_texts() {
    let f: PoolFile = golden::load_json("transport/pool_scripts.json");
    let dial_ip = || dial("ip:127.0.0.1:9", text("x"));
    let joined_no_address = || {
        TransportError::Joined(vec![
            dial_ip(),
            TransportError::Discovery(Box::new(TransportError::NoAddress)),
        ])
    };
    let mut got: HashMap<&str, String> = HashMap::new();
    got.insert("err_closed", TransportError::Closed.to_string());
    got.insert("invalid_key", TransportError::InvalidKey.to_string());
    got.insert("no_address", TransportError::NoAddress.to_string());
    got.insert("dial_ip", dial_ip().to_string());
    got.insert("dial_ipv6", dial("ip:[::1]:9", text("y")).to_string());
    got.insert("dial_custom", dial("1_abcd", text("x")).to_string());
    got.insert(
        "dial_relay",
        dial(
            "relay:https://use1-1.relay.n0.iroh-canary.iroh.link./",
            text("x"),
        )
        .to_string(),
    );
    got.insert(
        "joined_dial_discovery",
        TransportError::Joined(vec![
            dial_ip(),
            TransportError::Discovery(Box::new(text("y"))),
        ])
        .to_string(),
    );
    got.insert(
        "joined_dial_discovery_no_address",
        joined_no_address().to_string(),
    );
    got.insert(
        "joined_two_dials",
        TransportError::Joined(vec![dial_ip(), dial("ip:[::1]:9", text("y"))]).to_string(),
    );
    got.insert(
        "joined_nil_skipped",
        TransportError::Joined(vec![dial_ip()]).to_string(),
    );
    got.insert(
        "no_candidates",
        TransportError::NoCandidates("79b5562e8f".to_string()).to_string(),
    );
    // The client's wrapper text is dstore-client's; here only its layout around the joined error.
    got.insert(
        "bootstrap_wrapper",
        format!(
            "client: no bootstrap node answered: {}",
            joined_no_address()
        ),
    );
    assert_eq!(f.error_texts.len(), got.len());
    for e in &f.error_texts {
        match got.get(e.name.as_str()) {
            Some(g) => assert_eq!(g, &e.text, "{}", e.name),
            None => panic!("no Rust counterpart for {}", e.name),
        }
    }
}
