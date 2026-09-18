//! `client/fetch.go`: the fetcher behind `Cluster::get` and `Cluster::pull_tree` (part B, internal).
//!
//! Keys handed to a [`Fetcher`] are routed to the owner their attempt names in the key's read order and
//! accumulated per owner. `Jobs` workers run one `get` stream per batch, records stream out as each is
//! verified, and a key an owner did not return is re-asked down its read order, with at most one view
//! refresh per fetcher. Spec: port-notes/client-transfer.md §2.5, §4.3.

use std::collections::HashSet;
use std::future::Future;
use std::sync::{Mutex, MutexGuard, PoisonError};

use amber_store_core::amberpack::REC_HEADER_SIZE;
use amber_store_core::key::Key;
use dstore_transport::Stream;
use dstore_wire::{ALPN_CLIENT, Msg, PackReader, PackRecords, T_ABSENT, T_GET, WireError};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::task::{JoinHandle, JoinSet};

use crate::{
    Cluster, Ctx, Error, GET_BATCH_BYTES, GET_BATCH_KEYS, GET_EST_MAX, NodeId, verify_record,
};

/// Capacity of the fetcher's key input (Go `in`).
const INPUT_CAP: usize = 1024;
/// Capacity of the retry queue (Go `retry`).
const RETRY_CAP: usize = 1024;
/// Capacity of the results channel (Go `out`).
const RESULTS_CAP: usize = 256;

/// `estSize`: 46 + min(length, 64 KiB).
///
/// Go's estimate is negative for a length field of 2^63 or more; this clamps it to 0. The fetcher uses
/// [`est_size_i64`], which keeps Go's value, so only the tests call this PORTING.md §4.8 item.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn est_size(k: &[u8; 32]) -> usize {
    usize::try_from(est_size_i64(k)).unwrap_or(0)
}

/// `estSize` exactly: `46 + min(int(length), 64 KiB)`, where Go's `uint64 → int` conversion wraps, so a
/// length field of 2^63 gives `-9223372036854775762` and 2^64-1 gives 45 (`client/fetch.json`).
pub(crate) fn est_size_i64(k: &[u8; 32]) -> i64 {
    // Go int(uint64): the same bits read as two's complement.
    let length = Key(*k).length() as i64;
    REC_HEADER_SIZE as i64 + length.min(GET_EST_MAX as i64)
}

/// `fetchKey`: a key with its position in the read order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FetchKey {
    pub(crate) key: [u8; 32],
    pub(crate) attempt: usize,
}

/// `fetched`: one outcome; `rec` is None when no owner has the key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Fetched {
    pub(crate) key: [u8; 32],
    pub(crate) rec: Option<Vec<u8>>,
}

/// `fetchJob`: one batch for one owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FetchJob {
    pub(crate) node: NodeId,
    pub(crate) keys: Vec<FetchKey>,
}

/// `fetchAcc`: the keys queued at one owner and their estimated bytes (Go `int`, wrapping).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FetchAcc {
    pub(crate) keys: Vec<FetchKey>,
    pub(crate) bytes: i64,
}

/// The accumulators in first-use order. Go keeps a map and iterates it at random, so any order is
/// compatible (DD-10); every step of `client/fetch.json` has a single possible pick.
pub(crate) type Accs = Vec<(NodeId, FetchAcc)>;

