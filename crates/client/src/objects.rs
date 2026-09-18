//! `client/objects.go`: `Missing`, `Put`, `Placed`, `Get`, `VerifyRecord` (part B).
//!
//! Spec: port-notes/client-transfer.md §2.2-§2.5, §4.3.

use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use amber_store_core::amberpack::{self, REC_HEADER_SIZE};
use amber_store_core::key::Key;
use dstore_gocompat::slog::Attr;
use dstore_gocompat::time::{MILLISECOND, duration_round, duration_to_ns};
use dstore_view::ids_of;
use dstore_wire::{
    ALPN_CLIENT, CODE_BUSY, CODE_STALE_VIEW, KeyFailure, Msg, PackSender, T_MISSING, T_PUT,
    T_PUT_RESULT,
};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::fetch::{ClosingStream, Fetcher, add_key, lock, wire_error};
use crate::{BATCH_KEYS, Cluster, Ctx, Error, NodeId, PutObserver, RecordSizer, rate};

/// Reads a record to upload.
pub type RecordSource = Arc<dyn Fn(&[u8; 32]) -> Result<Vec<u8>, Error> + Send + Sync>;

/// `client.MissingResult`.
pub struct MissingResult {
    pub lacking: HashMap<NodeId, Vec<[u8; 32]>>,
    pub holders: HashMap<[u8; 32], Vec<NodeId>>,
    pub failed: HashMap<[u8; 32], Arc<Error>>,
}

impl MissingResult {
    fn empty() -> MissingResult {
        MissingResult {
            lacking: HashMap::new(),
            holders: HashMap::new(),
            failed: HashMap::new(),
        }
    }
}

/// `client.PutResult`.
pub struct PutResult {
    pub holders: HashMap<[u8; 32], Vec<NodeId>>,
    pub failed: HashMap<[u8; 32], Vec<KeyFailure>>,
    pub rejected: HashMap<[u8; 32], String>,
    pub errors: HashMap<NodeId, Arc<Error>>,
}

impl PutResult {
    fn empty() -> PutResult {
        PutResult {
            holders: HashMap::new(),
            failed: HashMap::new(),
            rejected: HashMap::new(),
            errors: HashMap::new(),
        }
    }

    /// `(*PutResult).merge`: folds one batch reply in; entries whose key is not 32 bytes are skipped. The
    /// reply's incarnation and epoch are not looked at.
    fn merge(&mut self, resp: &Msg) {
        for h in &resp.holders {
            if let Some(k) = key32(h.key.as_deref()) {
                self.holders.insert(k, ids_of(&h.holders));
            }
        }
        for f in &resp.failed {
            if let Some(k) = key32(f.key.as_deref()) {
                self.failed.entry(k).or_default().push(f.clone());
            }
        }
        for rj in &resp.rejected {
            if let Some(k) = key32(rj.key.as_deref()) {
                self.rejected.insert(k, rj.reason.clone());
            }
        }
    }
}

/// Go `len(b) == 32` then `[32]byte(b)`.
fn key32(b: Option<&[u8]>) -> Option<[u8; 32]> {
    b.and_then(|b| <[u8; 32]>::try_from(b).ok())
}

/// Go `len()` as an slog Int64 attribute.
pub(crate) fn len_i64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// One record of a `Get`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GetResult {
    pub key: [u8; 32],
    pub record: Vec<u8>,
}

type GetInner = Pin<Box<dyn futures::Stream<Item = Result<GetResult, Error>> + Send>>;

/// Lazy: nothing starts until first polled; dropping it cancels the fetcher. Records arrive in arrival
/// order. A ctx error after the last record is yielded as Err (Go yields ctx.Err()).
pub struct GetStream {
    inner: GetInner,
    missing: Arc<Mutex<Vec<[u8; 32]>>>,
}

impl futures::Stream for GetStream {
    type Item = Result<GetResult, Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().inner.as_mut().poll_next(cx)
    }
}

impl GetStream {
    /// The keys no owner returned so far. Accumulates across polls, as Go.
    pub fn missing(&self) -> Vec<[u8; 32]> {
        lock(&self.missing).clone()
    }
}

/// The negotiation state `askPrimaries` updates under its mutex.
struct Negotiation {
    res: MissingResult,
    tried: HashMap<[u8; 32], HashSet<NodeId>>,
    retry: Vec<[u8; 32]>,
}

/// What every put task shares.
#[derive(Clone)]
struct PutJob {
    c: Cluster,
    ctx: Ctx,
    src: RecordSource,
    size: RecordSizer,
    obs: PutObserver,
    res: Arc<Mutex<PutResult>>,
    sem: Arc<Semaphore>,
}

impl PutJob {
    /// The per-primary goroutine of `Put`: batches in order, `Conns` in flight, the global `Jobs` permit
    /// taken after the slot; the primary's remaining batches are not sent once one failed.
    async fn primary(self, p: NodeId, ks: Vec<[u8; 32]>) {
        let slots = Arc::new(Semaphore::new(self.c.cfg().conns.max(1)));
        let mut batches = JoinSet::new();
        for b in crate::batch::batches(&ks, &*self.size, self.c.cfg().batch_bytes, BATCH_KEYS) {
            let failed = lock(&self.res).errors.contains_key(&p);
            if failed {
                break; // the primary's remaining batches are not worth sending
            }
            let Ok(slot) = Arc::clone(&slots).acquire_owned().await else {
                break;
            };
            let Ok(permit) = Arc::clone(&self.sem).acquire_owned().await else {
                break;
            };
            let job = self.clone();
            batches.spawn(async move {
                let r = job
                    .c
                    .put_batch(&job.ctx, p, &b, &job.src, &job.size, &job.obs)
                    .await;
                {
                    let mut res = lock(&job.res);
                    match r {
                        Err(e) => {
                            res.errors.entry(p).or_insert_with(|| Arc::new(e));
                        }
                        Ok(resp) => res.merge(&resp),
                    }
                }
                drop(permit);
                drop(slot);
            });
        }
        while batches.join_next().await.is_some() {}
    }
}

