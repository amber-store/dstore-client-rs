//! `client/tree.go`: `Push`, `Pull`, `PullTree` (part B).
//!
//! Spec: port-notes/client-transfer.md §2.6-§2.8, §4.3; core-rs-gaps G12.

use std::collections::{HashMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use amber_store_core::amberpack::{self, REC_HEADER_SIZE};
use amber_store_core::fstree;
use amber_store_core::key::{self, Key, Type};
use amber_store_core::packstore::{self, Object, WriteOpts};
use amber_store_core::reference::Reference;
use dstore_gocompat::fmt::{hex_lower, v_strings};
use dstore_gocompat::slog::Attr;
use dstore_gocompat::time::{GoTime, MILLISECOND, duration_round, duration_to_ns};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::fetch::{Fetcher, lock};
use crate::objects::len_i64;
use crate::progress::{Tracker, count_keys};
use crate::{
    Cluster, Cond, Ctx, CtxError, Error, NodeId, PULL_WRITE_BYTES, Progress, ProgressReport,
    RecordSizer, RecordSource, rate,
};

/// `client.PushStats`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PushStats {
    pub keys: i64,
    pub uploaded: i64,
    pub bytes: i64,
    pub version: Vec<u8>,
}

/// `client.PullStats`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullStats {
    pub keys: i64,
    pub fetched: i64,
    pub bytes: i64,
    pub root: key::Key,
    pub record: Vec<u8>,
    pub version: Vec<u8>,
}

/// root = Key([0; 32]).
impl Default for PullStats {
    fn default() -> PullStats {
        PullStats {
            keys: 0,
            fetched: 0,
            bytes: 0,
            root: Key([0; 32]),
            record: Vec::new(),
            version: Vec::new(),
        }
    }
}

/// The object type of a key, or None for a reserved type nibble (core-rs `Key::type_` would panic; Go
/// returns `Type(n)`, which is neither a leaf nor a directory type).
fn key_type(k: &Key) -> Option<Type> {
    Type::from_u8(k.0[0] >> 4)
}

/// A Blob or XattrSet: nothing below it.
fn is_leaf(k: &Key) -> bool {
    matches!(key_type(k), Some(Type::Blob | Type::XattrSet))
}

/// Go `k[:8]`.
fn key8(k: &[u8; 32]) -> [u8; 8] {
    let mut b = [0u8; 8];
    b.copy_from_slice(&k[..8]);
    b
}

/// A task failure of `spawn_blocking` (a core-rs panic).
fn join_error(e: tokio::task::JoinError) -> Error {
    Error::Other(e.to_string())
}

/// `fstree.ReachableKeys(root, local.Get)`, off the async threads.
async fn reachable_keys(local: &Arc<packstore::Store>, root: Key) -> Result<Vec<Key>, Error> {
    if key_type(&root).is_none() {
        // Go reads such a root as an interior node; the store cannot hold it, so the read fails.
        if let Err(source) = local.get(root) {
            return Err(Error::WalkLocalTree(fstree::WalkError::Read {
                key: root,
                source,
            }));
        }
    }
    let l = Arc::clone(local);
    match tokio::task::spawn_blocking(move || fstree::reachable_keys(root, |k| l.get(k))).await {
        Ok(Ok(keys)) => Ok(keys),
        Ok(Err(e)) => Err(Error::WalkLocalTree(e)),
        Err(e) => Err(join_error(e)),
    }
}

/// `storedSizer`: 46 + slen when stored, else the key's length.
pub(crate) fn stored_size_of(st: &packstore::Store, k: &[u8; 32]) -> usize {
    match st.stored_size(Key(*k)) {
        Ok(Some(n)) => REC_HEADER_SIZE + n as usize,
        _ => Key(*k).length() as usize,
    }
}

/// `mergeIDs`: `a ++ b` without duplicates, first occurrence kept.
pub(crate) fn merge_ids(a: &[NodeId], b: &[NodeId]) -> Vec<NodeId> {
    let mut seen: HashSet<NodeId> = HashSet::with_capacity(a.len() + b.len());
    let mut out = Vec::with_capacity(a.len() + b.len());
    for id in a.iter().chain(b) {
        if seen.insert(*id) {
            out.push(*id);
        }
    }
    out
}

