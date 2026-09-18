//! `client/watch.go`: `WatchRefs`.
//!
//! The generator runs on the consumer's poll (Go's range-over-func pull semantics): nothing happens while
//! the consumer is not polling, and the idle timer keeps running while it handles an event. One watch
//! stream is a [`WatchSession`]: the request is written without FIN, frames are read by a spawned task
//! into a channel of one (`read_msg` is not cancel-safe), and dropping the session is Go's deferred
//! `wire.CloseStream` plus the reader's cancellation.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use dstore_gocompat::slog::Attr;
use dstore_gocompat::time::duration_to_ns;
use dstore_transport::{RecvStream, SendStream, Stream};
use dstore_view::short_id;
use dstore_wire::{
    ALPN_CLIENT, CODE_BAD_REQUEST, CODE_STALE_VIEW, CODE_UNAUTHORIZED, Msg, RefInfo, T_ERR,
    T_REF_CHANGES, T_REF_SYNCED, T_REF_WATCH, WireError,
};
use rand::Rng;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::{Cluster, Ctx, Error, NodeId};

/// The delay logged and waited after the first round in which no node served the watch.
pub(crate) const WATCH_DELAY_START: Duration = Duration::from_secs(1);
/// The delay doubles up to this.
pub(crate) const WATCH_DELAY_MAX: Duration = Duration::from_secs(30);
/// The pause after a stream that synced and then died.
pub(crate) const WATCH_SERVED_PAUSE: Duration = Duration::from_millis(200);
/// The bound of the view refresh of a round no node served.
pub(crate) const WATCH_REFRESH_TIMEOUT: Duration = Duration::from_secs(15);
/// `watchOnce` calls per node while the node answers `stale-view`.
const WATCH_ATTEMPTS: usize = 4;

/// `client.RefChange`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RefChange {
    pub name: String,
    pub key: Option<Vec<u8>>,
    pub version: Vec<u8>,
    pub created_at: i64,
    pub user: String,
    pub deleted: bool,
    pub synced: bool,
    pub node: NodeId,
}

/// The stream `watch_refs` returns.
pub type WatchStream =
    std::pin::Pin<Box<dyn futures::Stream<Item = Result<RefChange, Error>> + Send + 'static>>;

/// `watchResult`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WatchResult {
    /// Nothing came of this node; try the next.
    Failed,
    /// Stale view adopted; retry this node.
    Retry,
    /// The stream synced and then died; reconnect.
    Served,
    /// The ctx ended.
    Stop,
    /// A terminal error was yielded.
    Fatal,
}

/// What one step of a session produced.
enum Step {
    Event(RefChange),
    End(WatchResult),
    Fatal(Error),
}

/// An event of the current frame, not yet yielded.
enum Pending {
    Change(RefInfo),
    Deleted(String),
    Synced,
}

/// The names the watcher holds and their keys (nil keys kept apart from empty ones).
type State = HashMap<String, Option<Vec<u8>>>;

impl Cluster {
    /// `WatchRefs`: an async-stream generator (pull semantics). Ends without an item when ctx ends; yields
    /// Err for bad-request/unauthorized, then ends.
    ///
    /// `known` is copied when this is called. An empty key stands for Go's nil key (CBOR null in the known
    /// list).
    pub fn watch_refs(
        &self,
        ctx: Ctx,
        pattern: String,
        known: HashMap<String, Vec<u8>>,
    ) -> WatchStream {
        let c = self.clone();
        let mut state: State = known
            .into_iter()
            .map(|(name, key)| (name, (!key.is_empty()).then_some(key)))
            .collect();
        Box::pin(async_stream::stream! {
            let mut delay = WATCH_DELAY_START;
            while ctx.err().is_none() {
                let mut served = false;
                for id in c.preferred(&c.all_nodes()) {
                    let mut attempts = 0;
                    let res = loop {
                        let res = match c.watch_open(&ctx, id, &pattern, &state).await {
                            None => WatchResult::Failed,
                            Some(mut session) => loop {
                                match session.next(&c, &ctx, &pattern, &mut state).await {
                                    Step::Event(ev) => {
                                        yield Ok(ev);
                                    }
                                    Step::End(r) => break r,
                                    Step::Fatal(e) => {
                                        yield Err(e);
                                        break WatchResult::Fatal;
                                    }
                                }
                            },
                        };
                        attempts += 1;
                        if res != WatchResult::Retry || attempts >= WATCH_ATTEMPTS {
                            break res;
                        }
                    };
                    match res {
                        WatchResult::Stop | WatchResult::Fatal => return,
                        WatchResult::Served => {
                            served = true;
                            // Re-rank the nodes before the next attempt.
                            break;
                        }
                        WatchResult::Failed | WatchResult::Retry => {}
                    }
                }
                if served {
                    // The stream ran and died; the node is penalised, so the next attempt goes elsewhere.
                    delay = WATCH_DELAY_START;
                    if sleep_or_done(&ctx, WATCH_SERVED_PAUSE).await {
                        return;
                    }
                    continue;
                }
                let rctx = ctx.with_timeout(WATCH_REFRESH_TIMEOUT);
                let _ = c.refresh_view(&rctx).await;
                rctx.cancel();
                c.log().warn(
                    "watch: no node answered, retrying",
                    vec![
                        Attr::string("pattern", pattern.clone()),
                        Attr::duration("in", duration_to_ns(delay)),
                    ],
                );
                if sleep_or_done(&ctx, delay + jitter(delay)).await {
                    return;
                }
                delay = next_delay(delay);
            }
        })
    }