/// `defer obs.Done(p, flushed)`: runs on every exit path of `putOnce`, after the stream is closed.
struct DoneGuard<'a> {
    obs: &'a PutObserver,
    node: NodeId,
    flushed: bool,
}

impl Drop for DoneGuard<'_> {
    fn drop(&mut self) {
        if let Some(done) = &self.obs.done {
            done(self.node, self.flushed);
        }
    }
}

impl Cluster {
    /// `Missing`: groups keys by primary and asks each which it lacks, in chunks of 8192 keys, `Jobs` at a
    /// time. A key whose primary failed at the transport level is asked at its next owner; a remote error
    /// fails the key outright. Never Err in v0.1.9.
    pub async fn missing(
        &self,
        ctx: &Ctx,
        keys: &[[u8; 32]],
        pin: bool,
    ) -> Result<MissingResult, Error> {
        let state = Arc::new(Mutex::new(Negotiation {
            res: MissingResult::empty(),
            tried: HashMap::new(),
            retry: Vec::new(),
        }));
        let mut pending: Vec<[u8; 32]> = keys.to_vec();
        let mut attempt: i64 = 0;
        while !pending.is_empty() {
            // Primaries in first-use order, keys in input order (Go: a map; DD-10).
            let mut by_primary: Vec<(NodeId, Vec<[u8; 32]>)> = Vec::new();
            {
                let mut st = lock(&state);
                for k in &pending {
                    match self.primary_except(k, st.tried.get(k)) {
                        Some(p) => match by_primary.iter_mut().find(|(id, _)| *id == p) {
                            Some((_, ks)) => ks.push(*k),
                            None => by_primary.push((p, vec![*k])),
                        },
                        None => {
                            // Keep the last transport error when there is one.
                            st.res
                                .failed
                                .entry(*k)
                                .or_insert_with(|| Arc::new(Error::NoOwners));
                        }
                    }
                }
            }
            pending = self.ask_primaries(ctx, by_primary, pin, &state).await;
            if !pending.is_empty() {
                self.log().warn(
                    "negotiation failed at a primary, asking the next owner",
                    vec![
                        Attr::int64("objects", len_i64(pending.len())),
                        Attr::int64("attempt", attempt + 1),
                    ],
                );
            }
            attempt += 1;
        }
        let res = std::mem::replace(&mut lock(&state).res, MissingResult::empty());
        Ok(res)
    }

    /// `primaryExcept`: the preferred owner of `k` among those not tried yet.
    fn primary_except(&self, k: &[u8; 32], exclude: Option<&HashSet<NodeId>>) -> Option<NodeId> {
        let owners = self.placement().map(|pl| pl.owners(k)).unwrap_or_default();
        self.preferred(&owners)
            .into_iter()
            .find(|o| exclude.is_none_or(|t| !t.contains(o)))
    }

    /// `askPrimaries`: one negotiation round; returns the keys to ask at their next owner, in completion
    /// order.
    async fn ask_primaries(
        &self,
        ctx: &Ctx,
        by_primary: Vec<(NodeId, Vec<[u8; 32]>)>,
        pin: bool,
        state: &Arc<Mutex<Negotiation>>,
    ) -> Vec<[u8; 32]> {
        let sem = Arc::new(Semaphore::new(self.cfg().jobs.max(1)));
        let mut set = JoinSet::new();
        for (p, ks) in by_primary {
            for chunk in ks.chunks(BATCH_KEYS) {
                let chunk = chunk.to_vec();
                let c = self.clone();
                let ctx = ctx.clone();
                let sem = Arc::clone(&sem);
                let state = Arc::clone(state);
                set.spawn(async move {
                    let _permit = sem.acquire_owned().await.ok();
                    let mut m = Msg {
                        typ: T_MISSING,
                        keys: dstore_wire::raw_keys(&chunk),
                        pin,
                        ..Msg::default()
                    };
                    let resp = c.call_retry(&ctx, p, &mut m).await;
                    let mut guard = lock(&state);
                    let st = &mut *guard;
                    match resp {
                        Err(err) => {
                            let answered = err.remote().is_some();
                            let err = Arc::new(err);
                            for k in &chunk {
                                if answered || ctx.err().is_some() {
                                    st.res.failed.insert(*k, Arc::clone(&err));
                                    continue;
                                }
                                st.tried.entry(*k).or_default().insert(p);
                                st.res.failed.insert(*k, Arc::clone(&err));
                                st.retry.push(*k);
                            }
                        }
                        Ok(resp) => {
                            // v0.1.9: a malformed lacking list is ignored, so every key counts as held.
                            let lack: HashSet<[u8; 32]> = dstore_wire::keys32(&resp.keys)
                                .unwrap_or_default()
                                .into_iter()
                                .collect();
                            let mut short: HashMap<[u8; 32], Vec<NodeId>> = HashMap::new();
                            for kh in &resp.short {
                                if let Some(k) = key32(kh.key.as_deref()) {
                                    short.insert(k, ids_of(&kh.holders));
                                }
                            }
                            for k in &chunk {
                                st.res.failed.remove(k);
                                if lack.contains(k) {
                                    st.res.lacking.entry(p).or_default().push(*k);
                                    continue;
                                }
                                let h = match short.get(k) {
                                    Some(h) => h.clone(),
                                    None => c.write_set(k), // held everywhere
                                };
                                st.res.holders.insert(*k, h);
                            }
                        }
                    }
                });
            }
        }
        while set.join_next().await.is_some() {}
        std::mem::take(&mut lock(state).retry)
    }

