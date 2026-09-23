//! Shared helpers of the commands (cli.md §2.5): dial, admin, status printing, records, local stores,
//! hex, signals (`cmd/dstore/client.go:39-221`, `454-574`, `main.go:54-68`, `258-260`).
//!
//! Spec: PORTING.md §4.12, §2.3, §5.2-§5.4, §5.9, DD-7, DD-11, DD-15; port-notes/cli.md §2.3-§2.5, §3.4,
//! §3.8; view-placement.md §3.4; client-core.md §2.9, §3.8.

use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::sync::Arc;
use std::time::Duration;

use amber_store_core::key::Key;
use amber_store_core::{amberpack, packstore, refstore};
use dstore_client::{Cluster, Config};
use dstore_gocli::{CliError, Context};
use dstore_gocompat::ctx::Ctx;
use dstore_gocompat::errno::PathError;
use dstore_gocompat::fmt::hex_lower;
use dstore_gocompat::os::mkdir_all;
use dstore_gocompat::path::{join, to_path};
use dstore_gocompat::quote::quote;
use dstore_gocompat::slog::{Logger, TextHandler, log_level};
use dstore_gocompat::time::SystemZone;
use dstore_ticket::Ticket;
use dstore_transport::Endpoint;
use dstore_transport_iroh::{
    IrohConfig, IrohEndpoint, bind_iroh, generate_secret_key, relay_mode_of,
};
use dstore_view::{
    Node, VOTER_SYNC_PENDING, View, id_string, node_short_id, status_cluster_prefix,
};
use dstore_wire::{AdminReply, AdminRequest, decode_admin_reply, decode_status};
use futures::StreamExt;

/// `dialClusterLog` without a ticket source.
pub const NO_CLUSTER: &str = "no cluster: set --ticket or $DSTORE_TICKET";

/// `client.Config.GCInterval` as `dialTicket` sets it.
const CLI_GC_INTERVAL: Duration = Duration::from_secs(4 * 60 * 60);

/// DD-11: the endpoint close awaited at CLI exit.
const SESSION_CLOSE_TIMEOUT: Duration = Duration::from_secs(3);

/// printStatus: the timeout of each node's status call (`client.go:151`).
const STATUS_TIMEOUT: Duration = Duration::from_secs(5);

/// The Go runtime's first panic line for a nil pointer dereference.
const NIL_DEREFERENCE: &str = "runtime error: invalid memory address or nil pointer dereference";

/// `--relay`, `--no-relay`, `--no-discovery`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetOpts {
    pub relay: String,
    pub no_relay: bool,
    pub no_discovery: bool,
}

/// `netOptsOf`.
pub fn net_opts_of(c: &Context) -> NetOpts {
    NetOpts {
        relay: c.string("relay"),
        no_relay: c.bool("no-relay"),
        no_discovery: c.bool("no-discovery"),
    }
}

/// `logger(c)`: a `TextHandler` on stderr at `logLevel(c)`, built per call as Go does.
pub fn logger(c: &Context) -> Logger {
    let level = log_level(&c.string("log-level"));
    Logger::new(Arc::new(TextHandler::new(
        Box::new(std::io::stderr()),
        level,
        Arc::new(SystemZone),
    )))
}

/// `signalCtx`: SIGINT+SIGTERM → cancel; the handlers stay installed.
///
/// `signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)`: both signals are
/// registered before this returns, the first one cancels the ctx, and later ones are swallowed until the
/// process exits (tokio never restores the default disposition). Outside a tokio runtime nothing is
/// registered and the ctx only ends when cancelled.
pub fn signal_ctx() -> Ctx {
    use tokio::signal::unix::{SignalKind, signal};

    let ctx = Ctx::background().with_cancel();
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return ctx;
    };
    let mut int = signal(SignalKind::interrupt()).ok();
    let mut term = signal(SignalKind::terminate()).ok();
    let cancel = ctx.clone();
    handle.spawn(async move {
        tokio::select! {
            () = next_signal(&mut int) => {}
            () = next_signal(&mut term) => {}
        }
        cancel.cancel();
        // Keep both registrations alive, swallowing every later signal.
        loop {
            tokio::select! {
                () = next_signal(&mut int) => {}
                () = next_signal(&mut term) => {}
            }
        }
    });
    ctx
}

/// The next delivery of a registered signal; never resolves without a registration or once the signal
/// driver has gone.
async fn next_signal(s: &mut Option<tokio::signal::unix::Signal>) {
    if let Some(sig) = s
        && sig.recv().await.is_some()
    {
        return;
    }
    std::future::pending::<()>().await
}

/// A dialled cluster and its endpoint.
pub struct Session {
    pub cluster: Cluster,
    pub endpoint: Arc<IrohEndpoint>,
}

impl Session {
    /// `cluster.close()`, then `endpoint.close_bounded(3 s)` (DD-11).
    pub async fn close(self) {
        self.cluster.close();
        self.endpoint.close_bounded(SESSION_CLOSE_TIMEOUT).await;
    }
}

/// `dialClusterLog`: the ticket from `--ticket` (`$DSTORE_TICKET`), else derived from `--store`
/// (`$DSTORE_STORE`, PORTING.md §2.2 B), else [`NO_CLUSTER`]; then [`dial_ticket`].
pub async fn dial_cluster(ctx: &Ctx, c: &Context, log: &Logger) -> Result<Session, CliError> {
    let t = ticket_of(c)?;
    dial_ticket(ctx, t, &net_opts_of(c), log).await
}

/// The ticket source of `dialClusterLog`, in Go's order. `ticket.Parse` errors are returned verbatim.
fn ticket_of(c: &Context) -> Result<Ticket, CliError> {
    let s = c.os_string("ticket");
    if !s.is_empty() {
        return dstore_ticket::parse(s.as_bytes()).map_err(CliError::msg);
    }
    let dir = c.os_string("store");
    if !dir.is_empty() {
        return crate::nodeside::local_ticket(dir.as_bytes());
    }
    Err(CliError::Msg(NO_CLUSTER.to_string()))
}

/// `dialTicket`: a fresh ephemeral identity, `relayModeOf`, `BindIroh` (no ALPNs, discovery unless
/// `--no-discovery`, no announcing), then `client.Dial` with the logger and `GCInterval` 4 h. A failed
/// dial closes the endpoint (the library close, as Go's `ep.Close()`).
pub async fn dial_ticket(
    ctx: &Ctx,
    t: Ticket,
    n: &NetOpts,
    log: &Logger,
) -> Result<Session, CliError> {
    let secret_key = generate_secret_key();
    let relay = relay_mode_of(&n.relay, n.no_relay).map_err(CliError::Msg)?;
    let endpoint = bind_iroh(
        ctx,
        IrohConfig {
            secret_key,
            alpns: Vec::new(),
            relay,
            advertise: None,
            bind_addr: None,
            loopback: false,
            direct_timeout: None,
            discover: !n.no_discovery,
            announce: false,
            logger: Some(log.clone()),
        },
    )
    .await
    .map_err(CliError::msg)?;
    let ep: Arc<dyn Endpoint> = endpoint.clone();
    let cfg = Config {
        endpoint: Some(ep),
        ticket: t,
        logger: Some(log.clone()),
        gc_interval: CLI_GC_INTERVAL,
        ..Config::default()
    };
    match Cluster::dial(ctx, cfg).await {
        Ok(cluster) => Ok(Session { cluster, endpoint }),
        Err(e) => {
            Endpoint::close(&*endpoint).await;
            Err(CliError::msg(e))
        }
    }
}