    /// The first half of `watchOnce`: open the stream and write the request (no FIN).
    async fn watch_open(
        &self,
        ctx: &Ctx,
        id: NodeId,
        pattern: &str,
        state: &State,
    ) -> Option<WatchSession> {
        let octx = ctx.with_timeout(self.cfg().request_timeout);
        let opened = self.pool().open(&octx, id, ALPN_CLIENT).await;
        octx.cancel();
        let Stream { mut send, recv } = match opened {
            Ok(s) => s,
            Err(e) => {
                self.handle_err(id, &Error::Transport(e));
                return None;
            }
        };
        let mut m = Msg {
            typ: T_REF_WATCH,
            pattern: pattern.to_owned(),
            refs: known_list(state),
            ..Msg::default()
        };
        self.stamp(&mut m);
        if let Err(e) = dstore_wire::write_msg(&mut *send, &m).await {
            self.pool().drop_peer(id, ALPN_CLIENT);
            self.handle_err(id, &Error::Wire(e));
            Stream::new(send, recv).close_stream();
            return None;
        }
        let (tx, frames) = mpsc::channel(1);
        let reader = tokio::spawn(read_frames(recv, tx));
        Some(WatchSession {
            id,
            send,
            frames,
            reader,
            synced: false,
            idle: deadline_after(self.cfg().watch_idle),
            pending: VecDeque::new(),
        })
    }
}

/// The known list: one `{Name, Key}` per held name (`Version` null, `CreatedAt` 0), by name (Go iterates
/// the map at random, DD-10).
fn known_list(state: &State) -> Vec<RefInfo> {
    let mut refs: Vec<RefInfo> = state
        .iter()
        .map(|(name, key)| RefInfo {
            name: name.clone(),
            key: key.clone(),
            version: None,
            created_at: 0,
            user: String::new(),
        })
        .collect();
    refs.sort_by(|a, b| a.name.cmp(&b.name));
    refs
}

/// The reader goroutine: frames until the first error, or until the session is gone.
async fn read_frames(mut recv: Box<dyn RecvStream>, tx: mpsc::Sender<Result<Msg, WireError>>) {
    loop {
        let res = dstore_wire::read_msg(&mut *recv).await;
        let failed = res.is_err();
        if tx.send(res).await.is_err() || failed {
            return;
        }
    }
}

/// Sleeps `d`; true when the ctx ended first.
async fn sleep_or_done(ctx: &Ctx, d: Duration) -> bool {
    tokio::select! {
        () = ctx.done() => true,
        () = tokio::time::sleep(d) => false,
    }
}

/// `rand.Int64N(int64(delay/2)+1)`: uniform in [0, delay/2] nanoseconds.
fn jitter(delay: Duration) -> Duration {
    let max = u64::try_from(duration_to_ns(delay) / 2).unwrap_or(0);
    Duration::from_nanos(rand::rng().random_range(0..=max))
}

/// `delay = min(2*delay, 30*time.Second)`: the delay of the next round no node serves.
fn next_delay(delay: Duration) -> Duration {
    (delay * 2).min(WATCH_DELAY_MAX)
}

/// now + d, saturating far in the future.
fn deadline_after(d: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(d)
        .or_else(|| now.checked_add(Duration::from_secs(100 * 365 * 24 * 3600)))
        .unwrap_or(now)
}

/// One watch stream against a node (`watchOnce` after the request is written).
struct WatchSession {
    id: NodeId,
    send: Box<dyn SendStream>,
    frames: mpsc::Receiver<Result<Msg, WireError>>,
    reader: tokio::task::JoinHandle<()>,
    synced: bool,
    /// The idle timer's deadline, re-armed on every frame.
    idle: Instant,
    pending: VecDeque<Pending>,
}

impl WatchSession {
    /// `abandon`: cancel the read side (the reader task owns it: aborting it drops the receive half,
    /// which sends STOP_SENDING 0) and finish the send side.
    fn abandon(&mut self) {
        self.reader.abort();
        self.send.finish();
    }