    /// `Put`: uploads records to their primaries in byte-balanced batches, in parallel across primaries
    /// and with `Conns` batches in flight per primary, `Jobs` batches overall.
    pub async fn put(
        &self,
        ctx: &Ctx,
        by_primary: HashMap<NodeId, Vec<[u8; 32]>>,
        src: RecordSource,
        size: RecordSizer,
        obs: PutObserver,
    ) -> PutResult {
        let res = Arc::new(Mutex::new(PutResult::empty()));
        let sem = Arc::new(Semaphore::new(self.cfg().jobs.max(1)));
        // Go ranges over a map (random); any start order is compatible (DD-10).
        let mut primaries: Vec<(NodeId, Vec<[u8; 32]>)> = by_primary.into_iter().collect();
        primaries.sort_by_key(|(id, _)| *id);
        let mut set = JoinSet::new();
        for (p, ks) in primaries {
            let job = PutJob {
                c: self.clone(),
                ctx: ctx.clone(),
                src: Arc::clone(&src),
                size: Arc::clone(&size),
                obs: obs.clone(),
                res: Arc::clone(&res),
                sem: Arc::clone(&sem),
            };
            set.spawn(job.primary(p, ks));
        }
        while set.join_next().await.is_some() {}
        std::mem::replace(&mut *lock(&res), PutResult::empty())
    }

    /// `putBatch`: up to 4 attempts; `stale-view` adopts the carried view and retries the same primary at
    /// once; `busy` sleeps `retry_after` (1 s when absent) without watching ctx.
    async fn put_batch(
        &self,
        ctx: &Ctx,
        p: NodeId,
        keys: &[[u8; 32]],
        src: &RecordSource,
        size: &RecordSizer,
        obs: &PutObserver,
    ) -> Result<Msg, Error> {
        let mut last: Option<Error> = None;
        for attempt in 0..4i64 {
            let err = match self.put_once(ctx, p, keys, src, size, obs).await {
                Ok(resp) => return Ok(resp),
                Err(err) => err,
            };
            if err.is_code(CODE_STALE_VIEW) {
                self.log().warn(
                    "upload retry",
                    vec![
                        Attr::string("node", p.short()),
                        Attr::string("reason", "stale view"),
                        Attr::int64("attempt", attempt + 1),
                    ],
                );
                self.handle_err(p, &err);
                last = Some(err);
                continue;
            }
            if err.is_code(CODE_BUSY) {
                let mut wait = Duration::from_secs(1);
                if let Some(we) = err.remote()
                    && !we.retry_after.is_zero()
                {
                    wait = we.retry_after;
                }
                self.log().warn(
                    "upload retry",
                    vec![
                        Attr::string("node", p.short()),
                        Attr::string("reason", "busy"),
                        Attr::duration("wait", duration_to_ns(wait)),
                        Attr::int64("attempt", attempt + 1),
                    ],
                );
                tokio::time::sleep(wait).await;
                last = Some(err);
                continue;
            }
            self.log().warn(
                "upload failed",
                vec![
                    Attr::string("node", p.short()),
                    Attr::int64("objects", len_i64(keys.len())),
                    Attr::any("err", &err),
                ],
            );
            return Err(err);
        }
        Err(last.unwrap_or_else(|| Error::Other("upload failed".to_owned())))
    }

    /// `putOnce`: one put stream (request frame, the pack, FIN, then the put-result).
    async fn put_once(
        &self,
        ctx: &Ctx,
        p: NodeId,
        keys: &[[u8; 32]],
        src: &RecordSource,
        size: &RecordSizer,
        obs: &PutObserver,
    ) -> Result<Msg, Error> {
        let cctx = ctx.with_timeout(self.cfg().request_timeout.saturating_mul(10));
        let total = keys
            .iter()
            .fold(0i64, |t, k| t.wrapping_add(size(k) as i64));
        if let Some(start) = &obs.start {
            start(p);
        }
        let mut done = DoneGuard {
            obs,
            node: p,
            flushed: false,
        };
        let began = Instant::now();
        let s = match self.pool().open(&cctx, p, ALPN_CLIENT).await {
            Ok(s) => s,
            Err(e) => {
                let err = Error::Transport(e);
                self.handle_err(p, &err);
                return Err(err);
            }
        };
        let mut s = ClosingStream(s);
        let mut attrs = vec![
            Attr::string("node", p.short()),
            Attr::int64("objects", len_i64(keys.len())),
            Attr::int64("bytes", total),
        ];
        attrs.extend(self.path_attrs(p));
        self.log().info("uploading", attrs);
        let mut m = Msg {
            typ: T_PUT,
            ..Msg::default()
        };
        self.stamp(&mut m);
        dstore_wire::write_msg(&mut s.0.send, &m)
            .await
            .map_err(wire_error)?;
        {
            let mut sender = PackSender::new(&mut s.0.send);
            for k in keys {
                // v0.1.9: a record that cannot be read is skipped; the key stays short.
                let Ok(rec) = src(k) else {
                    continue;
                };
                sender.add_record(&rec).await.map_err(wire_error)?;
                if let Some(sent) = &obs.sent {
                    sent(p, rec.len());
                }
            }
            sender.finish().await.map_err(wire_error)?;
        }
        s.0.close_write();
        done.flushed = true;
        if let Some(flushed) = &obs.flushed {
            flushed(p);
        }
        self.log().info(
            "batch sent, waiting for the node to store and replicate it",
            vec![
                Attr::string("node", p.short()),
                Attr::int64("objects", len_i64(keys.len())),
                Attr::int64("bytes", total),
            ],
        );
        let resp = match dstore_wire::expect(&mut s.0.recv, T_PUT_RESULT).await {
            Ok(r) => r,
            Err(e) => {
                let err = wire_error(e);
                self.handle_err(p, &err);
                return Err(err);
            }
        };
        self.ok(p);
        let took = began.elapsed();
        self.log().info(
            "uploaded",
            vec![
                Attr::string("node", p.short()),
                Attr::int64("objects", len_i64(keys.len())),
                Attr::int64("bytes", total),
                Attr::duration("took", duration_round(duration_to_ns(took), MILLISECOND)),
                Attr::string("rate", rate(total, took)),
            ],
        );
        Ok(resp)
    }

