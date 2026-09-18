//! `client/progress.go`: progress reports, the put observer, the tracker, `HumanBytes` and `Rate`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use dstore_gocompat::slog::Attr;
use dstore_gocompat::time::{MILLISECOND, SECOND, duration_round, duration_to_ns};
use dstore_transport::PathInfo;

use crate::{Cluster, NodeId};

/// `client.Progress`: called under the tracker mutex; must not re-enter the client.
pub type Progress = Arc<dyn Fn(&ProgressReport) + Send + Sync>;

/// `client.ProgressReport`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProgressReport {
    pub objects: i64,
    pub total_objects: i64,
    pub bytes: i64,
    pub total_bytes: i64,
    pub nodes: Vec<NodeProgress>,
}

/// `client.NodeProgress`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeProgress {
    pub id: NodeId,
    pub direct: bool,
    pub rtt: Duration,
    pub in_flight: i64,
    pub awaiting: i64,
    pub bytes: i64,
}

/// `client.PutObserver`.
#[derive(Clone, Default)]
pub struct PutObserver {
    pub start: Option<Arc<dyn Fn(NodeId) + Send + Sync>>,
    pub sent: Option<Arc<dyn Fn(NodeId, usize) + Send + Sync>>,
    pub flushed: Option<Arc<dyn Fn(NodeId) + Send + Sync>>,
    pub done: Option<Arc<dyn Fn(NodeId, bool) + Send + Sync>>,
}

/// The pool path lookup a tracker reports (`t.c.pool.Path(id, wire.ALPNClient)`).
type PathFn = Arc<dyn Fn(NodeId) -> Option<PathInfo> + Send + Sync>;

/// `tracker`: accumulates a push's progress and reports it.
pub(crate) struct Tracker {
    path: PathFn,
    prog: Option<Progress>,
    state: Mutex<TrackerState>,
}

#[derive(Default)]
struct TrackerState {
    report: ProgressReport,
    per_node: HashMap<NodeId, NodeProgress>,
}

impl TrackerState {
    /// `tracker.node`: get or create the entry.
    fn node(&mut self, id: NodeId) -> &mut NodeProgress {
        self.per_node.entry(id).or_insert_with(|| NodeProgress {
            id,
            ..NodeProgress::default()
        })
    }
}

impl Tracker {
    /// `newTracker`.
    pub(crate) fn new(c: &Cluster, prog: Option<Progress>) -> Arc<Tracker> {
        let c = c.clone();
        Tracker::with_path(
            Arc::new(move |id| c.pool().path(id, dstore_wire::ALPN_CLIENT)),
            prog,
        )
    }

    fn with_path(path: PathFn, prog: Option<Progress>) -> Arc<Tracker> {
        Arc::new(Tracker {
            path,
            prog,
            state: Mutex::new(TrackerState::default()),
        })
    }

    /// The tracker lock; a panicking progress callback leaves consistent counters behind.
    fn lock(&self) -> MutexGuard<'_, TrackerState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `tracker.observer`.
    pub(crate) fn observer(self: &Arc<Self>) -> PutObserver {
        let start = self.clone();
        let sent = self.clone();
        let flushed = self.clone();
        let done = self.clone();
        PutObserver {
            start: Some(Arc::new(move |id| {
                start.update(|st| st.node(id).in_flight += 1);
            })),
            sent: Some(Arc::new(move |id, n| {
                sent.update(|st| {
                    let n = n as i64;
                    let np = st.node(id);
                    np.bytes = np.bytes.wrapping_add(n);
                    st.report.bytes = st.report.bytes.wrapping_add(n);
                });
            })),
            flushed: Some(Arc::new(move |id| {
                flushed.update(|st| st.node(id).awaiting += 1);
            })),
            done: Some(Arc::new(move |id, was_flushed| {
                done.update(|st| {
                    let np = st.node(id);
                    np.in_flight -= 1;
                    if was_flushed {
                        np.awaiting -= 1;
                    }
                });
            })),
        }
    }