/// `pickBatch`: takes a batch from a full accumulator, else from the one with the most keys (the first
/// one wins a tie).
pub(crate) fn pick_batch(acc: &mut Accs) -> Option<FetchJob> {
    let mut best: Option<usize> = None;
    for (i, (_, a)) in acc.iter().enumerate() {
        if a.keys.len() >= GET_BATCH_KEYS || a.bytes >= GET_BATCH_BYTES as i64 {
            best = Some(i);
            break;
        }
        match best {
            Some(b) if a.keys.len() <= acc[b].1.keys.len() => {}
            _ => best = Some(i),
        }
    }
    let i = best?;
    let (node, a) = &mut acc[i];
    let node = *node;
    let mut n = 0;
    let mut bytes: i64 = 0;
    while n < a.keys.len()
        && n < GET_BATCH_KEYS
        && (n == 0 || bytes.wrapping_add(est_size_i64(&a.keys[n].key)) <= GET_BATCH_BYTES as i64)
    {
        bytes = bytes.wrapping_add(est_size_i64(&a.keys[n].key));
        n += 1;
    }
    let keys: Vec<FetchKey> = a.keys.drain(..n).collect();
    a.bytes = a.bytes.wrapping_sub(bytes);
    if a.keys.is_empty() {
        acc.remove(i);
    }
    Some(FetchJob { node, keys })
}

/// Locks a std mutex, recovering a poisoned one (every update under these locks is a single map
/// operation, so the state stays consistent).
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A remote error read by `wire::expect` is the node's answer (`*wire.Error` in Go).
pub(crate) fn wire_error(e: WireError) -> Error {
    match e {
        WireError::Remote(r) => Error::Remote(r),
        e => Error::Wire(e),
    }
}

/// `defer wire.CloseStream(s)`: finish, then `cancel_read(0)`, on every exit path.
pub(crate) struct ClosingStream(pub(crate) Stream);

impl Drop for ClosingStream {
    fn drop(&mut self) {
        self.0.close_stream();
    }
}

/// `(*fetcher).add`: hands one key to the fetcher; false once the fetch is stopped.
pub(crate) async fn add_key(input: &mpsc::Sender<[u8; 32]>, ctx: &Ctx, k: [u8; 32]) -> bool {
    tokio::select! {
        r = input.send(k) => r.is_ok(),
        () = ctx.done() => false,
    }
}

/// `fetcher`: the dispatcher task, the workers and the channels between them.
///
/// Dropping a `Fetcher` cancels its context (Go `stop()` without the wait); [`Fetcher::stop`] also waits
/// for the dispatcher, which waits for the workers.
pub(crate) struct Fetcher {
    ctx: Ctx,
    input: Option<mpsc::Sender<[u8; 32]>>,
    results: mpsc::Receiver<Fetched>,
    run: Option<JoinHandle<()>>,
}

impl Fetcher {
    /// `newFetcher`: `Jobs` workers and the dispatcher, under a child of `ctx`.
    pub(crate) fn new(c: &Cluster, ctx: &Ctx) -> Fetcher {
        let fctx = ctx.with_cancel();
        let jobs = c.cfg().jobs.max(1);
        let (in_tx, in_rx) = mpsc::channel(INPUT_CAP);
        let (retry_tx, retry_rx) = mpsc::channel(RETRY_CAP);
        let (out_tx, out_rx) = mpsc::channel(RESULTS_CAP);
        // Go's unbuffered job channel: the dispatcher sends only while fewer than Jobs batches are in
        // flight, so a free worker exists and the one buffered slot does not change dispatch.
        let (job_tx, job_rx) = async_channel::bounded(1);
        let (done_tx, done_rx) = mpsc::channel(jobs);
        let mut workers = JoinSet::new();
        for _ in 0..jobs {
            let w = Worker {
                c: c.clone(),
                ctx: fctx.clone(),
                jobs: job_rx.clone(),
                retry: retry_tx.clone(),
                out: out_tx.clone(),
                done: done_tx.clone(),
            };
            workers.spawn(w.run());
        }
        let d = Dispatcher {
            c: c.clone(),
            ctx: fctx.clone(),
            jobs,
            input: in_rx,
            retry: retry_rx,
            out: out_tx,
            job_tx,
            job_done: done_rx,
            refreshed: false,
        };
        let run = tokio::spawn(d.run(workers));
        Fetcher {
            ctx: fctx,
            input: Some(in_tx),
            results: out_rx,
            run: Some(run),
        }
    }