    /// `Placed`: at least `min(min_replicas, owners)` owners under `nodes` hold the key and, during a
    /// transition, as many of the pending owners. Holders that are not owners do not count, duplicates do
    /// not inflate. Without a view nothing is required.
    pub fn placed(&self, key: &[u8; 32], holders: &[NodeId]) -> bool {
        let Some(pl) = self.placement() else {
            return true;
        };
        let min_r = usize::from(pl.view().min_replicas);
        let count = |owners: &[NodeId]| owners.iter().filter(|o| holders.contains(o)).count();
        let owners = pl.owners(key);
        if count(&owners) < min_r.min(owners.len()) {
            return false;
        }
        if let Some(po) = pl.pending_owners(key)
            && count(&po) < min_r.min(po.len())
        {
            return false;
        }
        true
    }

    /// `Get`: fetches records, streaming each as it is verified; keys are batched by preferred owner and
    /// re-asked down the read order. Duplicate input keys are fetched once; unrequested and duplicate
    /// records a node sends are passed on (v0.1.9).
    pub fn get(&self, ctx: &Ctx, keys: Vec<[u8; 32]>) -> GetStream {
        let missing: Arc<Mutex<Vec<[u8; 32]>>> = Arc::new(Mutex::new(Vec::new()));
        let acc = Arc::clone(&missing);
        let c = self.clone();
        let ctx = ctx.clone();
        let inner = async_stream::stream! {
            let mut f = Fetcher::new(&c, &ctx);
            if let Some(input) = f.take_input() {
                let fctx = f.ctx().clone();
                tokio::spawn(async move {
                    let mut seen: HashSet<[u8; 32]> = HashSet::with_capacity(keys.len());
                    for k in keys {
                        if !seen.insert(k) {
                            continue;
                        }
                        if !add_key(&input, &fctx, k).await {
                            return;
                        }
                    }
                    // Dropping `input` here is Go's finish().
                });
            }
            while let Some(r) = f.recv().await {
                match r.rec {
                    None => {
                        lock(&acc).push(r.key);
                    }
                    Some(rec) => {
                        yield Ok(GetResult { key: r.key, record: rec });
                    }
                }
            }
            if let Some(e) = ctx.err() {
                yield Err(Error::Ctx(e));
            }
            f.stop().await;
        };
        GetStream {
            inner: Box::pin(inner),
            missing,
        }
    }
}

/// `client.VerifyRecord`: validates the key, decodes the payload and checks its hash; returns the key and
/// a copy of the complete record. Unlike the node, the length field of a Blob or XattrSet is not checked
/// (v0.1.9).
pub fn verify_record(raw: &amberpack::RawRecord) -> Result<([u8; 32], Vec<u8>), Error> {
    let k = raw.record.key;
    k.validate().map_err(Error::Key)?;
    let stored = raw.bytes.get(REC_HEADER_SIZE..).unwrap_or_default();
    let payload = amberpack::decode_payload(raw.record.flags, raw.record.ulen, stored)
        .map_err(Error::Amberpack)?;
    let want = Key::new(k.type_(), k.length(), &payload);
    if want != k {
        return Err(Error::PayloadHash {
            want: want.to_string(),
            key: k.to_string(),
        });
    }
    Ok((k.0, raw.bytes.clone()))
}

#[cfg(test)]
mod tests {
    use amber_store_core::amberpack::{Record, encode_record, parse_record};
    use amber_store_core::key::Type;
    use dstore_testkit::splitmix;
    use dstore_wire::{KeyHolders, KeyReject};

    use super::*;

    fn raw_of(rec: &[u8]) -> amberpack::RawRecord {
        match parse_record(rec) {
            Ok(record) => amberpack::RawRecord {
                record,
                bytes: rec.to_vec(),
            },
            Err(e) => panic!("parse_record: {e}"),
        }
    }

    fn k1_record() -> (Key, Vec<u8>) {
        let data = splitmix::data(1, 100);
        let k = Key::new(Type::Blob, 100, &data);
        match encode_record(k, &data) {
            Ok(rec) => (k, rec),
            Err(e) => panic!("encode_record: {e}"),
        }
    }

    #[test]
    fn verify_record_accepts_a_raw_record() {
        let (k, rec) = k1_record();
        assert_eq!(
            k.to_string(),
            "00644b75a8842dc16ded120ee1f96d229ac409028f4d5d429499a4ad548aae85"
        );
        match verify_record(&raw_of(&rec)) {
            Ok((got, out)) => {
                assert_eq!(got, k.0);
                assert_eq!(out, rec);
            }
            Err(e) => panic!("verify_record: {e}"),
        }
    }