    /// `tracker.totals`: what the transfer has to move; `done` counts the objects the cluster already held.
    pub(crate) fn totals(&self, objects: i64, done: i64, bytes: i64) {
        self.update(|st| {
            st.report.total_objects = objects;
            st.report.objects = done;
            st.report.total_bytes = bytes;
        });
    }

    /// `tracker.more`: bytes that turned out to need sending.
    pub(crate) fn more(&self, bytes: i64) {
        self.update(|st| st.report.total_bytes = st.report.total_bytes.wrapping_add(bytes));
    }

    /// `tracker.objects`: n finished objects.
    pub(crate) fn objects(&self, n: i64) {
        self.update(|st| st.report.objects = st.report.objects.wrapping_add(n));
    }

    /// `tracker.bytes`: under the lock, without reporting.
    pub(crate) fn bytes(&self) -> i64 {
        self.lock().report.bytes
    }

    /// `tracker.update`: f, then the report, both under the lock.
    fn update(&self, f: impl FnOnce(&mut TrackerState)) {
        let mut st = self.lock();
        f(&mut st);
        if let Some(prog) = &self.prog {
            let rep = self.snapshot(&st);
            prog(&rep);
        }
    }

    /// `tracker.snapshot`: nodes ordered by id, with their current pool path when there is one.
    fn snapshot(&self, st: &TrackerState) -> ProgressReport {
        let mut rep = ProgressReport {
            nodes: Vec::with_capacity(st.per_node.len()),
            ..st.report.clone()
        };
        for np in st.per_node.values() {
            let mut n = np.clone();
            if let Some(p) = (self.path)(n.id) {
                n.direct = p.direct;
                n.rtt = p.rtt;
            }
            rep.nodes.push(n);
        }
        rep.nodes.sort_by_key(|n| n.id);
        rep
    }
}

/// `countKeys`: (objects, bytes).
pub(crate) fn count_keys(
    m: &HashMap<NodeId, Vec<[u8; 32]>>,
    size: &dyn Fn(&[u8; 32]) -> usize,
) -> (i64, i64) {
    let mut objects = 0i64;
    let mut bytes = 0i64;
    for ks in m.values() {
        objects = objects.wrapping_add(ks.len() as i64);
        for k in ks {
            bytes = bytes.wrapping_add(size(k) as i64);
        }
    }
    (objects, bytes)
}

/// `pathAttrs` of a pool path: `path=none`, or `path=direct|relay rtt=<RTT rounded to ms>`.
pub(crate) fn path_attrs_of(p: Option<PathInfo>) -> Vec<Attr> {
    match p {
        None => vec![Attr::string("path", "none")],
        Some(p) => vec![
            Attr::string("path", if p.direct { "direct" } else { "relay" }),
            Attr::duration("rtt", duration_round(duration_to_ns(p.rtt), MILLISECOND)),
        ],
    }
}

/// `client.HumanBytes`: "1024.0 KiB" for 1048575.
pub fn human_bytes(n: i64) -> String {
    const UNIT: i64 = 1024;
    if n < UNIT {
        return format!("{n} B");
    }
    let mut div = UNIT;
    let mut exp = 0usize;
    let mut m = n / UNIT;
    while m >= UNIT {
        div *= UNIT;
        exp += 1;
        m /= UNIT;
    }
    let unit = b"KMGTPE".get(exp).copied().map_or('?', char::from);
    format!("{:.1} {unit}iB", n as f64 / div as f64)
}

/// `client.Rate`: ZERO → "-".
pub fn rate(bytes: i64, took: Duration) -> String {
    rate_ns(bytes, duration_to_ns(took))
}