/// The error of `shortError`: `names` are `"<ShortID> (<n> keys)"` per owner not confirming, in first-use
/// order (Go ranges over a map; DD-10).
pub(crate) fn not_placed(count: usize, missing: &[(NodeId, i64)]) -> Error {
    let names: Vec<String> = missing
        .iter()
        .map(|(id, n)| format!("{} ({n} keys)", id.short()))
        .collect();
    Error::NotPlaced {
        count,
        names: v_strings(&names),
    }
}

/// The holders known for a key (Go: a nil slice when absent).
fn holders_of<'a>(holders: &'a HashMap<[u8; 32], Vec<NodeId>>, k: &[u8; 32]) -> &'a [NodeId] {
    holders.get(k).map_or(&[], Vec::as_slice)
}

/// The key whose negotiation failed, as Go's first map entry: the first in input order.
fn first_failed<'a>(
    all: &[[u8; 32]],
    failed: &'a HashMap<[u8; 32], Arc<Error>>,
) -> Option<([u8; 32], &'a Arc<Error>)> {
    if failed.is_empty() {
        return None;
    }
    all.iter()
        .find_map(|k| failed.get(k).map(|e| (*k, e)))
        .or_else(|| failed.iter().min_by_key(|(k, _)| **k).map(|(k, e)| (*k, e)))
}

/// The rejected key Push reports, as Go's first map entry: the first in input order.
fn first_rejected(
    all: &[[u8; 32]],
    mut rejected: HashMap<[u8; 32], String>,
) -> Option<([u8; 32], String)> {
    if rejected.is_empty() {
        return None;
    }
    if let Some(k) = all.iter().find(|k| rejected.contains_key(*k)) {
        return rejected.remove(k).map(|r| (*k, r));
    }
    rejected.into_iter().min_by_key(|(k, _)| *k)
}

