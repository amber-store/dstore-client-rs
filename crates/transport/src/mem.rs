//! `transport/mem.go`: an in-process network for tests, with down nodes, partitions and a dial delay.
//!
//! Semantics are Go's (port-notes/transport.md §2.9, §4.9):
//! - **Dial** checks, in order: the dialing endpoint is closed (`transport: closed`); the local node is
//!   down (`mem: local endpoint is down`); the peer is down or cut off (`mem: <short> unreachable`); the
//!   peer is missing or closed (`mem: <short> not bound`); the peer lacks the ALPN
//!   (`mem: <short> does not speak <alpn>`). Then it waits for the network delay (ctx-aware), tracks the new
//!   connection pair on both endpoints and queues the peer half for `accept` (64 slots). A ctx end while
//!   that queue is full closes the local half.
//! - **Accept** ignores the endpoint's closed flag, as Go does: queued connections are still returned.
//! - **Connections** queue up to 256 unaccepted streams; `open_stream` blocks while the peer's queue is
//!   full. `close` runs once, untracks the connection and closes the peer half from a spawned task (Go
//!   `go c.peer.Close()`; inline when no tokio runtime is running).
//! - **Streams** are two pipes of at most [`PIPE_LIMIT`] buffered bytes. A write blocks while the pipe is
//!   full; `finish` makes the peer read EOF after the buffered bytes and later writes fail with
//!   `io: read/write on closed pipe`; `cancel_read` discards the buffer, the local read fails with
//!   `mem: read canceled` and the peer's writes with `mem: stream reset by peer`.
//! - **Quirk kept** (PORTING.md C30): closing a connection does not touch its streams' pipes. A blocked read
//!   survives the close, and bytes written to the stream afterwards are still delivered.
//!
//! Rust-side decisions:
//! - **Drop.** A stream half handed to a caller does on drop what noq does: an unfinished send half
//!   finishes (FIN), and a receive half that has not reached EOF cancels its read. Go code always makes
//!   these calls explicitly (`wire.CloseStream`, the watch `abandon`), while the Rust port relies on the drop
//!   where it aborts a task (PORTING.md §5.1). Streams that were never handed out (queued but never
//!   accepted, or lost to a ctx end in `open_stream`) have no drop effect, so their peers block as in Go.
//! - **Ownership** follows Go's reachability. The [`Network`] holds the registration of every bound endpoint
//!   (id, ALPNs, accept queue, tracked connections, closed flag) until `bind` replaces it, and every
//!   [`MemEndpoint`] handle holds its network. So an endpoint keeps its network alive
//!   (`Network::new().bind(..)` dials as in Go), and an endpoint whose handle was dropped stays bound. There
//!   is no reference cycle: a registration does not refer to the network, and connection halves refer to
//!   their registration weakly.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    Conn, Ctx, Endpoint, NodeId, PathInfo, RecvStream, SendStream, Stream, TransportError,
};

/// Per-direction pipe buffer limit (Go `pipeLimit`).
pub const PIPE_LIMIT: usize = 4 << 20;

/// Capacity of an endpoint's accept queue (`make(chan Conn, 64)`).
const ACCEPT_QUEUE: usize = 64;
/// Capacity of a connection's queue of unaccepted streams (`make(chan Stream, 256)`).
const STREAM_QUEUE: usize = 256;
/// `memConn.Path()` RTT.
const MEM_RTT: Duration = Duration::from_millis(1);

/// Tracking keys of connection halves, unique within the process.
static NEXT_SERIAL: AtomicU64 = AtomicU64::new(0);

/// Locks a std mutex, recovering from poisoning: every state change here is a single step.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `transport.Network`: endpoints registered by id, down nodes, cut links and the dial delay.
pub struct Network {
    state: Mutex<NetState>,
}

struct NetState {
    endpoints: BTreeMap<NodeId, Arc<Registration>>,
    down: HashSet<NodeId>,
    /// Both directions of every cut link.
    cut: HashSet<(NodeId, NodeId)>,
    delay: Duration,
}

/// Why `reachable` refused a dial; the error text is built after the network lock is released.
enum Refusal {
    LocalDown,
    Unreachable,
    NotBound,
}

impl Network {
    /// `transport.NewNetwork`: an empty network.
    pub fn new() -> Arc<Network> {
        Arc::new(Network {
            state: Mutex::new(NetState {
                endpoints: BTreeMap::new(),
                down: HashSet::new(),
                cut: HashSet::new(),
                delay: Duration::ZERO,
            }),
        })
    }

    /// `Network.Bind`: registers a new endpoint under `id` with the given ALPNs. It replaces an endpoint
    /// already bound under `id` without closing it.
    pub fn bind(self: &Arc<Self>, id: NodeId, alpns: &[&str]) -> Arc<MemEndpoint> {
        let (accept_tx, accept_rx) = mpsc::channel(ACCEPT_QUEUE);
        let reg = Arc::new(Registration {
            id,
            alpns: alpns.iter().map(|a| (*a).to_owned()).collect(),
            accept_tx,
            accept_rx: tokio::sync::Mutex::new(accept_rx),
            conns: Mutex::new(BTreeMap::new()),
            closed: AtomicBool::new(false),
        });
        lock(&self.state).endpoints.insert(id, Arc::clone(&reg));
        Arc::new(MemEndpoint {
            net: Arc::clone(self),
            reg,
        })
    }

    /// `Network.SetDown`: while `down`, the node can neither dial nor be reached. Taking a node down
    /// closes all of its connections and every endpoint's connections to it.
    pub fn set_down(&self, id: NodeId, down: bool) {
        let mut st = lock(&self.state);
        if down {
            st.down.insert(id);
            if let Some(ep) = st.endpoints.get(&id) {
                ep.drop_all();
            }
            for ep in st.endpoints.values() {
                ep.drop_peer(id);
            }
        } else {
            st.down.remove(&id);
        }
    }

    /// `Network.Partition`: cuts (or restores) the link between `a` and `b` in both directions. Cutting
    /// closes the connections between them.
    pub fn partition(&self, a: NodeId, b: NodeId, cut: bool) {
        let mut st = lock(&self.state);
        if cut {
            st.cut.insert((a, b));
            st.cut.insert((b, a));
            if let Some(ep) = st.endpoints.get(&a) {
                ep.drop_peer(b);
            }
            if let Some(ep) = st.endpoints.get(&b) {
                ep.drop_peer(a);
            }
        } else {
            st.cut.remove(&(a, b));
            st.cut.remove(&(b, a));
        }
    }

    /// `Network.SetDelay`: a fixed latency added to every dial.
    pub fn set_delay(&self, d: Duration) {
        lock(&self.state).delay = d;
    }

    /// `Network.reachable`.
    fn reachable(&self, from: NodeId, to: NodeId) -> Result<Arc<Registration>, TransportError> {
        let refused = {
            let st = lock(&self.state);
            if st.down.contains(&from) {
                Refusal::LocalDown
            } else if st.down.contains(&to) || st.cut.contains(&(from, to)) {
                Refusal::Unreachable
            } else {
                match st.endpoints.get(&to) {
                    Some(ep) if !ep.closed.load(Ordering::SeqCst) => return Ok(Arc::clone(ep)),
                    _ => Refusal::NotBound,
                }
            }
        };
        Err(match refused {
            Refusal::LocalDown => TransportError::MemLocalDown,
            Refusal::Unreachable => TransportError::MemUnreachable(to.short()),
            Refusal::NotBound => TransportError::MemNotBound(to.short()),
        })
    }