/// `client.Rate` over a Go `time.Duration` in nanoseconds, which can be negative: `took <= 0` → "-".
pub fn rate_ns(bytes: i64, took_ns: i64) -> String {
    if took_ns <= 0 {
        return "-".to_owned();
    }
    // Duration.Seconds: float64(d/Second) + float64(d%Second)/1e9.
    let secs = (took_ns / SECOND) as f64 + (took_ns % SECOND) as f64 / 1e9;
    human_bytes((bytes as f64 / secs) as i64) + "/s"
}

#[cfg(test)]
mod tests {
    use dstore_gocompat::slog::{Record, Value, format_text_record};
    use dstore_gocompat::time::FixedZone;
    use dstore_testkit::golden::{self, decimal_i64};
    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    struct PathJson {
        direct: bool,
        #[serde(deserialize_with = "decimal_i64")]
        rtt_ns: i64,
    }

    impl PathJson {
        fn info(&self) -> PathInfo {
            PathInfo {
                direct: self.direct,
                rtt: nanos(self.rtt_ns),
            }
        }
    }

    fn nanos(ns: i64) -> Duration {
        match u64::try_from(ns) {
            Ok(n) => Duration::from_nanos(n),
            Err(_) => panic!("negative duration {ns} in a vector"),
        }
    }

    #[derive(Deserialize, Debug)]
    struct NodeJson {
        id: String,
        direct: bool,
        #[serde(deserialize_with = "decimal_i64")]
        rtt_ns: i64,
        in_flight: i64,
        awaiting: i64,
        #[serde(deserialize_with = "decimal_i64")]
        bytes: i64,
    }

    #[derive(Deserialize, Debug)]
    struct ReportJson {
        objects: i64,
        total_objects: i64,
        #[serde(deserialize_with = "decimal_i64")]
        bytes: i64,
        #[serde(deserialize_with = "decimal_i64")]
        total_bytes: i64,
        nodes: Vec<NodeJson>,
    }

    impl ReportJson {
        fn report(&self) -> ProgressReport {
            ProgressReport {
                objects: self.objects,
                total_objects: self.total_objects,
                bytes: self.bytes,
                total_bytes: self.total_bytes,
                nodes: self
                    .nodes
                    .iter()
                    .map(|n| NodeProgress {
                        id: NodeId::from_slice_lossy(&golden::hex(&n.id)),
                        direct: n.direct,
                        rtt: nanos(n.rtt_ns),
                        in_flight: n.in_flight,
                        awaiting: n.awaiting,
                        bytes: n.bytes,
                    })
                    .collect(),
            }
        }
    }

    #[derive(Deserialize)]
    struct OpJson {
        op: String,
        node: Option<usize>,
        n: Option<usize>,
        flushed: Option<bool>,
        objects: Option<i64>,
        done: Option<i64>,
        #[serde(default, deserialize_with = "opt_decimal_i64")]
        bytes: Option<i64>,
        #[serde(default, deserialize_with = "opt_decimal_i64")]
        result: Option<i64>,
        report: Option<ReportJson>,
    }

    fn opt_decimal_i64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
        #[derive(Deserialize)]
        struct W(#[serde(deserialize_with = "decimal_i64")] i64);
        Option::<W>::deserialize(d).map(|w| w.map(|W(v)| v))
    }

    #[derive(Deserialize)]
    struct TrackerCase {
        name: String,
        nodes: Vec<String>,
        paths: Vec<Option<PathJson>>,
        progress: bool,
        ops: Vec<OpJson>,
    }

    #[derive(Deserialize)]
    struct CountKeysCase {
        name: String,
        lists: Vec<Vec<usize>>,
        objects: i64,
        #[serde(deserialize_with = "decimal_i64")]
        bytes: i64,
    }

    #[derive(Deserialize)]
    struct HumanBytesCase {
        #[serde(deserialize_with = "decimal_i64")]
        n: i64,
        out: String,
    }

    #[derive(Deserialize)]
    struct RateCase {
        #[serde(deserialize_with = "decimal_i64")]
        bytes: i64,
        #[serde(deserialize_with = "decimal_i64")]
        took_ns: i64,
        out: String,
    }