impl Cluster {
    /// `Push`: uploads the tree under `root` from `local` and writes the reference. Three negotiate/put
    /// rounds with the ack policy checked per key, a direct fill of the short keys after round 2, a re-pin
    /// between rounds when the push runs long, then up to three ref-put attempts, renegotiating after an
    /// `incomplete` answer.
    #[allow(clippy::too_many_arguments)] // signature fixed by PORTING.md §4.8
    pub async fn push(
        &self,
        ctx: &Ctx,
        local: Arc<packstore::Store>,
        root: key::Key,
        name: &str,
        user: &str,
        cond: Cond,
        prog: Option<Progress>,
    ) -> Result<PushStats, Error> {
        let mut st = PushStats::default();
        let keys = reachable_keys(&local, root).await?;
        let all: Vec<[u8; 32]> = keys.iter().map(|k| k.0).collect();
        st.keys = len_i64(all.len());
        let src: RecordSource = {
            let l = Arc::clone(&local);
            Arc::new(move |k: &[u8; 32]| l.get_record(Key(*k)).map_err(Error::Packstore))
        };
        let size: RecordSizer = {
            let l = Arc::clone(&local);
            Arc::new(move |k: &[u8; 32]| stored_size_of(&l, k))
        };
        let tr = Tracker::new(self, prog);
        let obs = tr.observer();

        let start = Instant::now();
        let mut last_pin = start;
        let mut holders: HashMap<[u8; 32], Vec<NodeId>> = HashMap::new();
        let mut uploaded: i64 = 0;
        let mut last_err: Option<Arc<Error>> = None;
        self.probe_hinted(ctx).await;
        for round in 0..3i64 {
            let mr = self.missing(ctx, &all, true).await?;
            for (k, h) in &mr.holders {
                holders.insert(*k, h.clone());
            }
            if let Some((k, err)) = first_failed(&all, &mr.failed) {
                return Err(Error::Negotiate {
                    key8: key8(&k),
                    source: Arc::clone(err),
                });
            }
            let (lacking, lack_bytes) = count_keys(&mr.lacking, &*size);
            if round == 0 {
                let objects = len_i64(all.len());
                tr.totals(objects, objects - lacking, lack_bytes);
                self.log().info(
                    "negotiated",
                    vec![
                        Attr::int64("objects", objects),
                        Attr::int64("present", objects - lacking),
                        Attr::int64("upload", lacking),
                        Attr::int64("bytes", lack_bytes),
                        Attr::int64("primaries", len_i64(mr.lacking.len())),
                    ],
                );
            } else {
                tr.more(lack_bytes);
                self.log().info(
                    "re-sending objects short at their primaries",
                    vec![
                        Attr::int64("round", round + 1),
                        Attr::int64("objects", lacking),
                        Attr::int64("bytes", lack_bytes),
                    ],
                );
            }
            if !mr.lacking.is_empty() {
                let pr = self
                    .put(
                        ctx,
                        mr.lacking,
                        Arc::clone(&src),
                        Arc::clone(&size),
                        obs.clone(),
                    )
                    .await;
                let mut n = 0i64;
                for (k, h) in pr.holders {
                    if !holders.contains_key(&k) {
                        uploaded += 1;
                        n += 1;
                    }
                    holders.insert(k, h);
                }
                tr.objects(n);
                let mut errors: Vec<(NodeId, Arc<Error>)> = pr.errors.into_iter().collect();
                errors.sort_by_key(|(id, _)| *id);
                for (p, err) in errors {
                    // The keys stay short; the next round negotiates them at another owner, the failed
                    // one being penalised.
                    self.log().warn(
                        "upload to a primary failed, its objects go to another owner",
                        vec![Attr::string("node", p.short()), Attr::any("err", &err)],
                    );
                    last_err = Some(Arc::new(Error::UploadTo {
                        node: p.short(),
                        source: err,
                    }));
                }
                if let Some((k, reason)) = first_rejected(&all, pr.rejected) {
                    return Err(Error::RecordRejected {
                        key8: key8(&k),
                        reason,
                    });
                }
            }
            // Ack policy: every key placed?
            let short: Vec<[u8; 32]> = all
                .iter()
                .filter(|k| !self.placed(k, holders_of(&holders, k)))
                .copied()
                .collect();
            if short.is_empty() {
                break;
            }
            if round == 2 {
                let err = self.short_error(&short, &holders);
                return Err(match last_err {
                    Some(last) => Error::NotPlacedLastError {
                        placed: Box::new(err),
                        last,
                    },
                    None => err,
                });
            }
            if round == 1 {
                self.direct_fill(ctx, &short, &mut holders, &src, &size, &tr)
                    .await?;
            }
            // Re-pin everything uploaded so far if the push runs long.
            if last_pin.elapsed() > self.cfg().gc_interval / 2 {
                let _ = self.missing(ctx, &all, true).await;
                last_pin = Instant::now();
            }
        }
        st.uploaded = uploaded;
        st.bytes = tr.bytes();
        let took = start.elapsed();
        self.log().info(
            "upload complete",
            vec![
                Attr::int64("uploaded", uploaded),
                Attr::int64("bytes", st.bytes),
                Attr::duration("took", duration_round(duration_to_ns(took), MILLISECOND)),
                Attr::string("rate", rate(st.bytes, took)),
            ],
        );

        let rec = Reference {
            name: name.to_owned(),
            key: root.0.to_vec(),
            user: user.to_owned(),
            created_at: GoTime::now().unix_nano(),
            signature: Vec::new(),
            public_key: Vec::new(),
        };
        let enc = rec.encode().map_err(Error::Reference)?;
        for attempt in 0..3i64 {
            let err = match self.ref_put(ctx, &enc, &cond).await {
                Ok(version) => {
                    self.log().info(
                        "reference written",
                        vec![
                            Attr::string("name", name),
                            Attr::string("version", hex_lower(&version)),
                        ],
                    );
                    st.version = version;
                    return Ok(st);
                }
                Err(err) => err,
            };
            if err.incomplete().is_none() || attempt == 2 {
                return Err(err);
            }
            // GC may have reaped a dedup hit between negotiation and gate: re-run the whole negotiation
            // and send what is missing anywhere.
            self.log().warn(
                "reference write incomplete, renegotiating",
                vec![Attr::int64("attempt", attempt + 1), Attr::any("err", &err)],
            );
            let mut mr = self.missing(ctx, &all, true).await?;
            if !mr.lacking.is_empty() {
                let (_, lack_bytes) = count_keys(&mr.lacking, &*size);
                tr.more(lack_bytes);
                let lacking = std::mem::take(&mut mr.lacking);
                // v0.1.9: the result is ignored, so the keys just uploaded are direct-filled below.
                let _ = self
                    .put(
                        ctx,
                        lacking,
                        Arc::clone(&src),
                        Arc::clone(&size),
                        obs.clone(),
                    )
                    .await;
            }
            let short: Vec<[u8; 32]> = all
                .iter()
                .filter(|k| match mr.holders.get(*k) {
                    Some(h) => !self.placed(k, h),
                    None => true,
                })
                .copied()
                .collect();
            if !short.is_empty() {
                self.direct_fill(ctx, &short, &mut mr.holders, &src, &size, &tr)
                    .await?;
            }
            st.bytes = tr.bytes();
        }
        Err(Error::Other(
            "push: reference write did not complete".to_owned(),
        ))
    }