/// `admin`: `cl.Admin(ctx, view.NodeID{}, req)` (any node, ranked by the client), then the reply
/// decoded as `node.AdminReply` (unknown keys ignored).
pub async fn admin(ctx: &Ctx, cl: &Cluster, req: &AdminRequest) -> Result<AdminReply, CliError> {
    let b = cl.admin(ctx, None, req).await.map_err(CliError::msg)?;
    decode_admin_reply(&b).map_err(CliError::msg)
}

/// `adminAction`: `signalCtx`, dial, [`admin`], then print `Text`, each of `Names`, and `key %x` when
/// `Key` has 32 bytes. The session is closed after printing, or after an admin error.
pub async fn admin_action(c: &Context, req: AdminRequest) -> Result<(), CliError> {
    let ctx = signal_ctx();
    let session = dial_cluster(&ctx, c, &logger(c)).await?;
    let result = admin(&ctx, &session.cluster, &req).await;
    if let Ok(r) = &result {
        print_admin_reply(r);
    }
    session.close().await;
    result.map(|_| ())
}

/// `adminAction`'s prints on stdout.
fn print_admin_reply(r: &AdminReply) {
    let mut out = std::io::stdout().lock();
    write_admin_reply(&mut out, r);
}

/// `adminAction`'s print block: `fmt.Println(r.Text)` when non-empty, `fmt.Println(n)` per name, and
/// `fmt.Printf("key %x\n", r.Key)` for a 32-byte key. Write errors are ignored, as `fmt` ignores them.
fn write_admin_reply(out: &mut dyn Write, r: &AdminReply) {
    if !r.text.is_empty() {
        write_text(out, &format!("{}\n", r.text));
    }
    for n in &r.names {
        write_text(out, &format!("{n}\n"));
    }
    if r.key.len() == 32 {
        write_text(out, &format!("key {}\n", hex_lower(&r.key)));
    }
}

/// One Go `fmt.Print*` call: written and flushed at once (Go's stdout is unbuffered).
fn write_text(out: &mut dyn Write, s: &str) {
    let _ = out.write_all(s.as_bytes());
    let _ = out.flush();
}

/// `printStatus` (`client.go:134-193`) on `out`, byte for byte.
///
/// The view is `cl.View()`. A cluster id shorter than 4 bytes ends the process as Go's slice-bounds panic
/// does (DD-7, with the length as the capacity, DD-15). Then each node of the view, in order: its line,
/// and its `Status` with a 5 s timeout: `<line> — unreachable: <err>` on an error, `<line> — bad status`
/// when it does not decode, else the line and its status lines.
pub async fn print_status(
    ctx: &Ctx,
    cl: &Cluster,
    out: &mut (dyn std::io::Write + Send),
) -> Result<(), CliError> {
    let Some(v) = cl.view() else {
        // Unreachable after a successful dial; Go would dereference the nil view.
        close_before_panic(cl, out).await;
        crate::go_panic_exit(NIL_DEREFERENCE);
    };
    let header = match status_header(&v) {
        Ok(h) => h,
        Err(panic) => {
            close_before_panic(cl, out).await;
            crate::go_panic_exit(&panic);
        }
    };
    write_text(out, &header);
    for nd in v.nodes() {
        let line = node_line(&v, nd);
        let sctx = ctx.with_timeout(STATUS_TIMEOUT);
        let res = cl.status(&sctx, nd.nid()).await;
        sctx.cancel();
        let text = match res {
            Err(e) => node_unreachable_text(&line, &e),
            Ok(b) => node_status_text(&line, &b),
        };
        write_text(out, &text);
    }
    Ok(())
}

/// What Go does between a runtime panic in `printStatus` and the panic line: the `cluster status` action's
/// deferred `cl.Close()` runs while the panic unwinds, so the nodes see CONNECTION_CLOSE. Here the pool is
/// closed and the endpoint close awaited for 3 s (DD-11); `out` is flushed first (nothing is written to it
/// before the panic).
async fn close_before_panic(cl: &Cluster, out: &mut (dyn Write + Send)) {
    let _ = out.flush();
    cl.close();
    let _ = tokio::time::timeout(SESSION_CLOSE_TIMEOUT, cl.endpoint().close()).await;
}

/// printStatus's view lines (view-placement §3.4): the cluster line, the replicas line, and the
/// transition and voter-change lines when they apply. `Err` is the Go runtime panic text of
/// `v.ClusterID[:4]` for a cluster id shorter than 4 bytes; nothing is printed before it.
///
/// DD-15: the capacity Go slices up to is taken to be the length (a definite-length byte string).
fn status_header(v: &View) -> Result<String, String> {
    let cap = v.cluster_id.as_ref().map_or(0, Vec::len);
    let prefix = status_cluster_prefix(v.cluster_id.as_deref(), cap)?;
    let voters = v.voters.as_ref().map_or(0, Vec::len);
    let mut s = format!(
        "cluster {prefix} incarnation {} epoch {} version {}\n",
        v.incarnation, v.epoch, v.version
    );
    s.push_str(&format!(
        "replicas {} min_replicas {} nodes {} voters {voters}",
        v.replicas,
        v.min_replicas,
        v.nodes().len()
    ));
    if voters < 3 {
        s.push_str(" (no catalog fault tolerance)");
    }
    s.push('\n');
    if let Some(p) = &v.pending {
        s.push_str(&format!(
            "transition {} ({}): frozen={} acked={} participants={} done={}\n",
            p.id,
            p.reason,
            p.frozen,
            p.participants_ack.len(),
            p.participants.len(),
            p.done.len()
        ));
    }
    if v.voter_sync == VOTER_SYNC_PENDING {
        s.push_str(&format!(
            "voter change in progress (target {})\n",
            node_short_id(&v.voter_sync_target)
        ));
    }
    Ok(s)
}

/// `fmt.Sprintf("  %s weight %d zone %q voter=%v writable=%v", view.IDString(id), nd.Weight, nd.Zone,
/// v.IsVoter(id), nd.Writable)` with `id = nd.NID()`.
fn node_line(v: &View, nd: &Node) -> String {
    let id = nd.nid();
    format!(
        "  {} weight {} zone {} voter={} writable={}",
        id_string(&id),
        nd.weight,
        quote(nd.zone.as_bytes()),
        v.is_voter(&id),
        nd.writable
    )
}

/// `fmt.Println(line, "— unreachable:", err)`.
fn node_unreachable_text(line: &str, err: &dyn std::fmt::Display) -> String {
    format!("{line} \u{2014} unreachable: {err}\n")
}

/// A node's output for its status reply `b`: `<line> — bad status` when `node.DecodeStatus` fails, else
/// the line, the `epoch …` line with its tags, and the `cannot reach`, `gc`, `transition` and `voter`
/// lines that apply.
fn node_status_text(line: &str, b: &[u8]) -> String {
    let Ok(st) = decode_status(b) else {
        return format!("{line} \u{2014} bad status\n");
    };
    let mut s = format!("{line}\n");
    s.push_str(&format!(
        "      epoch {} packs {} records {} bytes {} pins {} pending-packs {} free {} GiB",
        st.epoch,
        st.packs,
        st.records,
        st.bytes,
        st.pins,
        st.pending_packs,
        st.free_bytes >> 30
    ));
    if st.is_holder {
        s.push_str(" [lease holder]");
    }
    if st.amnesiac {
        s.push_str(" [AMNESIAC]");
    }
    if st.retired {
        s.push_str(" [retired]");
    }
    s.push('\n');
    if !st.unreachable.is_empty() {
        let ids: Vec<String> = st.unreachable.iter().map(|u| node_short_id(u)).collect();
        s.push_str(&format!("      cannot reach: {}\n", ids.join(" ")));
    }
    if !st.gc.is_empty() {
        s.push_str(&format!("      gc: {}\n", st.gc));
    }
    if !st.transition.is_empty() && st.transition != "idle" {
        s.push_str(&format!("      transition: {}\n", st.transition));
    }
    for vs in &st.voters {
        s.push_str(&format!(
            "      voter {}: {} calls, {} failures, p99 {} ms\n",
            node_short_id(vs.id.as_deref().unwrap_or_default()),
            vs.calls,
            vs.failures,
            vs.p99ms
        ));
    }
    s
}

