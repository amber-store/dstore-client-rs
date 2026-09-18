//! Golden tests of `dstore-cli` `progress` (owner cli-progress).
//!
//! `cli/text.json` (family `cli`, schema in `tools/vectorgen/docs/vectorgen-cli.md`), sections
//! `status_line` (with `fraction`), `rate_meter`, `node_state`, `tea_handler`, `format_event`, `blend1d`,
//! `progress_bar` and `ui_model`, and of `progress::colorprofile`: `color_profile`, `convert256` and
//! `downsample`. The file's other sections belong to other modules: `human_bytes` and `rate` to
//! `client.rs`; `hex_decode`, `resolve_ticket`, `log_level`, `describe_change` and `filter_paths` to
//! `cli.rs`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dstore_cli::progress::colorprofile::{self, Environ, Profile};
use dstore_cli::progress::{self, Latest, RateMeter, TeaHandler, UiModel, UiMsg};
use dstore_client::{NodeProgress, ProgressReport};
use dstore_gocompat::ctx::Ctx;
use dstore_gocompat::slog::{Attr, Level, Logger, Value};
use dstore_gocompat::time::{FixedZone, GoTime};
use dstore_testkit::golden::{self, decimal_i64};
use dstore_view::NodeId;
use serde::{Deserialize, Deserializer};

const VECTORS: &str = "cli/text.json";

/// A float written as its shortest round-trip decimal string (`FormatFloat(f, 'g', -1, 64)`).
fn decimal_f64<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    let s = String::deserialize(d)?;
    s.parse().map_err(serde::de::Error::custom)
}

#[derive(Deserialize)]
struct Vectors {
    status_line: Vec<StatusLineCase>,
    rate_meter: Vec<RateMeterCase>,
    node_state: Vec<NodeStateCase>,
    tea_handler: Vec<TeaCase>,
    format_event: Vec<FormatEventCase>,
    blend1d: Vec<BlendCase>,
    progress_bar: Vec<BarCase>,
    ui_model: Vec<UiScenario>,
    color_profile: Vec<ColorProfileCase>,
    convert256: Vec<ConvertCase>,
    downsample: Vec<DownsampleCase>,
}

fn vectors() -> Vectors {
    golden::load_json(VECTORS)
}

#[derive(Deserialize)]
struct Report {
    objects: i64,
    total_objects: i64,
    #[serde(deserialize_with = "decimal_i64")]
    bytes: i64,
    #[serde(deserialize_with = "decimal_i64")]
    total_bytes: i64,
    nodes: Vec<Node>,
}

#[derive(Deserialize)]
struct Node {
    id: String,
    direct: bool,
    #[serde(deserialize_with = "decimal_i64")]
    rtt_ns: i64,
    in_flight: i64,
    awaiting: i64,
    #[serde(deserialize_with = "decimal_i64")]
    bytes: i64,
}

impl Report {
    fn to_client(&self) -> ProgressReport {
        ProgressReport {
            objects: self.objects,
            total_objects: self.total_objects,
            bytes: self.bytes,
            total_bytes: self.total_bytes,
            nodes: self.nodes.iter().map(Node::to_client).collect(),
        }
    }
}

impl Node {
    fn to_client(&self) -> NodeProgress {
        let id: [u8; 32] = match golden::hex(&self.id).try_into() {
            Ok(id) => id,
            Err(b) => panic!("node id {} has {} bytes", self.id, b.len()),
        };
        let rtt = match u64::try_from(self.rtt_ns) {
            Ok(ns) => Duration::from_nanos(ns),
            Err(_) => panic!("negative rtt {}", self.rtt_ns),
        };
        NodeProgress {
            id: NodeId(id),
            direct: self.direct,
            rtt,
            in_flight: self.in_flight,
            awaiting: self.awaiting,
            bytes: self.bytes,
        }
    }
}

fn nanos(ns: i64) -> Duration {
    match u64::try_from(ns) {
        Ok(ns) => Duration::from_nanos(ns),
        Err(_) => panic!("negative offset {ns}"),
    }
}

#[derive(Deserialize)]
struct StatusLineCase {
    report: Report,
    #[serde(deserialize_with = "decimal_f64")]
    rate: f64,
    out: String,
    #[serde(deserialize_with = "decimal_f64")]
    fraction: f64,
}