    /// `directFill`: sends short keys straight to every write-set owner not among their holders; the put's
    /// errors, failures and rejections are ignored. Never Err in v0.1.9.
    async fn direct_fill(
        &self,
        ctx: &Ctx,
        short: &[[u8; 32]],
        holders: &mut HashMap<[u8; 32], Vec<NodeId>>,
        src: &RecordSource,
        size: &RecordSizer,
        tr: &Arc<Tracker>,
    ) -> Result<(), Error> {
        let mut by_owner: HashMap<NodeId, Vec<[u8; 32]>> = HashMap::new();
        for k in short {
            let have = holders_of(holders, k);
            for o in self.write_set(k) {
                if !have.contains(&o) {
                    by_owner.entry(o).or_default().push(*k);
                }
            }
        }
        if by_owner.is_empty() {
            return Ok(());
        }
        let (_, bytes) = count_keys(&by_owner, &**size);
        tr.more(bytes);
        self.log().info(
            "sending short objects to their owners directly",
            vec![
                Attr::int64("objects", len_i64(short.len())),
                Attr::int64("owners", len_i64(by_owner.len())),
                Attr::int64("bytes", bytes),
            ],
        );
        let pr = self
            .put(
                ctx,
                by_owner,
                Arc::clone(src),
                Arc::clone(size),
                tr.observer(),
            )
            .await;
        for (k, h) in pr.holders {
            let merged = merge_ids(holders_of(holders, &k), &h);
            holders.insert(k, merged);
        }
        Ok(())
    }

    /// `shortError`.
    fn short_error(&self, short: &[[u8; 32]], holders: &HashMap<[u8; 32], Vec<NodeId>>) -> Error {
        let mut missing: Vec<(NodeId, i64)> = Vec::new();
        for k in short {
            let have = holders_of(holders, k);
            for o in self.write_set(k) {
                if have.contains(&o) {
                    continue;
                }
                match missing.iter_mut().find(|(id, _)| *id == o) {
                    Some((_, n)) => *n += 1,
                    None => missing.push((o, 1)),
                }
            }
        }
        not_placed(short.len(), &missing)
    }

    /// `Pull`: resolves the reference, probes hinted nodes, then `pull_tree`. The local reference is not
    /// written; the caller does that.
    pub async fn pull(
        &self,
        ctx: &Ctx,
        local: Arc<packstore::Store>,
        name: &str,
        prog: Option<Progress>,
    ) -> Result<PullStats, Error> {
        let mut st = PullStats::default();
        let r = self.ref_get(ctx, name).await?;
        let root = Key::parse(&r.reference.key).map_err(Error::Key)?;
        st.root = root;
        st.record = r.record;
        st.version = r.version;
        self.probe_hinted(ctx).await;
        self.pull_tree(ctx, local, root, &mut st, prog).await?;
        Ok(st)
    }

    /// `PullTree`: a frontier walk that fetches keys as they are discovered, prunes subtrees held complete
    /// locally, writes records as they arrive on a writer task, and ends with a completeness check.
    /// `st.keys` counts the keys that had to be fetched.
    pub async fn pull_tree(
        &self,
        ctx: &Ctx,
        local: Arc<packstore::Store>,
        root: key::Key,
        st: &mut PullStats,
        prog: Option<Progress>,
    ) -> Result<(), Error> {
        let ctx = ctx.with_cancel();
        let mut run = PullRun {
            fetcher: Fetcher::new(self, &ctx),
            writer: LocalWriter::new(Arc::clone(&local), self.cfg().jobs),
            local,
            walk: Walk::default(),
            ctx,
        };
        let res = self.pull_tree_run(&mut run, root, st, prog.as_ref()).await;
        // Go's defers: w.stop(), f.stop(), cancel().
        run.writer.stop().await;
        run.fetcher.stop().await;
        run.ctx.cancel();
        res
    }