/// `recordPayload`: `amberpack.ParseRecord(rec)`, then `DecodePayload(r.Flags, r.Ulen,
/// rec[RecHeaderSize:])`.
pub fn record_payload(rec: &[u8]) -> Result<Vec<u8>, amberpack::Error> {
    let r = amberpack::parse_record(rec)?;
    let stored = rec.get(amberpack::REC_HEADER_SIZE..).unwrap_or_default();
    amberpack::decode_payload(r.flags, r.ulen, stored)
}

/// PORTING.md §2.3.
///
/// `openLocal` (`client.go:209-221`): `<local>/packstore` opened with sync, then the refs store at
/// `<local>/refs`, closing the packstore when that fails. Since core v0.0.10 the references are in
/// `<local>/refs/refs.sqlite`, one file that Go and Rust share. A `<local>/refs` that still holds the
/// Pebble database of Go dstore v0.1.10 or earlier is refused (DD-2): Go imports it on its first open,
/// core-rs cannot, and it decides what such a directory is (`refstore::Error::PebbleStore`). A `refs.redb`
/// of an earlier dstore-client-rs is imported by core-rs.
pub fn open_local(c: &Context) -> Result<(Arc<packstore::Store>, refstore::Store), CliError> {
    let local = c.os_string("local");
    let dir = local.as_bytes();
    let st = open_packstore(&join(&[dir, b"packstore"]))?;
    let refs_dir = join(&[dir, b"refs"]);
    // Go `refstore.Open`: MkdirAll(dir, 0o755), wrapped as `refstore: creating <dir>: …` (core-rs-gaps
    // G5, G6: core-rs creates with 0777 and renders Rust's errno text).
    if let Err(e) = mkdir_all(&refs_dir, 0o755) {
        drop(st);
        return Err(CliError::Msg(format!(
            "refstore: creating {}: {e}",
            String::from_utf8_lossy(&refs_dir)
        )));
    }
    match refstore::Store::open(to_path(&refs_dir), true) {
        Ok(refs) => Ok((Arc::new(st), refs)),
        Err(refstore::Error::PebbleStore { .. }) => {
            drop(st);
            Err(CliError::Msg(pebble_refs_text(&refs_dir)))
        }
        Err(e) => {
            drop(st);
            Err(CliError::msg(e))
        }
    }
}

/// The DD-2 text of PORTING.md §2.3 for a `<local>/refs` that holds a Pebble database and no `refs.sqlite`.
fn pebble_refs_text(refs_dir: &[u8]) -> String {
    format!(
        "refstore: {} holds a Pebble database written by Go dstore v0.1.10 or earlier; dstore-client-rs cannot import it: open the --local directory once with Go dstore v0.1.11 or later, which does",
        String::from_utf8_lossy(refs_dir)
    )
}

/// Go `packstore.Open(dir, WithSync(true))` with Go's texts (core-rs-gaps G3, G5, G6): `MkdirAll(dir,
/// 0o755)` wrapped as `packstore: creating <dir>: …`, then the raw `os.Open(dir)` error, then core-rs
/// (the shared flock's `packstore: <dir> is held by an older release, which needs the store to itself: …`,
/// whose errno the top-level print rewrites; a store that a current release has open is shared).
fn open_packstore(dir: &[u8]) -> Result<packstore::Store, CliError> {
    if let Err(e) = mkdir_all(dir, 0o755) {
        return Err(CliError::Msg(format!(
            "packstore: creating {}: {e}",
            String::from_utf8_lossy(dir)
        )));
    }
    if let Err(err) = std::fs::File::open(to_path(dir)) {
        return Err(CliError::msg(PathError {
            op: "open",
            path: dir.to_vec(),
            err,
        }));
    }
    packstore::Store::open_with(to_path(dir), packstore::Options::new().sync(true))
        .map_err(CliError::msg)
}

/// `clusterGet(ctx, cl)(k)` (`client.go:454-468`): the payload of the first record `cl.Get(ctx, [k])`
/// yields, or its error; with no record, `object <64 hex> not found` when the key is missing, else
/// `no data`. Dropping the stream after the first record stops the fetcher, as Go's early return does.
pub async fn cluster_get(ctx: &Ctx, cl: &Cluster, k: Key) -> Result<Vec<u8>, dstore_client::Error> {
    let mut seq = cl.get(ctx, vec![k.0]);
    match seq.next().await {
        Some(Err(e)) => Err(e),
        Some(Ok(r)) => record_payload(&r.record).map_err(dstore_client::Error::Amberpack),
        None if !seq.missing().is_empty() => Err(dstore_client::Error::Other(format!(
            "object {} not found",
            hex_lower(&k.0)
        ))),
        None => Err(dstore_client::Error::Other("no data".to_string())),
    }
}