#[test]
fn fraction_cases() {
    let v = vectors();
    assert!(!v.status_line.is_empty());
    for c in &v.status_line {
        let got = progress::fraction(&c.report.to_client());
        assert_eq!(
            got.to_bits(),
            c.fraction.to_bits(),
            "fraction of the report of {:?}: {got}, want {}",
            c.out,
            c.fraction
        );
    }
}

#[test]
fn status_line_cases() {
    let v = vectors();
    assert!(!v.status_line.is_empty());
    for c in &v.status_line {
        assert_eq!(
            progress::status_line(&c.report.to_client(), c.rate),
            c.out,
            "rate {}",
            c.rate
        );
    }
}

#[derive(Deserialize)]
struct RateMeterCase {
    name: String,
    #[serde(deserialize_with = "decimal_i64")]
    window_ns: i64,
    adds: Vec<RateAdd>,
}

#[derive(Deserialize)]
struct RateAdd {
    #[serde(deserialize_with = "decimal_i64")]
    t_ns: i64,
    #[serde(deserialize_with = "decimal_i64")]
    n: i64,
    #[serde(deserialize_with = "decimal_f64")]
    rate: f64,
}

#[test]
fn rate_meter_cases() {
    let v = vectors();
    assert!(!v.rate_meter.is_empty());
    for c in &v.rate_meter {
        let mut m = RateMeter::new(nanos(c.window_ns));
        let t0 = Instant::now();
        for (i, add) in c.adds.iter().enumerate() {
            let got = m.add(t0 + nanos(add.t_ns), add.n);
            assert_eq!(
                got.to_bits(),
                add.rate.to_bits(),
                "{} add {i} ({} ns, {}): {got}, want {}",
                c.name,
                add.t_ns,
                add.n,
                add.rate
            );
        }
    }
}

#[derive(Deserialize)]
struct NodeStateCase {
    in_flight: i64,
    awaiting: i64,
    out: String,
}

#[test]
fn node_state_cases() {
    let v = vectors();
    assert!(!v.node_state.is_empty());
    for c in &v.node_state {
        let n = NodeProgress {
            in_flight: c.in_flight,
            awaiting: c.awaiting,
            ..NodeProgress::default()
        };
        assert_eq!(
            progress::node_state(&n),
            c.out,
            "in_flight {} awaiting {}",
            c.in_flight,
            c.awaiting
        );
    }
}

#[derive(Deserialize)]
struct TeaCase {
    name: String,
    handler_level: i32,
    with: Vec<TeaAttr>,
    level: i32,
    msg: String,
    attrs: Vec<TeaAttr>,
    enabled: bool,
    text: Option<String>,
}

#[derive(Deserialize)]
struct TeaAttr {
    key: String,
    kind: String,
    value: String,
}

fn parsed<T: std::str::FromStr>(a: &TeaAttr) -> T
where
    T::Err: std::fmt::Display,
{
    match a.value.parse() {
        Ok(v) => v,
        Err(e) => panic!("attribute {} ({}) {:?}: {e}", a.key, a.kind, a.value),
    }
}

/// The slog attribute of a vector attribute (vectorgen-cli.md `tea_handler`).
fn attr(a: &TeaAttr) -> Attr {
    match a.kind.as_str() {
        "string" => Attr::string(&a.key, a.value.clone()),
        "int64" => Attr::int64(&a.key, parsed(a)),
        "uint64" => Attr::uint64(&a.key, parsed(a)),
        "float64" => Attr {
            key: a.key.clone(),
            value: Value::Float64(parsed(a)),
        },
        "bool" => Attr::bool(&a.key, parsed(a)),
        "duration" => Attr::duration(&a.key, parsed(a)),
        "error" => Attr::any(&a.key, &a.value),
        other => panic!("attribute {}: unknown kind {other}", a.key),
    }
}

/// Whether a case logs a `bytes` Int64 attribute, which `TeaHandler` humanises with
/// `dstore_client::human_bytes`.
fn humanises_bytes(c: &TeaCase) -> bool {
    c.with
        .iter()
        .chain(&c.attrs)
        .any(|a| a.key == "bytes" && a.kind == "int64")
}