    async fn pull_tree_run(
        &self,
        run: &mut PullRun,
        root: Key,
        st: &mut PullStats,
        prog: Option<&Progress>,
    ) -> Result<(), Error> {
        self.want(run, st, &[root]).await?;
        let mut input = run.fetcher.take_input();
        while run.walk.pending > 0 {
            let next = run.walk.queue.front().copied();
            tokio::select! {
                sent = send_key(input.as_ref(), next) => {
                    if sent {
                        run.walk.queue.pop_front();
                    } else {
                        input = None;
                    }
                }
                r = run.fetcher.recv() => {
                    let Some(r) = r else {
                        return Err(run.ctx.err().map_or(Error::FetchEndedEarly, Error::Ctx));
                    };
                    let Some(rec) = r.rec else {
                        return Err(Error::PullObjectNotFound { key8: key8(&r.key) });
                    };
                    let k = Key(r.key);
                    let kids = if is_leaf(&k) || key_type(&k).is_none() {
                        Ok(Vec::new())
                    } else {
                        children_of(k, &rec)
                    };
                    let len = len_i64(rec.len());
                    run.writer
                        .write(Object { key: k, data: Vec::new(), record: Some(rec) })
                        .await?;
                    st.fetched += 1;
                    st.bytes += len;
                    if let Some(prog) = prog {
                        prog(&ProgressReport {
                            objects: st.fetched,
                            total_objects: st.keys,
                            bytes: st.bytes,
                            total_bytes: 0,
                            nodes: Vec::new(),
                        });
                    }
                    let kids = kids?;
                    self.want(run, st, &kids).await?;
                    run.walk.pending -= 1;
                }
                () = run.writer.failed() => return run.writer.close().await,
                () = run.ctx.done() => {
                    return Err(Error::Ctx(run.ctx.err().unwrap_or(CtxError::Canceled)));
                }
            }
        }
        drop(input); // f.finish()
        run.writer.close().await?;
        let l = Arc::clone(&run.local);
        let jobs = self.cfg().jobs;
        let gate = tokio::task::spawn_blocking(move || {
            fstree::check_complete(root, |k| l.get(k), |k| l.has(k), jobs)
        })
        .await;
        match gate {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => Err(Error::PullIncomplete(e)),
            Err(e) => Err(join_error(e)),
        }
    }

    /// `want` for each key of `start` in order: nothing when held complete, expand locally when held but
    /// incomplete below, else queue the key for fetching. Iterative, with Go's pre-order queue order.
    async fn want(
        &self,
        run: &mut PullRun,
        st: &mut PullStats,
        start: &[Key],
    ) -> Result<(), Error> {
        let mut stack: Vec<Key> = start.iter().rev().copied().collect();
        while let Some(k) = stack.pop() {
            if !run.walk.seen.insert(k) {
                continue;
            }
            if run.local.has(k).map_err(Error::Packstore)? {
                match key_type(&k) {
                    Some(Type::Blob | Type::XattrSet) => continue,
                    Some(_) => {
                        if complete_locally(&run.local, k).await {
                            continue;
                        }
                        if let Ok(data) = run.local.get(k)
                            && let Ok(kids) = fstree::child_keys(k, &data)
                        {
                            stack.extend(kids.into_iter().rev());
                            continue;
                        }
                    }
                    None => {}
                }
            }
            run.walk.queue.push_back(k.0);
            run.walk.pending += 1;
            st.keys += 1;
        }
        Ok(())
    }
}

/// The children of a fetched interior record.
fn children_of(k: Key, rec: &[u8]) -> Result<Vec<Key>, Error> {
    let h = amberpack::parse_record(rec).map_err(Error::Amberpack)?;
    let data = amberpack::decode_payload(
        h.flags,
        h.ulen,
        rec.get(REC_HEADER_SIZE..).unwrap_or_default(),
    )
    .map_err(Error::Amberpack)?;
    fstree::child_keys(k, &data).map_err(Error::ChildKeys)
}

/// Go `case f.input() <- queue[0]`: pending forever without a key or once the input is closed.
async fn send_key(tx: Option<&mpsc::Sender<[u8; 32]>>, k: Option<[u8; 32]>) -> bool {
    match (tx, k) {
        (Some(tx), Some(k)) => tx.send(k).await.is_ok(),
        _ => std::future::pending().await,
    }
}