    /// The fetch's context (a child of the caller's).
    pub(crate) fn ctx(&self) -> &Ctx {
        &self.ctx
    }

    /// The input channel (Go `input()`); dropping it is Go's `finish()`.
    pub(crate) fn take_input(&mut self) -> Option<mpsc::Sender<[u8; 32]>> {
        self.input.take()
    }

    /// The next outcome (Go `<-results()`); None once the fetch is over.
    pub(crate) async fn recv(&mut self) -> Option<Fetched> {
        self.results.recv().await
    }

    /// `stop`: cancels the fetch and waits for the dispatcher and its workers.
    pub(crate) async fn stop(&mut self) {
        self.ctx.cancel();
        if let Some(h) = self.run.take() {
            let _ = h.await;
        }
    }
}

impl Drop for Fetcher {
    fn drop(&mut self) {
        self.ctx.cancel();
    }
}

/// `(*fetcher).run`: the dispatcher.
struct Dispatcher {
    c: Cluster,
    ctx: Ctx,
    jobs: usize,
    input: mpsc::Receiver<[u8; 32]>,
    retry: mpsc::Receiver<FetchKey>,
    out: mpsc::Sender<Fetched>,
    job_tx: async_channel::Sender<FetchJob>,
    job_done: mpsc::Receiver<()>,
    refreshed: bool,
}

impl Dispatcher {
    /// The deferred part of Go's `run`: close `jobs`, wait for the workers, then close `out` (by dropping
    /// the last sender).
    async fn run(mut self, mut workers: JoinSet<()>) {
        self.dispatch().await;
        self.job_tx.close();
        while workers.join_next().await.is_some() {}
    }

    async fn dispatch(&mut self) {
        let mut acc: Accs = Vec::new();
        let mut inflight = 0usize;
        let mut in_open = true;
        loop {
            // Take everything queued before dispatching, so that batches fill while the workers are busy.
            while in_open {
                match self.input.try_recv() {
                    Ok(k) => self.route(&mut acc, FetchKey { key: k, attempt: 0 }).await,
                    Err(TryRecvError::Disconnected) => in_open = false,
                    Err(TryRecvError::Empty) => break,
                }
            }
            while let Ok(r) = self.retry.try_recv() {
                self.route(&mut acc, r).await;
            }
            while inflight < self.jobs {
                let Some(job) = pick_batch(&mut acc) else {
                    break;
                };
                inflight += 1;
                tokio::select! {
                    r = self.job_tx.send(job) => {
                        if r.is_err() {
                            return;
                        }
                    }
                    () = self.ctx.done() => return,
                }
            }
            if !in_open && inflight == 0 && acc.is_empty() {
                return;
            }
            tokio::select! {
                k = self.input.recv(), if in_open => match k {
                    Some(k) => self.route(&mut acc, FetchKey { key: k, attempt: 0 }).await,
                    None => in_open = false,
                },
                Some(r) = self.retry.recv() => self.route(&mut acc, r).await,
                Some(()) = self.job_done.recv() => inflight = inflight.saturating_sub(1),
                () = self.ctx.done() => return,
            }
        }
    }

    /// `route`: queues a key at the owner its attempt names. Past the end of the read order the view is
    /// refreshed once per fetcher, inline, and the key starts over; after that it is reported missing.
    /// The read order is recomputed at every routing, with the current penalties (v0.1.9).
    async fn route(&mut self, acc: &mut Accs, mut fk: FetchKey) {
        let mut order = self.c.read_order(&fk.key);
        if fk.attempt >= order.len() && !self.refreshed {
            self.refreshed = true;
            let _ = self.c.refresh_view(&self.ctx).await;
            order = self.c.read_order(&fk.key);
            fk.attempt = 0;
        }
        let Some(&node) = order.get(fk.attempt) else {
            tokio::select! {
                _ = self.out.send(Fetched { key: fk.key, rec: None }) => {}
                () = self.ctx.done() => {}
            }
            return;
        };
        let est = est_size_i64(&fk.key);
        match acc.iter_mut().find(|(id, _)| *id == node) {
            Some((_, a)) => {
                a.keys.push(fk);
                a.bytes = a.bytes.wrapping_add(est);
            }
            None => acc.push((
                node,
                FetchAcc {
                    keys: vec![fk],
                    bytes: est,
                },
            )),
        }
    }
}