fn check_tea_case(c: &TeaCase) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let log = Logger::new(Arc::new(TeaHandler::new(Level(c.handler_level), tx)))
        .with(c.with.iter().map(attr).collect());
    assert_eq!(
        log.enabled(Level(c.level)),
        c.enabled,
        "{}: enabled",
        c.name
    );
    log.log(Level(c.level), &c.msg, c.attrs.iter().map(attr).collect());
    let mut got = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        got.push(msg);
    }
    match &c.text {
        None => assert!(got.is_empty(), "{}: events {got:?}", c.name),
        Some(want) => match got.as_slice() {
            [UiMsg::Event { level, text, .. }] => {
                assert_eq!(*level, Level(c.level), "{}: event level", c.name);
                assert_eq!(text, want, "{}: event text", c.name);
            }
            other => panic!("{}: events {other:?}, want one", c.name),
        },
    }
}

#[test]
fn tea_handler_cases() {
    let v = vectors();
    assert!(!v.tea_handler.is_empty());
    assert!(
        v.tea_handler.iter().any(humanises_bytes),
        "no case humanises an Int64 bytes attribute"
    );
    for c in &v.tea_handler {
        check_tea_case(c);
    }
}

#[derive(Deserialize)]
struct FormatEventCase {
    #[serde(deserialize_with = "decimal_i64")]
    at_unix_ns: i64,
    offset_secs: i32,
    level: i32,
    text: String,
    out: String,
}

#[test]
fn format_event_cases() {
    let v = vectors();
    assert!(!v.format_event.is_empty());
    for c in &v.format_event {
        assert_eq!(
            progress::format_event(
                GoTime::from_unix_nano(c.at_unix_ns),
                Level(c.level),
                &c.text,
                &FixedZone(c.offset_secs)
            ),
            c.out,
            "at {} offset {} level {}",
            c.at_unix_ns,
            c.offset_secs,
            c.level
        );
    }
}

#[derive(Deserialize)]
struct BlendCase {
    steps: usize,
    a: [u8; 3],
    b: [u8; 3],
    out: Vec<[u8; 3]>,
}

#[test]
fn blend1d_cases() {
    let v = vectors();
    assert!(!v.blend1d.is_empty());
    for c in &v.blend1d {
        assert_eq!(
            progress::blend1d(c.steps, c.a, c.b),
            c.out,
            "steps {} from {:?} to {:?}",
            c.steps,
            c.a,
            c.b
        );
    }
}

#[derive(Deserialize)]
struct BarCase {
    width: i64,
    #[serde(deserialize_with = "decimal_f64")]
    percent: f64,
    out: String,
}

#[test]
fn progress_bar_cases() {
    let v = vectors();
    assert!(!v.progress_bar.is_empty());
    for c in &v.progress_bar {
        assert_eq!(
            progress::progress_bar(c.width, c.percent),
            c.out,
            "width {} percent {}",
            c.width,
            c.percent
        );
    }
}

#[derive(Deserialize)]
struct ColorProfileCase {
    env: Vec<String>,
    tty: bool,
    out: String,
}

/// `colorprofile.Env(env)` for a terminal, `colorprofile.Detect(&bytes.Buffer{}, env)` otherwise.
#[test]
fn color_profile_cases() {
    let v = vectors();
    assert!(v.color_profile.iter().any(|c| c.tty) && v.color_profile.iter().any(|c| !c.tty));
    for c in &v.color_profile {
        let env = Environ::new(&c.env);
        let got = if c.tty {
            colorprofile::env_profile(&env)
        } else {
            colorprofile::detect(false, &env)
        };
        assert_eq!(got.name(), c.out, "env {:?} tty {}", c.env, c.tty);
        assert_eq!(
            colorprofile::color_profile(c.tty, &env).name(),
            c.out,
            "env {:?} tty {}",
            c.env,
            c.tty
        );
    }
}

#[derive(Deserialize)]
struct ConvertCase {
    rgb: [u8; 3],
    c256: u8,
    c16: u8,
}

#[test]
fn convert256_cases() {
    let v = vectors();
    assert!(!v.convert256.is_empty());
    for c in &v.convert256 {
        assert_eq!(colorprofile::convert256(c.rgb), c.c256, "{:?}", c.rgb);
        assert_eq!(colorprofile::convert16(c.rgb), c.c16, "{:?}", c.rgb);
    }
}

#[derive(Deserialize)]
struct DownsampleCase {
    profile: String,
    #[serde(rename = "in")]
    input: String,
    out: String,
}

fn profile_named(name: &str) -> Profile {
    [
        Profile::NoTty,
        Profile::Ascii,
        Profile::Ansi,
        Profile::Ansi256,
        Profile::TrueColor,
    ]
    .into_iter()
    .find(|p| p.name() == name)
    .unwrap_or_else(|| panic!("unknown profile {name:?}"))
}