/// Whether the subtree under `root` is complete locally: the same boolean as `fstree::check_complete`
/// (every interior node readable and decodable, every leaf present), computed sequentially on a blocking
/// thread (core-rs-gaps G12).
async fn complete_locally(local: &Arc<packstore::Store>, root: Key) -> bool {
    let l = Arc::clone(local);
    tokio::task::spawn_blocking(move || subtree_complete(&l, root))
        .await
        .unwrap_or(false)
}

/// The sequential completeness check behind [`complete_locally`].
pub(crate) fn subtree_complete(local: &packstore::Store, root: Key) -> bool {
    let mut seen: HashSet<Key> = HashSet::from([root]);
    let mut stack = vec![root];
    while let Some(k) = stack.pop() {
        match key_type(&k) {
            Some(Type::Blob | Type::XattrSet) => {
                if !matches!(local.has(k), Ok(true)) {
                    return false;
                }
            }
            Some(_) => {
                let Ok(data) = local.get(k) else {
                    return false;
                };
                let Ok(kids) = fstree::child_keys(k, &data) else {
                    return false;
                };
                for kid in kids {
                    if seen.insert(kid) {
                        stack.push(kid);
                    }
                }
            }
            None => return false,
        }
    }
    true
}

/// The walk state of `PullTree`.
#[derive(Default)]
struct Walk {
    seen: HashSet<Key>,
    queue: VecDeque<[u8; 32]>,
    pending: i64,
}

/// What one `pull_tree` owns.
struct PullRun {
    ctx: Ctx,
    local: Arc<packstore::Store>,
    fetcher: Fetcher,
    writer: LocalWriter,
    walk: Walk,
}

/// `localWriter`: writes fetched records to the local store in 16 MiB batches on a task of its own, so
/// that writing overlaps fetching.
pub(crate) struct LocalWriter {
    batch: Vec<Object>,
    bytes: usize,
    tx: Option<mpsc::Sender<Vec<Object>>>,
    failed: CancellationToken,
    err: Arc<Mutex<Option<Error>>>,
    done: Option<JoinHandle<()>>,
    closed: bool,
}

impl LocalWriter {
    /// `newLocalWriter`.
    pub(crate) fn new(local: Arc<packstore::Store>, jobs: usize) -> LocalWriter {
        let (tx, mut rx) = mpsc::channel::<Vec<Object>>(1);
        let failed = CancellationToken::new();
        let err: Arc<Mutex<Option<Error>>> = Arc::new(Mutex::new(None));
        let (task_failed, task_err) = (failed.clone(), Arc::clone(&err));
        let done = tokio::spawn(async move {
            while let Some(objs) = rx.recv().await {
                if lock(&task_err).is_some() {
                    continue;
                }
                let l = Arc::clone(&local);
                let r = tokio::task::spawn_blocking(move || {
                    let opts = WriteOpts {
                        writers: jobs,
                        batch_size: 0,
                        verify: false,
                    };
                    l.write_parallel(objs.into_iter().map(Ok::<Object, Infallible>), opts)
                        .1
                })
                .await;
                let e = match r {
                    Ok(Ok(())) => None,
                    Ok(Err(e)) => Some(Error::Packstore(e)),
                    Err(e) => Some(join_error(e)),
                };
                if let Some(e) = e {
                    *lock(&task_err) = Some(e);
                    task_failed.cancel();
                }
            }
        });
        LocalWriter {
            batch: Vec::new(),
            bytes: 0,
            tx: Some(tx),
            failed,
            err,
            done: Some(done),
            closed: false,
        }
    }

    /// Resolves once a batch failed (Go `<-w.failed`).
    pub(crate) async fn failed(&self) {
        self.failed.cancelled().await;
    }

    /// `write`: queues a record; hands the batch over once it reaches 16 MiB.
    pub(crate) async fn write(&mut self, o: Object) -> Result<(), Error> {
        self.bytes += o.record.as_ref().map_or(0, Vec::len);
        self.batch.push(o);
        if self.bytes < PULL_WRITE_BYTES {
            return Ok(());
        }
        self.flush().await
    }

    /// `flush`.
    async fn flush(&mut self) -> Result<(), Error> {
        if self.batch.is_empty() {
            return Ok(());
        }
        let objs = std::mem::take(&mut self.batch);
        self.bytes = 0;
        let Some(tx) = self.tx.as_ref() else {
            return Ok(());
        };
        tokio::select! {
            _ = tx.send(objs) => Ok(()),
            () = self.failed.cancelled() => Err(self.take_err()),
        }
    }