/// `(*fetcher).worker`.
struct Worker {
    c: Cluster,
    ctx: Ctx,
    jobs: async_channel::Receiver<FetchJob>,
    retry: mpsc::Sender<FetchKey>,
    out: mpsc::Sender<Fetched>,
    done: mpsc::Sender<()>,
}

impl Worker {
    async fn run(self) {
        while let Ok(job) = self.jobs.recv().await {
            // A job left in the channel's one buffered slot after a stop is not started (Go's unbuffered
            // channel never hands one out after the dispatcher returned).
            if self.ctx.err().is_none() {
                self.fetch(job).await;
            }
            let _ = self.done.send(()).await;
        }
    }

    /// `fetch`: runs one batch; keys the owner did not return go back for the next owner in their read
    /// order. The error of the stream is ignored, as in Go.
    async fn fetch(&self, job: FetchJob) {
        let keys: Vec<[u8; 32]> = job.keys.iter().map(|fk| fk.key).collect();
        let mut got: HashSet<[u8; 32]> = HashSet::with_capacity(keys.len());
        let out = &self.out;
        let ctx = &self.ctx;
        let _ = self
            .c
            .get_stream(ctx, job.node, &keys, |k, rec| {
                got.insert(k);
                async move {
                    tokio::select! {
                        r = out.send(Fetched { key: k, rec: Some(rec) }) => r.is_ok(),
                        () = ctx.done() => false,
                    }
                }
            })
            .await;
        for fk in &job.keys {
            if got.contains(&fk.key) {
                continue;
            }
            let next = FetchKey {
                key: fk.key,
                attempt: fk.attempt + 1,
            };
            tokio::select! {
                r = self.retry.send(next) => {
                    if r.is_err() {
                        return;
                    }
                }
                () = self.ctx.done() => return,
            }
        }
    }
}

impl Cluster {
    /// `getStream`: fetches a batch from one node, handing over each verified record; `emit` returning
    /// false ends the stream.
    ///
    /// The absent list is ignored (absence is inferred from what arrives), records are not checked against
    /// the requested keys, a corrupt copy is skipped, and a mid-stream failure neither penalises nor clears
    /// the node. Once the stream is open the exchange is bounded by `ctx`, the fetch's context, so a stopped
    /// fetch holds no stream open (Go reads without a context; port-notes/impl-client-b.md).
    pub(crate) async fn get_stream<F, Fut>(
        &self,
        ctx: &Ctx,
        id: NodeId,
        keys: &[[u8; 32]],
        mut emit: F,
    ) -> Result<(), Error>
    where
        F: FnMut([u8; 32], Vec<u8>) -> Fut,
        Fut: Future<Output = bool>,
    {
        let cctx = ctx.with_timeout(self.cfg().request_timeout.saturating_mul(10));
        let s = match self.pool().open(&cctx, id, ALPN_CLIENT).await {
            Ok(s) => s,
            Err(e) => {
                let err = Error::Transport(e);
                self.handle_err(id, &err);
                return Err(err);
            }
        };
        let mut s = ClosingStream(s);
        match ctx
            .run(self.get_exchange(ctx, id, keys, &mut s.0, &mut emit))
            .await
        {
            Ok(r) => r,
            Err(e) => Err(Error::Ctx(e)),
        }
    }