/// `colorprofile.Writer{Profile: p}.Write(in)`.
#[test]
fn downsample_cases() {
    let v = vectors();
    assert!(!v.downsample.is_empty());
    for c in &v.downsample {
        let p = profile_named(&c.profile);
        assert_eq!(
            &*colorprofile::downsample(&c.input, p),
            c.out.as_str(),
            "{} {:?}",
            c.profile,
            c.input
        );
    }
}

#[derive(Deserialize)]
struct UiScenario {
    name: String,
    title: String,
    offset_secs: i32,
    steps: Vec<UiStep>,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum UiStep {
    Set {
        report: Report,
    },
    Resize {
        width: u16,
        quit: bool,
        cancelled: bool,
    },
    Tick {
        #[serde(deserialize_with = "decimal_i64")]
        offset_ns: i64,
        quit: bool,
        cancelled: bool,
    },
    Event {
        #[serde(deserialize_with = "decimal_i64")]
        at_unix_ns: i64,
        level: i32,
        text: String,
        quit: bool,
        cancelled: bool,
    },
    CtrlC {
        quit: bool,
        cancelled: bool,
    },
    Done {
        #[serde(default)]
        error: Option<String>,
        quit: bool,
        cancelled: bool,
    },
    View {
        #[serde(default)]
        out: Option<String>,
        #[serde(default)]
        contains: Option<Vec<String>>,
    },
}

/// `{CLOCK}` stands for the clock of the `cancelling` event ctrl+c adds: the 8 bytes before
/// `  cancelling`.
fn with_clock_placeholder(view: &str) -> String {
    const MARK: &str = "  cancelling";
    let mut out = String::with_capacity(view.len());
    let mut rest = view;
    while let Some(i) = rest.find(MARK) {
        match i.checked_sub(8).filter(|&s| rest.is_char_boundary(s)) {
            Some(s) => {
                out.push_str(&rest[..s]);
                out.push_str("{CLOCK}");
            }
            None => out.push_str(&rest[..i]),
        }
        out.push_str(MARK);
        rest = &rest[i + MARK.len()..];
    }
    out.push_str(rest);
    out
}

fn run_scenario(sc: &UiScenario) {
    let latest = Latest::new();
    let set = latest.progress();
    let cancel = Ctx::background().with_cancel();
    // Just before the model's own start; offsets avoid half-second boundaries.
    let start = Instant::now();
    let mut m = UiModel::new(
        sc.title.clone(),
        Arc::clone(&latest),
        cancel.clone(),
        Arc::new(FixedZone(sc.offset_secs)),
    );
    for (i, step) in sc.steps.iter().enumerate() {
        let what = format!("{} step {i}", sc.name);
        let (msg, quit, cancelled) = match step {
            UiStep::Set { report } => {
                set(&report.to_client());
                continue;
            }
            UiStep::View { out, contains } => {
                let view = with_clock_placeholder(&m.view());
                if let Some(want) = out {
                    assert_eq!(&view, want, "{what}: view");
                }
                for s in contains.iter().flatten() {
                    assert!(
                        view.contains(s.as_str()),
                        "{what}: view lacks {s:?}:\n{view}"
                    );
                }
                continue;
            }
            UiStep::Resize {
                width,
                quit,
                cancelled,
            } => (UiMsg::Resize(*width), quit, cancelled),
            UiStep::Tick {
                offset_ns,
                quit,
                cancelled,
            } => (UiMsg::Tick(start + nanos(*offset_ns)), quit, cancelled),
            UiStep::Event {
                at_unix_ns,
                level,
                text,
                quit,
                cancelled,
            } => (
                UiMsg::Event {
                    at: GoTime::from_unix_nano(*at_unix_ns),
                    level: Level(*level),
                    text: text.clone(),
                },
                quit,
                cancelled,
            ),
            UiStep::CtrlC { quit, cancelled } => (UiMsg::CtrlC, quit, cancelled),
            UiStep::Done {
                error,
                quit,
                cancelled,
            } => (UiMsg::Done(error.clone()), quit, cancelled),
        };
        assert_eq!(m.update(msg), *quit, "{what}: quit");
        assert_eq!(cancel.err().is_some(), *cancelled, "{what}: cancelled");
    }
}

#[test]
fn ui_model_scenarios() {
    let v = vectors();
    assert!(!v.ui_model.is_empty());
    for sc in &v.ui_model {
        run_scenario(sc);
    }
}