    #[derive(Deserialize)]
    struct AttrJson {
        key: String,
        kind: String,
        string: Option<String>,
        #[serde(default, deserialize_with = "opt_decimal_i64")]
        duration_ns: Option<i64>,
    }

    #[derive(Deserialize)]
    struct PathAttrsCase {
        path: Option<PathJson>,
        attrs: Vec<AttrJson>,
        text: String,
    }

    #[derive(Deserialize)]
    struct File {
        tracker: Vec<TrackerCase>,
        count_keys: Vec<CountKeysCase>,
        human_bytes: Vec<HumanBytesCase>,
        rate: Vec<RateCase>,
        path_attrs: Vec<PathAttrsCase>,
    }

    fn load() -> File {
        golden::load_json("client/progress.json")
    }

    #[test]
    fn golden_tracker() {
        let f = load();
        assert!(f.tracker.len() >= 5);
        for c in &f.tracker {
            let ids: Vec<NodeId> = c
                .nodes
                .iter()
                .map(|h| NodeId::from_slice_lossy(&golden::hex(h)))
                .collect();
            let paths: HashMap<NodeId, PathInfo> = ids
                .iter()
                .zip(&c.paths)
                .filter_map(|(id, p)| p.as_ref().map(|p| (*id, p.info())))
                .collect();
            let reports: Arc<Mutex<Vec<ProgressReport>>> = Arc::default();
            let prog: Option<Progress> = c.progress.then(|| {
                let reports = reports.clone();
                Arc::new(move |r: &ProgressReport| {
                    reports
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(r.clone());
                }) as Progress
            });
            let tracker = Tracker::with_path(Arc::new(move |id| paths.get(&id).copied()), prog);
            let obs = tracker.observer();
            for (i, op) in c.ops.iter().enumerate() {
                let what = format!("{} op {i} ({})", c.name, op.op);
                let node = || ids[op.node.unwrap_or_default()];
                let before = reports.lock().unwrap_or_else(PoisonError::into_inner).len();
                match op.op.as_str() {
                    "start" => (obs.start.as_ref().expect("observer start"))(node()),
                    "sent" => (obs.sent.as_ref().expect("observer sent"))(
                        node(),
                        op.n.unwrap_or_default(),
                    ),
                    "flushed" => (obs.flushed.as_ref().expect("observer flushed"))(node()),
                    "done" => (obs.done.as_ref().expect("observer done"))(
                        node(),
                        op.flushed.unwrap_or_default(),
                    ),
                    "totals" => tracker.totals(
                        op.objects.unwrap_or_default(),
                        op.done.unwrap_or_default(),
                        op.bytes.unwrap_or_default(),
                    ),
                    "more" => tracker.more(op.bytes.unwrap_or_default()),
                    "objects" => tracker.objects(op.n.map_or(0, |n| n as i64)),
                    "bytes" => assert_eq!(Some(tracker.bytes()), op.result, "{what}"),
                    other => panic!("{what}: unknown op {other}"),
                }
                let got = reports.lock().unwrap_or_else(PoisonError::into_inner);
                match &op.report {
                    None => assert_eq!(got.len(), before, "{what}: no report expected"),
                    Some(want) => {
                        assert_eq!(got.len(), before + 1, "{what}: one report expected");
                        assert_eq!(got[before], want.report(), "{what}");
                    }
                }
            }
        }
    }

