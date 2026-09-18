//! `transport.Pool` (`transport/transport.go:63-243`): grow to `per_peer`, round robin, the 2 s
//! failed-dial window only with no live conn, only `open_stream` failures drop, never shrink. The dial
//! lock is a `tokio::sync::Mutex<()>` awaited outside `ctx.run` (not ctx-aware, as in Go).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::{AddrsFn, CallError, Conn, Ctx, Endpoint, NodeId, PathInfo, Stream, TransportError};

/// A failed dial refuses further dials for this long while the peer has no live connection
/// (`transport.go:113-116`).
const FAILED_DIAL_WINDOW: Duration = Duration::from_secs(2);

/// `transport.Pool`.
pub struct Pool {
    ep: Arc<dyn Endpoint>,
    addrs: AddrsFn,
    per_peer: usize,
    state: Mutex<PoolState>,
}

/// (peer, ALPN).
type PeerKey = (NodeId, String);

#[derive(Default)]
struct PoolState {
    conns: HashMap<PeerKey, Vec<Arc<dyn Conn>>>,
    /// Round-robin counters; never reset.
    next: HashMap<PeerKey, usize>,
    /// Per-key dial locks; never deleted.
    dialing: HashMap<PeerKey, Arc<tokio::sync::Mutex<()>>>,
    /// When the last dial failed. The tokio clock, so paused-time tests apply.
    failed: HashMap<PeerKey, tokio::time::Instant>,
}

/// Runs `wire.CloseStream` when a `call` ends, whichever way it ends (including when its future is
/// dropped): Go's `defer wire.CloseStream(s)`.
struct ClosingStream(Stream);

impl Drop for ClosingStream {
    fn drop(&mut self) {
        self.0.close_stream();
    }
}

impl Pool {
    /// `transport.NewPool`; `per_peer` 0 → 1.
    pub fn new(ep: Arc<dyn Endpoint>, addrs: AddrsFn, per_peer: usize) -> Pool {
        Pool {
            ep,
            addrs,
            per_peer: per_peer.max(1),
            state: Mutex::new(PoolState::default()),
        }
    }

    pub fn endpoint(&self) -> &Arc<dyn Endpoint> {
        &self.ep
    }

    /// The pool lock. The state stays consistent under panics (every update is a single map operation),
    /// so a poisoned lock is recovered.
    fn state(&self) -> MutexGuard<'_, PoolState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `Pool.Get`.
    pub async fn get(
        &self,
        ctx: &Ctx,
        id: NodeId,
        alpn: &str,
    ) -> Result<Arc<dyn Conn>, TransportError> {
        let k: PeerKey = (id, alpn.to_string());
        let dm = {
            let mut guard = self.state();
            let st = &mut *guard;
            // Filter the conns in place, keeping those not closed.
            let live = st.conns.entry(k.clone()).or_default();
            live.retain(|c| !c.is_closed());
            let n = live.len();
            if n >= self.per_peer {
                let next = st.next.entry(k.clone()).or_insert(0);
                let i = *next % n;
                *next = next.wrapping_add(1);
                if let Some(c) = live.get(i) {
                    return Ok(c.clone());
                }
            }
            if n == 0
                && st
                    .failed
                    .get(&k)
                    .is_some_and(|t| t.elapsed() < FAILED_DIAL_WINDOW)
            {
                return Err(TransportError::RecentlyUnreachable);
            }
            st.dialing.entry(k.clone()).or_default().clone()
        };

        // Not ctx-aware: a waiting get blocks until the running dial finishes, whatever its ctx.
        let _dialing = dm.lock().await;
        {
            let st = self.state();
            // The unfiltered list, and no round robin.
            if let Some(conns) = st.conns.get(&k)
                && conns.len() >= self.per_peer
                && let Some(c) = conns.first()
            {
                return Ok(c.clone());
            }
        }
        let addrs = (self.addrs)(id);
        match self.ep.dial(ctx, id, addrs, alpn).await {
            Err(e) => {
                self.state().failed.insert(k, tokio::time::Instant::now());
                Err(e)
            }
            Ok(c) => {
                let mut st = self.state();
                st.failed.remove(&k);
                st.conns.entry(k).or_default().push(c.clone());
                Ok(c)
            }
        }
    }