    #[test]
    fn verify_record_refuses_a_flipped_payload_byte() {
        let (k, rec) = k1_record();
        let mut bad = rec.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        // Build the RawRecord from the original header, as a stream with a recomputed CRC would.
        let raw = amberpack::RawRecord {
            record: Record {
                key: k,
                flags: 0,
                ulen: 100,
                slen: 100,
            },
            bytes: bad,
        };
        match verify_record(&raw) {
            Err(Error::PayloadHash { key, .. }) => assert_eq!(key, k.to_string()),
            other => panic!("want a payload hash error, got {:?}", other.map(|r| r.0)),
        }
    }

    #[test]
    fn verify_record_validates_the_key_first() {
        let (mut k, rec) = k1_record();
        k.0[0] |= 0x08;
        let raw = amberpack::RawRecord {
            record: Record {
                key: k,
                flags: 0,
                ulen: 100,
                slen: 100,
            },
            bytes: rec,
        };
        match verify_record(&raw) {
            Err(e) => assert_eq!(e.to_string(), "key: reserved header bit is set"),
            Ok(_) => panic!("reserved bit accepted"),
        }
    }

    #[test]
    fn verify_record_survives_a_short_record() {
        let (k, _) = k1_record();
        let raw = amberpack::RawRecord {
            record: Record {
                key: k,
                flags: 0,
                ulen: 100,
                slen: 100,
            },
            bytes: vec![1, 2, 3],
        };
        assert!(verify_record(&raw).is_err());
    }

    #[test]
    fn merge_skips_bad_keys_replaces_holders_and_appends_failures() {
        let k = [7u8; 32];
        let id = |b: u8| {
            let mut v = vec![0u8; 32];
            v[0] = b;
            v
        };
        let mut r = PutResult::empty();
        let reply = Msg {
            holders: vec![
                KeyHolders {
                    key: Some(k.to_vec()),
                    holders: vec![id(1), id(2)],
                },
                KeyHolders {
                    key: Some(vec![1; 31]),
                    holders: vec![id(3)],
                },
                KeyHolders {
                    key: None,
                    holders: vec![id(4)],
                },
            ],
            failed: vec![KeyFailure {
                key: Some(k.to_vec()),
                node: Some(id(3)),
                reason: "busy".to_owned(),
                retry_after: 1234,
            }],
            rejected: vec![KeyReject {
                key: Some(k.to_vec()),
                reason: "not-owner".to_owned(),
            }],
            ..Msg::default()
        };
        r.merge(&reply);
        let second = Msg {
            holders: vec![KeyHolders {
                key: Some(k.to_vec()),
                holders: vec![vec![5; 16], vec![6; 40]],
            }],
            failed: reply.failed.clone(),
            rejected: vec![KeyReject {
                key: Some(k.to_vec()),
                reason: "verify: x".to_owned(),
            }],
            ..Msg::default()
        };
        r.merge(&second);
        assert_eq!(r.holders.len(), 1);
        let mut padded = [0u8; 32];
        padded[..16].copy_from_slice(&[5; 16]);
        assert_eq!(r.holders[&k], vec![NodeId(padded), NodeId([6; 32])]);
        assert_eq!(r.failed[&k].len(), 2);
        assert_eq!(r.rejected[&k], "verify: x");
        assert!(r.errors.is_empty());
    }

    #[test]
    fn key32_needs_exactly_32_bytes() {
        assert_eq!(key32(Some(&[3u8; 32])), Some([3u8; 32]));
        assert_eq!(key32(Some(&[3u8; 33])), None);
        assert_eq!(key32(Some(&[])), None);
        assert_eq!(key32(None), None);
    }

    /// `Get` over one scripted node on `dstore_transport::mem` (R = 1): the node answers `view`, and `get`
    /// with `absent` then one pack of the records it holds, followed by any extra records it is told to
    /// send. Root tests cannot iterate a `futures::Stream` (no `futures` dependency), so `Get` lives here.
    mod get {
        use std::collections::BTreeMap;
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
        use std::time::Duration;

        use amber_store_core::{fstree, ingest, packstore};
        use dstore_gocompat::slog::{Attr, Handler, Level, Logger, Record};
        use dstore_testkit::fake::{FakeCluster, FakeClusterConfig};
        use dstore_ticket::{Member, Ticket};
        use dstore_transport::mem::{MemEndpoint, Network};
        use dstore_transport::{Endpoint, Stream};
        use dstore_view::{Node, View, Voter};
        use dstore_wire::{T_ABSENT, T_GET, T_VIEW, T_VIEW_REPLY};
        use futures::StreamExt;

        use super::*;
        use crate::{Cond, Config};

        struct Quiet;

        impl Handler for Quiet {
            fn enabled(&self, _level: Level) -> bool {
                false
            }

            fn handle(&self, _handler_attrs: &[Attr], _r: &Record) {}
        }

        #[derive(Default)]
        struct NodeState {
            records: BTreeMap<[u8; 32], Vec<u8>>,
            /// Records sent after the requested ones (unrequested keys, duplicates).
            extra: Vec<Vec<u8>>,
            /// A pause after the first record of every pack.
            stall: Option<Duration>,
            requests: Vec<Msg>,
        }

        struct OneNode {
            net: Arc<Network>,
            id: NodeId,
            state: Arc<Mutex<NodeState>>,
            ctx: Ctx,
        }

        impl Drop for OneNode {
            fn drop(&mut self) {
                self.ctx.cancel();
            }
        }

        fn view_of(id: NodeId) -> View {
            View {
                cluster_id: Some(vec![0x11; 16]),
                incarnation: 1,
                epoch: 7,
                version: 7,
                placement_epoch: 1,
                replicas: 1,
                min_replicas: 1,
                voters: Some(vec![Voter {
                    id: Some(id.0.to_vec()),
                    since: 1,
                }]),
                nodes: Some(vec![Node {
                    id: Some(id.0.to_vec()),
                    weight: 100,
                    writable: true,
                    ..Node::default()
                }]),
                ..View::default()
            }
        }