    #[test]
    fn golden_count_keys() {
        let f = load();
        for c in &f.count_keys {
            let mut m: HashMap<NodeId, Vec<[u8; 32]>> = HashMap::new();
            let mut sizes: HashMap<[u8; 32], usize> = HashMap::new();
            for (i, list) in c.lists.iter().enumerate() {
                let mut id = [0u8; 32];
                id[0] = i as u8;
                let keys = list
                    .iter()
                    .enumerate()
                    .map(|(j, &size)| {
                        let mut k = [0u8; 32];
                        k[0] = i as u8;
                        k[1..9].copy_from_slice(&(j as u64).to_be_bytes());
                        sizes.insert(k, size);
                        k
                    })
                    .collect();
                m.insert(NodeId(id), keys);
            }
            let got = count_keys(&m, &|k| sizes.get(k).copied().unwrap_or_default());
            assert_eq!(got, (c.objects, c.bytes), "{}", c.name);
        }
    }

    #[test]
    fn golden_human_bytes_and_rate() {
        let f = load();
        assert!(f.human_bytes.len() > 20);
        for c in &f.human_bytes {
            assert_eq!(human_bytes(c.n), c.out, "HumanBytes({})", c.n);
        }
        assert!(f.rate.iter().any(|c| c.took_ns < 0));
        for c in &f.rate {
            assert_eq!(
                rate_ns(c.bytes, c.took_ns),
                c.out,
                "Rate({}, {}ns)",
                c.bytes,
                c.took_ns
            );
            if let Ok(ns) = u64::try_from(c.took_ns) {
                assert_eq!(rate(c.bytes, Duration::from_nanos(ns)), c.out);
            }
        }
    }

    #[test]
    fn golden_path_attrs() {
        let f = load();
        assert!(f.path_attrs.len() >= 3);
        for c in &f.path_attrs {
            let got = path_attrs_of(c.path.as_ref().map(PathJson::info));
            let want: Vec<Attr> = c
                .attrs
                .iter()
                .map(|a| match a.kind.as_str() {
                    "String" => Attr::string(&a.key, a.string.clone().unwrap_or_default()),
                    "Duration" => Attr::duration(&a.key, a.duration_ns.unwrap_or_default()),
                    other => panic!("unknown attr kind {other}"),
                })
                .collect();
            assert_eq!(got, want, "{}", c.text);
            let line = format_text_record(
                &[],
                &Record {
                    time: None,
                    level: dstore_gocompat::slog::Level::INFO,
                    message: "m".to_owned(),
                    attrs: got,
                },
                &FixedZone(0),
            );
            assert_eq!(
                String::from_utf8_lossy(&line),
                format!("level=INFO msg=m {}\n", c.text)
            );
        }
    }

    // client-core §3.7, verified.
    #[test]
    fn human_bytes_spec_values() {
        for (n, want) in [
            (0, "0 B"),
            (1023, "1023 B"),
            (1024, "1.0 KiB"),
            (1280, "1.2 KiB"),
            (1792, "1.8 KiB"),
            (1048575, "1024.0 KiB"),
            (1 << 60, "1.0 EiB"),
            (i64::MAX, "8.0 EiB"),
            (-2048, "-2048 B"),
        ] {
            assert_eq!(human_bytes(n), want);
        }
        assert_eq!(rate(1 << 20, Duration::from_secs(2)), "512.0 KiB/s");
        assert_eq!(rate(5, Duration::ZERO), "-");
        assert_eq!(rate_ns(100, -SECOND), "-");
        assert_eq!(rate(123456789, Duration::from_millis(1234)), "95.4 MiB/s");
    }

    #[test]
    fn tracker_bytes_does_not_report() {
        let count = Arc::new(Mutex::new(0usize));
        let c2 = count.clone();
        let prog: Progress =
            Arc::new(move |_| *c2.lock().unwrap_or_else(PoisonError::into_inner) += 1);
        let t = Tracker::with_path(Arc::new(|_| None), Some(prog));
        (t.observer().sent.expect("sent"))(NodeId([1; 32]), 7);
        assert_eq!(t.bytes(), 7);
        assert_eq!(*count.lock().unwrap_or_else(PoisonError::into_inner), 1);
        assert!(matches!(path_attrs_of(None)[0].value, Value::String(ref s) if s == "none"));
    }
}