    fn take_err(&self) -> Error {
        lock(&self.err)
            .take()
            .unwrap_or_else(|| Error::Other("pull: local write failed".to_owned()))
    }

    /// `close`: writes what is left and waits for the writer.
    pub(crate) async fn close(&mut self) -> Result<(), Error> {
        if self.closed {
            return match lock(&self.err).take() {
                Some(e) => Err(e),
                None => Ok(()),
            };
        }
        self.closed = true;
        let r = self.flush().await;
        self.tx = None;
        if let Some(h) = self.done.take() {
            let _ = h.await;
        }
        r?;
        match lock(&self.err).take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// `stop`: abandons the unwritten batch and waits for the writer.
    pub(crate) async fn stop(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.tx = None;
        if let Some(h) = self.done.take() {
            let _ = h.await;
        }
    }
}

impl Drop for LocalWriter {
    /// A dropped `pull_tree` future: close the channel; queued batches are still written by the task.
    fn drop(&mut self) {
        self.tx = None;
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use amber_store_core::amberpack::encode_record;
    use amber_store_core::fstree::{Entry, encode_blob, encode_dir_leaf};
    use dstore_testkit::splitmix;

    use super::*;

    static DIRS: AtomicUsize = AtomicUsize::new(0);

    /// A scratch directory removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> TempDir {
            let n = DIRS.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "dstore-client-tree-test-{}-{n}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            if let Err(e) = std::fs::create_dir_all(&dir) {
                panic!("mkdir {}: {e}", dir.display());
            }
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn open_store(dir: &TempDir) -> Arc<packstore::Store> {
        match packstore::Store::open_with(dir.0.join("packstore"), packstore::Options::new()) {
            Ok(s) => Arc::new(s),
            Err(e) => panic!("open packstore: {e}"),
        }
    }

    fn ok<T, E: std::fmt::Display>(r: Result<T, E>) -> T {
        match r {
            Ok(v) => v,
            Err(e) => panic!("{e}"),
        }
    }

    /// A DirLeaf root over two blob files.
    fn small_tree(st: &packstore::Store) -> (Key, Vec<Key>) {
        let a = encode_blob(&splitmix::data(11, 3000));
        let b = encode_blob(&splitmix::data(12, 500));
        let entry = |name: &str, k: Key| Entry {
            name: name.as_bytes().to_vec(),
            mode: 0o100644,
            content_key: k.0.to_vec(),
            ..Entry::default()
        };
        let leaf = ok(encode_dir_leaf(&[entry("a", a.key), entry("b", b.key)]));
        ok(st.put(a.key, &a.bytes));
        ok(st.put(b.key, &b.bytes));
        ok(st.put(leaf.key, &leaf.bytes));
        (leaf.key, vec![a.key, b.key])
    }

    #[test]
    fn merge_ids_keeps_first_occurrences_in_order() {
        let id = |b: u8| NodeId([b; 32]);
        assert_eq!(
            merge_ids(&[id(3), id(1), id(3)], &[id(2), id(1), id(4)]),
            vec![id(3), id(1), id(2), id(4)]
        );
        assert!(merge_ids(&[], &[]).is_empty());
    }

    #[test]
    fn not_placed_renders_go_percent_v() {
        let mut a = [0u8; 32];
        a[0] = 3;
        let mut b = [0u8; 32];
        b[0] = 4;
        assert_eq!(
            not_placed(6, &[(NodeId(a), 5), (NodeId(b), 2)]).to_string(),
            "push: 6 keys could not be placed; owners not confirming: [03000000 (5 keys) 04000000 (2 keys)]"
        );
        assert_eq!(
            not_placed(0, &[]).to_string(),
            "push: 0 keys could not be placed; owners not confirming: []"
        );
    }