    /// `served`.
    fn served(&self) -> WatchResult {
        if self.synced {
            WatchResult::Served
        } else {
            WatchResult::Failed
        }
    }

    /// A frame read failed: the stream ended.
    fn ended(&mut self, c: &Cluster, err: Error) -> Step {
        self.abandon();
        c.pool().drop_peer(self.id, ALPN_CLIENT);
        c.handle_err(self.id, &err);
        c.log().warn(
            "watch: stream ended, reconnecting",
            vec![
                Attr::string("node", short_id(&self.id)),
                Attr::any("error", &err),
            ],
        );
        Step::End(self.served())
    }

    /// The next event of the stream, or how it ended.
    async fn next(&mut self, c: &Cluster, ctx: &Ctx, pattern: &str, state: &mut State) -> Step {
        loop {
            if let Some(p) = self.pending.pop_front() {
                return Step::Event(match p {
                    Pending::Change(r) => {
                        state.insert(r.name.clone(), r.key.clone());
                        RefChange {
                            name: r.name,
                            key: r.key,
                            version: r.version.unwrap_or_default(),
                            created_at: r.created_at,
                            user: r.user,
                            ..RefChange::default()
                        }
                    }
                    Pending::Deleted(name) => {
                        state.remove(&name);
                        RefChange {
                            name,
                            deleted: true,
                            ..RefChange::default()
                        }
                    }
                    Pending::Synced => RefChange {
                        synced: true,
                        node: self.id,
                        ..RefChange::default()
                    },
                });
            }
            let frame = tokio::select! {
                () = ctx.done() => {
                    self.abandon();
                    return Step::End(WatchResult::Stop);
                }
                () = tokio::time::sleep_until(self.idle) => {
                    self.abandon();
                    c.pool().drop_peer(self.id, ALPN_CLIENT);
                    c.handle_err(self.id, &Error::WatchIdle);
                    c.log().warn(
                        "watch: stream idle, reconnecting",
                        vec![Attr::string("node", short_id(&self.id))],
                    );
                    return Step::End(self.served());
                }
                f = self.frames.recv() => f,
            };
            let m = match frame {
                Some(Ok(m)) => m,
                Some(Err(e)) => return self.ended(c, Error::Wire(e)),
                // The reader cannot end without a frame while the session holds it.
                None => return self.ended(c, Error::Wire(WireError::Eof)),
            };
            self.idle = deadline_after(c.cfg().watch_idle);
            match m.typ {
                T_ERR => {
                    let e = dstore_wire::error_from_msg(&m);
                    let code = e.code.clone();
                    let err = Error::Remote(e);
                    c.handle_err(self.id, &err);
                    return match code.as_str() {
                        CODE_STALE_VIEW => Step::End(WatchResult::Retry),
                        CODE_BAD_REQUEST | CODE_UNAUTHORIZED => Step::Fatal(err),
                        _ => {
                            c.log().warn(
                                "watch: node refused, trying the next",
                                vec![
                                    Attr::string("node", short_id(&self.id)),
                                    Attr::any("error", &err),
                                ],
                            );
                            Step::End(WatchResult::Failed)
                        }
                    };
                }
                T_REF_CHANGES => {
                    c.ok(self.id);
                    self.pending.extend(m.refs.into_iter().map(Pending::Change));
                    self.pending
                        .extend(m.deleted.into_iter().map(Pending::Deleted));
                }
                T_REF_SYNCED => {
                    c.ok(self.id);
                    if !self.synced {
                        self.synced = true;
                        c.log().info(
                            "watch: synced",
                            vec![
                                Attr::string("pattern", pattern),
                                Attr::string("node", short_id(&self.id)),
                                Attr::int64("refs", state.len() as i64),
                            ],
                        );
                        self.pending.push_back(Pending::Synced);
                    }
                }
                other => {
                    self.abandon();
                    c.pool().drop_peer(self.id, ALPN_CLIENT);
                    c.handle_err(self.id, &Error::UnexpectedFrame(other));
                    return Step::End(self.served());
                }
            }
        }
    }
}