/// "bad hex %q"; an odd trailing nibble is dropped.
///
/// `hexDecode` (`client.go:554-574`): `len(s)/2` bytes, either case, any other character →
/// `bad hex "<s>"`.
pub fn hex_decode(s: &[u8]) -> Result<Vec<u8>, String> {
    let mut b = Vec::with_capacity(s.len() / 2);
    for pair in s.chunks_exact(2) {
        let mut v = 0u8;
        for &ch in pair {
            let d = match ch {
                b'0'..=b'9' => ch - b'0',
                b'a'..=b'f' => ch - b'a' + 10,
                b'A'..=b'F' => ch - b'A' + 10,
                _ => return Err(format!("bad hex {}", quote(s))),
            };
            v = (v << 4) | d;
        }
        b.push(v);
    }
    Ok(b)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::path::PathBuf;

    use amber_store_core::fstree;
    use dstore_client::PutObserver;
    use dstore_gocompat::errno::rewrite_os_errors;
    use dstore_gocompat::slog::{Attr, Handler, Level, Record};
    use dstore_testkit::fake::{FakeCluster, FakeClusterConfig, Injection};
    use dstore_ticket::Member;
    use dstore_transport::mem::Network;
    use dstore_view::{NodeId, Pending, Voter};

    use super::*;

    // ---- a minimal JSON reader for the golden vectors (serde is not a dependency of this crate) ----

    #[derive(Clone, Debug)]
    enum J {
        Null,
        Bool(bool),
        Num(String),
        Str(String),
        Arr(Vec<J>),
        Obj(Vec<(String, J)>),
    }

    const NULL: J = J::Null;

    impl J {
        fn get(&self, k: &str) -> &J {
            match self {
                J::Obj(m) => m.iter().find(|(n, _)| n == k).map_or(&NULL, |(_, v)| v),
                _ => &NULL,
            }
        }
        fn str(&self) -> &str {
            match self {
                J::Str(s) => s,
                other => panic!("not a JSON string: {other:?}"),
            }
        }
        fn opt_str(&self) -> Option<&str> {
            match self {
                J::Null => None,
                other => Some(other.str()),
            }
        }
        fn bool(&self) -> bool {
            match self {
                J::Bool(b) => *b,
                J::Null => false,
                other => panic!("not a JSON bool: {other:?}"),
            }
        }
        fn u64(&self) -> u64 {
            match self {
                J::Num(n) | J::Str(n) => match n.parse() {
                    Ok(v) => v,
                    Err(e) => panic!("not a u64: {n:?}: {e}"),
                },
                other => panic!("not a JSON number: {other:?}"),
            }
        }
        fn i64(&self) -> i64 {
            match self {
                J::Num(n) | J::Str(n) => match n.parse() {
                    Ok(v) => v,
                    Err(e) => panic!("not an i64: {n:?}: {e}"),
                },
                other => panic!("not a JSON number: {other:?}"),
            }
        }
        fn arr(&self) -> &[J] {
            match self {
                J::Arr(a) => a,
                J::Null => &[],
                other => panic!("not a JSON array: {other:?}"),
            }
        }
    }

    struct Parser<'a> {
        b: &'a [u8],
        i: usize,
    }

    impl Parser<'_> {
        fn peek(&self) -> u8 {
            self.b.get(self.i).copied().unwrap_or(0)
        }
        fn bump(&mut self) -> u8 {
            let c = self.peek();
            self.i += 1;
            c
        }
        fn ws(&mut self) {
            while matches!(self.peek(), b' ' | b'\t' | b'\n' | b'\r') {
                self.i += 1;
            }
        }
        fn expect(&mut self, lit: &[u8]) {
            let got = self.b.get(self.i..self.i + lit.len());
            assert_eq!(got, Some(lit), "JSON: expected {lit:?} at {}", self.i);
            self.i += lit.len();
        }
        fn value(&mut self) -> J {
            self.ws();
            match self.peek() {
                b'{' => {
                    self.i += 1;
                    let mut m = Vec::new();
                    self.ws();
                    if self.peek() == b'}' {
                        self.i += 1;
                        return J::Obj(m);
                    }
                    loop {
                        self.ws();
                        let k = self.string();
                        self.ws();
                        self.expect(b":");
                        let v = self.value();
                        m.push((k, v));
                        self.ws();
                        match self.bump() {
                            b',' => continue,
                            b'}' => return J::Obj(m),
                            c => panic!("JSON: unexpected {:?} in object at {}", c as char, self.i),
                        }
                    }
                }
                b'[' => {
                    self.i += 1;
                    let mut a = Vec::new();
                    self.ws();
                    if self.peek() == b']' {
                        self.i += 1;
                        return J::Arr(a);
                    }
                    loop {
                        a.push(self.value());
                        self.ws();
                        match self.bump() {
                            b',' => continue,
                            b']' => return J::Arr(a),
                            c => panic!("JSON: unexpected {:?} in array at {}", c as char, self.i),
                        }
                    }
                }
                b'"' => J::Str(self.string()),
                b't' => {
                    self.expect(b"true");
                    J::Bool(true)
                }
                b'f' => {
                    self.expect(b"false");
                    J::Bool(false)
                }
                b'n' => {
                    self.expect(b"null");
                    J::Null
                }
                _ => {
                    let start = self.i;
                    while matches!(self.peek(), b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                        self.i += 1;
                    }
                    assert!(self.i > start, "JSON: unexpected byte at {start}");
                    J::Num(String::from_utf8_lossy(&self.b[start..self.i]).into_owned())
                }
            }
        }
        fn hex4(&mut self) -> u32 {
            let s = String::from_utf8_lossy(&self.b[self.i..self.i + 4]).into_owned();
            self.i += 4;
            match u32::from_str_radix(&s, 16) {
                Ok(v) => v,
                Err(e) => panic!("JSON: bad \\u escape {s:?}: {e}"),
            }
        }
        fn string(&mut self) -> String {
            self.expect(b"\"");
            let mut out = Vec::new();
            loop {
                match self.bump() {
                    b'"' => break,
                    b'\\' => {
                        let c = match self.bump() {
                            b'"' => '"',
                            b'\\' => '\\',
                            b'/' => '/',
                            b'b' => '\u{8}',
                            b'f' => '\u{c}',
                            b'n' => '\n',
                            b'r' => '\r',
                            b't' => '\t',
                            b'u' => {
                                let hi = self.hex4();
                                let cp = if (0xd800..0xdc00).contains(&hi)
                                    && self.b.get(self.i..self.i + 2) == Some(b"\\u")
                                {
                                    self.i += 2;
                                    let lo = self.hex4();
                                    0x10000 + ((hi - 0xd800) << 10) + (lo - 0xdc00)
                                } else {
                                    hi
                                };
                                char::from_u32(cp).unwrap_or('\u{fffd}')
                            }
                            c => panic!("JSON: bad escape {:?}", c as char),
                        };
                        let mut buf = [0u8; 4];
                        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                    }
                    0 if self.i > self.b.len() => panic!("JSON: unterminated string"),
                    c => out.push(c),
                }
            }
            match String::from_utf8(out) {
                Ok(s) => s,
                Err(e) => panic!("JSON: invalid UTF-8 in a string: {e}"),
            }
        }
    }

    /// Loads a golden vector file; a missing file fails the test.
    fn load(rel: &str) -> J {
        let path = dstore_testkit::golden::golden_dir().join(rel);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => panic!(
                "golden vector {} is missing: {e} (regenerate with tools/vectorgen)",
                path.display()
            ),
        };
        let mut p = Parser { b: &bytes, i: 0 };
        let v = p.value();
        p.ws();
        assert_eq!(p.i, bytes.len(), "JSON: trailing data in {rel}");
        v
    }

    fn hx(s: &str) -> Vec<u8> {
        dstore_testkit::golden::hex(s)
    }

    /// A log handler that drops everything.
    struct Discard;

    impl Handler for Discard {
        fn enabled(&self, _level: Level) -> bool {
            false
        }
        fn handle(&self, _handler_attrs: &[Attr], _r: &Record) {}
    }

    /// The command-line `Context` of `dstore <args…>` with the environment `env`.
    fn context(args: &[&str], env: &[(&str, &str)]) -> Context {
        let mut argv: Vec<OsString> = vec!["dstore".into()];
        argv.extend(args.iter().map(OsString::from));
        let env: HashMap<String, OsString> = env
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        let mut sink = Vec::new();
        match dstore_gocli::dispatch(&crate::app(), argv, &mut sink, &|n| env.get(n).cloned()) {
            Ok(Some((_, c))) => c,
            Ok(None) => panic!("{args:?}: no action reached"),
            Err(e) => panic!("{args:?}: {e}"),
        }
    }

    fn msg(e: CliError) -> String {
        match e {
            CliError::Msg(m) => m,
            CliError::Exit { msg, code } => panic!("unexpected exit error {code}: {msg}"),
        }
    }

    /// A scratch directory under the system temp dir, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            use std::sync::atomic::{AtomicU32, Ordering};
            static N: AtomicU32 = AtomicU32::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "dstore-cli-common-{tag}-{}-{n}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            if let Err(e) = std::fs::create_dir_all(&dir) {
                panic!("scratch dir: {e}");
            }
            Scratch(dir)
        }
        fn path(&self) -> String {
            self.0.display().to_string()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn id32(b: u8) -> Vec<u8> {
        vec![b; 32]
    }

    // ---- hex_decode ----

    #[test]
    fn hex_decode_like_go() {
        let ok = |s: &[u8]| match hex_decode(s) {
            Ok(b) => b,
            Err(e) => panic!("{s:?}: {e}"),
        };
        assert_eq!(ok(b""), Vec::<u8>::new());
        assert_eq!(ok(b"a"), Vec::<u8>::new());
        assert_eq!(ok(b"abc"), vec![0xab]);
        assert_eq!(ok(b"0A0b"), vec![0x0a, 0x0b]);
        assert_eq!(ok(b"ffFF00"), vec![0xff, 0xff, 0x00]);
        // The dropped odd nibble is never looked at.
        assert_eq!(ok(b"abz"), vec![0xab]);
        assert_eq!(hex_decode(b"zz"), Err(r#"bad hex "zz""#.to_string()));
        assert_eq!(hex_decode(b"0g12"), Err(r#"bad hex "0g12""#.to_string()));
        assert_eq!(hex_decode(b"12 3"), Err(r#"bad hex "12 3""#.to_string()));
        assert_eq!(
            hex_decode(b"\xff\xfe"),
            Err(r#"bad hex "\xff\xfe""#.to_string())
        );
        assert_eq!(
            hex_decode("é1".as_bytes()),
            Err(r#"bad hex "é1""#.to_string())
        );
    }

    /// cli/text.json `hex_decode`: Go `hexDecode(in)` (VECTORS.md family `cli`).
    #[test]
    fn hex_decode_vectors() {
        let file = load("cli/text.json");
        let cases = file.get("hex_decode").arr();
        assert!(cases.len() >= 16, "only {} hex_decode cases", cases.len());
        for c in cases {
            let input = c.get("in").str();
            let want = if c.get("ok").bool() {
                Ok(hx(c.get("out").str()))
            } else {
                Err(c.get("error").str().to_string())
            };
            assert_eq!(hex_decode(input.as_bytes()), want, "{input:?}");
        }
    }

    // ---- record_payload ----

    #[test]
    fn record_payload_of_raw_records() {
        let data = b"hello, dstore\n".to_vec();
        let obj = fstree::encode_blob(&data);
        let rec = match amberpack::encode_record(obj.key, &obj.bytes) {
            Ok(r) => r,
            Err(e) => panic!("encode record: {e}"),
        };
        assert_eq!(record_payload(&rec).ok(), Some(obj.bytes.clone()));
        let e = match record_payload(&rec[..10]) {
            Ok(_) => panic!("truncated record decoded"),
            Err(e) => e.to_string(),
        };
        assert_eq!(e, "amberpack: corrupt pack data: truncated record header");
        let mut bad = rec.clone();
        bad[0] = 0x02;
        let e = match record_payload(&bad) {
            Ok(_) => panic!("bad tag decoded"),
            Err(e) => e.to_string(),
        };
        assert_eq!(e, "amberpack: corrupt pack data: unexpected record tag 0x2");
    }

    // ---- admin replies ----

    fn printed_reply(r: &AdminReply) -> String {
        let mut out = Vec::new();
        write_admin_reply(&mut out, r);
        match String::from_utf8(out) {
            Ok(s) => s,
            Err(e) => panic!("output is not UTF-8: {e}"),
        }
    }

    #[test]
    fn admin_replies_print_as_go() {
        let file = load("admin/replies.json");
        let mut n = 0;
        for c in file
            .get("cases")
            .arr()
            .iter()
            .chain(file.get("decode").arr())
        {
            let name = c.get("name").str();
            let Some(printed) = c.get("printed").opt_str() else {
                continue;
            };
            let r = match decode_admin_reply(&hx(c.get("status_hex").str())) {
                Ok(r) => r,
                Err(e) => panic!("{name}: decode: {e}"),
            };
            assert_eq!(printed_reply(&r), printed, "{name}");
            n += 1;
        }
        assert!(n >= 20, "only {n} printed replies checked");
    }

    #[test]
    fn admin_reply_key_needs_32_bytes() {
        let mut r = AdminReply {
            text: "backup written".into(),
            key: vec![0x5a; 32],
            ..AdminReply::default()
        };
        assert_eq!(
            printed_reply(&r),
            format!("backup written\nkey {}\n", "5a".repeat(32))
        );
        r.key.pop();
        assert_eq!(printed_reply(&r), "backup written\n");
        r.key = vec![0x5a; 33];
        assert_eq!(printed_reply(&r), "backup written\n");
        assert_eq!(printed_reply(&AdminReply::default()), "");
    }

    // ---- status formatting ----

    /// The node of a status vector and a view holding it (a voter when `voter` is set).
    fn vector_node(j: &J) -> (View, Node) {
        let id = hx(j.get("id").str());
        let nd = Node {
            id: Some(id.clone()),
            weight: j.get("weight").u64() as u32,
            zone: j.get("zone").str().to_string(),
            writable: j.get("writable").bool(),
            ..Node::default()
        };
        let voters = if j.get("voter").bool() {
            vec![Voter {
                id: Some(id),
                since: 1,
            }]
        } else {
            Vec::new()
        };
        let v = View {
            voters: Some(voters),
            nodes: Some(vec![nd.clone()]),
            ..View::default()
        };
        (v, nd)
    }

    #[test]
    fn status_vectors_print_as_go() {
        let file = load("status/status.json");
        let mut n = 0;
        for c in file
            .get("cases")
            .arr()
            .iter()
            .chain(file.get("decode").arr())
        {
            let name = c.get("name").str();
            let (v, nd) = vector_node(c.get("node"));
            let line = node_line(&v, &nd);
            assert_eq!(line, c.get("node").get("line").str(), "{name}: node line");
            let got = node_status_text(&line, &hx(c.get("hex").str()));
            assert_eq!(got, c.get("printed").str(), "{name}");
            n += 1;
        }
        for c in file.get("unreachable").arr() {
            let name = c.get("name").str();
            let (v, nd) = vector_node(c.get("node"));
            let line = node_line(&v, &nd);
            assert_eq!(line, c.get("node").get("line").str(), "{name}: node line");
            let got = node_unreachable_text(&line, &c.get("error").str());
            assert_eq!(got, c.get("printed").str(), "{name}");
            n += 1;
        }
        assert_eq!(n, 18, "status vector cases");
    }

    #[test]
    fn node_lines_quote_zones_and_match_voters_exactly() {
        let nd = |id: Vec<u8>, zone: &str| Node {
            id: Some(id),
            weight: 7,
            zone: zone.to_string(),
            writable: false,
            ..Node::default()
        };
        let v = View {
            voters: Some(vec![Voter {
                id: Some(id32(0xab)),
                since: 1,
            }]),
            ..View::default()
        };
        let ab = "ab".repeat(32);
        assert_eq!(
            node_line(&v, &nd(id32(0xab), "a\"b")),
            format!("  {ab} weight 7 zone \"a\\\"b\" voter=true writable=false")
        );
        assert_eq!(
            node_line(&v, &nd(id32(0xab), "\u{ad}")),
            format!("  {ab} weight 7 zone \"\\u00ad\" voter=true writable=false")
        );
        // A short id prints zero-padded (NID) and is not the 32-byte voter.
        let mut short = id32(0xab);
        short.truncate(31);
        assert_eq!(
            node_line(&v, &nd(short, "")),
            format!(
                "  {}00 weight 7 zone \"\" voter=false writable=false",
                "ab".repeat(31)
            )
        );
    }

    fn header(v: &View) -> String {
        match status_header(v) {
            Ok(h) => h,
            Err(p) => panic!("unexpected panic path: {p}"),
        }
    }

    #[test]
    fn status_header_lines() {
        let voters = |n: u8| -> Option<Vec<Voter>> {
            Some(
                (1..=n)
                    .map(|i| Voter {
                        id: Some(id32(i)),
                        since: 1,
                    })
                    .collect(),
            )
        };
        let nodes = |n: u8| -> Option<Vec<Node>> {
            Some(
                (1..=n)
                    .map(|i| Node {
                        id: Some(id32(i)),
                        ..Node::default()
                    })
                    .collect(),
            )
        };
        let mut v = View {
            cluster_id: Some(vec![0xde, 0xad, 0xbe, 0xef, 0x01, 0x02]),
            incarnation: 2,
            epoch: 9,
            version: 17,
            replicas: 3,
            min_replicas: 2,
            voters: voters(3),
            nodes: nodes(4),
            ..View::default()
        };
        assert_eq!(
            header(&v),
            "cluster deadbeef incarnation 2 epoch 9 version 17\nreplicas 3 min_replicas 2 nodes 4 voters 3\n"
        );
        v.voters = voters(2);
        v.nodes = None;
        assert_eq!(
            header(&v),
            "cluster deadbeef incarnation 2 epoch 9 version 17\nreplicas 3 min_replicas 2 nodes 0 voters 2 (no catalog fault tolerance)\n"
        );
        v.voters = None;
        v.pending = Some(Box::new(Pending {
            id: 4,
            reason: "replicas 2".into(),
            frozen: true,
            participants_ack: vec![Some(id32(1)), None],
            participants: vec![Some(id32(1)), Some(id32(2)), Some(id32(3))],
            done: vec![Some(id32(1))],
            ..Pending::default()
        }));
        v.voter_sync = VOTER_SYNC_PENDING;
        v.voter_sync_target = id32(0x12);
        assert_eq!(
            header(&v),
            "cluster deadbeef incarnation 2 epoch 9 version 17\nreplicas 3 min_replicas 2 nodes 0 voters 0 (no catalog fault tolerance)\ntransition 4 (replicas 2): frozen=true acked=2 participants=3 done=1\nvoter change in progress (target 12121212)\n"
        );
        v.pending = Some(Box::default());
        v.voter_sync_target = vec![0x12; 4];
        assert!(header(&v).ends_with(
            "transition 0 (): frozen=false acked=0 participants=0 done=0\nvoter change in progress (target ?)\n"
        ));
        // Any other VoterSync prints no voter-change line.
        v.voter_sync = 2;
        assert!(!header(&v).contains("voter change"));
    }

    /// view_placement.json `status_cluster_prefix`: Go for every definite-length encoding, DD-15 for the
    /// indefinite-length ones.
    #[test]
    fn status_header_cluster_prefix_vectors() {
        let file = load("view/view_placement.json");
        let cases = file.get("status_cluster_prefix").arr();
        assert!(cases.len() >= 13, "status_cluster_prefix cases");
        let mut dd15 = 0;
        for c in cases {
            let name = c.get("name").str();
            let v = match View::decode(&hx(c.get("view").str())) {
                Ok(v) => v,
                Err(e) => panic!("{name}: view decode: {e}"),
            };
            let id = c.get("cluster_id").opt_str().map(hx);
            assert_eq!(v.cluster_id, id, "{name}: cluster id");
            let len = id.as_ref().map_or(0, Vec::len);
            let got = status_header(&v);
            if c.get("cap").u64() == len as u64 {
                match (c.get("panic").opt_str(), c.get("prefix").opt_str(), &got) {
                    (Some(p), None, Err(e)) => assert_eq!(e, p, "{name}"),
                    (None, Some(prefix), Ok(h)) => {
                        assert!(h.starts_with(&format!("cluster {prefix} ")), "{name}: {h}")
                    }
                    other => panic!("{name}: {other:?}"),
                }
                continue;
            }
            // DD-15: Rust slices up to the length.
            dd15 += 1;
            if len < 4 {
                assert_eq!(
                    got,
                    Err(format!(
                        "runtime error: slice bounds out of range [:4] with capacity {len}"
                    )),
                    "{name}"
                );
            } else {
                let prefix = hex_lower(&id.unwrap_or_default()[..4]);
                assert!(
                    got.as_ref()
                        .is_ok_and(|h| h.starts_with(&format!("cluster {prefix} "))),
                    "{name}: {got:?}"
                );
            }
        }
        assert_eq!(dd15, 3, "indefinite-length cases that differ in capacity");
    }

    // ---- over a fake cluster ----

    fn client_config(net: &Arc<Network>, fc: &FakeCluster) -> Config {
        let endpoint: Arc<dyn Endpoint> = net.bind(NodeId([0xc1; 32]), &[]);
        Config {
            endpoint: Some(endpoint),
            ticket: fc.ticket(),
            logger: Some(Logger::new(Arc::new(Discard))),
            gc_interval: CLI_GC_INTERVAL,
            ..Config::default()
        }
    }

    async fn dial(net: &Arc<Network>, fc: &FakeCluster) -> Cluster {
        match Cluster::dial(&Ctx::background(), client_config(net, fc)).await {
            Ok(c) => c,
            Err(e) => panic!("dial: {e}"),
        }
    }

    fn hex_id(id: &NodeId) -> String {
        hex_lower(&id.0)
    }

    /// The whole `cluster status` output over three fake nodes: one answers, one errors, one stays silent
    /// past the 5 s timeout.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn print_status_over_a_fake_cluster() {
        let net = Network::new();
        let fc = FakeCluster::start(
            &net,
            FakeClusterConfig {
                weights: vec![100, 50, 7],
                zones: vec![String::new(), "rack-1".into(), "a\"b".into()],
                ..FakeClusterConfig::default()
            },
        )
        .await;
        let ids = fc.ids();
        fc.inject(
            ids[1],
            dstore_wire::T_STATUS,
            1,
            Injection::Delay(Duration::from_secs(7)),
        );
        fc.inject(
            ids[2],
            dstore_wire::T_STATUS,
            1,
            Injection::Err {
                code: "unavailable".into(),
                text: "no view".into(),
                with_view: false,
                retry_after_ms: 0,
            },
        );
        let cl = dial(&net, &fc).await;
        let mut out = Vec::new();
        let started = std::time::Instant::now();
        if let Err(e) = print_status(&Ctx::background(), &cl, &mut out).await {
            panic!("print_status: {}", msg(e));
        }
        let took = started.elapsed();
        let cid = fc.view().cluster_id.unwrap_or_default();
        let want = format!(
            "cluster {} incarnation 1 epoch 1 version 1\n\
             replicas 3 min_replicas 2 nodes 3 voters 3\n  \
             {} weight 100 zone \"\" voter=true writable=true\n      \
             epoch 1 packs 0 records 0 bytes 0 pins 0 pending-packs 0 free 1024 GiB [lease holder]\n      \
             gc: epoch 0 idle\n  \
             {} weight 50 zone \"rack-1\" voter=true writable=true \u{2014} unreachable: context deadline exceeded\n  \
             {} weight 7 zone \"a\\\"b\" voter=true writable=true \u{2014} unreachable: remote: unavailable: no view\n",
            hex_lower(&cid[..4]),
            hex_id(&ids[0]),
            hex_id(&ids[1]),
            hex_id(&ids[2]),
        );
        assert_eq!(String::from_utf8_lossy(&out), want);
        assert!(
            took >= Duration::from_millis(4900) && took < Duration::from_secs(7),
            "the status timeout is 5 s, took {took:?}"
        );
        cl.close();
        fc.close().await;
    }

    /// The close that precedes a DD-7 exit of `print_status` (the exit itself cannot run in a test): nothing
    /// is written, the pool and the endpoint are closed, so a later call fails, and it ends well within the
    /// 3 s bound.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn close_before_panic_closes_the_cluster() {
        let net = Network::new();
        let fc = FakeCluster::start(&net, FakeClusterConfig::default()).await;
        let cl = dial(&net, &fc).await;
        let ctx = Ctx::background();
        let id = fc.ids()[0];
        if let Err(e) = cl.status(&ctx, id).await {
            panic!("status before the close: {e}");
        }
        let mut out = Vec::new();
        let started = std::time::Instant::now();
        close_before_panic(&cl, &mut out).await;
        assert!(
            started.elapsed() < SESSION_CLOSE_TIMEOUT,
            "took {:?}",
            started.elapsed()
        );
        assert!(out.is_empty(), "wrote {out:?}");
        assert!(
            cl.status(&ctx, id).await.is_err(),
            "status answered after the close"
        );
        fc.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn admin_over_a_fake_cluster() {
        let net = Network::new();
        let fc = FakeCluster::start(&net, FakeClusterConfig::default()).await;
        let cl = dial(&net, &fc).await;
        let ctx = Ctx::background();
        let req = |op: &str| AdminRequest {
            op: op.to_string(),
            ..AdminRequest::default()
        };
        let r = match admin(&ctx, &cl, &req("transition-status")).await {
            Ok(r) => r,
            Err(e) => panic!("transition-status: {}", msg(e)),
        };
        assert_eq!(printed_reply(&r), "idle\n");
        let r = match admin(&ctx, &cl, &req("catalog-backup")).await {
            Ok(r) => r,
            Err(e) => panic!("catalog-backup: {}", msg(e)),
        };
        assert_eq!(r.key.len(), 32);
        assert_eq!(
            printed_reply(&r),
            format!("backup written\nkey {}\n", hex_lower(&r.key))
        );
        let backup = hex_lower(&r.key);
        let r = match admin(&ctx, &cl, &req("catalog-backups")).await {
            Ok(r) => r,
            Err(e) => panic!("catalog-backups: {}", msg(e)),
        };
        // Names are printed one per line; no key.
        assert_eq!(printed_reply(&r), format!("{backup}\n"));
        let e = match admin(&ctx, &cl, &req("nope")).await {
            Ok(r) => panic!("unknown op answered {r:?}"),
            Err(e) => msg(e),
        };
        assert_eq!(e, "remote: bad-request: unknown admin op nope");
        cl.close();
        fc.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cluster_get_over_a_fake_cluster() {
        let net = Network::new();
        let fc = FakeCluster::start(&net, FakeClusterConfig::default()).await;
        let cl = dial(&net, &fc).await;
        let ctx = Ctx::background();
        let data = b"the payload of a blob\n".to_vec();
        let obj = fstree::encode_blob(&data);
        let rec = match amberpack::encode_record(obj.key, &obj.bytes) {
            Ok(r) => r,
            Err(e) => panic!("encode record: {e}"),
        };
        let Some(primary) = cl.primary(&obj.key.0) else {
            panic!("no primary");
        };
        let size = rec.len();
        let res = cl
            .put(
                &ctx,
                HashMap::from([(primary, vec![obj.key.0])]),
                Arc::new(move |_| Ok(rec.clone())),
                Arc::new(move |_| size),
                PutObserver::default(),
            )
            .await;
        assert!(res.errors.is_empty(), "put errors: {:?}", res.errors.keys());
        match cluster_get(&ctx, &cl, obj.key).await {
            Ok(b) => assert_eq!(b, obj.bytes),
            Err(e) => panic!("cluster_get: {e}"),
        }
        let absent = fstree::encode_blob(b"never stored").key;
        match cluster_get(&ctx, &cl, absent).await {
            Ok(b) => panic!("absent key gave {b:?}"),
            Err(e) => assert_eq!(e.to_string(), format!("object {absent} not found")),
        }
        cl.close();
        fc.close().await;
    }

    // ---- dialing, up to the bind (no sockets) ----

    #[test]
    fn net_opts_from_flags() {
        let c = context(&["refs"], &[]);
        assert_eq!(net_opts_of(&c), NetOpts::default());
        let c = context(
            &[
                "refs",
                "--relay",
                "https://relay.example",
                "--no-relay",
                "--no-discovery",
            ],
            &[],
        );
        assert_eq!(
            net_opts_of(&c),
            NetOpts {
                relay: "https://relay.example".into(),
                no_relay: true,
                no_discovery: true,
            }
        );
        let c = context(&["refs"], &[("DSTORE_NO_DISCOVERY", "true")]);
        assert!(net_opts_of(&c).no_discovery);
    }

    #[test]
    fn logger_levels() {
        let lg = logger(&context(&["refs"], &[]));
        assert!(!lg.enabled(Level::DEBUG) && lg.enabled(Level::INFO));
        let lg = logger(&context(&["--log-level", "DEBUG", "refs"], &[]));
        assert!(lg.enabled(Level::DEBUG));
        let lg = logger(&context(&["refs"], &[("DSTORE_LOG_LEVEL", "error")]));
        assert!(!lg.enabled(Level::WARN) && lg.enabled(Level::ERROR));
        let lg = logger(&context(&["--log-level", "verbose", "refs"], &[]));
        assert!(!lg.enabled(Level::DEBUG) && lg.enabled(Level::INFO));
    }

    /// cli/text.json `log_level`: Go `logLevel(c)` with `--log-level value`, here through `logger(c)`: the
    /// handler takes the vector's level and nothing below it.
    #[test]
    fn log_level_vectors() {
        let file = load("cli/text.json");
        let cases = file.get("log_level").arr();
        assert!(cases.len() >= 17, "only {} log_level cases", cases.len());
        for c in cases {
            let value = c.get("value").str();
            let level = match i32::try_from(c.get("level").i64()) {
                Ok(l) => Level(l),
                Err(e) => panic!("{value:?}: level: {e}"),
            };
            let lg = logger(&context(&["--log-level", value, "refs"], &[]));
            assert!(lg.enabled(level), "{value:?}: {level:?} disabled");
            assert!(
                !lg.enabled(Level(level.0 - 1)),
                "{value:?}: below {level:?} enabled"
            );
        }
    }

    async fn dial_error(args: &[&str], env: &[(&str, &str)]) -> String {
        let c = context(args, env);
        let log = Logger::new(Arc::new(Discard));
        match dial_cluster(&Ctx::background(), &c, &log).await {
            Ok(_) => panic!("{args:?}: dialled"),
            Err(e) => msg(e),
        }
    }

    #[tokio::test]
    async fn dial_cluster_ticket_sources() {
        assert_eq!(dial_error(&["refs"], &[]).await, NO_CLUSTER);
        assert_eq!(dial_error(&["refs", "--ticket="], &[]).await, NO_CLUSTER);
        let bogus = match dstore_ticket::parse(b"bogus") {
            Ok(_) => panic!("bogus parsed"),
            Err(e) => e.to_string(),
        };
        assert_eq!(dial_error(&["refs", "--ticket", "bogus"], &[]).await, bogus);
        assert_eq!(
            dial_error(&["refs"], &[("DSTORE_TICKET", "bogus")]).await,
            bogus
        );
        // --ticket wins over --store; refs has no --store flag, so $DSTORE_STORE is not read there.
        assert_eq!(
            dial_error(
                &[
                    "cluster", "status", "--ticket", "bogus", "--store", "nostore"
                ],
                &[]
            )
            .await,
            bogus
        );
        assert_eq!(
            dial_error(&["refs"], &[("DSTORE_STORE", "nostore")]).await,
            NO_CLUSTER
        );
        let scratch = Scratch::new("store");
        let nostore = format!("{}/nostore", scratch.path());
        let want = format!(
            "node: no identity in {nostore}: open {nostore}/identity: no such file or directory"
        );
        assert_eq!(
            dial_error(&["cluster", "status", "--store", &nostore], &[]).await,
            want
        );
        assert_eq!(
            dial_error(&["cluster", "status"], &[("DSTORE_STORE", &nostore)]).await,
            want
        );
    }

    /// The relay mode is checked after the ticket and before anything is bound.
    #[tokio::test]
    async fn dial_ticket_relay_errors_come_before_the_bind() {
        let id = generate_secret_key().public();
        let t = Ticket {
            cluster_id: None,
            incarnation: 0,
            members: Some(vec![Member {
                id: Some(id.as_bytes().to_vec()),
                addrs: Vec::new(),
            }]),
        };
        let n = NetOpts {
            relay: ":x".into(),
            ..NetOpts::default()
        };
        let log = Logger::new(Arc::new(Discard));
        let e = match dial_ticket(&Ctx::background(), t, &n, &log).await {
            Ok(_) => panic!("dialled"),
            Err(e) => msg(e),
        };
        assert_eq!(
            e,
            r#"failed to parse relay URL: parse ":x": missing protocol scheme"#
        );
        let e = dial_error(
            &[
                "refs",
                "--ticket",
                &hex_lower(id.as_bytes()),
                "--relay",
                "http://[::1",
            ],
            &[],
        )
        .await;
        assert_eq!(
            e,
            r#"failed to parse relay URL: parse "http://[::1": missing ']' in host"#
        );
    }

    // ---- signals ----

    fn kill(sig: &str) {
        let pid = std::process::id().to_string();
        match std::process::Command::new("kill")
            .args([sig, pid.as_str()])
            .status()
        {
            Ok(st) => assert!(st.success(), "kill {sig}: {st}"),
            Err(e) => panic!("kill {sig}: {e}"),
        }
    }

    /// The first SIGTERM cancels; later SIGTERM and SIGINT are swallowed while the process lives on.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn signal_ctx_cancels_and_swallows_later_signals() {
        let ctx = signal_ctx();
        assert_eq!(ctx.err(), None);
        kill("-TERM");
        if tokio::time::timeout(Duration::from_secs(10), ctx.done())
            .await
            .is_err()
        {
            panic!("SIGTERM did not cancel the ctx");
        }
        assert_eq!(ctx.err(), Some(dstore_gocompat::ctx::CtxError::Canceled));
        kill("-TERM");
        kill("-INT");
        tokio::time::sleep(Duration::from_millis(300)).await;
        // Still alive: a second ctx works too.
        let again = signal_ctx();
        assert_eq!(again.err(), None);
    }

    #[test]
    fn signal_ctx_outside_a_runtime_registers_nothing() {
        let ctx = signal_ctx();
        assert_eq!(ctx.err(), None);
        ctx.cancel();
        assert_eq!(ctx.err(), Some(dstore_gocompat::ctx::CtxError::Canceled));
    }

    // ---- local stores ----

    fn local(dir: &str) -> Context {
        context(&["store", "pull", "--local", dir, "trees/x"], &[])
    }

    #[test]
    fn open_local_creates_both_stores() {
        let s = Scratch::new("fresh");
        let d = s.path();
        match open_local(&local(&d)) {
            Ok((st, refs)) => {
                drop(refs);
                drop(st);
            }
            Err(e) => panic!("open_local: {}", msg(e)),
        }
        assert!(s.0.join("packstore").is_dir());
        assert!(s.0.join("refs").join("refs.sqlite").is_file());
        // Since core v0.0.10 any number of opens share both stores.
        let held = match open_local(&local(&d)) {
            Ok(v) => v,
            Err(e) => panic!("reopen: {}", msg(e)),
        };
        match open_local(&local(&d)) {
            Ok(again) => drop(again),
            Err(e) => panic!("a second open while the first is held: {}", msg(e)),
        }
        drop(held);
        // A release from before that holds the packstore directory exclusively: Go's text.
        let older = hold_as_an_older_release(&s.0.join("packstore"));
        let e = match open_local(&local(&d)) {
            Ok(_) => panic!("opened a store that an older release holds"),
            Err(e) => rewrite_os_errors(&msg(e)),
        };
        assert_eq!(
            e,
            format!(
                "packstore: {d}/packstore is held by an older release, which needs the store to itself: resource temporarily unavailable"
            )
        );
        drop(older);
    }

    /// The exclusive flock that a release from before core v0.0.10 holds on a store it has open. Retried
    /// for a while: a process that another test forks holds, until it execs, a copy of the shared lock
    /// that a closed store has just let go of.
    fn hold_as_an_older_release(dir: &std::path::Path) -> std::fs::File {
        use std::os::unix::io::AsRawFd;
        let f = match std::fs::File::open(dir) {
            Ok(f) => f,
            Err(e) => panic!("open {}: {e}", dir.display()),
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            // SAFETY: flock(2) on a descriptor this function owns; no memory is involved.
            if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return f;
            }
            let e = std::io::Error::last_os_error();
            assert!(
                e.raw_os_error() == Some(libc::EWOULDBLOCK) && std::time::Instant::now() < deadline,
                "flock: {e}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn open_local_refuses_pebble_refs_and_releases_the_packstore() {
        let s = Scratch::new("pebble");
        let d = s.path();
        let refs = s.0.join("refs");
        if let Err(e) = std::fs::create_dir_all(&refs) {
            panic!("mkdir refs: {e}");
        }
        // core-rs knows a Pebble store by the marker that names its manifest.
        for f in [
            "000002.log",
            "LOCK",
            "MANIFEST-000001",
            "marker.manifest.000001.MANIFEST-000001",
        ] {
            if let Err(e) = std::fs::write(refs.join(f), b"") {
                panic!("write {f}: {e}");
            }
        }
        let e = match open_local(&local(&d)) {
            Ok(_) => panic!("opened a Pebble refs dir"),
            Err(e) => msg(e),
        };
        assert_eq!(
            e,
            format!(
                "refstore: {d}/refs holds a Pebble database written by Go dstore v0.1.10 or earlier; dstore-client-rs cannot import it: open the --local directory once with Go dstore v0.1.11 or later, which does"
            )
        );
        assert!(
            !refs.join("refs.sqlite").exists(),
            "a database was created next to the Pebble store"
        );
        // The packstore was closed.
        if let Err(e) = std::fs::remove_dir_all(&refs) {
            panic!("remove refs: {e}");
        }
        if let Err(e) = open_local(&local(&d)) {
            panic!("open after refusal: {}", msg(e));
        }
    }

    /// What Go's import leaves behind opens: `refs.sqlite` beside the poison marker that keeps Pebble-based
    /// releases out, the retired files, and the `LOCK` such a release may have left since.
    #[test]
    fn open_local_opens_refs_that_go_imported() {
        let s = Scratch::new("imported");
        let d = s.path();
        let refs = s.0.join("refs");
        if let Err(e) = std::fs::create_dir_all(refs.join("pebble-migrated")) {
            panic!("mkdir refs: {e}");
        }
        for f in ["refs.sqlite", "marker.format-version.999999.999", "LOCK"] {
            if let Err(e) = std::fs::write(refs.join(f), b"") {
                panic!("write {f}: {e}");
            }
        }
        if let Err(e) = open_local(&local(&d)) {
            panic!("open_local: {}", msg(e));
        }
    }

    #[test]
    fn open_local_refs_errors_have_go_texts() {
        let s = Scratch::new("refs-errors");
        let d = s.path();
        if let Err(e) = std::fs::write(s.0.join("refs"), b"") {
            panic!("write refs file: {e}");
        }
        let e = match open_local(&local(&d)) {
            Ok(_) => panic!("opened a file as refs"),
            Err(e) => msg(e),
        };
        assert_eq!(
            e,
            format!("refstore: creating {d}/refs: mkdir {d}/refs: not a directory")
        );
    }

    #[test]
    fn open_local_packstore_errors_have_go_texts() {
        let s = Scratch::new("errors");
        let d = s.path();
        if let Err(e) = std::fs::write(s.0.join("packstore"), b"") {
            panic!("write packstore file: {e}");
        }
        let e = match open_local(&local(&d)) {
            Ok(_) => panic!("opened a file as packstore"),
            Err(e) => msg(e),
        };
        assert_eq!(
            e,
            format!("packstore: creating {d}/packstore: mkdir {d}/packstore: not a directory")
        );
        // The directory is cleaned as filepath.Join does.
        let e = match open_local(&local(&format!("{d}/./"))) {
            Ok(_) => panic!("opened a file as packstore"),
            Err(e) => msg(e),
        };
        assert_eq!(
            e,
            format!("packstore: creating {d}/packstore: mkdir {d}/packstore: not a directory")
        );
    }
}