    fn delay(&self) -> Duration {
        lock(&self.state).delay
    }
}

/// An endpoint on a [`Network`]: `addrs()` = ["mem:<shortid>"]; path {direct: true, rtt: 1ms}.
pub struct MemEndpoint {
    /// Go `MemEndpoint.net`: kept alive by every endpoint.
    net: Arc<Network>,
    reg: Arc<Registration>,
}

/// The state of a bound endpoint (Go's `MemEndpoint` fields besides `net`). The network holds it until
/// `bind` replaces it, whether or not a [`MemEndpoint`] handle still exists.
struct Registration {
    id: NodeId,
    alpns: HashSet<String>,
    accept_tx: mpsc::Sender<Arc<dyn Conn>>,
    accept_rx: tokio::sync::Mutex<mpsc::Receiver<Arc<dyn Conn>>>,
    /// Open connection halves of this endpoint, by serial (creation order).
    conns: Mutex<BTreeMap<u64, Arc<MemConn>>>,
    closed: AtomicBool,
}

impl Registration {
    fn track(&self, c: &Arc<MemConn>) {
        lock(&self.conns).insert(c.this().serial, Arc::clone(c));
    }

    fn untrack(&self, serial: u64) {
        lock(&self.conns).remove(&serial);
    }

    /// Closes every connection of this endpoint.
    fn drop_all(&self) {
        let conns: Vec<Arc<MemConn>> = lock(&self.conns).values().cloned().collect();
        for c in conns {
            c.close();
        }
    }

    /// Closes this endpoint's connections to `id`.
    fn drop_peer(&self, id: NodeId) {
        let conns: Vec<Arc<MemConn>> = lock(&self.conns)
            .values()
            .filter(|c| c.peer().id == id)
            .cloned()
            .collect();
        for c in conns {
            c.close();
        }
    }
}

#[async_trait::async_trait]
impl Endpoint for MemEndpoint {
    fn id(&self) -> NodeId {
        self.reg.id
    }

    /// `MemEndpoint.Dial`; `addrs` are ignored.
    async fn dial(
        &self,
        ctx: &Ctx,
        id: NodeId,
        _addrs: Vec<String>,
        alpn: &str,
    ) -> Result<Arc<dyn Conn>, TransportError> {
        let me = &self.reg;
        if me.closed.load(Ordering::SeqCst) {
            return Err(TransportError::Closed);
        }
        let peer = self.net.reachable(me.id, id)?;
        if !peer.alpns.contains(alpn) {
            return Err(TransportError::MemAlpn(id.short(), alpn.to_owned()));
        }
        let delay = self.net.delay();
        if !delay.is_zero() {
            ctx.sleep(delay).await?;
        }
        // Nothing is checked again after the delay, as in Go.
        let (local, remote) = MemConn::pair(me, &peer, alpn);
        me.track(&local);
        peer.track(&remote);
        let remote: Arc<dyn Conn> = remote;
        match ctx.run(peer.accept_tx.send(remote)).await {
            Ok(Ok(())) => Ok(local),
            // The accept queue lives as long as `peer`, so a send cannot fail; treat it as closed.
            Ok(Err(_)) => {
                local.close();
                Err(TransportError::Closed)
            }
            Err(e) => {
                local.close();
                Err(e.into())
            }
        }
    }

    /// `MemEndpoint.Accept`: the next queued connection, or the ctx error. A closed endpoint still
    /// returns what is queued.
    async fn accept(&self, ctx: &Ctx) -> Result<Arc<dyn Conn>, TransportError> {
        let next = ctx
            .run(async { self.reg.accept_rx.lock().await.recv().await })
            .await?;
        // The registration holds a sender, so the queue never reports its end.
        next.ok_or(TransportError::Closed)
    }

    fn addrs(&self) -> Vec<String> {
        vec![format!("mem:{}", self.reg.id.short())]
    }

    /// `MemEndpoint.Close`: marks the endpoint closed and closes its connections.
    async fn close(&self) {
        self.reg.closed.store(true, Ordering::SeqCst);
        self.reg.drop_all();
    }
}

/// Both halves of one in-memory connection.
struct ConnPair {
    alpn: String,
    dialer: Half,
    acceptor: Half,
}

/// One half of a connection: its endpoint, its queue of unaccepted streams and its `done` state.
struct Half {
    ep: Weak<Registration>,
    /// The id of this half's endpoint.
    id: NodeId,
    /// The key of this half in its endpoint's tracking map.
    serial: u64,
    streams_tx: mpsc::Sender<PendingStream>,
    streams_rx: tokio::sync::Mutex<mpsc::Receiver<PendingStream>>,
    /// Go `done`: cancelled when this half is closed.
    done: CancellationToken,
    /// Serializes `close` (Go `sync.Once`): a concurrent caller returns only after `done` is cancelled.
    close_lock: Mutex<()>,
}

impl Half {
    fn new(ep: &Arc<Registration>) -> Half {
        let (streams_tx, streams_rx) = mpsc::channel(STREAM_QUEUE);
        Half {
            ep: Arc::downgrade(ep),
            id: ep.id,
            serial: NEXT_SERIAL.fetch_add(1, Ordering::Relaxed),
            streams_tx,
            streams_rx: tokio::sync::Mutex::new(streams_rx),
            done: CancellationToken::new(),
            close_lock: Mutex::new(()),
        }
    }
}

impl ConnPair {
    fn half(&self, dialer: bool) -> &Half {
        if dialer { &self.dialer } else { &self.acceptor }
    }

    /// `memConn.Close` of one half.
    fn close_half(pair: &Arc<ConnPair>, dialer: bool) {
        let half = pair.half(dialer);
        {
            let _once = lock(&half.close_lock);
            if half.done.is_cancelled() {
                return;
            }
            half.done.cancel();
            if let Some(ep) = half.ep.upgrade() {
                ep.untrack(half.serial);
            }
        }
        let pair = Arc::clone(pair);
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => drop(rt.spawn(async move { ConnPair::close_half(&pair, !dialer) })),
            Err(_) => ConnPair::close_half(&pair, !dialer),
        }
    }
}

/// One side of an in-memory connection.
pub(crate) struct MemConn {
    pair: Arc<ConnPair>,
    /// Whether this is the dialing half.
    dialer: bool,
}

impl MemConn {
    /// A new connection between `dialer` and `acceptor`: the dialing half and the accepting half.
    fn pair(
        dialer: &Arc<Registration>,
        acceptor: &Arc<Registration>,
        alpn: &str,
    ) -> (Arc<MemConn>, Arc<MemConn>) {
        let pair = Arc::new(ConnPair {
            alpn: alpn.to_owned(),
            dialer: Half::new(dialer),
            acceptor: Half::new(acceptor),
        });
        let local = Arc::new(MemConn {
            pair: Arc::clone(&pair),
            dialer: true,
        });
        let remote = Arc::new(MemConn {
            pair,
            dialer: false,
        });
        (local, remote)
    }

    fn this(&self) -> &Half {
        self.pair.half(self.dialer)
    }

    fn peer(&self) -> &Half {
        self.pair.half(!self.dialer)
    }
}

#[async_trait::async_trait]
impl Conn for MemConn {
    fn remote_id(&self) -> NodeId {
        self.peer().id
    }

    fn alpn(&self) -> String {
        self.pair.alpn.clone()
    }