    #[test]
    fn first_failed_and_rejected_follow_input_order() {
        let all = [[1u8; 32], [2u8; 32], [3u8; 32]];
        let mut failed = HashMap::new();
        failed.insert([3u8; 32], Arc::new(Error::NoOwners));
        failed.insert([2u8; 32], Arc::new(Error::FetchEndedEarly));
        assert_eq!(first_failed(&all, &failed).map(|(k, _)| k), Some([2u8; 32]));
        assert!(first_failed(&all, &HashMap::new()).is_none());
        let mut rejected = HashMap::new();
        rejected.insert([9u8; 32], "x".to_owned());
        rejected.insert([3u8; 32], "not-owner".to_owned());
        assert_eq!(
            first_rejected(&all, rejected),
            Some(([3u8; 32], "not-owner".to_owned()))
        );
        let mut stranger = HashMap::new();
        stranger.insert([9u8; 32], "a".to_owned());
        stranger.insert([8u8; 32], "b".to_owned());
        assert_eq!(
            first_rejected(&all, stranger),
            Some(([8u8; 32], "b".to_owned()))
        );
    }

    #[test]
    fn stored_size_of_uses_the_index_then_the_length_field() {
        let dir = TempDir::new();
        let st = open_store(&dir);
        let data = splitmix::data(5, 1000);
        let blob = encode_blob(&data);
        ok(st.put(blob.key, &blob.bytes));
        let rec = ok(st.get_record(blob.key));
        assert_eq!(stored_size_of(&st, &blob.key.0), rec.len());
        // Not stored: the key's length field (for a tree object, its subtree size).
        let other = Key::new(Type::DirNode, 123_456, b"x");
        assert_eq!(stored_size_of(&st, &other.0), 123_456);
    }

    #[test]
    fn subtree_complete_matches_check_complete() {
        let dir = TempDir::new();
        let st = open_store(&dir);
        let (root, blobs) = small_tree(&st);
        assert!(subtree_complete(&st, root));
        assert!(fstree::check_complete(root, |k| st.get(k), |k| st.has(k), 2).is_ok());
        // A blob that is not stored.
        let dir2 = TempDir::new();
        let st2 = open_store(&dir2);
        let leaf = ok(st.get(root));
        ok(st2.put(root, &leaf));
        ok(st2.put(blobs[0], &ok(st.get(blobs[0]))));
        assert!(!subtree_complete(&st2, root));
        assert!(fstree::check_complete(root, |k| st2.get(k), |k| st2.has(k), 2).is_err());
        // An interior node that is not stored.
        let dir3 = TempDir::new();
        let st3 = open_store(&dir3);
        assert!(!subtree_complete(&st3, root));
        assert!(!subtree_complete(&st3, Key([0x50; 32])));
        assert!(subtree_complete(&st, blobs[1]));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn local_writer_writes_batches_and_reports_failures() {
        let dir = TempDir::new();
        let st = open_store(&dir);
        let mut w = LocalWriter::new(Arc::clone(&st), 2);
        let mut keys = Vec::new();
        // Enough bytes for two handovers at 16 MiB plus a remainder.
        for i in 0..40u64 {
            let data = splitmix::data(100 + i, 1 << 20);
            let blob = encode_blob(&data);
            let rec = ok(encode_record(blob.key, &blob.bytes));
            keys.push(blob.key);
            ok(w.write(Object {
                key: blob.key,
                data: Vec::new(),
                record: Some(rec),
            })
            .await);
        }
        ok(w.close().await);
        for k in &keys {
            assert!(ok(st.has(*k)), "{k} not written");
        }
        assert!(w.close().await.is_ok());

        // A record under the wrong key fails the batch, and the error surfaces once.
        let mut w = LocalWriter::new(Arc::clone(&st), 2);
        let a = encode_blob(&splitmix::data(1, 10));
        let b = encode_blob(&splitmix::data(2, 10));
        let rec_b = ok(encode_record(b.key, &b.bytes));
        ok(w.write(Object {
            key: a.key,
            data: Vec::new(),
            record: Some(rec_b),
        })
        .await);
        match w.close().await {
            Err(e) => assert!(e.to_string().contains("does not match"), "{e}"),
            Ok(()) => panic!("a mismatched record was written"),
        }
        assert!(!ok(st.has(a.key)));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn local_writer_stop_drops_the_unwritten_batch() {
        let dir = TempDir::new();
        let st = open_store(&dir);
        let mut w = LocalWriter::new(Arc::clone(&st), 1);
        let a = encode_blob(&splitmix::data(3, 10));
        let rec = ok(encode_record(a.key, &a.bytes));
        ok(w.write(Object {
            key: a.key,
            data: Vec::new(),
            record: Some(rec),
        })
        .await);
        w.stop().await;
        assert!(!ok(st.has(a.key)));
    }
}