    /// Go `Drop`: forgets every conn of the key and its failed-dial mark, then closes the conns. The
    /// round-robin counter and the dial lock are kept.
    pub fn drop_peer(&self, id: NodeId, alpn: &str) {
        let k: PeerKey = (id, alpn.to_string());
        let conns = {
            let mut st = self.state();
            st.failed.remove(&k);
            st.conns.remove(&k)
        };
        for c in conns.into_iter().flatten() {
            c.close();
        }
    }

    /// The path of the first live conn.
    pub fn path(&self, id: NodeId, alpn: &str) -> Option<PathInfo> {
        let st = self.state();
        st.conns
            .get(&(id, alpn.to_string()))?
            .iter()
            .find(|c| !c.is_closed())
            .map(|c| c.path())
    }

    /// Go `Close`: closes every conn and forgets them. Failed-dial marks, counters and dial locks are kept,
    /// and the pool stays usable.
    pub fn close(&self) {
        let all = std::mem::take(&mut self.state().conns);
        for c in all.into_values().flatten() {
            c.close();
        }
    }

    /// Go `Pool.Call`: get; open_stream (error → drop_peer); write_msg; close_write; read one frame under
    /// ctx (ctx end → cancel_read(0) + finish, CtxError); TErr → Remote; close_stream on every exit.
    pub async fn call(
        &self,
        ctx: &Ctx,
        id: NodeId,
        alpn: &str,
        req: &dstore_wire::Msg,
    ) -> Result<dstore_wire::Msg, CallError> {
        let c = self
            .get(ctx, id, alpn)
            .await
            .map_err(CallError::Transport)?;
        let s = match c.open_stream(ctx).await {
            Ok(s) => s,
            Err(e) => {
                self.drop_peer(id, alpn);
                return Err(CallError::Transport(e));
            }
        };
        let mut s = ClosingStream(s);
        exchange(ctx, &mut s.0, req).await
    }

    /// `Pool.Open`: the caller owns the stream.
    pub async fn open(&self, ctx: &Ctx, id: NodeId, alpn: &str) -> Result<Stream, TransportError> {
        let c = self.get(ctx, id, alpn).await?;
        match c.open_stream(ctx).await {
            Ok(s) => Ok(s),
            Err(e) => {
                self.drop_peer(id, alpn);
                Err(e)
            }
        }
    }
}