    async fn get_exchange<F, Fut>(
        &self,
        ctx: &Ctx,
        id: NodeId,
        keys: &[[u8; 32]],
        s: &mut Stream,
        emit: &mut F,
    ) -> Result<(), Error>
    where
        F: FnMut([u8; 32], Vec<u8>) -> Fut,
        Fut: Future<Output = bool>,
    {
        let mut m = Msg {
            typ: T_GET,
            keys: dstore_wire::raw_keys(keys),
            ..Msg::default()
        };
        self.stamp(&mut m);
        dstore_wire::write_msg(&mut s.send, &m)
            .await
            .map_err(wire_error)?;
        s.close_write();
        if let Err(e) = dstore_wire::expect(&mut s.recv, T_ABSENT).await {
            let err = wire_error(e);
            self.handle_err(id, &err);
            return Err(err);
        }
        let mut records = PackRecords::new(PackReader::new(&mut s.recv));
        while let Some(item) = records.next().await {
            let raw = item.map_err(Error::PackRecords)?;
            let Ok((k, rec)) = verify_record(&raw) else {
                continue; // a corrupt copy: the next owner is asked
            };
            if !emit(k, rec).await {
                return ctx.err().map_or(Ok(()), |e| Err(Error::Ctx(e)));
            }
        }
        let _ = records.drain().await;
        self.ok(id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use amber_store_core::key::Type;
    use dstore_testkit::golden::{self, decimal_i64, decimal_u64};
    use dstore_testkit::splitmix;
    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    struct FetchVectors {
        get_batch_keys: usize,
        get_batch_bytes: usize,
        get_est_max: u64,
        est_size: Vec<EstCase>,
        pick_batch: Vec<PickCase>,
    }

    #[derive(Deserialize)]
    struct EstCase {
        name: String,
        key: String,
        #[serde(deserialize_with = "decimal_u64")]
        length: u64,
        #[serde(deserialize_with = "decimal_i64")]
        est: i64,
    }

    #[derive(Deserialize)]
    struct PickCase {
        name: String,
        accs: Vec<AccCase>,
        jobs: Vec<JobCase>,
    }

    #[derive(Deserialize)]
    struct AccCase {
        node: String,
        keys: Vec<KeyRun>,
        #[serde(deserialize_with = "decimal_i64")]
        bytes: i64,
    }

    #[derive(Deserialize)]
    struct KeyRun {
        #[serde(deserialize_with = "decimal_u64")]
        length: u64,
        count: usize,
    }

    #[derive(Deserialize)]
    struct JobCase {
        node: usize,
        count: usize,
        #[serde(deserialize_with = "decimal_i64")]
        bytes: i64,
    }

    fn arr32(b: &[u8]) -> [u8; 32] {
        match <[u8; 32]>::try_from(b) {
            Ok(a) => a,
            Err(_) => panic!("expected 32 bytes, got {}", b.len()),
        }
    }

    #[test]
    fn constants_match_go() {
        let v: FetchVectors = golden::load_json("client/fetch.json");
        assert_eq!(v.get_batch_keys, GET_BATCH_KEYS);
        assert_eq!(v.get_batch_bytes, GET_BATCH_BYTES);
        assert_eq!(v.get_est_max, GET_EST_MAX);
    }

    #[test]
    fn est_size_vectors() {
        let v: FetchVectors = golden::load_json("client/fetch.json");
        assert!(!v.est_size.is_empty());
        for c in &v.est_size {
            let k = arr32(&golden::hex(&c.key));
            assert_eq!(Key(k).length(), c.length, "{}: length", c.name);
            assert_eq!(est_size_i64(&k), c.est, "{}: est", c.name);
            assert_eq!(
                est_size(&k),
                usize::try_from(c.est).unwrap_or(0),
                "{}",
                c.name
            );
        }
        // The cases that wrap in Go are present.
        assert!(v.est_size.iter().any(|c| c.est < 0));
        assert!(
            v.est_size
                .iter()
                .any(|c| c.length == u64::MAX && c.est == 45)
        );
    }

    #[test]
    fn pick_batch_vectors() {
        let v: FetchVectors = golden::load_json("client/fetch.json");
        assert!(!v.pick_batch.is_empty());
        for c in &v.pick_batch {
            let mut seed: u64 = 0x5049_434b;
            let mut acc: Accs = Vec::new();
            let mut ids = Vec::new();
            for a in &c.accs {
                let node = NodeId(arr32(&golden::hex(&a.node)));
                ids.push(node);
                let mut fa = FetchAcc::default();
                for run in &a.keys {
                    for _ in 0..run.count {
                        let hash = arr32(&splitmix::data(seed, 32));
                        seed += 1;
                        let k = Key::new_from_hash(Type::Blob, run.length, hash);
                        fa.bytes = fa.bytes.wrapping_add(est_size_i64(&k.0));
                        fa.keys.push(FetchKey {
                            key: k.0,
                            attempt: 0,
                        });
                    }
                }
                assert_eq!(fa.bytes, a.bytes, "{}: accumulator bytes", c.name);
                acc.push((node, fa));
            }
            let fronts: Vec<(NodeId, Vec<FetchKey>)> =
                acc.iter().map(|(id, a)| (*id, a.keys.clone())).collect();
            let mut taken = vec![0usize; ids.len()];
            for (step, want) in c.jobs.iter().enumerate() {
                let Some(job) = pick_batch(&mut acc) else {
                    panic!("{}: step {step}: no job, want {:?}", c.name, want.node);
                };
                let idx = ids.iter().position(|id| *id == job.node);
                assert_eq!(idx, Some(want.node), "{}: step {step}: node", c.name);
                assert_eq!(job.keys.len(), want.count, "{}: step {step}: count", c.name);
                let bytes = job
                    .keys
                    .iter()
                    .fold(0i64, |b, fk| b.wrapping_add(est_size_i64(&fk.key)));
                assert_eq!(bytes, want.bytes, "{}: step {step}: bytes", c.name);
                let front = &fronts[want.node].1;
                let start = taken[want.node];
                assert_eq!(
                    job.keys,
                    front[start..start + want.count],
                    "{}: step {step}: keys come from the front",
                    c.name
                );
                taken[want.node] += want.count;
            }
            assert!(pick_batch(&mut acc).is_none(), "{}: extra job", c.name);
            assert!(acc.is_empty(), "{}: accumulators left", c.name);
        }
    }

    #[test]
    fn pick_batch_prefers_the_first_of_equal_accumulators() {
        let node = |b: u8| NodeId([b; 32]);
        let fk = |b: u8| FetchKey {
            key: Key::new_from_hash(Type::Blob, 10, [b; 32]).0,
            attempt: 0,
        };
        let mut acc: Accs = vec![
            (
                node(1),
                FetchAcc {
                    keys: vec![fk(1)],
                    bytes: 56,
                },
            ),
            (
                node(2),
                FetchAcc {
                    keys: vec![fk(2)],
                    bytes: 56,
                },
            ),
        ];
        let job = pick_batch(&mut acc);
        assert_eq!(job.map(|j| j.node), Some(node(1)));
        assert_eq!(acc.len(), 1);
    }

    #[test]
    fn pick_batch_takes_an_oversized_first_key_alone() {
        // A negative estimate (length 2^63) keeps an accumulator "not full" by bytes, as in Go.
        let mut k = [0u8; 32];
        k[0] = 0x07; // Blob, 8-byte length field
        k[1] = 0x80;
        assert!(est_size_i64(&k) < 0);
        let mut acc: Accs = vec![(
            NodeId([9; 32]),
            FetchAcc {
                keys: vec![
                    FetchKey { key: k, attempt: 0 },
                    FetchKey { key: k, attempt: 1 },
                ],
                bytes: est_size_i64(&k).wrapping_mul(2),
            },
        )];
        let job = pick_batch(&mut acc);
        assert_eq!(job.map(|j| j.keys.len()), Some(2));
        assert!(acc.is_empty());
    }
}