        async fn handle(
            st: &Mutex<NodeState>,
            id: NodeId,
            mut s: Stream,
        ) -> Result<(), dstore_wire::WireError> {
            let m = dstore_wire::read_msg(&mut s.recv).await?;
            lock(st).requests.push(m.clone());
            if m.typ == T_VIEW {
                let reply = Msg {
                    typ: T_VIEW_REPLY,
                    incarnation: 1,
                    epoch: 7,
                    view: view_of(id).encode(),
                    ..Msg::default()
                };
                dstore_wire::write_msg(&mut s.send, &reply).await?;
                s.close_write();
                return Ok(());
            }
            if m.typ != T_GET {
                let reply = dstore_wire::err_msg("bad-request", "unknown operation");
                return dstore_wire::write_msg(&mut s.send, &reply).await;
            }
            let (absent, recs, stall) = {
                let st = lock(st);
                let mut absent = Vec::new();
                let mut recs = Vec::new();
                for k in dstore_wire::keys32(&m.keys).unwrap_or_default() {
                    match st.records.get(&k) {
                        Some(rec) => recs.push(rec.clone()),
                        None => absent.push(k),
                    }
                }
                recs.extend(st.extra.iter().cloned());
                (absent, recs, st.stall)
            };
            let reply = Msg {
                typ: T_ABSENT,
                keys: dstore_wire::raw_keys(&absent),
                ..Msg::default()
            };
            dstore_wire::write_msg(&mut s.send, &reply).await?;
            match stall {
                None => {
                    let mut sender = PackSender::new(&mut s.send);
                    for rec in &recs {
                        sender.add_record(rec).await?;
                    }
                    sender.finish().await?;
                }
                Some(d) => {
                    // Frames by hand: the magic and the first record, a pause, then the rest and the end
                    // marker (a PackSender would hold short records back until a 1 MiB chunk fills).
                    use tokio::io::AsyncWriteExt;
                    let mut first = dstore_wire::PACK_MAGIC.to_vec();
                    let mut rest = Vec::new();
                    for (i, rec) in recs.iter().enumerate() {
                        if i == 0 {
                            first.extend_from_slice(rec);
                        } else {
                            rest.extend_from_slice(rec);
                        }
                    }
                    rest.push(0);
                    s.send
                        .write_all(&tdata(&first))
                        .await
                        .map_err(dstore_wire::WireError::Io)?;
                    tokio::time::sleep(d).await;
                    s.send
                        .write_all(&tdata(&rest))
                        .await
                        .map_err(dstore_wire::WireError::Io)?;
                    let end = [0x00, 0x00, 0x00, 0x03, 0xa1, 0x00, 0x08];
                    s.send
                        .write_all(&end)
                        .await
                        .map_err(dstore_wire::WireError::Io)?;
                }
            }
            s.close_write();
            Ok(())
        }

        /// A TData frame `{0: 7, 8: data}` with its length prefix.
        fn tdata(data: &[u8]) -> Vec<u8> {
            let mut e = dstore_codec::Enc::new();
            e.head(5, 2);
            e.uint(0);
            e.uint(7);
            e.uint(8);
            e.bytes(data);
            let payload = e.into_bytes();
            let mut frame = u32::try_from(payload.len())
                .unwrap_or(u32::MAX)
                .to_be_bytes()
                .to_vec();
            frame.extend_from_slice(&payload);
            frame
        }

        async fn serve(ep: Arc<MemEndpoint>, st: Arc<Mutex<NodeState>>, id: NodeId, ctx: Ctx) {
            while let Ok(conn) = ep.accept(&ctx).await {
                let (st, ctx) = (Arc::clone(&st), ctx.clone());
                tokio::spawn(async move {
                    while let Ok(s) = conn.accept_stream(&ctx).await {
                        let st = Arc::clone(&st);
                        tokio::spawn(async move {
                            let _ = handle(&st, id, s).await;
                        });
                    }
                });
            }
        }

        impl OneNode {
            fn start(records: usize) -> (OneNode, Vec<[u8; 32]>) {
                let net = Network::new();
                let mut id = [0u8; 32];
                id[0] = 1;
                id[31] = 1;
                let id = NodeId(id);
                let mut st = NodeState::default();
                let mut keys = Vec::new();
                for i in 0..records {
                    let data = splitmix::data(500 + i as u64, 200 + i);
                    let k = Key::new(Type::Blob, data.len() as u64, &data);
                    match encode_record(k, &data) {
                        Ok(rec) => st.records.insert(k.0, rec),
                        Err(e) => panic!("encode_record: {e}"),
                    };
                    keys.push(k.0);
                }
                let state = Arc::new(Mutex::new(st));
                let ctx = Ctx::background().with_cancel();
                tokio::spawn(serve(
                    net.bind(id, &[ALPN_CLIENT]),
                    Arc::clone(&state),
                    id,
                    ctx.clone(),
                ));
                (
                    OneNode {
                        net,
                        id,
                        state,
                        ctx,
                    },
                    keys,
                )
            }

            async fn dial(&self, jobs: usize) -> Cluster {
                let ep: Arc<dyn Endpoint> = self.net.bind(NodeId([0xfe; 32]), &[ALPN_CLIENT]);
                let cfg = Config {
                    endpoint: Some(ep),
                    ticket: Ticket {
                        cluster_id: Some(vec![0x11; 16]),
                        incarnation: 1,
                        members: Some(vec![Member {
                            id: Some(self.id.0.to_vec()),
                            addrs: Vec::new(),
                        }]),
                    },
                    logger: Some(Logger::new(Arc::new(Quiet))),
                    request_timeout: Duration::from_secs(20),
                    jobs,
                    ..Config::default()
                };
                match Cluster::dial(&Ctx::background(), cfg).await {
                    Ok(c) => c,
                    Err(e) => panic!("dial: {e}"),
                }
            }