    /// `memConn.OpenStream`: queues the peer's half of a new stream on the peer's stream queue.
    async fn open_stream(&self, ctx: &Ctx) -> Result<Stream, TransportError> {
        let me = self.this();
        if me.done.is_cancelled() {
            return Err(TransportError::Closed);
        }
        let forward = Pipe::new();
        let backward = Pipe::new();
        let pending = PendingStream {
            forward: Arc::clone(&forward),
            backward: Arc::clone(&backward),
        };
        let queue = &self.peer().streams_tx;
        let queued = ctx
            .run(async {
                tokio::select! {
                    sent = queue.send(pending) => sent.is_ok(),
                    () = me.done.cancelled() => false,
                }
            })
            .await?;
        if !queued {
            return Err(TransportError::Closed);
        }
        Ok(Stream {
            send: Box::new(MemSendStream { pipe: forward }),
            recv: Box::new(MemRecvStream { pipe: backward }),
        })
    }

    /// `memConn.AcceptStream`.
    async fn accept_stream(&self, ctx: &Ctx) -> Result<Stream, TransportError> {
        let me = self.this();
        let next = ctx
            .run(async {
                tokio::select! {
                    s = async { me.streams_rx.lock().await.recv().await } => s,
                    () = me.done.cancelled() => None,
                }
            })
            .await?;
        match next {
            Some(p) => Ok(p.into_acceptor_stream()),
            None => Err(TransportError::Closed),
        }
    }

    fn close(&self) {
        ConnPair::close_half(&self.pair, self.dialer);
    }

    fn path(&self) -> PathInfo {
        PathInfo {
            direct: true,
            rtt: MEM_RTT,
        }
    }

    fn is_closed(&self) -> bool {
        self.this().done.is_cancelled()
    }

    async fn closed(&self) {
        self.this().done.cancelled().await;
    }
}

/// The accepting half of a stream while it waits in a connection's queue. Dropping it has no effect on
/// the pipes.
struct PendingStream {
    /// Opener to acceptor.
    forward: Arc<Pipe>,
    /// Acceptor to opener.
    backward: Arc<Pipe>,
}

impl PendingStream {
    fn into_acceptor_stream(self) -> Stream {
        Stream {
            send: Box::new(MemSendStream {
                pipe: self.backward,
            }),
            recv: Box::new(MemRecvStream { pipe: self.forward }),
        }
    }
}

/// Go `pipe`: a buffered byte pipe with EOF on writer close and an error on reader cancel.
struct Pipe {
    state: Mutex<PipeState>,
}

#[derive(Default)]
struct PipeState {
    buf: VecDeque<u8>,
    /// The writer finished.
    closed: bool,
    /// The reader cancelled.
    canceled: bool,
    reader: Option<Waker>,
    writer: Option<Waker>,
}

fn register(slot: &mut Option<Waker>, cx: &Context<'_>) {
    match slot {
        Some(w) if w.will_wake(cx.waker()) => {}
        _ => *slot = Some(cx.waker().clone()),
    }
}

fn wake(w: Option<Waker>) {
    if let Some(w) = w {
        w.wake();
    }
}

fn reset_by_peer() -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionReset, "mem: stream reset by peer")
}

fn closed_pipe() -> io::Error {
    // io.ErrClosedPipe
    io::Error::new(io::ErrorKind::BrokenPipe, "io: read/write on closed pipe")
}

fn read_canceled() -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionAborted, "mem: read canceled")
}

impl Pipe {
    fn new() -> Arc<Pipe> {
        Arc::new(Pipe {
            state: Mutex::new(PipeState::default()),
        })
    }

    /// `pipe.Write`, one chunk per call: waits while the buffer is full, then takes what fits.
    fn poll_write(&self, cx: &Context<'_>, b: &[u8]) -> Poll<io::Result<usize>> {
        if b.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let (taken, reader) = {
            let mut st = lock(&self.state);
            if st.buf.len() >= PIPE_LIMIT && !st.canceled && !st.closed {
                register(&mut st.writer, cx);
                return Poll::Pending;
            }
            if st.canceled {
                return Poll::Ready(Err(reset_by_peer()));
            }
            if st.closed {
                return Poll::Ready(Err(closed_pipe()));
            }
            let take = (PIPE_LIMIT - st.buf.len()).min(b.len());
            st.buf.extend(&b[..take]);
            (take, st.reader.take())
        };
        wake(reader);
        Poll::Ready(Ok(taken))
    }

    /// `pipe.Read`: waits for data, close or cancel. Cancel wins; an empty closed pipe is EOF.
    fn poll_read(&self, cx: &Context<'_>, dst: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let writer = {
            let mut st = lock(&self.state);
            if st.buf.is_empty() && !st.closed && !st.canceled {
                register(&mut st.reader, cx);
                return Poll::Pending;
            }
            if st.canceled {
                return Poll::Ready(Err(read_canceled()));
            }
            if st.buf.is_empty() {
                return Poll::Ready(Ok(()));
            }
            let n = dst.remaining().min(st.buf.len());
            let (front, back) = st.buf.as_slices();
            let from_front = n.min(front.len());
            dst.put_slice(&front[..from_front]);
            dst.put_slice(&back[..n - from_front]);
            st.buf.drain(..n);
            st.writer.take()
        };
        wake(writer);
        Poll::Ready(Ok(()))
    }

    /// `pipe.CloseWrite`.
    fn close_write(&self) {
        let (reader, writer) = {
            let mut st = lock(&self.state);
            st.closed = true;
            (st.reader.take(), st.writer.take())
        };
        wake(reader);
        wake(writer);
    }

    /// `pipe.CancelRead`: discards the buffer.
    fn cancel_read(&self) {
        let (reader, writer) = {
            let mut st = lock(&self.state);
            st.canceled = true;
            st.buf = VecDeque::new();
            (st.reader.take(), st.writer.take())
        };
        wake(reader);
        wake(writer);
    }

    /// noq's receive-side drop: stop unless everything up to EOF has been read.
    fn cancel_unless_at_eof(&self) {
        let at_eof = {
            let st = lock(&self.state);
            st.canceled || (st.closed && st.buf.is_empty())
        };
        if !at_eof {
            self.cancel_read();
        }
    }
}

/// The send half of a mem stream (`memStream.w`).
struct MemSendStream {
    pipe: Arc<Pipe>,
}

impl AsyncWrite for MemSendStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.pipe.poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    /// As noq: shutdown finishes the stream.
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.pipe.close_write();
        Poll::Ready(Ok(()))
    }
}

impl SendStream for MemSendStream {
    /// `memStream.CloseWrite` / `Close`.
    fn finish(&mut self) {
        self.pipe.close_write();
    }
}

impl Drop for MemSendStream {
    fn drop(&mut self) {
        self.pipe.close_write();
    }
}

/// The receive half of a mem stream (`memStream.r`).
struct MemRecvStream {
    pipe: Arc<Pipe>,
}

impl AsyncRead for MemRecvStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.pipe.poll_read(cx, buf)
    }
}

impl RecvStream for MemRecvStream {
    /// `memStream.CancelRead`: the code is ignored.
    fn cancel_read(&mut self, _code: u64) {
        self.pipe.cancel_read();
    }
}