impl Drop for WatchSession {
    /// `defer wire.CloseStream(s)` and the reader's `rcancel`.
    fn drop(&mut self) {
        self.send.finish();
        self.reader.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use dstore_gocompat::slog::{Level, Record};
    use dstore_testkit::golden::{self, decimal_i64};
    use dstore_wire::{CODE_UNAVAILABLE, T_OK, T_VIEW, err_msg};
    use futures::StreamExt;
    use serde::Deserialize;

    use super::*;
    use crate::Config;
    use crate::cluster::test_node::{Handler, Reply, TestNet, lock, node_id, view_reply};

    /// Per node, the replies to ref-watch requests, in order; views are answered, anything else closed.
    #[derive(Default)]
    struct Script(Mutex<HashMap<NodeId, VecDeque<Reply>>>);

    impl Script {
        fn push(&self, id: NodeId, r: Reply) {
            lock(&self.0).entry(id).or_default().push_back(r);
        }
    }

    fn handler(script: Arc<Script>) -> Handler {
        Arc::new(move |v, id, req| {
            if req.typ == T_VIEW {
                return Reply::Frames(vec![view_reply(v, &[])]);
            }
            lock(&script.0)
                .get_mut(&id)
                .and_then(VecDeque::pop_front)
                .unwrap_or(Reply::Close)
        })
    }

    /// A stream reply with these frames queued; keep the sender to keep the stream open.
    fn stream_of(frames: Vec<Msg>) -> (Reply, mpsc::Sender<Msg>) {
        let (tx, rx) = mpsc::channel(64);
        for f in frames {
            if tx.try_send(f).is_err() {
                panic!("stream_of: too many frames");
            }
        }
        (Reply::Stream(rx), tx)
    }

    fn synced() -> Msg {
        Msg {
            typ: T_REF_SYNCED,
            incarnation: 1,
            epoch: 7,
            ..Msg::default()
        }
    }

    fn info(name: &str, key: u8) -> RefInfo {
        RefInfo {
            name: name.to_owned(),
            key: Some(vec![key; 32]),
            version: Some(vec![0x30, key]),
            created_at: 1_700_000_000_123_456_789,
            user: "alice".to_owned(),
        }
    }

    fn changes(refs: Vec<RefInfo>, deleted: &[&str]) -> Msg {
        Msg {
            typ: T_REF_CHANGES,
            incarnation: 1,
            epoch: 7,
            refs,
            deleted: deleted.iter().map(|s| s.to_string()).collect(),
            ..Msg::default()
        }
    }

    fn change(name: &str, key: u8) -> RefChange {
        RefChange {
            name: name.to_owned(),
            key: Some(vec![key; 32]),
            version: vec![0x30, key],
            created_at: 1_700_000_000_123_456_789,
            user: "alice".to_owned(),
            ..RefChange::default()
        }
    }

    fn synced_on(id: NodeId) -> RefChange {
        RefChange {
            synced: true,
            node: id,
            ..RefChange::default()
        }
    }

    async fn next_event(s: &mut WatchStream) -> RefChange {
        match s.next().await {
            Some(Ok(ev)) => ev,
            other => panic!("want an event, got {other:?}"),
        }
    }

    fn watch_logs(tn: &TestNet) -> Vec<Record> {
        tn.records()
            .into_iter()
            .filter(|r| r.message.starts_with("watch:"))
            .collect()
    }

    fn frame_hex(m: &Msg) -> String {
        match dstore_wire::encode_frame(m) {
            Ok(b) => dstore_gocompat::hex::encode(&b),
            Err(e) => panic!("encode: {e}"),
        }
    }

    /// (pattern, known list, request frame hex).
    type FrameCase = (&'static str, HashMap<String, Vec<u8>>, &'static str);

    // client-core §3.2 (verified hex): ref-watch request frames.
    #[tokio::test(start_paused = true)]
    async fn watch_request_frames() {
        let script = Arc::new(Script::default());
        let mut keep = Vec::new();
        for _ in 0..3 {
            let (r, tx) = stream_of(vec![synced()]);
            script.push(node_id(0), r);
            keep.push(tx);
        }
        let tn = TestNet::start(1, handler(script)).await;
        let c = tn.dial().await;
        let cases: Vec<FrameCase> = vec![
            (
                "trees/**",
                HashMap::new(),
                "00000025a500182a0150000102030405060708090a0b0c0d0e0f02010307183c6874726565732f2a2a",
            ),
            (
                "trees/**",
                HashMap::from([("trees/a".to_owned(), vec![0xaa; 32])]),
                "00000058a600182a0150000102030405060708090a0b0c0d0e0f020103071581a4006774726565732f61015820aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa02f60300183c6874726565732f2a2a",
            ),
            (
                "trees/*",
                HashMap::from([("x".to_owned(), Vec::new())]),
                "00000030a600182a0150000102030405060708090a0b0c0d0e0f020103071581a400617801f602f60300183c6774726565732f2a",
            ),
        ];
        for (i, (pattern, known, want)) in cases.into_iter().enumerate() {
            let mut s = c.watch_refs(Ctx::background(), pattern.to_owned(), known);
            assert_eq!(next_event(&mut s).await, synced_on(node_id(0)));
            drop(s);
            let reqs = tn.requests_of(T_REF_WATCH);
            assert_eq!(frame_hex(&reqs[i].1), want, "case {i}");
        }
        // Dropping the stream leaves the pool connection in place.
        assert!(c.pool().path(node_id(0), ALPN_CLIENT).is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn watch_yields_changes_then_reconnects_with_the_updated_known_list() {
        let script = Arc::new(Script::default());
        let (first, tx0) = stream_of(vec![
            changes(
                vec![info("trees/a", 0xaa), info("trees/b", 0xbb)],
                &["trees/gone"],
            ),
            synced(),
            synced(),
            changes(vec![info("trees/c", 0xcc)], &[]),
        ]);
        drop(tx0); // FIN after the queued frames: the client reads EOF.
        script.push(node_id(0), first);
        let (second, _tx1) = stream_of(vec![synced()]);
        script.push(node_id(1), second);
        let tn = TestNet::start(3, handler(script)).await;
        let c = tn.dial().await;

        let known = HashMap::from([
            ("trees/gone".to_owned(), vec![1; 32]),
            ("trees/x".to_owned(), vec![2; 32]),
        ]);
        let start = Instant::now();
        let mut s = c.watch_refs(Ctx::background(), "trees/**".to_owned(), known);
        assert_eq!(next_event(&mut s).await, change("trees/a", 0xaa));
        assert_eq!(next_event(&mut s).await, change("trees/b", 0xbb));
        assert_eq!(
            next_event(&mut s).await,
            RefChange {
                name: "trees/gone".into(),
                deleted: true,
                ..RefChange::default()
            }
        );
        assert_eq!(next_event(&mut s).await, synced_on(node_id(0)));
        assert_eq!(next_event(&mut s).await, change("trees/c", 0xcc));
        // The heartbeat was silent; the stream ended; node 0 is penalised, node 1 serves.
        assert_eq!(next_event(&mut s).await, synced_on(node_id(1)));
        assert!(start.elapsed() >= WATCH_SERVED_PAUSE);
        assert_eq!(c.penalty(node_id(0)), 2);

        let reqs = tn.requests_of(T_REF_WATCH);
        assert_eq!(reqs.len(), 2);
        let names = |m: &Msg| -> Vec<String> { m.refs.iter().map(|r| r.name.clone()).collect() };
        assert_eq!(names(&reqs[0].1), ["trees/gone", "trees/x"]);
        assert_eq!(reqs[1].0, node_id(1));
        assert_eq!(
            names(&reqs[1].1),
            ["trees/a", "trees/b", "trees/c", "trees/x"]
        );
        assert!(
            reqs[1]
                .1
                .refs
                .iter()
                .all(|r| r.version.is_none() && r.created_at == 0)
        );
        assert_eq!(reqs[1].1.refs[0].key, Some(vec![0xaa; 32]));

        let logs = watch_logs(&tn);
        let summary: Vec<(Level, String, Vec<Attr>)> = logs
            .into_iter()
            .map(|r| (r.level, r.message, r.attrs))
            .collect();
        assert_eq!(
            summary,
            vec![
                (
                    Level::INFO,
                    "watch: synced".to_owned(),
                    vec![
                        Attr::string("pattern", "trees/**"),
                        Attr::string("node", "01010101"),
                        Attr::int64("refs", 3),
                    ]
                ),
                (
                    Level::WARN,
                    "watch: stream ended, reconnecting".to_owned(),
                    vec![Attr::string("node", "01010101"), Attr::any("error", "EOF")]
                ),
                (
                    Level::INFO,
                    "watch: synced".to_owned(),
                    vec![
                        Attr::string("pattern", "trees/**"),
                        Attr::string("node", "02020202"),
                        Attr::int64("refs", 4),
                    ]
                ),
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn watch_bad_request_and_unauthorized_are_terminal() {
        for (code, text) in [
            (CODE_BAD_REQUEST, "refglob: unterminated character class"),
            (CODE_UNAUTHORIZED, "not on the allowlist"),
        ] {
            let script = Arc::new(Script::default());
            script.push(node_id(0), Reply::Frames(vec![err_msg(code, text)]));
            let tn = TestNet::start(2, handler(script)).await;
            let c = tn.dial().await;
            let mut s = c.watch_refs(Ctx::background(), "trees/[".to_owned(), HashMap::new());
            match s.next().await {
                Some(Err(e)) => {
                    assert!(e.is_code(code));
                    assert_eq!(e.to_string(), format!("remote: {code}: {text}"));
                }
                other => panic!("want the remote error, got {other:?}"),
            }
            assert!(s.next().await.is_none());
            assert_eq!(tn.requests_of(T_REF_WATCH).len(), 1);
            assert_eq!(c.penalty(node_id(0)), 0, "a remote answer is not penalised");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn watch_retries_stale_view_four_times_then_waits_with_growing_delays() {
        let script = Arc::new(Script::default());
        for i in 0..2 {
            for _ in 0..16 {
                script.push(
                    node_id(i),
                    Reply::Frames(vec![err_msg(CODE_STALE_VIEW, "request epoch is behind")]),
                );
            }
        }
        let tn = TestNet::start(2, handler(script)).await;
        let c = tn.dial().await;
        let ctx = Ctx::background();
        let mut s = c.watch_refs(ctx.clone(), "trees/*".to_owned(), HashMap::new());
        assert!(
            tokio::time::timeout(Duration::from_millis(999), s.next())
                .await
                .is_err()
        );
        let per_node =
            |reqs: &[(NodeId, Msg)], id: NodeId| reqs.iter().filter(|(n, _)| *n == id).count();
        let reqs = tn.requests_of(T_REF_WATCH);
        assert_eq!(
            (per_node(&reqs, node_id(0)), per_node(&reqs, node_id(1))),
            (4, 4)
        );
        // The unserved round refreshed the view (stamped) and logged the delay.
        assert_eq!(
            tn.requests_of(T_VIEW)
                .iter()
                .filter(|(_, m)| m.epoch == 7)
                .count(),
            1
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(501), s.next())
                .await
                .is_err()
        );
        let reqs = tn.requests_of(T_REF_WATCH);
        assert_eq!(reqs.len(), 16, "the second round started within 1.5 s");
        let delays: Vec<Attr> = watch_logs(&tn)
            .into_iter()
            .filter(|r| r.message == "watch: no node answered, retrying")
            .flat_map(|r| r.attrs)
            .collect();
        assert_eq!(
            delays,
            vec![
                Attr::string("pattern", "trees/*"),
                Attr::duration("in", 1_000_000_000),
                Attr::string("pattern", "trees/*"),
                Attr::duration("in", 2_000_000_000),
            ]
        );
        // Stale-view answers are remote: no penalty.
        assert_eq!(c.penalty(node_id(0)), 0);
        ctx.cancel();
        assert!(s.next().await.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn watch_idle_reconnects_elsewhere() {
        let script = Arc::new(Script::default());
        let (first, _tx0) = stream_of(vec![synced()]);
        script.push(node_id(0), first);
        let (second, _tx1) = stream_of(vec![synced()]);
        script.push(node_id(1), second);
        let tn = TestNet::start(3, handler(script)).await;
        let cfg = Config {
            watch_idle: Duration::from_secs(5),
            ..tn.config()
        };
        let c = match Cluster::dial(&Ctx::background(), cfg).await {
            Ok(c) => c,
            Err(e) => panic!("dial: {e}"),
        };
        let start = Instant::now();
        let mut s = c.watch_refs(Ctx::background(), "trees/*".to_owned(), HashMap::new());
        assert_eq!(next_event(&mut s).await, synced_on(node_id(0)));
        assert_eq!(next_event(&mut s).await, synced_on(node_id(1)));
        assert!(start.elapsed() >= Duration::from_secs(5) + WATCH_SERVED_PAUSE);
        assert_eq!(c.penalty(node_id(0)), 2);
        assert!(
            c.pool().path(node_id(0), ALPN_CLIENT).is_none(),
            "the pool dropped the node"
        );
        let logs: Vec<(String, Vec<Attr>)> = watch_logs(&tn)
            .into_iter()
            .map(|r| (r.message, r.attrs))
            .collect();
        assert_eq!(
            logs[1],
            (
                "watch: stream idle, reconnecting".to_owned(),
                vec![Attr::string("node", "01010101")]
            )
        );
    }

    #[tokio::test(start_paused = true)]
    async fn watch_refusals_and_unexpected_frames_move_to_the_next_node() {
        let script = Arc::new(Script::default());
        script.push(
            node_id(0),
            Reply::Frames(vec![err_msg(CODE_UNAVAILABLE, "no view")]),
        );
        script.push(
            node_id(1),
            Reply::Frames(vec![Msg {
                typ: T_OK,
                ..Msg::default()
            }]),
        );
        let (third, _tx) = stream_of(vec![synced()]);
        script.push(node_id(2), third);
        let tn = TestNet::start(3, handler(script)).await;
        let c = tn.dial().await;
        let mut s = c.watch_refs(Ctx::background(), "trees/*".to_owned(), HashMap::new());
        assert_eq!(next_event(&mut s).await, synced_on(node_id(2)));
        assert_eq!(c.penalty(node_id(0)), 0);
        assert_eq!(c.penalty(node_id(1)), 2, "an unexpected frame is penalised");
        let logs: Vec<(String, Vec<Attr>)> = watch_logs(&tn)
            .into_iter()
            .map(|r| (r.message, r.attrs))
            .collect();
        assert_eq!(
            logs[0],
            (
                "watch: node refused, trying the next".to_owned(),
                vec![
                    Attr::string("node", "01010101"),
                    Attr::any("error", "remote: unavailable: no view")
                ]
            )
        );
        assert_eq!(
            logs.len(),
            2,
            "the unexpected frame is not logged: {logs:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn watch_ends_on_cancel_without_an_item() {
        let script = Arc::new(Script::default());
        let (r, _tx) = stream_of(vec![synced()]);
        script.push(node_id(0), r);
        let tn = TestNet::start(1, handler(script)).await;
        let c = tn.dial().await;
        let ctx = Ctx::background();
        let mut s = c.watch_refs(ctx.clone(), "trees/*".to_owned(), HashMap::new());
        assert_eq!(next_event(&mut s).await, synced_on(node_id(0)));
        ctx.cancel();
        assert!(s.next().await.is_none());
        assert!(s.next().await.is_none());
        // A cancelled ctx before the first poll ends at once.
        let mut s = c.watch_refs(ctx, "trees/*".to_owned(), HashMap::new());
        assert!(s.next().await.is_none());
        assert_eq!(tn.requests_of(T_REF_WATCH).len(), 1);
    }

    #[derive(Deserialize)]
    struct Reconnect {
        round: u64,
        #[serde(deserialize_with = "decimal_i64")]
        delay_ns: i64,
        #[serde(deserialize_with = "decimal_i64")]
        jitter_max_ns: i64,
    }

    #[derive(Deserialize)]
    struct BackoffFile {
        watch_reconnect: Vec<Reconnect>,
        #[serde(deserialize_with = "decimal_i64")]
        watch_served_pause_ns: i64,
        #[serde(deserialize_with = "decimal_i64")]
        watch_refresh_timeout_ns: i64,
    }

    #[test]
    fn golden_watch_timing() {
        let f: BackoffFile = golden::load_json("client/backoff.json");
        assert_eq!(duration_to_ns(WATCH_SERVED_PAUSE), f.watch_served_pause_ns);
        assert_eq!(
            duration_to_ns(WATCH_REFRESH_TIMEOUT),
            f.watch_refresh_timeout_ns
        );
        let mut delay = WATCH_DELAY_START;
        let mut round = 1;
        for c in &f.watch_reconnect {
            while round < c.round {
                delay = next_delay(delay);
                round += 1;
            }
            assert_eq!(duration_to_ns(delay), c.delay_ns, "round {}", c.round);
            assert_eq!(
                duration_to_ns(delay) / 2,
                c.jitter_max_ns,
                "round {}",
                c.round
            );
            for _ in 0..64 {
                let j = duration_to_ns(jitter(delay));
                assert!(
                    (0..=c.jitter_max_ns).contains(&j),
                    "jitter {j} for round {}",
                    c.round
                );
            }
        }
    }

    #[test]
    fn known_list_is_sorted_with_null_versions() {
        let state: State = HashMap::from([("b".to_owned(), Some(vec![1])), ("a".to_owned(), None)]);
        let refs = known_list(&state);
        assert_eq!(refs[0].name, "a");
        assert_eq!(refs[0].key, None);
        assert_eq!(refs[1].key, Some(vec![1]));
        assert!(
            refs.iter()
                .all(|r| r.version.is_none() && r.created_at == 0 && r.user.is_empty())
        );
    }

    // PORTING.md §1.4: the idle timer keeps running while the consumer handles an event. The frame that
    // re-armed it came before the consumer's pause, so the stream idles out as soon as the consumer
    // resumes, not WatchIdle later.
    #[tokio::test(start_paused = true)]
    async fn watch_idle_timer_runs_while_the_consumer_holds_an_event() {
        let script = Arc::new(Script::default());
        let (first, _tx0) = stream_of(vec![changes(vec![info("trees/a", 0xaa)], &[])]);
        script.push(node_id(0), first);
        let (second, _tx1) = stream_of(vec![synced()]);
        script.push(node_id(1), second);
        let tn = TestNet::start(2, handler(script)).await;
        let cfg = Config {
            watch_idle: Duration::from_secs(5),
            ..tn.config()
        };
        let c = match Cluster::dial(&Ctx::background(), cfg).await {
            Ok(c) => c,
            Err(e) => panic!("dial: {e}"),
        };
        let mut s = c.watch_refs(Ctx::background(), "trees/*".to_owned(), HashMap::new());
        assert_eq!(next_event(&mut s).await, change("trees/a", 0xaa));
        // The consumer holds the event for longer than WatchIdle, with the stream still open.
        tokio::time::sleep(Duration::from_secs(6)).await;
        let resumed = Instant::now();
        // Node 0 never synced, so the next node is asked at once, without the served pause.
        assert_eq!(next_event(&mut s).await, synced_on(node_id(1)));
        assert!(
            resumed.elapsed() < Duration::from_secs(1),
            "idled out {:?} after the consumer resumed",
            resumed.elapsed()
        );
        assert_eq!(c.penalty(node_id(0)), 2);
        let logs: Vec<String> = watch_logs(&tn).into_iter().map(|r| r.message).collect();
        assert_eq!(logs, ["watch: stream idle, reconnecting", "watch: synced"]);
    }

    // watch.go:70-81: a stream that synced and then died resets the delay, so the next round that no node
    // serves logs `in=1s` again instead of doubling.
    #[tokio::test(start_paused = true)]
    async fn watch_served_stream_resets_the_delay() {
        let refuse = || Reply::Frames(vec![err_msg(CODE_UNAVAILABLE, "no view")]);
        let script = Arc::new(Script::default());
        // Round 1: both nodes refuse. Round 2: node 0 syncs, then its stream ends (FIN).
        // Round 3: node 1 (unpenalised) and node 0 refuse.
        script.push(node_id(0), refuse());
        script.push(node_id(1), refuse());
        let (served, tx) = stream_of(vec![synced()]);
        drop(tx);
        script.push(node_id(0), served);
        for i in 0..2 {
            for _ in 0..8 {
                script.push(node_id(i), refuse());
            }
        }
        let tn = TestNet::start(2, handler(script)).await;
        let c = tn.dial().await;
        let mut s = c.watch_refs(Ctx::background(), "trees/*".to_owned(), HashMap::new());
        assert_eq!(next_event(&mut s).await, synced_on(node_id(0)));
        // The stream ends, the 200 ms pause, round 3 and its log; round 4 waits at least 1 s.
        assert!(
            tokio::time::timeout(Duration::from_millis(1100), s.next())
                .await
                .is_err()
        );
        let logs: Vec<(String, Vec<Attr>)> = watch_logs(&tn)
            .into_iter()
            .map(|r| (r.message, r.attrs))
            .collect();
        let messages: Vec<&str> = logs.iter().map(|(m, _)| m.as_str()).collect();
        assert_eq!(
            messages,
            [
                "watch: node refused, trying the next",
                "watch: node refused, trying the next",
                "watch: no node answered, retrying",
                "watch: synced",
                "watch: stream ended, reconnecting",
                "watch: node refused, trying the next",
                "watch: node refused, trying the next",
                "watch: no node answered, retrying",
            ]
        );
        let delays: Vec<&Attr> = logs
            .iter()
            .filter(|(m, _)| m == "watch: no node answered, retrying")
            .flat_map(|(_, attrs)| attrs.iter().filter(|a| a.key == "in"))
            .collect();
        assert_eq!(
            delays,
            [
                &Attr::duration("in", 1_000_000_000),
                &Attr::duration("in", 1_000_000_000)
            ]
        );
        // Round 3 went to the unpenalised node first.
        let round3: Vec<NodeId> = tn
            .requests_of(T_REF_WATCH)
            .into_iter()
            .skip(3)
            .map(|(id, _)| id)
            .collect();
        assert_eq!(round3, [node_id(1), node_id(0)]);
    }

    // handleErr adopts the view a stale-view answer carries, so the retry to the same node is stamped with
    // the newer epoch; a remote answer is not penalised.
    #[tokio::test(start_paused = true)]
    async fn watch_stale_view_retry_carries_the_adopted_epoch() {
        let ids: Vec<NodeId> = (0..2).map(node_id).collect();
        let newer = dstore_view::View {
            epoch: 8,
            version: 1,
            ..crate::cluster::test_node::test_view(&ids)
        };
        let script = Arc::new(Script::default());
        script.push(
            node_id(0),
            Reply::Frames(vec![Msg {
                typ: T_ERR,
                incarnation: 1,
                epoch: 8,
                code: CODE_STALE_VIEW.to_owned(),
                text: "request epoch is behind".to_owned(),
                view: newer.encode(),
                ..Msg::default()
            }]),
        );
        let (served, _tx) = stream_of(vec![synced()]);
        script.push(node_id(0), served);
        let tn = TestNet::start(2, handler(script)).await;
        let c = tn.dial().await;
        let mut s = c.watch_refs(Ctx::background(), "trees/*".to_owned(), HashMap::new());
        assert_eq!(next_event(&mut s).await, synced_on(node_id(0)));
        let got: Vec<(NodeId, u64)> = tn
            .requests_of(T_REF_WATCH)
            .into_iter()
            .map(|(id, m)| (id, m.epoch))
            .collect();
        assert_eq!(got, [(node_id(0), 7), (node_id(0), 8)]);
        assert_eq!(c.view().map(|v| v.epoch), Some(8));
        assert_eq!(c.penalty(node_id(0)), 0);
    }
}