            fn count(&self, typ: i64) -> usize {
                lock(&self.state)
                    .requests
                    .iter()
                    .filter(|m| m.typ == typ)
                    .count()
            }
        }

        async fn collect(s: &mut GetStream) -> (Vec<GetResult>, Vec<String>) {
            let mut recs = Vec::new();
            let mut errs = Vec::new();
            while let Some(item) = s.next().await {
                match item {
                    Ok(r) => recs.push(r),
                    Err(e) => errs.push(e.to_string()),
                }
            }
            (recs, errs)
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn streams_records_and_reports_missing_keys_once_refreshed() {
            let (node, keys) = OneNode::start(5);
            let cl = node.dial(0).await;
            let unknown = Key::new(Type::Blob, 3, b"abc").0;
            let views = node.count(T_VIEW);
            let mut s = cl.get(
                &Ctx::background(),
                vec![keys[0], keys[1], keys[2], keys[0], unknown],
            );
            assert!(s.missing().is_empty());
            let (recs, errs) = collect(&mut s).await;
            assert!(errs.is_empty(), "{errs:?}");
            let mut got: Vec<[u8; 32]> = recs.iter().map(|r| r.key).collect();
            got.sort();
            let mut want = vec![keys[0], keys[1], keys[2]];
            want.sort();
            assert_eq!(got, want, "a duplicate input key is fetched once");
            for r in &recs {
                assert_eq!(Some(&r.record), lock(&node.state).records.get(&r.key));
            }
            assert_eq!(s.missing(), vec![unknown]);
            assert_eq!(node.count(T_VIEW), views + 1, "one view refresh");
            let asked = lock(&node.state)
                .requests
                .iter()
                .filter(|m| m.typ == T_GET && m.keys.iter().any(|k| k.as_slice() == unknown))
                .count();
            assert_eq!(asked, 2, "asked, then asked again after the refresh");
            cl.close();
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn passes_unrequested_and_duplicate_records_on() {
            let (node, keys) = OneNode::start(3);
            {
                let mut st = lock(&node.state);
                let extra = vec![st.records[&keys[2]].clone(), st.records[&keys[0]].clone()];
                st.extra = extra;
            }
            let cl = node.dial(0).await;
            let mut s = cl.get(&Ctx::background(), vec![keys[0]]);
            let (recs, errs) = collect(&mut s).await;
            assert!(errs.is_empty(), "{errs:?}");
            let got: Vec<[u8; 32]> = recs.iter().map(|r| r.key).collect();
            assert_eq!(got, vec![keys[0], keys[2], keys[0]]);
            assert!(s.missing().is_empty());
            cl.close();
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn stops_early_cleanly() {
            // TestClusterGetStopsEarlyCleanly.
            let (node, keys) = OneNode::start(30);
            let cl = node.dial(0).await;
            for round in 0..3 {
                let mut s = cl.get(&Ctx::background(), keys.clone());
                let mut n = 0;
                while let Some(item) = s.next().await {
                    if let Err(e) = item {
                        panic!("round {round}: {e}");
                    }
                    n += 1;
                    if n == 2 {
                        break;
                    }
                }
                assert_eq!(n, 2, "round {round}");
                drop(s);
            }
            let mut s = cl.get(&Ctx::background(), keys.clone());
            let (recs, errs) = collect(&mut s).await;
            assert!(errs.is_empty(), "{errs:?}");
            assert_eq!(recs.len(), keys.len());
            assert!(s.missing().is_empty());
            let closed = tokio::time::timeout(Duration::from_secs(10), async { cl.close() }).await;
            assert!(closed.is_ok(), "close hung after early drops");
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn yields_before_the_batch_is_complete() {
            // Records stream out as they arrive: the first record of a pack is yielded while the node still
            // holds the rest back (the stall is twice the bound, so a loaded machine does not fail it).
            let (node, keys) = OneNode::start(8);
            lock(&node.state).stall = Some(Duration::from_secs(2));
            let cl = node.dial(1).await;
            let start = std::time::Instant::now();
            let mut s = cl.get(&Ctx::background(), keys.clone());
            let first = s.next().await;
            assert!(matches!(first, Some(Ok(_))));
            let took = start.elapsed();
            assert!(took < Duration::from_secs(1), "first record after {took:?}");
            let (recs, _) = collect(&mut s).await;
            assert_eq!(recs.len() + 1, keys.len());
            cl.close();
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_cancelled_ctx_is_yielded_after_the_last_record() {
            let (node, keys) = OneNode::start(4);
            let cl = node.dial(0).await;
            let ctx = Ctx::background().with_cancel();
            ctx.cancel();
            let mut s = cl.get(&ctx, keys);
            let (recs, errs) = collect(&mut s).await;
            assert!(recs.is_empty());
            assert_eq!(errs, vec!["context canceled".to_owned()]);
            cl.close();
        }

        // ---- node/cluster_test.go Get tests over dstore_testkit::fake ----

        /// A scratch directory (with `src/sub`) removed on drop.
        struct TempDir(std::path::PathBuf);

        impl TempDir {
            fn new() -> TempDir {
                static DIRS: AtomicUsize = AtomicUsize::new(0);
                let n = DIRS.fetch_add(1, AtomicOrdering::Relaxed);
                let dir = std::env::temp_dir().join(format!(
                    "dstore-client-objects-test-{}-{n}",
                    std::process::id()
                ));
                let _ = std::fs::remove_dir_all(&dir);
                if let Err(e) = std::fs::create_dir_all(dir.join("src").join("sub")) {
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

        /// Go `cluster3(t)` and `h.clientWith(t, 100, cfg.Jobs = jobs)` over the testkit's fake cluster, with
        /// a ticket naming node 1 only.
        async fn cluster3(jobs: usize) -> (Arc<Network>, Arc<FakeCluster>, Cluster) {
            let net = Network::new();
            let fc = FakeCluster::start(&net, FakeClusterConfig::default()).await;
            let first = fc.ticket().members.unwrap_or_default().into_iter().next();
            let mut id = [0u8; 32];
            id[0] = 100;
            id[31] = 100;
            let ep: Arc<dyn Endpoint> = net.bind(NodeId(id), &[ALPN_CLIENT]);
            let cfg = Config {
                endpoint: Some(ep),
                ticket: Ticket {
                    cluster_id: None,
                    incarnation: 0,
                    members: first.map(|m| vec![m]),
                },
                logger: Some(Logger::new(Arc::new(Quiet))),
                request_timeout: Duration::from_secs(20),
                jobs,
                ..Config::default()
            };
            match Cluster::dial(&Ctx::background(), cfg).await {
                Ok(c) => (net, fc, c),
                Err(e) => panic!("dial: {e}"),
            }
        }

        /// Go `pushTree(t, c, files, size, name)`, with `makeTree`'s layout over splitmix data: the tree is
        /// ingested into a packstore under `dir` and pushed. Returns the reachable keys.
        async fn push_tree(
            cl: &Cluster,
            dir: &TempDir,
            files: usize,
            size: usize,
            name: &str,
        ) -> Vec<[u8; 32]> {
            let src = dir.0.join("src");
            for i in 0..files {
                let data = splitmix::data(0x5452_4545 + i as u64, size + i * 37);
                let p = if i % 3 == 0 {
                    src.join("sub").join(format!("g{i:03}"))
                } else {
                    src.join(format!("f{i:03}"))
                };
                if let Err(e) = std::fs::write(&p, data) {
                    panic!("write {}: {e}", p.display());
                }
            }
            let st = match packstore::Store::open_with(
                dir.0.join("packstore"),
                packstore::Options::new(),
            ) {
                Ok(s) => Arc::new(s),
                Err(e) => panic!("open packstore: {e}"),
            };
            let opts = ingest::Opts {
                jobs: 2,
                ..ingest::Opts::default()
            };
            let root = match ingest::dir(&st, &src, opts).1 {
                Ok(k) => k,
                Err(e) => panic!("ingest: {e}"),
            };
            let force = Cond {
                force: true,
                ..Cond::default()
            };
            if let Err(e) = cl
                .push(
                    &Ctx::background(),
                    Arc::clone(&st),
                    root,
                    name,
                    "tester",
                    force,
                    None,
                )
                .await
            {
                panic!("push: {e}");
            }
            match fstree::reachable_keys(root, |k| st.get(k)) {
                Ok(keys) => keys.into_iter().map(|k| k.0).collect(),
                Err(e) => panic!("reachable keys: {e}"),
            }
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn cluster_get_yields_before_every_batch_is_fetched() {
            // TestClusterGetYieldsBeforeEveryBatchIsFetched: with one worker and a dial latency, the first
            // record arrives after one latency, not after every per-node batch has completed. The pool
            // dials until Conns connections exist, so each owner's first get stream pays the latency.
            let (net, fc, cl) = cluster3(1).await;
            let dir = TempDir::new();
            let keys = push_tree(&cl, &dir, 30, 20_000, "trees/get").await;
            const DELAY: Duration = Duration::from_millis(300);
            net.set_delay(DELAY);
            let start = std::time::Instant::now();
            let mut s = cl.get(&Ctx::background(), keys);
            let first = match s.next().await {
                Some(Ok(_)) => start.elapsed(),
                Some(Err(e)) => panic!("get: {e}"),
                None => panic!("get yielded no record"),
            };
            drop(s);
            net.set_delay(Duration::ZERO);
            assert!(
                first < 2 * DELAY,
                "first record after {first:?} with a {DELAY:?} dial latency: Get waited for every batch"
            );
            cl.close();
            fc.close().await;
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn cluster_get_stops_early_cleanly() {
            // TestClusterGetStopsEarlyCleanly: breaking out of a Get neither leaks nor blocks; a full Get
            // afterwards returns every record, and Close does not hang.
            let (_net, fc, cl) = cluster3(0).await;
            let dir = TempDir::new();
            let keys = push_tree(&cl, &dir, 30, 20_000, "trees/early").await;
            for round in 0..3 {
                let mut s = cl.get(&Ctx::background(), keys.clone());
                let mut n = 0;
                while let Some(item) = s.next().await {
                    if let Err(e) = item {
                        panic!("round {round}: {e}");
                    }
                    n += 1;
                    if n == 2 {
                        break;
                    }
                }
                assert_eq!(n, 2, "round {round}: records before breaking");
            }
            let mut s = cl.get(&Ctx::background(), keys.clone());
            let (recs, errs) = collect(&mut s).await;
            assert!(errs.is_empty(), "{errs:?}");
            assert_eq!(
                recs.len(),
                keys.len(),
                "full get after early breaks: {} of {} records",
                recs.len(),
                keys.len()
            );
            assert!(s.missing().is_empty(), "{} missing", s.missing().len());
            let closed = tokio::time::timeout(Duration::from_secs(10), async { cl.close() }).await;
            assert!(closed.is_ok(), "Close hung after early breaks");
            fc.close().await;
        }
    }
}