/// The request/reply half of `Pool.Call`. Only `open_stream` failures drop the peer, so nothing here does.
async fn exchange(
    ctx: &Ctx,
    s: &mut Stream,
    req: &dstore_wire::Msg,
) -> Result<dstore_wire::Msg, CallError> {
    dstore_wire::write_msg(&mut *s.send, req)
        .await
        .map_err(CallError::Wire)?;
    s.close_write();
    // `read_msg` is not cancel-safe: when ctx ends the read is abandoned with the stream.
    let reply = match ctx.run(dstore_wire::read_msg(&mut *s.recv)).await {
        Ok(r) => r.map_err(CallError::Wire)?,
        Err(e) => {
            s.cancel_read(0);
            s.send.finish();
            return Err(CallError::Ctx(e));
        }
    };
    if reply.typ == dstore_wire::T_ERR {
        return Err(CallError::Remote(dstore_wire::error_from_msg(&reply)));
    }
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io;
    use std::pin::{Pin, pin};
    use std::task::{Context, Poll};

    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    use tokio::sync::Semaphore;
    use tokio_util::sync::CancellationToken;

    use crate::{RecvStream, SendStream};

    const ALPN: &str = "amber-dstore/1";

    /// `{0: 105, 3: 7}`: TPing epoch 7.
    const PING_FRAME: [u8; 10] = [0, 0, 0, 6, 0xa2, 0x00, 0x18, 0x69, 0x03, 0x07];
    /// `{0: 106, 3: 8}`: TPong epoch 8.
    const PONG_FRAME: [u8; 10] = [0, 0, 0, 6, 0xa2, 0x00, 0x18, 0x6a, 0x03, 0x08];
    /// `{0: 10, 10: "busy", 11: "slow down", 28: 1500}`.
    const TERR_FRAME: [u8; 29] = [
        0, 0, 0, 25, 0xa4, 0x00, 0x0a, 0x0a, 0x64, b'b', b'u', b's', b'y', 0x0b, 0x69, b's', b'l',
        b'o', b'w', b' ', b'd', b'o', b'w', b'n', 0x18, 0x1c, 0x19, 0x05, 0xdc,
    ];

    fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn node(b: u8) -> NodeId {
        NodeId([b; 32])
    }

    fn ping() -> dstore_wire::Msg {
        dstore_wire::Msg {
            typ: dstore_wire::T_PING,
            epoch: 7,
            ..Default::default()
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Ev {
        Write(Vec<u8>),
        Finish,
        Cancel(u64),
    }

    type Log = Arc<Mutex<Vec<Ev>>>;

    /// What the peer does with a stream.
    #[derive(Clone)]
    enum Peer {
        /// These bytes can be read, then EOF.
        Reply(Vec<u8>),
        /// Reads never complete.
        Silent,
        /// Writes fail.
        WriteFails,
    }

    struct FakeSend {
        log: Log,
        fail: bool,
    }

    impl AsyncWrite for FakeSend {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            if self.fail {
                return Poll::Ready(Err(io::Error::other("mem: stream reset by peer")));
            }
            lock(&self.log).push(Ev::Write(buf.to_vec()));
            Poll::Ready(Ok(buf.len()))
        }
        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    impl SendStream for FakeSend {
        fn finish(&mut self) {
            lock(&self.log).push(Ev::Finish);
        }
    }

    struct FakeRecv {
        log: Log,
        data: Vec<u8>,
        pos: usize,
        silent: bool,
    }

    impl AsyncRead for FakeRecv {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let this = self.get_mut();
            if this.silent {
                return Poll::Pending;
            }
            let rest = this.data.get(this.pos..).unwrap_or_default();
            let n = rest.len().min(buf.remaining());
            buf.put_slice(rest.get(..n).unwrap_or_default());
            this.pos += n;
            Poll::Ready(Ok(()))
        }
    }

    impl RecvStream for FakeRecv {
        fn cancel_read(&mut self, code: u64) {
            lock(&self.log).push(Ev::Cancel(code));
        }
    }

    struct FakeConn {
        remote: NodeId,
        alpn: String,
        index: usize,
        peer: Peer,
        open_err: Mutex<Option<TransportError>>,
        closed: CancellationToken,
        streams: Mutex<Vec<Log>>,
    }

    impl FakeConn {
        fn stream_log(&self, i: usize) -> Vec<Ev> {
            match lock(&self.streams).get(i) {
                Some(l) => lock(l).clone(),
                None => panic!("no stream {i}"),
            }
        }
    }

    #[async_trait::async_trait]
    impl Conn for FakeConn {
        fn remote_id(&self) -> NodeId {
            self.remote
        }
        fn alpn(&self) -> String {
            self.alpn.clone()
        }
        async fn open_stream(&self, _ctx: &Ctx) -> Result<Stream, TransportError> {
            if let Some(e) = lock(&self.open_err).clone() {
                return Err(e);
            }
            let log = Log::default();
            lock(&self.streams).push(log.clone());
            let (data, silent, fail) = match &self.peer {
                Peer::Reply(d) => (d.clone(), false, false),
                Peer::Silent => (Vec::new(), true, false),
                Peer::WriteFails => (Vec::new(), false, true),
            };
            Ok(Stream::new(
                Box::new(FakeSend {
                    log: log.clone(),
                    fail,
                }),
                Box::new(FakeRecv {
                    log,
                    data,
                    pos: 0,
                    silent,
                }),
            ))
        }
        async fn accept_stream(&self, _ctx: &Ctx) -> Result<Stream, TransportError> {
            Err(TransportError::Closed)
        }
        fn close(&self) {
            self.closed.cancel();
        }
        fn path(&self) -> PathInfo {
            PathInfo {
                direct: self.index.is_multiple_of(2),
                rtt: Duration::from_millis(self.index as u64 + 1),
            }
        }
        fn is_closed(&self) -> bool {
            self.closed.is_cancelled()
        }
        async fn closed(&self) {
            self.closed.cancelled().await
        }
    }

    type Attempt = (NodeId, Vec<String>, String);

    struct FakeEndpoint {
        peer: Peer,
        gate: Option<Arc<Semaphore>>,
        attempts: Mutex<Vec<Attempt>>,
        fail_next: Mutex<Option<TransportError>>,
        conns: Mutex<Vec<Arc<FakeConn>>>,
    }

    impl FakeEndpoint {
        fn new(peer: Peer) -> Arc<FakeEndpoint> {
            Arc::new(FakeEndpoint {
                peer,
                gate: None,
                attempts: Mutex::new(Vec::new()),
                fail_next: Mutex::new(None),
                conns: Mutex::new(Vec::new()),
            })
        }

        /// Every dial waits for a permit of the returned semaphore.
        fn gated() -> (Arc<FakeEndpoint>, Arc<Semaphore>) {
            let gate = Arc::new(Semaphore::new(0));
            let ep = Arc::new(FakeEndpoint {
                peer: Peer::Silent,
                gate: Some(gate.clone()),
                attempts: Mutex::new(Vec::new()),
                fail_next: Mutex::new(None),
                conns: Mutex::new(Vec::new()),
            });
            (ep, gate)
        }

        fn attempts(&self) -> usize {
            lock(&self.attempts).len()
        }

        fn conn(&self, i: usize) -> Arc<FakeConn> {
            match lock(&self.conns).get(i) {
                Some(c) => c.clone(),
                None => panic!("no conn {i}"),
            }
        }

        /// The dial number of a conn the pool returned.
        fn index(&self, c: &Arc<dyn Conn>) -> usize {
            let want = Arc::as_ptr(c) as *const ();
            match lock(&self.conns)
                .iter()
                .position(|x| Arc::as_ptr(x) as *const () == want)
            {
                Some(i) => i,
                None => panic!("the pool returned a conn the endpoint never dialed"),
            }
        }

        fn fail_next(&self, text: &str) {
            *lock(&self.fail_next) = Some(TransportError::Quic(text.to_string()));
        }
    }

    #[async_trait::async_trait]
    impl Endpoint for FakeEndpoint {
        fn id(&self) -> NodeId {
            node(0)
        }
        async fn dial(
            &self,
            _ctx: &Ctx,
            id: NodeId,
            addrs: Vec<String>,
            alpn: &str,
        ) -> Result<Arc<dyn Conn>, TransportError> {
            lock(&self.attempts).push((id, addrs, alpn.to_string()));
            if let Some(gate) = &self.gate {
                match gate.acquire().await {
                    Ok(permit) => permit.forget(),
                    Err(_) => return Err(TransportError::Closed),
                }
            }
            if let Some(e) = lock(&self.fail_next).take() {
                return Err(e);
            }
            let mut conns = lock(&self.conns);
            let c = Arc::new(FakeConn {
                remote: id,
                alpn: alpn.to_string(),
                index: conns.len(),
                peer: self.peer.clone(),
                open_err: Mutex::new(None),
                closed: CancellationToken::new(),
                streams: Mutex::new(Vec::new()),
            });
            conns.push(c.clone());
            Ok(c)
        }
        async fn accept(&self, _ctx: &Ctx) -> Result<Arc<dyn Conn>, TransportError> {
            Err(TransportError::Closed)
        }
        fn addrs(&self) -> Vec<String> {
            Vec::new()
        }
        async fn close(&self) {}
    }

    fn pool(ep: &Arc<FakeEndpoint>, per_peer: usize) -> Pool {
        let ep: Arc<dyn Endpoint> = ep.clone();
        Pool::new(ep, Arc::new(|_| Vec::new()), per_peer)
    }

    async fn get(p: &Pool, ep: &FakeEndpoint, peer: NodeId) -> usize {
        match p.get(&Ctx::background(), peer, ALPN).await {
            Ok(c) => ep.index(&c),
            Err(e) => panic!("get: {e}"),
        }
    }

    async fn get_err(p: &Pool, peer: NodeId) -> String {
        match p.get(&Ctx::background(), peer, ALPN).await {
            Ok(_) => panic!("get succeeded"),
            Err(e) => e.to_string(),
        }
    }

    fn assert_send<T: Send>(_: T) {}

    #[test]
    fn futures_are_send() {
        let ep = FakeEndpoint::new(Peer::Silent);
        let p = pool(&ep, 1);
        let ctx = Ctx::background();
        let req = ping();
        assert_send(p.get(&ctx, node(1), ALPN));
        assert_send(p.open(&ctx, node(1), ALPN));
        assert_send(p.call(&ctx, node(1), ALPN, &req));
    }

    #[test]
    fn new_turns_zero_per_peer_into_one() {
        let ep = FakeEndpoint::new(Peer::Silent);
        assert_eq!(pool(&ep, 0).per_peer, 1);
        assert_eq!(pool(&ep, 4).per_peer, 4);
        let p = pool(&ep, 1);
        assert_eq!(p.endpoint().id(), node(0));
    }

    // pool_scripts per_peer_zero_means_one.
    #[tokio::test]
    async fn per_peer_zero_dials_once() {
        let ep = FakeEndpoint::new(Peer::Silent);
        let p = pool(&ep, 0);
        for _ in 0..3 {
            assert_eq!(get(&p, &ep, node(1)).await, 0);
        }
        assert_eq!(ep.attempts(), 1);
    }

    // transport §5.9 (1): every get dials until per_peer conns are live, then round robin.
    #[tokio::test]
    async fn grows_to_per_peer_then_round_robin() {
        let ep = FakeEndpoint::new(Peer::Silent);
        let p = pool(&ep, 4);
        let mut got = Vec::new();
        for _ in 0..7 {
            got.push(get(&p, &ep, node(1)).await);
        }
        assert_eq!(got, [0, 1, 2, 3, 0, 1, 2]);
        assert_eq!(ep.attempts(), 4);
        // Other keys have their own conns.
        assert_eq!(
            match p
                .get(&Ctx::background(), node(1), "amber-dstore-cluster/1")
                .await
            {
                Ok(c) => ep.index(&c),
                Err(e) => panic!("get: {e}"),
            },
            4
        );
        assert_eq!(get(&p, &ep, node(2)).await, 5);
    }

    // transport §5.9 (2): a failed dial with no live conn refuses dials for 2 s without dialing.
    #[tokio::test(start_paused = true)]
    async fn failed_dial_refuses_for_two_seconds_without_dialing() {
        let ep = FakeEndpoint::new(Peer::Silent);
        let p = pool(&ep, 2);
        ep.fail_next("mem: 62297a9a unreachable");
        assert_eq!(get_err(&p, node(1)).await, "mem: 62297a9a unreachable");
        assert_eq!(
            get_err(&p, node(1)).await,
            "transport: peer recently unreachable"
        );
        assert_eq!(ep.attempts(), 1);
        tokio::time::advance(Duration::from_millis(1999)).await;
        assert_eq!(
            get_err(&p, node(1)).await,
            "transport: peer recently unreachable"
        );
        assert_eq!(ep.attempts(), 1);
        // time.Since(failed) < 2s: at exactly 2 s the pool dials again.
        tokio::time::advance(Duration::from_millis(1)).await;
        assert_eq!(get(&p, &ep, node(1)).await, 0);
        assert_eq!(ep.attempts(), 2);
        // A successful dial clears the mark: with no live conn the next get dials at once.
        ep.conn(0).close();
        assert_eq!(get(&p, &ep, node(1)).await, 1);
        assert_eq!(ep.attempts(), 3);
        // The window is per key.
        ep.fail_next("x");
        assert_eq!(get_err(&p, node(2)).await, "x");
        assert_eq!(get(&p, &ep, node(1)).await, 2);
    }

    // pool_scripts failed_dial_ignored_with_live_conn.
    #[tokio::test]
    async fn failed_dial_is_ignored_with_a_live_conn() {
        let ep = FakeEndpoint::new(Peer::Silent);
        let p = pool(&ep, 2);
        assert_eq!(get(&p, &ep, node(1)).await, 0);
        ep.fail_next("scripted dial failure");
        assert_eq!(get_err(&p, node(1)).await, "scripted dial failure");
        assert_eq!(get(&p, &ep, node(1)).await, 1);
        assert_eq!(ep.attempts(), 3);
        let mut got = Vec::new();
        for _ in 0..3 {
            got.push(get(&p, &ep, node(1)).await);
        }
        assert_eq!(got, [0, 1, 0]);
        assert_eq!(ep.attempts(), 3);
    }

    // pool_scripts closed_conns_are_filtered.
    #[tokio::test]
    async fn closed_conns_are_filtered_and_the_counter_is_never_reset() {
        let ep = FakeEndpoint::new(Peer::Silent);
        let p = pool(&ep, 2);
        let mut got = Vec::new();
        for _ in 0..4 {
            got.push(get(&p, &ep, node(1)).await);
        }
        assert_eq!(got, [0, 1, 0, 1]);
        ep.conn(0).close();
        // One live conn below per_peer: dial, with no failure window involved.
        assert_eq!(get(&p, &ep, node(1)).await, 2);
        // Round robin over [1, 2] continues from counter 2.
        let mut got = Vec::new();
        for _ in 0..3 {
            got.push(get(&p, &ep, node(1)).await);
        }
        assert_eq!(got, [1, 2, 1]);
        assert_eq!(ep.attempts(), 3);
    }

    // transport.go:149-187: Drop and Close forget the conns but keep the round-robin counter.
    #[tokio::test]
    async fn drop_and_close_keep_the_round_robin_counter() {
        let ep = FakeEndpoint::new(Peer::Silent);
        let p = pool(&ep, 2);
        let mut got = Vec::new();
        for _ in 0..3 {
            got.push(get(&p, &ep, node(1)).await);
        }
        p.drop_peer(node(1), ALPN);
        for _ in 0..4 {
            got.push(get(&p, &ep, node(1)).await);
        }
        p.close();
        for _ in 0..3 {
            got.push(get(&p, &ep, node(1)).await);
        }
        // A reset counter would give [.., 2, 3, 2, 3, 4, 5, 4].
        assert_eq!(got, [0, 1, 0, 2, 3, 3, 2, 4, 5, 5]);
        assert_eq!(ep.attempts(), 6);
    }

    // transport §5.9 (3).
    #[tokio::test]
    async fn drop_peer_closes_and_forgets_conns_and_failures() {
        let ep = FakeEndpoint::new(Peer::Silent);
        let p = pool(&ep, 1);
        assert_eq!(get(&p, &ep, node(1)).await, 0);
        assert!(p.path(node(1), ALPN).is_some());
        p.drop_peer(node(1), ALPN);
        assert!(ep.conn(0).is_closed());
        assert_eq!(p.path(node(1), ALPN), None);
        assert_eq!(get(&p, &ep, node(1)).await, 1);
        assert_eq!(ep.attempts(), 2);

        ep.fail_next("x");
        assert_eq!(get_err(&p, node(2)).await, "x");
        assert_eq!(
            get_err(&p, node(2)).await,
            "transport: peer recently unreachable"
        );
        p.drop_peer(node(2), ALPN);
        assert_eq!(get(&p, &ep, node(2)).await, 2);
        // Dropping an unknown key is a no-op.
        p.drop_peer(node(9), ALPN);
    }

    #[tokio::test]
    async fn path_reports_the_first_live_conn() {
        let ep = FakeEndpoint::new(Peer::Silent);
        let p = pool(&ep, 2);
        assert_eq!(p.path(node(1), ALPN), None);
        get(&p, &ep, node(1)).await;
        get(&p, &ep, node(1)).await;
        assert_eq!(p.path(node(1), ALPN), Some(ep.conn(0).path()));
        ep.conn(0).close();
        assert_eq!(p.path(node(1), ALPN), Some(ep.conn(1).path()));
        ep.conn(1).close();
        assert_eq!(p.path(node(1), ALPN), None);
        assert_eq!(p.path(node(1), "other/1"), None);
    }

    #[tokio::test]
    async fn close_closes_every_conn_and_keeps_failures() {
        let ep = FakeEndpoint::new(Peer::Silent);
        let p = pool(&ep, 1);
        assert_eq!(get(&p, &ep, node(1)).await, 0);
        assert_eq!(get(&p, &ep, node(2)).await, 1);
        ep.fail_next("x");
        assert_eq!(get_err(&p, node(3)).await, "x");
        p.close();
        assert!(ep.conn(0).is_closed());
        assert!(ep.conn(1).is_closed());
        assert_eq!(p.path(node(1), ALPN), None);
        // The pool stays usable.
        assert_eq!(get(&p, &ep, node(1)).await, 2);
        assert_eq!(
            get_err(&p, node(3)).await,
            "transport: peer recently unreachable"
        );
    }

    #[tokio::test]
    async fn dial_gets_the_addrs_of_the_peer() {
        let ep = FakeEndpoint::new(Peer::Silent);
        let dyn_ep: Arc<dyn Endpoint> = ep.clone();
        let p = Pool::new(
            dyn_ep,
            Arc::new(|id: NodeId| vec![format!("ip:10.0.0.{}:1", id.0[0])]),
            1,
        );
        let r = p.get(&Ctx::background(), node(7), "x/1").await;
        assert!(r.is_ok());
        assert_eq!(
            lock(&ep.attempts).clone(),
            [(
                node(7),
                vec!["ip:10.0.0.7:1".to_string()],
                "x/1".to_string()
            )]
        );
    }

    // transport.go:124-130: after the dial lock, the check runs against the unfiltered list and returns
    // element 0, even a closed conn, without round robin.
    #[tokio::test]
    async fn second_check_uses_the_unfiltered_list() {
        let (ep, gate) = FakeEndpoint::gated();
        let p = pool(&ep, 2);
        gate.add_permits(1);
        assert_eq!(get(&p, &ep, node(1)).await, 0);

        let ctx = Ctx::background();
        let mut b = pin!(p.get(&ctx, node(1), ALPN));
        assert!(
            futures::poll!(&mut b).is_pending(),
            "b dials and waits for the gate"
        );
        assert_eq!(ep.attempts(), 2);
        let mut c = pin!(p.get(&ctx, node(1), ALPN));
        assert!(
            futures::poll!(&mut c).is_pending(),
            "c waits for the dial lock"
        );
        assert_eq!(ep.attempts(), 2);

        ep.conn(0).close();
        gate.add_permits(1);
        match b.await {
            Ok(conn) => assert_eq!(ep.index(&conn), 1),
            Err(e) => panic!("b: {e}"),
        }
        match c.await {
            Ok(conn) => {
                assert_eq!(ep.index(&conn), 0);
                assert!(conn.is_closed());
            }
            Err(e) => panic!("c: {e}"),
        }
        assert_eq!(ep.attempts(), 2);
    }

    // A get waiting for the dial lock ignores its ctx (transport.go:124).
    #[tokio::test]
    async fn dial_lock_is_not_ctx_aware() {
        let (ep, gate) = FakeEndpoint::gated();
        let p = pool(&ep, 1);
        let bg = Ctx::background();
        let mut a = pin!(p.get(&bg, node(1), ALPN));
        assert!(futures::poll!(&mut a).is_pending());
        let cancelled = Ctx::background().with_cancel();
        cancelled.cancel();
        let mut b = pin!(p.get(&cancelled, node(1), ALPN));
        assert!(futures::poll!(&mut b).is_pending());
        gate.add_permits(1);
        match a.await {
            Ok(conn) => assert_eq!(ep.index(&conn), 0),
            Err(e) => panic!("a: {e}"),
        }
        match b.await {
            Ok(conn) => assert_eq!(ep.index(&conn), 0),
            Err(e) => panic!("b: {e}"),
        }
        assert_eq!(ep.attempts(), 1);
    }

    #[tokio::test]
    async fn open_returns_the_stream_and_open_stream_failures_drop() {
        let ep = FakeEndpoint::new(Peer::Silent);
        let p = pool(&ep, 1);
        let bg = Ctx::background();
        assert!(p.open(&bg, node(1), ALPN).await.is_ok());
        assert!(!ep.conn(0).is_closed());
        *lock(&ep.conn(0).open_err) = Some(TransportError::Ctx(
            dstore_gocompat::ctx::CtxError::DeadlineExceeded,
        ));
        match p.open(&bg, node(1), ALPN).await {
            Ok(_) => panic!("open succeeded"),
            Err(e) => assert_eq!(e.to_string(), "context deadline exceeded"),
        }
        assert!(ep.conn(0).is_closed());
        assert_eq!(p.path(node(1), ALPN), None);
        assert_eq!(get(&p, &ep, node(1)).await, 1);

        ep.fail_next("boom");
        match p.open(&bg, node(2), ALPN).await {
            Ok(_) => panic!("open succeeded"),
            Err(e) => assert_eq!(e.to_string(), "boom"),
        }
    }

    #[tokio::test]
    async fn call_returns_get_and_open_stream_failures() {
        let ep = FakeEndpoint::new(Peer::Silent);
        let p = pool(&ep, 1);
        let bg = Ctx::background();
        ep.fail_next("boom");
        match p.call(&bg, node(1), ALPN, &ping()).await {
            Err(CallError::Transport(e)) => assert_eq!(e.to_string(), "boom"),
            other => panic!("call: {other:?}"),
        }
        assert_eq!(get(&p, &ep, node(2)).await, 0);
        *lock(&ep.conn(0).open_err) = Some(TransportError::Closed);
        match p.call(&bg, node(2), ALPN, &ping()).await {
            Err(CallError::Transport(e)) => assert_eq!(e.to_string(), "transport: closed"),
            other => panic!("call: {other:?}"),
        }
        assert!(
            ep.conn(0).is_closed(),
            "an open_stream failure drops the peer"
        );
    }

    // transport §5.9 (5) and pool_scripts call_reply: one frame each way; the conn stays pooled.
    #[tokio::test]
    async fn call_reply_one_frame_each_way() {
        let ep = FakeEndpoint::new(Peer::Reply(PONG_FRAME.to_vec()));
        let p = pool(&ep, 1);
        match p.call(&Ctx::background(), node(1), ALPN, &ping()).await {
            Ok(reply) => {
                assert_eq!(reply.typ, dstore_wire::T_PONG);
                assert_eq!(reply.epoch, 8);
            }
            Err(e) => panic!("call: {e}"),
        }
        assert_eq!(
            ep.conn(0).stream_log(0),
            [
                Ev::Write(PING_FRAME.to_vec()),
                Ev::Finish,
                Ev::Finish,
                Ev::Cancel(0)
            ]
        );
        assert!(!ep.conn(0).is_closed());
        assert_eq!(get(&p, &ep, node(1)).await, 0);
        assert_eq!(ep.attempts(), 1);
    }

    // pool_scripts call_remote_error.
    #[tokio::test]
    async fn call_terr_is_a_remote_error() {
        let ep = FakeEndpoint::new(Peer::Reply(TERR_FRAME.to_vec()));
        let p = pool(&ep, 1);
        match p.call(&Ctx::background(), node(1), ALPN, &ping()).await {
            Err(CallError::Remote(r)) => {
                assert_eq!(r.code, "busy");
                assert_eq!(r.text, "slow down");
                assert_eq!(r.retry_after, Duration::from_millis(1500));
                assert_eq!(CallError::Remote(r).to_string(), "remote: busy: slow down");
            }
            other => panic!("call: {other:?}"),
        }
        assert_eq!(ep.conn(0).stream_log(0).last(), Some(&Ev::Cancel(0)));
        assert!(!ep.conn(0).is_closed());
    }

    // pool_scripts call_eof_does_not_drop.
    #[tokio::test]
    async fn call_read_error_does_not_drop() {
        let ep = FakeEndpoint::new(Peer::Reply(Vec::new()));
        let p = pool(&ep, 1);
        match p.call(&Ctx::background(), node(1), ALPN, &ping()).await {
            Err(CallError::Wire(e)) => assert_eq!(e.to_string(), "EOF"),
            other => panic!("call: {other:?}"),
        }
        assert_eq!(
            ep.conn(0).stream_log(0),
            [
                Ev::Write(PING_FRAME.to_vec()),
                Ev::Finish,
                Ev::Finish,
                Ev::Cancel(0)
            ]
        );
        assert!(!ep.conn(0).is_closed());
        assert_eq!(get(&p, &ep, node(1)).await, 0);
    }

    #[tokio::test]
    async fn call_write_error_does_not_drop() {
        let ep = FakeEndpoint::new(Peer::WriteFails);
        let p = pool(&ep, 1);
        match p.call(&Ctx::background(), node(1), ALPN, &ping()).await {
            Err(CallError::Wire(e)) => assert_eq!(e.to_string(), "mem: stream reset by peer"),
            other => panic!("call: {other:?}"),
        }
        assert_eq!(ep.conn(0).stream_log(0), [Ev::Finish, Ev::Cancel(0)]);
        assert!(!ep.conn(0).is_closed());
    }

    // transport §5.9 (4) and pool_scripts call_ctx_deadline.
    #[tokio::test(start_paused = true)]
    async fn call_ctx_end_cancels_the_read_and_keeps_the_conn() {
        let ep = FakeEndpoint::new(Peer::Silent);
        let p = pool(&ep, 1);
        let ctx = Ctx::background().with_timeout(Duration::from_millis(50));
        match p.call(&ctx, node(1), ALPN, &ping()).await {
            Err(CallError::Ctx(e)) => assert_eq!(e.to_string(), "context deadline exceeded"),
            other => panic!("call: {other:?}"),
        }
        assert_eq!(
            ep.conn(0).stream_log(0),
            [
                Ev::Write(PING_FRAME.to_vec()),
                Ev::Finish,
                Ev::Cancel(0),
                Ev::Finish,
                Ev::Finish,
                Ev::Cancel(0)
            ]
        );
        assert!(!ep.conn(0).is_closed());
        assert_eq!(get(&p, &ep, node(1)).await, 0);
    }
}