impl Drop for MemRecvStream {
    fn drop(&mut self) {
        self.pipe.cancel_unless_at_eof();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::future::Future;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::CtxError;

    const ALPN: &str = "amber-dstore/1";
    const CLUSTER: &str = "amber-dstore-cluster/1";
    const WAIT: Duration = Duration::from_secs(5);

    fn id(b: u8) -> NodeId {
        NodeId([b; 32])
    }

    /// `view.ShortID`, computed independently of `dstore_view`.
    fn short(id: NodeId) -> String {
        id.0[..4].iter().map(|b| format!("{b:02x}")).collect()
    }

    async fn within<F: Future>(what: &str, f: F) -> F::Output {
        match tokio::time::timeout(WAIT, f).await {
            Ok(v) => v,
            Err(_) => panic!("{what}: timed out"),
        }
    }

    fn must<T>(what: &str, r: Result<T, TransportError>) -> T {
        match r {
            Ok(v) => v,
            Err(e) => panic!("{what}: {e}"),
        }
    }

    fn fails<T>(what: &str, r: Result<T, TransportError>) -> TransportError {
        match r {
            Ok(_) => panic!("{what}: unexpected success"),
            Err(e) => e,
        }
    }

    fn io_fails<T: std::fmt::Debug>(what: &str, r: io::Result<T>) -> String {
        match r {
            Ok(v) => panic!("{what}: unexpected success {v:?}"),
            Err(e) => e.to_string(),
        }
    }

    async fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
        let deadline = tokio::time::Instant::now() + WAIT;
        while !cond() {
            if tokio::time::Instant::now() >= deadline {
                panic!("timed out waiting for {what}");
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }

    fn tracked(ep: &MemEndpoint) -> usize {
        lock(&ep.reg.conns).len()
    }

    async fn dial(from: &MemEndpoint, to: NodeId, alpn: &str) -> Arc<dyn Conn> {
        must(
            "dial",
            within("dial", from.dial(&Ctx::background(), to, Vec::new(), alpn)).await,
        )
    }

    async fn accept(ep: &MemEndpoint) -> Arc<dyn Conn> {
        must(
            "accept",
            within("accept", ep.accept(&Ctx::background())).await,
        )
    }

    /// Dials `to` from `from` and accepts the connection on `to`.
    async fn connect(
        from: &MemEndpoint,
        to: &MemEndpoint,
        alpn: &str,
    ) -> (Arc<dyn Conn>, Arc<dyn Conn>) {
        let local = dial(from, to.id(), alpn).await;
        let remote = accept(to).await;
        (local, remote)
    }

    async fn stream_pair(opener: &Arc<dyn Conn>, acceptor: &Arc<dyn Conn>) -> (Stream, Stream) {
        let bg = Ctx::background();
        let a = must(
            "open_stream",
            within("open_stream", opener.open_stream(&bg)).await,
        );
        let b = must(
            "accept_stream",
            within("accept_stream", acceptor.accept_stream(&bg)).await,
        );
        (a, b)
    }

    /// One read into a `buf_len` buffer.
    async fn read_once(s: &mut Stream, buf_len: usize) -> io::Result<Vec<u8>> {
        let mut buf = vec![0u8; buf_len];
        let n = within("read", s.recv.read(&mut buf)).await?;
        buf.truncate(n);
        Ok(buf)
    }

    async fn write_once(s: &mut Stream, data: &[u8]) -> io::Result<usize> {
        within("write", s.send.write(data)).await
    }

    fn ctx_err(e: &TransportError) -> Option<CtxError> {
        match e {
            TransportError::Ctx(c) => Some(*c),
            _ => None,
        }
    }

    #[tokio::test]
    async fn ids_alpn_and_path() {
        let net = Network::new();
        let a = net.bind(id(1), &[]);
        let b = net.bind(id(2), &[ALPN, CLUSTER]);
        assert_eq!(a.id(), id(1));
        assert_eq!(b.id(), id(2));
        let (local, remote) = connect(&a, &b, CLUSTER).await;
        assert_eq!(local.remote_id(), id(2));
        assert_eq!(remote.remote_id(), id(1));
        assert_eq!(local.alpn(), CLUSTER);
        assert_eq!(remote.alpn(), CLUSTER);
        let want = PathInfo {
            direct: true,
            rtt: Duration::from_millis(1),
        };
        assert_eq!(local.path(), want);
        assert_eq!(remote.path(), want);
        assert!(!local.is_closed());
        assert!(!remote.is_closed());
        assert_eq!(tracked(&a), 1);
        assert_eq!(tracked(&b), 1);
    }

    #[tokio::test]
    async fn dial_to_self() {
        let net = Network::new();
        let a = net.bind(id(1), &[ALPN]);
        let (local, remote) = connect(&a, &a, ALPN).await;
        assert_eq!(local.remote_id(), id(1));
        assert_eq!(remote.remote_id(), id(1));
        assert_eq!(tracked(&a), 2);
        let (mut s1, mut s2) = stream_pair(&local, &remote).await;
        assert_eq!(write_once(&mut s1, b"me").await.ok(), Some(2));
        assert_eq!(read_once(&mut s2, 8).await.ok(), Some(b"me".to_vec()));
    }

    #[tokio::test]
    async fn addrs_are_the_short_id() {
        let net = Network::new();
        let mut raw = [0u8; 32];
        raw[..5].copy_from_slice(&[0xab, 0x01, 0xcd, 0x02, 0xff]);
        let ep = net.bind(NodeId(raw), &[ALPN]);
        assert_eq!(ep.addrs(), vec!["mem:ab01cd02".to_owned()]);
    }

    #[tokio::test]
    async fn dial_error_texts_in_go_order() {
        let net = Network::new();
        let bg = Ctx::background();
        let a = net.bind(id(1), &[]);
        let b = net.bind(id(2), &[CLUSTER]);
        let unbound = id(3);
        let d = net.bind(id(4), &[ALPN]);

        let e = fails("alpn", a.dial(&bg, b.id(), Vec::new(), ALPN).await);
        assert_eq!(
            e.to_string(),
            format!("mem: {} does not speak amber-dstore/1", short(id(2)))
        );
        assert!(
            matches!(e, TransportError::MemAlpn(ref s, ref p) if *s == short(id(2)) && p == ALPN)
        );

        let e = fails("unbound", a.dial(&bg, unbound, Vec::new(), ALPN).await);
        assert_eq!(e.to_string(), format!("mem: {} not bound", short(unbound)));
        assert!(matches!(e, TransportError::MemNotBound(_)));

        // A cut link is unreachable in both directions, before the ALPN check.
        net.partition(a.id(), d.id(), true);
        let e = fails("cut", a.dial(&bg, d.id(), Vec::new(), ALPN).await);
        assert_eq!(e.to_string(), format!("mem: {} unreachable", short(id(4))));
        assert!(matches!(e, TransportError::MemUnreachable(_)));
        let e = fails("cut back", d.dial(&bg, a.id(), Vec::new(), "none").await);
        assert_eq!(e.to_string(), format!("mem: {} unreachable", short(id(1))));
        net.partition(a.id(), d.id(), false);
        drop(dial(&a, d.id(), ALPN).await);

        // A down peer is unreachable, before the bound and ALPN checks.
        net.set_down(d.id(), true);
        let e = fails("down peer", a.dial(&bg, d.id(), Vec::new(), "none").await);
        assert_eq!(e.to_string(), format!("mem: {} unreachable", short(id(4))));
        net.set_down(unbound, true);
        let e = fails("down unbound", a.dial(&bg, unbound, Vec::new(), ALPN).await);
        assert_eq!(
            e.to_string(),
            format!("mem: {} unreachable", short(unbound))
        );
        net.set_down(unbound, false);

        // A down dialer comes first.
        net.set_down(a.id(), true);
        let e = fails("local down", a.dial(&bg, unbound, Vec::new(), ALPN).await);
        assert_eq!(e.to_string(), "mem: local endpoint is down");
        net.set_down(a.id(), false);
        net.set_down(d.id(), false);

        // A closed peer is not bound.
        d.close().await;
        let e = fails("closed peer", a.dial(&bg, d.id(), Vec::new(), ALPN).await);
        assert_eq!(e.to_string(), format!("mem: {} not bound", short(id(4))));

        // Validation happens before the delay.
        net.set_delay(Duration::from_secs(30));
        let e = fails(
            "delay",
            within("delay", a.dial(&bg, unbound, Vec::new(), ALPN)).await,
        );
        assert!(matches!(e, TransportError::MemNotBound(_)));
        net.set_delay(Duration::ZERO);

        // A closed dialer comes before everything.
        a.close().await;
        net.set_down(a.id(), true);
        let e = fails(
            "closed dialer",
            a.dial(&bg, unbound, Vec::new(), ALPN).await,
        );
        assert_eq!(e.to_string(), "transport: closed");
        assert!(matches!(e, TransportError::Closed));
    }

    #[tokio::test]
    async fn endpoints_keep_their_network_and_unheld_endpoints_stay_bound() {
        // As Go's `transport.NewNetwork().Bind(..)`: nobody but the endpoints holds the network.
        let (a, b, silent) = {
            let net = Network::new();
            let a = net.bind(id(1), &[ALPN]);
            let b = net.bind(id(2), &[ALPN]);
            // Bound, and its handle dropped at once.
            drop(net.bind(id(3), &[ALPN]));
            (a, b, id(3))
        };
        let (ab, ba) = connect(&a, &b, ALPN).await;
        let (mut s1, mut s2) = stream_pair(&ab, &ba).await;
        assert_eq!(write_once(&mut s1, b"hi").await.ok(), Some(2));
        assert_eq!(read_once(&mut s2, 8).await.ok(), Some(b"hi".to_vec()));

        // The unheld endpoint is still registered: the dial succeeds and waits in its accept queue.
        let c = dial(&a, silent, ALPN).await;
        assert_eq!(c.remote_id(), silent);
        assert!(!c.is_closed());
        assert_eq!(tracked(&a), 2);

        // Dropping every handle frees the network, the registrations and the connections.
        let weak_net = Arc::downgrade(&a.net);
        let weak_pair = {
            let Some(mc) = lock(&a.reg.conns).values().next().cloned() else {
                panic!("no tracked connection");
            };
            Arc::downgrade(&mc.pair)
        };
        drop((ab, ba, s1, s2, c, a, b));
        assert!(weak_net.upgrade().is_none(), "the network was freed");
        assert!(weak_pair.upgrade().is_none(), "the connection was freed");
    }

    #[tokio::test]
    async fn local_down_and_closed_fail_before_the_delay() {
        let net = Network::new();
        let bg = Ctx::background();
        let a = net.bind(id(1), &[ALPN]);
        let b = net.bind(id(2), &[ALPN]);
        net.set_delay(Duration::from_secs(30));
        net.set_down(a.id(), true);
        let e = fails(
            "local down",
            within("down", a.dial(&bg, b.id(), Vec::new(), ALPN)).await,
        );
        assert!(matches!(e, TransportError::MemLocalDown));
        assert_eq!(e.to_string(), "mem: local endpoint is down");
        net.set_down(a.id(), false);
        a.close().await;
        let e = fails(
            "closed",
            within("closed", a.dial(&bg, b.id(), Vec::new(), ALPN)).await,
        );
        assert!(matches!(e, TransportError::Closed));
    }

    #[tokio::test]
    async fn stream_fin_and_write_after_finish() {
        let net = Network::new();
        let a = net.bind(id(1), &[]);
        let b = net.bind(id(2), &[ALPN]);
        let (local, remote) = connect(&a, &b, ALPN).await;
        let (mut cs, mut ss) = stream_pair(&local, &remote).await;

        assert_eq!(write_once(&mut cs, b"hello").await.ok(), Some(5));
        assert_eq!(read_once(&mut ss, 16).await.ok(), Some(b"hello".to_vec()));
        cs.send.finish();
        assert_eq!(read_once(&mut ss, 16).await.ok(), Some(Vec::new()), "EOF");
        assert_eq!(
            read_once(&mut ss, 16).await.ok(),
            Some(Vec::new()),
            "EOF again"
        );
        let e = io_fails("write after finish", write_once(&mut cs, b"x").await);
        assert_eq!(e, "io: read/write on closed pipe");

        // The other direction is independent.
        assert_eq!(write_once(&mut ss, b"abc").await.ok(), Some(3));
        assert_eq!(read_once(&mut cs, 2).await.ok(), Some(b"ab".to_vec()));
        assert_eq!(read_once(&mut cs, 16).await.ok(), Some(b"c".to_vec()));
        ss.send.finish();
        assert_eq!(read_once(&mut cs, 16).await.ok(), Some(Vec::new()), "EOF");
        let e = io_fails("late write", write_once(&mut ss, b"late").await);
        assert_eq!(e, "io: read/write on closed pipe");

        // Buffered bytes are read before EOF; an empty write succeeds even after finish.
        let (mut cs, mut ss) = stream_pair(&local, &remote).await;
        assert_eq!(write_once(&mut cs, b"tail").await.ok(), Some(4));
        cs.send.finish();
        assert_eq!(write_once(&mut cs, b"").await.ok(), Some(0));
        assert_eq!(read_once(&mut ss, 16).await.ok(), Some(b"tail".to_vec()));
        assert_eq!(read_once(&mut ss, 16).await.ok(), Some(Vec::new()));
    }

    #[tokio::test]
    async fn cancel_read_resets_the_writer() {
        let net = Network::new();
        let a = net.bind(id(1), &[]);
        let b = net.bind(id(2), &[ALPN]);
        let (local, remote) = connect(&a, &b, ALPN).await;
        let (mut cs, mut ss) = stream_pair(&local, &remote).await;

        cs.recv.cancel_read(0);
        let e = io_fails("canceled read", read_once(&mut cs, 16).await);
        assert_eq!(e, "mem: read canceled");
        let e = io_fails("reset write", write_once(&mut ss, b"z").await);
        assert_eq!(e, "mem: stream reset by peer");
        ss.recv.cancel_read(7);
        let e = io_fails("reset write back", write_once(&mut cs, b"q").await);
        assert_eq!(e, "mem: stream reset by peer");

        // Cancel discards buffered bytes and wins over EOF.
        let (mut cs, mut ss) = stream_pair(&local, &remote).await;
        assert_eq!(write_once(&mut ss, b"abc").await.ok(), Some(3));
        ss.send.finish();
        cs.recv.cancel_read(0);
        let e = io_fails("canceled after fin", read_once(&mut cs, 16).await);
        assert_eq!(e, "mem: read canceled");
        // A finished writer whose reader cancelled reports the reset first.
        let e = io_fails("write after both", write_once(&mut ss, b"x").await);
        assert_eq!(e, "mem: stream reset by peer");
    }

    #[tokio::test]
    async fn shutdown_finishes_and_flush_is_a_no_op() {
        let net = Network::new();
        let a = net.bind(id(1), &[]);
        let b = net.bind(id(2), &[ALPN]);
        let (local, remote) = connect(&a, &b, ALPN).await;
        let (mut cs, mut ss) = stream_pair(&local, &remote).await;
        within("write_all", cs.send.write_all(b"bye")).await.ok();
        assert!(within("flush", cs.send.flush()).await.is_ok());
        assert!(within("shutdown", cs.send.shutdown()).await.is_ok());
        let mut all = Vec::new();
        assert_eq!(
            within("read_to_end", ss.recv.read_to_end(&mut all))
                .await
                .ok(),
            Some(3)
        );
        assert_eq!(all, b"bye");
    }

    #[tokio::test]
    async fn dropping_halves_finishes_and_cancels() {
        let net = Network::new();
        let a = net.bind(id(1), &[]);
        let b = net.bind(id(2), &[ALPN]);
        let (local, remote) = connect(&a, &b, ALPN).await;

        // An unfinished send half finishes on drop; an unread receive half cancels.
        let (cs, mut ss) = stream_pair(&local, &remote).await;
        let Stream { send, recv } = cs;
        drop(send);
        assert_eq!(read_once(&mut ss, 8).await.ok(), Some(Vec::new()), "EOF");
        drop(recv);
        let e = io_fails("write to dropped reader", write_once(&mut ss, b"x").await);
        assert_eq!(e, "mem: stream reset by peer");

        // A receive half dropped at EOF does not cancel.
        let (cs, mut ss) = stream_pair(&local, &remote).await;
        within("write_all", ss.send.write_all(b"ab")).await.ok();
        ss.send.finish();
        let Stream { send, mut recv } = cs;
        let mut all = Vec::new();
        assert_eq!(
            within("read_to_end", recv.read_to_end(&mut all)).await.ok(),
            Some(2)
        );
        drop(recv);
        let e = io_fails("write after own finish", write_once(&mut ss, b"x").await);
        assert_eq!(e, "io: read/write on closed pipe");
        drop(send);
        assert_eq!(read_once(&mut ss, 8).await.ok(), Some(Vec::new()));
    }

    #[tokio::test]
    async fn pipe_backpressure_at_the_limit() {
        let net = Network::new();
        let a = net.bind(id(1), &[]);
        let b = net.bind(id(2), &[ALPN]);
        let (local, remote) = connect(&a, &b, ALPN).await;
        let (mut cs, mut ss) = stream_pair(&local, &remote).await;

        let big: Vec<u8> = (0..PIPE_LIMIT + 10).map(|i| (i % 251) as u8).collect();
        // One write takes what fits.
        assert_eq!(write_once(&mut cs, &big).await.ok(), Some(PIPE_LIMIT));
        let rest = big[PIPE_LIMIT..].to_vec();
        let writer = tokio::spawn(async move {
            let r = cs.send.write_all(&rest).await;
            (r, cs)
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!writer.is_finished(), "a full pipe blocks the writer");

        // Reading makes room.
        let first = must_io("read", read_once(&mut ss, 64).await);
        assert_eq!(first, big[..64]);
        let (r, mut cs) = match within("writer", writer).await {
            Ok(v) => v,
            Err(e) => panic!("writer task: {e}"),
        };
        assert!(r.is_ok());
        cs.send.finish();
        let mut all = first;
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = must_io("drain", within("drain", ss.recv.read(&mut buf)).await);
            if n == 0 {
                break;
            }
            all.extend_from_slice(&buf[..n]);
        }
        assert_eq!(all.len(), big.len());
        assert!(all == big, "bytes arrive in order");

        // A reader's cancel wakes a blocked writer with the reset.
        let (mut cs, mut ss) = stream_pair(&local, &remote).await;
        assert_eq!(write_once(&mut cs, &big).await.ok(), Some(PIPE_LIMIT));
        let writer = tokio::spawn(async move { cs.send.write_all(b"more").await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!writer.is_finished());
        ss.recv.cancel_read(0);
        match within("blocked writer", writer).await {
            Ok(r) => assert_eq!(io_fails("blocked writer", r), "mem: stream reset by peer"),
            Err(e) => panic!("writer task: {e}"),
        }
    }

    fn must_io<T>(what: &str, r: io::Result<T>) -> T {
        match r {
            Ok(v) => v,
            Err(e) => panic!("{what}: {e}"),
        }
    }

    #[tokio::test]
    async fn conn_close_reaches_the_peer() {
        let net = Network::new();
        let bg = Ctx::background();
        let a = net.bind(id(1), &[]);
        let b = net.bind(id(2), &[ALPN]);
        let (local, remote) = connect(&a, &b, ALPN).await;

        let waiting = {
            let remote = Arc::clone(&remote);
            tokio::spawn(async move { remote.accept_stream(&Ctx::background()).await.err() })
        };
        tokio::time::sleep(Duration::from_millis(10)).await;
        local.close();
        assert!(local.is_closed());
        within("local closed()", local.closed()).await;
        within("peer closed()", remote.closed()).await;
        assert!(remote.is_closed());
        match within("waiting accept_stream", waiting).await {
            Ok(Some(TransportError::Closed)) => {}
            Ok(other) => panic!("waiting accept_stream: {other:?}"),
            Err(e) => panic!("task: {e}"),
        }
        for c in [&local, &remote] {
            let e = fails("open_stream", c.open_stream(&bg).await);
            assert_eq!(e.to_string(), "transport: closed");
            let e = fails("accept_stream", c.accept_stream(&bg).await);
            assert!(matches!(e, TransportError::Closed));
        }
        local.close();
        remote.close();
        eventually("untracked", || tracked(&a) == 0 && tracked(&b) == 0).await;
    }

    #[tokio::test]
    async fn blocked_read_survives_conn_close() {
        let net = Network::new();
        let a = net.bind(id(1), &[]);
        let b = net.bind(id(2), &[ALPN]);
        let (local, remote) = connect(&a, &b, ALPN).await;
        let (mut cs, mut ss) = stream_pair(&local, &remote).await;

        let reader = tokio::spawn(async move {
            let mut buf = [0u8; 16];
            let r = cs.recv.read(&mut buf).await.map(|n| buf[..n].to_vec());
            (r, cs)
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        local.close();
        within("peer closed", remote.closed()).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !reader.is_finished(),
            "closing the conn leaves the pipes alone"
        );

        // The stream still carries bytes after the close.
        assert_eq!(write_once(&mut ss, b"late").await.ok(), Some(4));
        match within("reader", reader).await {
            Ok((r, _cs)) => assert_eq!(r.ok(), Some(b"late".to_vec())),
            Err(e) => panic!("reader task: {e}"),
        }

        // Taking the node down or closing its endpoint leaves the pipes alone too.
        let (local, remote) = connect(&a, &b, ALPN).await;
        let (mut cs, mut ss) = stream_pair(&local, &remote).await;
        net.set_down(b.id(), true);
        assert!(remote.is_closed());
        within("dialer closed", local.closed()).await;
        assert_eq!(write_once(&mut ss, b"down").await.ok(), Some(4));
        assert_eq!(read_once(&mut cs, 16).await.ok(), Some(b"down".to_vec()));
        net.set_down(b.id(), false);

        let (local, remote) = connect(&a, &b, ALPN).await;
        let (mut cs, mut ss) = stream_pair(&local, &remote).await;
        a.close().await;
        assert!(local.is_closed());
        within("acceptor closed", remote.closed()).await;
        assert_eq!(write_once(&mut cs, b"gone").await.ok(), Some(4));
        assert_eq!(read_once(&mut ss, 16).await.ok(), Some(b"gone".to_vec()));
    }

    #[tokio::test]
    async fn open_stream_blocks_when_256_streams_are_queued() {
        let net = Network::new();
        let bg = Ctx::background();
        let a = net.bind(id(1), &[]);
        let b = net.bind(id(2), &[ALPN]);
        let (local, remote) = connect(&a, &b, ALPN).await;

        let mut opened = Vec::new();
        for i in 0..STREAM_QUEUE {
            opened.push(must(
                &format!("open {i}"),
                within("open", local.open_stream(&bg)).await,
            ));
        }
        assert_eq!(STREAM_QUEUE, 256);
        let e = fails(
            "257th open",
            within(
                "257th",
                local.open_stream(&bg.with_timeout(Duration::from_millis(50))),
            )
            .await,
        );
        assert_eq!(ctx_err(&e), Some(CtxError::DeadlineExceeded));
        assert_eq!(e.to_string(), "context deadline exceeded");

        // Accepting one makes room; the accepted stream is the first one opened.
        let mut first = must("accept", within("accept", remote.accept_stream(&bg)).await);
        let mut next = must("open again", within("open", local.open_stream(&bg)).await);
        assert_eq!(write_once(&mut opened[0], b"0").await.ok(), Some(1));
        assert_eq!(read_once(&mut first, 4).await.ok(), Some(b"0".to_vec()));
        drop(next.send.write(b"").await);

        // A blocked open_stream fails with transport: closed when its conn closes.
        let blocked = {
            let local = Arc::clone(&local);
            tokio::spawn(async move { local.open_stream(&Ctx::background()).await.err() })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!blocked.is_finished());
        local.close();
        match within("blocked open", blocked).await {
            Ok(Some(TransportError::Closed)) => {}
            Ok(other) => panic!("blocked open_stream: {other:?}"),
            Err(e) => panic!("task: {e}"),
        }
    }

    #[tokio::test]
    async fn dial_blocks_when_64_conns_are_queued() {
        let net = Network::new();
        let bg = Ctx::background();
        let a = net.bind(id(1), &[]);
        let b = net.bind(id(2), &[ALPN]);
        let mut conns = Vec::new();
        for _ in 0..ACCEPT_QUEUE {
            conns.push(dial(&a, b.id(), ALPN).await);
        }
        assert_eq!(ACCEPT_QUEUE, 64);
        let ctx = bg.with_timeout(Duration::from_millis(50));
        let e = fails(
            "65th dial",
            within("65th", a.dial(&ctx, b.id(), Vec::new(), ALPN)).await,
        );
        assert_eq!(ctx_err(&e), Some(CtxError::DeadlineExceeded));
        // The failed dial closed its local half, which closed and untracked the remote half.
        eventually("failed pair untracked", || {
            tracked(&a) == 64 && tracked(&b) == 64
        })
        .await;

        let first = accept(&b).await;
        assert_eq!(first.remote_id(), a.id());
        assert!(!first.is_closed());
        conns.push(dial(&a, b.id(), ALPN).await);
        assert_eq!(tracked(&a), 65);
    }

    #[tokio::test]
    async fn accept_and_accept_stream_honour_the_ctx() {
        let net = Network::new();
        let bg = Ctx::background();
        let a = net.bind(id(1), &[]);
        let b = net.bind(id(2), &[ALPN]);
        let e = fails(
            "accept deadline",
            within(
                "accept",
                b.accept(&bg.with_timeout(Duration::from_millis(20))),
            )
            .await,
        );
        assert_eq!(ctx_err(&e), Some(CtxError::DeadlineExceeded));
        let canceled = bg.with_cancel();
        canceled.cancel();
        let e = fails("accept canceled", b.accept(&canceled).await);
        assert_eq!(ctx_err(&e), Some(CtxError::Canceled));
        assert_eq!(e.to_string(), "context canceled");

        let (local, _remote) = connect(&a, &b, ALPN).await;
        let e = fails(
            "accept_stream deadline",
            within(
                "accept_stream",
                local.accept_stream(&bg.with_timeout(Duration::from_millis(20))),
            )
            .await,
        );
        assert_eq!(ctx_err(&e), Some(CtxError::DeadlineExceeded));
    }

    /// Paused clock (PORTING.md §5.5): the elapsed times are virtual and exact.
    #[tokio::test(start_paused = true)]
    async fn dial_waits_for_the_delay() {
        let net = Network::new();
        let bg = Ctx::background();
        let a = net.bind(id(1), &[]);
        let b = net.bind(id(2), &[ALPN]);
        let delay = Duration::from_millis(150);
        net.set_delay(delay);

        let start = tokio::time::Instant::now();
        let c = dial(&a, b.id(), ALPN).await;
        let took = start.elapsed();
        assert!(took >= delay && took < delay * 2, "dial took {took:?}");
        assert!(!c.is_closed());

        // A ctx ending during the delay fails the dial and creates nothing.
        let start = tokio::time::Instant::now();
        let ctx = bg.with_timeout(Duration::from_millis(30));
        let e = fails(
            "delayed dial",
            within("delay", a.dial(&ctx, b.id(), Vec::new(), ALPN)).await,
        );
        assert_eq!(ctx_err(&e), Some(CtxError::DeadlineExceeded));
        let took = start.elapsed();
        assert!(
            took >= Duration::from_millis(30) && took < delay,
            "failed dial took {took:?}"
        );
        assert_eq!(tracked(&a), 1);
        assert_eq!(tracked(&b), 1);
        drop(accept(&b).await);
        let e = fails(
            "nothing queued",
            within(
                "accept",
                b.accept(&bg.with_timeout(Duration::from_millis(30))),
            )
            .await,
        );
        assert_eq!(ctx_err(&e), Some(CtxError::DeadlineExceeded));

        // Nothing is checked again after the delay: a peer taken down while the dial waits still gets
        // the new connection, which the earlier set_down could not close.
        let dialing = {
            let a = Arc::clone(&a);
            let to = b.id();
            tokio::spawn(async move { a.dial(&Ctx::background(), to, Vec::new(), ALPN).await })
        };
        tokio::time::sleep(delay / 2).await;
        assert!(!dialing.is_finished());
        net.set_down(b.id(), true);
        let c = match within("dial across set_down", dialing).await {
            Ok(r) => must("dial across set_down", r),
            Err(e) => panic!("dial task: {e}"),
        };
        assert!(!c.is_closed());
        let got = accept(&b).await;
        assert!(!got.is_closed());
        let e = fails("dial to down", a.dial(&bg, b.id(), Vec::new(), ALPN).await);
        assert!(matches!(e, TransportError::MemUnreachable(_)));
        net.set_down(b.id(), false);

        net.set_delay(Duration::ZERO);
        let start = tokio::time::Instant::now();
        drop(dial(&a, b.id(), ALPN).await);
        assert!(start.elapsed() < delay);
    }

    #[tokio::test]
    async fn set_down_closes_the_nodes_conns() {
        let net = Network::new();
        let bg = Ctx::background();
        let a = net.bind(id(1), &[ALPN]);
        let b = net.bind(id(2), &[ALPN]);
        let c = net.bind(id(3), &[ALPN]);
        let (ab, ba) = connect(&a, &b, ALPN).await;
        let (ca, ac) = connect(&c, &a, ALPN).await;
        let (bc, cb) = connect(&b, &c, ALPN).await;

        net.set_down(a.id(), true);
        for (name, conn) in [("ab", &ab), ("ba", &ba), ("ca", &ca), ("ac", &ac)] {
            within(name, conn.closed()).await;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!bc.is_closed() && !cb.is_closed(), "other links stay up");
        let e = fails("local down", a.dial(&bg, c.id(), Vec::new(), ALPN).await);
        assert!(matches!(e, TransportError::MemLocalDown));
        eventually("a untracked", || tracked(&a) == 0).await;

        // Back up: dials work again, and closed conns stay closed.
        net.set_down(a.id(), false);
        let (ab2, _ba2) = connect(&a, &b, ALPN).await;
        assert!(!ab2.is_closed());
        assert!(ab.is_closed());
        drop(dial(&c, a.id(), ALPN).await);
    }

    #[tokio::test]
    async fn partition_closes_only_that_link() {
        let net = Network::new();
        let a = net.bind(id(1), &[ALPN]);
        let b = net.bind(id(2), &[ALPN]);
        let c = net.bind(id(3), &[ALPN]);
        let (ab, ba) = connect(&a, &b, ALPN).await;
        let (b_a, a_b) = connect(&b, &a, ALPN).await;
        let (ac, ca) = connect(&a, &c, ALPN).await;

        net.partition(a.id(), b.id(), true);
        for (name, conn) in [("ab", &ab), ("ba", &ba), ("b_a", &b_a), ("a_b", &a_b)] {
            within(name, conn.closed()).await;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!ac.is_closed() && !ca.is_closed());
        drop(connect(&c, &a, ALPN).await);

        net.partition(b.id(), a.id(), false);
        let (ab2, _) = connect(&a, &b, ALPN).await;
        let (ba2, _) = connect(&b, &a, ALPN).await;
        assert!(!ab2.is_closed() && !ba2.is_closed());
    }

    #[tokio::test]
    async fn endpoint_close_closes_conns_but_accept_still_drains() {
        let net = Network::new();
        let bg = Ctx::background();
        let a = net.bind(id(1), &[ALPN]);
        let b = net.bind(id(2), &[ALPN]);
        let (ab, ba) = connect(&a, &b, ALPN).await;
        let queued = dial(&a, b.id(), ALPN).await;

        b.close().await;
        assert!(ba.is_closed());
        within("ab closed", ab.closed()).await;
        within("queued closed", queued.closed()).await;
        assert_eq!(tracked(&b), 0);

        // Accept ignores the closed flag: the queued conn comes out, closed.
        let got = accept(&b).await;
        assert!(got.is_closed());
        let e = fails(
            "accept on closed endpoint",
            within(
                "accept",
                b.accept(&bg.with_timeout(Duration::from_millis(20))),
            )
            .await,
        );
        assert_eq!(ctx_err(&e), Some(CtxError::DeadlineExceeded));

        let e = fails(
            "dial from closed",
            b.dial(&bg, a.id(), Vec::new(), ALPN).await,
        );
        assert!(matches!(e, TransportError::Closed));
    }

    #[tokio::test]
    async fn bind_replaces_without_closing() {
        let net = Network::new();
        let bg = Ctx::background();
        let a = net.bind(id(1), &[]);
        let old = net.bind(id(2), &[ALPN]);
        let (to_old, at_old) = connect(&a, &old, ALPN).await;

        let new = net.bind(id(2), &[ALPN]);
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!to_old.is_closed() && !at_old.is_closed());
        let (mut cs, mut ss) = stream_pair(&to_old, &at_old).await;
        assert_eq!(write_once(&mut cs, b"old").await.ok(), Some(3));
        assert_eq!(read_once(&mut ss, 8).await.ok(), Some(b"old".to_vec()));

        let (_to_new, at_new) = connect(&a, &new, ALPN).await;
        assert_eq!(at_new.remote_id(), a.id());
        let e = fails(
            "old endpoint gets nothing",
            within(
                "accept",
                old.accept(&bg.with_timeout(Duration::from_millis(20))),
            )
            .await,
        );
        assert_eq!(ctx_err(&e), Some(CtxError::DeadlineExceeded));
    }

    #[tokio::test]
    async fn streams_opened_by_the_acceptor() {
        let net = Network::new();
        let a = net.bind(id(1), &[]);
        let b = net.bind(id(2), &[ALPN]);
        let (local, remote) = connect(&a, &b, ALPN).await;
        let (mut ss, mut cs) = stream_pair(&remote, &local).await;
        assert_eq!(write_once(&mut ss, b"srv").await.ok(), Some(3));
        assert_eq!(read_once(&mut cs, 8).await.ok(), Some(b"srv".to_vec()));
        assert_eq!(write_once(&mut cs, b"cli").await.ok(), Some(3));
        assert_eq!(read_once(&mut ss, 8).await.ok(), Some(b"cli".to_vec()));
        ss.close_stream();
        assert_eq!(read_once(&mut cs, 8).await.ok(), Some(Vec::new()), "EOF");
        let e = io_fails("write to closed stream", write_once(&mut cs, b"x").await);
        assert_eq!(e, "mem: stream reset by peer");
    }

    #[tokio::test]
    async fn blocked_reads_wake_on_fin_and_on_an_aborted_peer() {
        let net = Network::new();
        let a = net.bind(id(1), &[]);
        let b = net.bind(id(2), &[ALPN]);
        let (local, remote) = connect(&a, &b, ALPN).await;

        // A read blocked on an empty pipe returns EOF when the peer finishes.
        let (mut cs, mut ss) = stream_pair(&local, &remote).await;
        let reader = tokio::spawn(async move {
            let mut buf = [0u8; 8];
            let r = ss.recv.read(&mut buf).await;
            (r, ss)
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!reader.is_finished());
        cs.send.finish();
        match within("reader woken by FIN", reader).await {
            Ok((r, _ss)) => assert_eq!(r.ok(), Some(0)),
            Err(e) => panic!("reader task: {e}"),
        }

        // PORTING.md §5.1: aborting the task that holds a stream drops both halves. The peer's blocked
        // read returns EOF, and its writes fail because the dropped receive half cancelled its read.
        let (cs, ss) = stream_pair(&local, &remote).await;
        let holder = tokio::spawn(async move {
            let _held = cs;
            std::future::pending::<()>().await;
        });
        let reader = tokio::spawn(async move {
            let mut ss = ss;
            let mut buf = [0u8; 8];
            let r = ss.recv.read(&mut buf).await;
            (r, ss)
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!reader.is_finished());
        holder.abort();
        match within("reader woken by abort", reader).await {
            Ok((r, mut ss)) => {
                assert_eq!(r.ok(), Some(0), "EOF");
                let e = io_fails("write to aborted holder", write_once(&mut ss, b"x").await);
                assert_eq!(e, "mem: stream reset by peer");
            }
            Err(e) => panic!("reader task: {e}"),
        }
    }

    #[test]
    fn close_without_a_runtime_closes_the_peer_inline() {
        let net = Network::new();
        let a = net.bind(id(1), &[]);
        let b = net.bind(id(2), &[ALPN]);
        let (local, remote) = MemConn::pair(&a.reg, &b.reg, ALPN);
        a.reg.track(&local);
        b.reg.track(&remote);
        remote.close();
        assert!(remote.is_closed());
        assert!(local.is_closed());
        assert_eq!(tracked(&a), 0);
        assert_eq!(tracked(&b), 0);

        let (local, remote) = MemConn::pair(&a.reg, &b.reg, ALPN);
        a.reg.track(&local);
        b.reg.track(&remote);
        net.set_down(b.id(), true);
        assert!(local.is_closed() && remote.is_closed());
        assert_eq!(tracked(&a) + tracked(&b), 0);
    }
}
