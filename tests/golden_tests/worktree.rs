//! Golden tests of `dstore-worktree` (owners worktree-offline, then worktree-flow): `worktree/config.json`,
//! `worktree/state.json`, `worktree/trees/`, `worktree/diff_trees.json`, `worktree/merge.json`,
//! `worktree/unified.json`, `worktree/cli.json` `kind_string` and `type_name`, and `errors/worktree_text.json`
//! (generator `tools/vectorgen/family_worktree.go`, schemas in VECTORS.md "Family `worktree`").
//!
//! Error texts that embed an fstree walk error or a cborx decode error go through
//! `dstore_client::corefmt`; those checks live in the `*_corefmt` tests.

use std::collections::HashMap;
use std::io::Write as _;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use amber_store_core::fstree::{self, Entry};
use amber_store_core::key::Key;
use amber_store_core::{ingest, packstore};
use dstore_gocompat::time::GoTime;
use dstore_testkit::golden::{Payload, golden_dir, hex, load_json};
use dstore_worktree as wt;
use serde::Deserialize;

// ---- shared helpers ----

/// Asserts that no case failed, showing the first failures.
fn check(family: &str, total: usize, failures: &[String]) {
    assert!(total > 0, "{family}: no cases");
    assert!(
        failures.is_empty(),
        "{family}: {} of {total} cases fail:\n{}",
        failures.len(),
        failures
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// A text field in its `x` / `x_hex` form.
fn text(s: &Option<String>, h: &Option<String>) -> Vec<u8> {
    match (s, h) {
        (Some(s), _) => s.as_bytes().to_vec(),
        (None, Some(h)) => hex(h),
        (None, None) => Vec::new(),
    }
}

fn dec_u64(s: &str) -> u64 {
    s.parse()
        .unwrap_or_else(|e| panic!("bad decimal u64 {s:?}: {e}"))
}

fn dec_i64(s: &str) -> i64 {
    s.parse()
        .unwrap_or_else(|e| panic!("bad decimal i64 {s:?}: {e}"))
}

fn key_of(h: &str) -> Key {
    let b = hex(h);
    let arr: [u8; 32] = b
        .as_slice()
        .try_into()
        .unwrap_or_else(|_| panic!("key {h} is not 32 bytes"));
    Key(arr)
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// A temporary directory like Go's `os.MkdirTemp("", …)`; removed after giving every directory rwx.
struct Temp(tempfile::TempDir);

impl Temp {
    fn new() -> Temp {
        Temp(
            tempfile::Builder::new()
                .prefix("worktree-golden-")
                .tempdir()
                .expect("tempdir"),
        )
    }

    fn path(&self) -> &Path {
        self.0.path()
    }

    fn bytes(&self) -> Vec<u8> {
        self.path().as_os_str().as_bytes().to_vec()
    }

    fn root_str(&self) -> String {
        self.path().display().to_string()
    }

    fn join(&self, rel: &str) -> PathBuf {
        self.path().join(rel)
    }
}

fn open_up(p: &Path) {
    if let Ok(md) = std::fs::symlink_metadata(p)
        && md.is_dir()
    {
        let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700));
        if let Ok(rd) = std::fs::read_dir(p) {
            for de in rd.flatten() {
                open_up(&de.path());
            }
        }
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        open_up(self.path());
    }
}

/// `err.Error()` with the temporary root replaced by `{ROOT}` (vectorgen `wtErrText`).
fn err_text(e: &wt::Error, root: &str) -> String {
    with_root(e.to_string(), root)
}

/// Replaces the temporary root by `{ROOT}`; an empty root replaces nothing.
fn with_root(s: String, root: &str) -> String {
    if root.is_empty() {
        s
    } else {
        s.replace(root, "{ROOT}")
    }
}

/// The text of an error returned raw by `diff_trees` or `scan`. Without `corefmt`, a walk error is rendered by
/// core-rs, which gives Go's text for errors that carry no entry names (every walk error in these vectors).
fn raw_err_text(e: &wt::Error, root: &str, via_corefmt: bool) -> String {
    match e {
        wt::Error::Walk(w) if !via_corefmt => with_root(w.to_string(), root),
        other => err_text(other, root),
    }
}

fn kind_of(s: &str) -> wt::Kind {
    match s {
        "new" => wt::Kind::Added,
        "deleted" => wt::Kind::Deleted,
        "modified" => wt::Kind::Modified,
        "type" => wt::Kind::TypeChanged,
        "mode" => wt::Kind::ModeChanged,
        "meta" => wt::Kind::MetaChanged,
        other => panic!("unknown kind {other:?}"),
    }
}

// ---- entries and changes ----

#[derive(Deserialize, Debug)]
struct EntryJson {
    name: Option<String>,
    name_hex: Option<String>,
    mode: String,
    uid: String,
    gid: String,
    mtime: String,
    #[serde(default)]
    content_key: String,
    link_target: Option<String>,
    link_target_hex: Option<String>,
    #[serde(default)]
    rdev: Vec<String>,
    #[serde(default)]
    xattrs_in: String,
    #[serde(default)]
    xattrs_key: String,
}

impl EntryJson {
    fn to_entry(&self) -> Entry {
        Entry {
            name: text(&self.name, &self.name_hex),
            mode: dec_u64(&self.mode),
            uid: dec_u64(&self.uid),
            gid: dec_u64(&self.gid),
            mtime: dec_i64(&self.mtime),
            content_key: hex(&self.content_key),
            link_target: text(&self.link_target, &self.link_target_hex),
            rdev: self.rdev.iter().map(|s| dec_u64(s)).collect(),
            xattrs_in: hex(&self.xattrs_in),
            xattrs_key: hex(&self.xattrs_key),
        }
    }
}

#[derive(Deserialize, Debug)]
struct ChangeJson {
    path: Option<String>,
    path_hex: Option<String>,
    kind: String,
    old: Option<EntryJson>,
    new: Option<EntryJson>,
}

impl ChangeJson {
    fn to_change(&self) -> wt::Change {
        wt::Change {
            path: text(&self.path, &self.path_hex),
            kind: kind_of(&self.kind),
            old: self.old.as_ref().map(|e| Arc::new(e.to_entry())),
            new: self.new.as_ref().map(|e| Arc::new(e.to_entry())),
        }
    }
}

fn changes_of(v: &Option<Vec<ChangeJson>>) -> Vec<wt::Change> {
    v.as_deref()
        .unwrap_or_default()
        .iter()
        .map(ChangeJson::to_change)
        .collect()
}

/// A change as comparable data: path, kind string, old and new entries.
type Flat = (Vec<u8>, String, Option<Entry>, Option<Entry>);

fn flatten(cs: &[wt::Change]) -> Vec<Flat> {
    cs.iter()
        .map(|c| {
            (
                c.path.clone(),
                c.kind.to_string(),
                c.old.as_deref().cloned(),
                c.new.as_deref().cloned(),
            )
        })
        .collect()
}

// ---- the tree fixtures (worktree/trees) ----

#[derive(Deserialize)]
struct TreeDoc {
    name: String,
    root: String,
}

#[derive(Deserialize)]
struct TreesJson {
    trees: Vec<TreeDoc>,
}

struct Fixtures {
    objects: HashMap<[u8; 32], Vec<u8>>,
    roots: HashMap<String, Key>,
}

impl Fixtures {
    fn load() -> Fixtures {
        let path = golden_dir().join("worktree/trees/objects.bin");
        let bin = std::fs::read(&path)
            .unwrap_or_else(|e| panic!("golden vector {} is missing: {e}", path.display()));
        let mut objects = HashMap::new();
        let mut rest = bin.as_slice();
        while !rest.is_empty() {
            let (head, tail) = rest.split_at(40);
            let k: [u8; 32] = head[..32].try_into().expect("key");
            let len = u64::from_be_bytes(head[32..40].try_into().expect("length"));
            let (bytes, tail) = tail.split_at(usize::try_from(len).expect("length"));
            objects.insert(k, bytes.to_vec());
            rest = tail;
        }
        let doc: TreesJson = load_json("worktree/trees/trees.json");
        let roots = doc
            .trees
            .iter()
            .map(|t| (t.name.clone(), key_of(&t.root)))
            .collect();
        Fixtures { objects, roots }
    }

    fn root(&self, name: &str) -> Key {
        *self
            .roots
            .get(name)
            .unwrap_or_else(|| panic!("no tree {name}"))
    }

    /// A getter over objects.bin: `packstore: object not found` for an absent key, as `Tree.Get`.
    fn get(&self, k: Key) -> Result<Vec<u8>, packstore::Error> {
        self.objects
            .get(&k.0)
            .cloned()
            .ok_or(packstore::Error::NotFound)
    }
}

#[derive(Deserialize)]
struct ObjectDoc {
    key: String,
    size: usize,
}

#[derive(Deserialize)]
struct XattrAttr {
    name: Option<String>,
    name_hex: Option<String>,
    value: String,
}

#[derive(Deserialize)]
struct XattrDoc {
    name: String,
    attrs: Vec<XattrAttr>,
    encoded: String,
    inline: bool,
    xattr_set_key: String,
}

#[derive(Deserialize)]
struct TreesInventory {
    objects: Vec<ObjectDoc>,
    omitted: Vec<String>,
    trees: Vec<TreeDoc>,
    xattrs: Vec<XattrDoc>,
}

/// `trees.json`: objects.bin holds exactly the listed objects (sizes included) and none of the omitted ones.
#[test]
fn trees_inventory() {
    let inv: TreesInventory = load_json("worktree/trees/trees.json");
    let fx = Fixtures::load();
    let mut failures = Vec::new();
    if fx.objects.len() != inv.objects.len() {
        failures.push(format!(
            "objects.bin holds {} objects, trees.json lists {}",
            fx.objects.len(),
            inv.objects.len()
        ));
    }
    for o in &inv.objects {
        match fx.objects.get(&key_of(&o.key).0) {
            Some(b) if b.len() == o.size => {}
            Some(b) => failures.push(format!("{}: {} bytes, want {}", o.key, b.len(), o.size)),
            None => failures.push(format!("{}: not in objects.bin", o.key)),
        }
    }
    for k in &inv.omitted {
        if fx.objects.contains_key(&key_of(k).0) {
            failures.push(format!("{k}: omitted but present"));
        }
    }
    for t in &inv.trees {
        if fx.root(&t.name) != key_of(&t.root) {
            failures.push(format!("tree {}: root differs", t.name));
        }
    }
    check(
        "trees inventory",
        inv.objects.len() + inv.omitted.len() + inv.trees.len(),
        &failures,
    );
}

/// `trees.json` `xattrs` (worktree §5 item 13): the encoding and the inline-or-spilled decision the scan
/// makes for a disk entry (`cborx.EncodeXattrs`, `<= ingest.DefaultXattrInlineMax`, `fstree.EncodeXattrSet`).
#[test]
fn xattr_encodings() {
    let inv: TreesInventory = load_json("worktree/trees/trees.json");
    let mut failures = Vec::new();
    for x in &inv.xattrs {
        let m: std::collections::BTreeMap<Vec<u8>, Vec<u8>> = x
            .attrs
            .iter()
            .map(|a| (text(&a.name, &a.name_hex), hex(&a.value)))
            .collect();
        let enc = amber_store_core::cbor::encode_xattrs(&m);
        if enc != hex(&x.encoded) {
            failures.push(format!(
                "{}: encoded {}, want {}",
                x.name,
                dstore_gocompat::hex::encode(&enc),
                x.encoded
            ));
        }
        let inline = enc.len() <= ingest::DEFAULT_XATTR_INLINE_MAX;
        if inline != x.inline {
            failures.push(format!("{}: inline {inline}, want {}", x.name, x.inline));
        }
        let set_key = fstree::encode_xattr_set(&m).key.to_string();
        if set_key != x.xattr_set_key {
            failures.push(format!(
                "{}: xattr set key {set_key}, want {}",
                x.name, x.xattr_set_key
            ));
        }
    }
    check("xattrs", inv.xattrs.len(), &failures);
}

// ---- disk scripts ----

#[derive(Deserialize, Debug)]
struct Op {
    op: String,
    path: String,
    mode: Option<u32>,
    text: Option<String>,
    content: Option<Payload>,
    target: Option<String>,
    mtime_unix: Option<String>,
    mtime_nsec: Option<u32>,
    size: Option<String>,
}

/// vectorgen `wtRunOps`.
fn run_ops(root: &Path, ops: &[Op]) {
    for op in ops {
        let full = root.join(&op.path);
        let perm = std::fs::Permissions::from_mode(op.mode.unwrap_or(0));
        let chmod = |p: &Path| std::fs::set_permissions(p, perm.clone()).expect("chmod");
        match op.op.as_str() {
            "dir" => {
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .create(&full)
                    .expect("mkdir");
                chmod(&full);
            }
            "file" => {
                let data = match (&op.text, &op.content) {
                    (Some(t), _) => t.as_bytes().to_vec(),
                    (None, Some(c)) => c.bytes(),
                    (None, None) => Vec::new(),
                };
                let mut f = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o600)
                    .open(&full)
                    .expect("open file");
                f.write_all(&data).expect("write file");
                drop(f);
                chmod(&full);
            }
            "symlink" => std::os::unix::fs::symlink(op.target.as_deref().expect("target"), &full)
                .expect("symlink"),
            "fifo" => {
                wt::sys::mkfifo(&full, 0o600).expect("mkfifo");
                chmod(&full);
            }
            "remove" => {
                dstore_gocompat::os::remove_all(full.as_os_str().as_bytes()).expect("remove")
            }
            "chmod" => chmod(&full),
            "mtime" => {
                let secs = dec_i64(op.mtime_unix.as_deref().expect("mtime_unix"));
                let ns = i128::from(secs) * 1_000_000_000
                    + i128::from(op.mtime_nsec.expect("mtime_nsec"));
                let t = std::time::UNIX_EPOCH
                    + std::time::Duration::from_nanos(u64::try_from(ns).expect("post-1970"));
                std::fs::File::open(&full)
                    .expect("open for times")
                    .set_times(std::fs::FileTimes::new().set_accessed(t).set_modified(t))
                    .expect("set times");
            }
            "truncate" => {
                let size = dec_u64(op.size.as_deref().expect("size"));
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&full)
                    .expect("open for truncate")
                    .set_len(size)
                    .expect("truncate");
            }
            other => panic!("unknown op {other:?}"),
        }
    }
}

fn open_temp_store(dir: &Temp) -> packstore::Store {
    packstore::Store::open_with(dir.join("ps"), packstore::Options::new().sync(false))
        .expect("open packstore")
}

fn ingest_base(st: &packstore::Store, root: &Path) -> Key {
    let opts = ingest::Opts {
        jobs: 2,
        ..Default::default()
    };
    ingest::dir(st, root, opts).1.expect("ingest base")
}

// ---- worktree/state.json and worktree/config.json ----

#[derive(Deserialize)]
struct EmptyTreeJson {
    key: String,
    bytes: String,
    short: String,
}

#[derive(Deserialize, Debug)]
struct StateJson {
    base: String,
    remote: String,
    has_remote: bool,
    remote_version: Option<String>,
    synced_at_unix: String,
    synced_at_nsec: u32,
}

impl StateJson {
    fn to_state(&self) -> wt::State {
        wt::State {
            base: key_of(&self.base),
            remote: key_of(&self.remote),
            has_remote: self.has_remote,
            remote_version: self.remote_version.as_deref().map(hex),
            synced_at: GoTime {
                unix_secs: dec_i64(&self.synced_at_unix),
                nanos: self.synced_at_nsec,
            },
        }
    }
}

#[derive(Deserialize)]
struct StateEncodeCase {
    name: String,
    state: StateJson,
    file: String,
}

#[derive(Deserialize)]
struct StateDecodeCase {
    name: String,
    setup: String,
    file: Option<String>,
    file_hex: Option<String>,
    ok: bool,
    state: Option<StateJson>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct StateVectors {
    empty_tree: EmptyTreeJson,
    encode: Vec<StateEncodeCase>,
    decode: Vec<StateDecodeCase>,
}

#[derive(Deserialize, Debug)]
struct ConfigJson {
    ticket: Option<String>,
    ticket_hex: Option<String>,
    name: Option<String>,
    name_hex: Option<String>,
    relay: Option<String>,
    relay_hex: Option<String>,
    no_relay: bool,
    no_discovery: bool,
    user: Option<String>,
    user_hex: Option<String>,
}

impl ConfigJson {
    fn to_config(&self) -> wt::Config {
        wt::Config {
            ticket: text(&self.ticket, &self.ticket_hex),
            name: text(&self.name, &self.name_hex),
            relay: text(&self.relay, &self.relay_hex),
            no_relay: self.no_relay,
            no_discovery: self.no_discovery,
            user: text(&self.user, &self.user_hex),
        }
    }
}

#[derive(Deserialize)]
struct ConfigEncodeCase {
    name: String,
    config: ConfigJson,
    file: Option<String>,
    file_hex: Option<String>,
}

#[derive(Deserialize)]
struct ConfigDecodeCase {
    name: String,
    file: Option<String>,
    file_hex: Option<String>,
    ok: bool,
    config: ConfigJson,
    error: Option<String>,
}

#[derive(Deserialize)]
struct ConfigVectors {
    encode: Vec<ConfigEncodeCase>,
    decode: Vec<ConfigDecodeCase>,
}

fn small_config() -> wt::Config {
    wt::Config {
        ticket: b"t".to_vec(),
        name: b"trees/x".to_vec(),
        ..Default::default()
    }
}

const VALID_STATE: &str = r#"{"base":"2001bbe6a9f5a0146a1f4d0381e9b0ed1ac2f1a979ce9d5ad84e46ff0b58f36b","synced_at":"2023-11-14T22:13:20Z"}"#;

#[test]
fn empty_tree() {
    let v: StateVectors = load_json("worktree/state.json");
    let (k, b) = wt::empty_tree();
    assert_eq!(k.to_string(), v.empty_tree.key);
    assert_eq!(b, hex(&v.empty_tree.bytes));
    assert_eq!(&k.to_string()[..16], v.empty_tree.short);
}

#[test]
fn config_encode() {
    let v: ConfigVectors = load_json("worktree/config.json");
    let mut failures = Vec::new();
    for c in &v.encode {
        let dir = Temp::new();
        let want = text(&c.file, &c.file_hex);
        let tr = match wt::Tree::create(&dir.bytes(), c.config.to_config()) {
            Ok(t) => t,
            Err(e) => {
                failures.push(format!("{}: create: {e}", c.name));
                continue;
            }
        };
        let got = std::fs::read(dir.join(".dstore/config")).expect("config file");
        if got != want {
            failures.push(format!(
                "{}: file {:?}, want {:?}",
                c.name,
                lossy(&got),
                lossy(&want)
            ));
        }
        if dir.join(".dstore/config.tmp").exists() {
            failures.push(format!("{}: config.tmp left behind", c.name));
        }
        std::fs::remove_file(dir.join(".dstore/config")).expect("remove config");
        if let Err(e) = tr.save_config() {
            failures.push(format!("{}: save_config: {e}", c.name));
        }
        let saved = std::fs::read(dir.join(".dstore/config")).expect("saved config");
        if saved != want {
            failures.push(format!("{}: save_config wrote {:?}", c.name, lossy(&saved)));
        }
        tr.close().expect("close");
    }
    check("config encode", v.encode.len(), &failures);
}

#[test]
fn config_decode() {
    let v: ConfigVectors = load_json("worktree/config.json");
    let mut failures = Vec::new();
    for c in &v.decode {
        let dir = Temp::new();
        std::fs::create_dir(dir.join(".dstore")).expect("mkdir .dstore");
        std::fs::write(dir.join(".dstore/config"), text(&c.file, &c.file_hex)).expect("config");
        std::fs::write(dir.join(".dstore/state"), VALID_STATE).expect("state");
        match (wt::Tree::open(&dir.bytes()), c.ok) {
            (Ok(t), true) => {
                let want = c.config.to_config();
                if t.config != want {
                    failures.push(format!(
                        "{}: config {:?}, want {:?}",
                        c.name, t.config, want
                    ));
                }
                t.close().expect("close");
            }
            (Ok(t), false) => {
                failures.push(format!("{}: decoded, want error {:?}", c.name, c.error));
                t.close().expect("close");
            }
            (Err(e), true) => failures.push(format!("{}: {e}", c.name)),
            (Err(e), false) => {
                let want = format!(
                    "working copy {{ROOT}}: bad config: {}",
                    c.error.as_deref().unwrap_or_default()
                );
                let got = err_text(&e, &dir.root_str());
                if got != want {
                    failures.push(format!("{}: {got:?}, want {want:?}", c.name));
                }
            }
        }
    }
    check("config decode", v.decode.len(), &failures);
}

#[test]
fn state_encode() {
    let v: StateVectors = load_json("worktree/state.json");
    let mut failures = Vec::new();
    for c in &v.encode {
        let dir = Temp::new();
        let mut tr = wt::Tree::create(&dir.bytes(), small_config()).expect("create");
        tr.state = c.state.to_state();
        if let Err(e) = tr.save_state() {
            failures.push(format!("{}: save_state: {e}", c.name));
        }
        let got = std::fs::read(dir.join(".dstore/state")).unwrap_or_default();
        if got != c.file.as_bytes() {
            failures.push(format!(
                "{}: file {:?}, want {:?}",
                c.name,
                lossy(&got),
                c.file
            ));
        }
        if dir.join(".dstore/state.tmp").exists() {
            failures.push(format!("{}: state.tmp left behind", c.name));
        }
        tr.close().expect("close");
    }
    check("state encode", v.encode.len(), &failures);
}

#[test]
fn state_decode() {
    let v: StateVectors = load_json("worktree/state.json");
    let mut failures = Vec::new();
    for c in &v.decode {
        let dir = Temp::new();
        wt::Tree::create(&dir.bytes(), small_config())
            .expect("create")
            .close()
            .expect("close");
        match c.setup.as_str() {
            "file" => std::fs::write(dir.join(".dstore/state"), text(&c.file, &c.file_hex))
                .expect("write state"),
            "missing" => {}
            "directory" => std::fs::create_dir(dir.join(".dstore/state")).expect("mkdir state"),
            other => panic!("unknown setup {other:?}"),
        }
        match (wt::Tree::open(&dir.bytes()), c.ok) {
            (Ok(t), true) => {
                let want = c.state.as_ref().map(StateJson::to_state);
                if Some(&t.state) != want.as_ref() {
                    failures.push(format!("{}: state {:?}, want {want:?}", c.name, t.state));
                }
                t.close().expect("close");
            }
            (Ok(t), false) => {
                failures.push(format!("{}: opened, want error {:?}", c.name, c.error));
                t.close().expect("close");
            }
            (Err(e), true) => failures.push(format!("{}: {e}", c.name)),
            (Err(e), false) => {
                let got = err_text(&e, &dir.root_str());
                if Some(got.as_str()) != c.error.as_deref() {
                    failures.push(format!("{}: {got:?}, want {:?}", c.name, c.error));
                }
            }
        }
    }
    check("state decode", v.decode.len(), &failures);
}

// ---- worktree/diff_trees.json ----

#[derive(Deserialize)]
struct DiffTreesCase {
    name: String,
    a: String,
    b: String,
    get_calls: usize,
    changes: Option<Vec<ChangeJson>>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct BaseObject {
    key: String,
    bytes: String,
}

#[derive(Deserialize)]
struct ScanChange {
    path: Option<String>,
    path_hex: Option<String>,
    kind: String,
    old_mode: Option<String>,
    new_mode: Option<String>,
}

#[derive(Deserialize)]
struct ScanCase {
    name: String,
    setup: Vec<Op>,
    base_key: Option<String>,
    base_objects: Option<Vec<BaseObject>>,
    edit: Vec<Op>,
    synced_at_unix: String,
    synced_at_nsec: u32,
    jobs: usize,
    changes: Option<Vec<ScanChange>>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct DiffTreesVectors {
    diff_trees: Vec<DiffTreesCase>,
    scan: Vec<ScanCase>,
}

/// Runs the DiffTrees cases; `texts_of_walk_errors` also compares the texts of fstree walk errors.
fn diff_trees_cases(texts_of_walk_errors: bool) {
    let v: DiffTreesVectors = load_json("worktree/diff_trees.json");
    let fx = Fixtures::load();
    let mut failures = Vec::new();
    for c in &v.diff_trees {
        let calls = AtomicUsize::new(0);
        let get = |k: Key| {
            calls.fetch_add(1, Ordering::Relaxed);
            fx.get(k)
        };
        let res = wt::diff_trees(&get, fx.root(&c.a), fx.root(&c.b));
        let n = calls.load(Ordering::Relaxed);
        if n != c.get_calls {
            failures.push(format!(
                "{}: {n} getter calls, want {}",
                c.name, c.get_calls
            ));
        }
        match (res, &c.error) {
            (Ok(got), None) => {
                let want = changes_of(&c.changes);
                if flatten(&got) != flatten(&want) {
                    failures.push(format!(
                        "{}: changes {:#?}, want {:#?}",
                        c.name,
                        flatten(&got),
                        flatten(&want)
                    ));
                }
            }
            (Ok(_), Some(want)) => failures.push(format!("{}: no error, want {want:?}", c.name)),
            (Err(e), None) => failures.push(format!(
                "{}: {}",
                c.name,
                raw_err_text(&e, "", texts_of_walk_errors)
            )),
            (Err(e), Some(want)) => {
                let got = raw_err_text(&e, "", texts_of_walk_errors);
                if &got != want {
                    failures.push(format!("{}: {got:?}, want {want:?}", c.name));
                }
            }
        }
    }
    check("diff_trees", v.diff_trees.len(), &failures);
}

#[test]
fn diff_trees() {
    diff_trees_cases(false);
}

#[test]
fn diff_trees_corefmt() {
    diff_trees_cases(true);
}

/// A scan change as comparable data: path, kind string, old mode, new mode.
type ScanRow = (Vec<u8>, String, Option<u64>, Option<u64>);

/// The mode a scan vector records: symlink permission bits masked.
fn scan_mode(e: Option<&Entry>) -> Option<u64> {
    e.map(|e| {
        if e.mode & 0o170000 == 0o120000 {
            0o120000
        } else {
            e.mode
        }
    })
}

fn scan_cases(texts_of_walk_errors: bool) {
    let v: DiffTreesVectors = load_json("worktree/diff_trees.json");
    let mut failures = Vec::new();
    for c in &v.scan {
        let root = Temp::new();
        let store_dir = Temp::new();
        let st = open_temp_store(&store_dir);
        run_ops(root.path(), &c.setup);
        let base = match &c.base_key {
            Some(k) => {
                for o in c.base_objects.as_deref().unwrap_or_default() {
                    st.put(key_of(&o.key), &hex(&o.bytes))
                        .expect("put base object");
                }
                key_of(k)
            }
            None => ingest_base(&st, root.path()),
        };
        run_ops(root.path(), &c.edit);
        let get = |k: Key| st.get(k);
        let synced_at = GoTime {
            unix_secs: dec_i64(&c.synced_at_unix),
            nanos: c.synced_at_nsec,
        };
        let res = wt::scan(&root.bytes(), base, &get, synced_at, c.jobs);
        match (res, &c.error) {
            (Ok(got), None) => {
                let got: Vec<ScanRow> = got
                    .iter()
                    .map(|ch| {
                        (
                            ch.path.clone(),
                            ch.kind.to_string(),
                            scan_mode(ch.old.as_deref()),
                            scan_mode(ch.new.as_deref()),
                        )
                    })
                    .collect();
                let want: Vec<ScanRow> = c
                    .changes
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .map(|ch| {
                        (
                            text(&ch.path, &ch.path_hex),
                            ch.kind.clone(),
                            ch.old_mode.as_deref().map(dec_u64),
                            ch.new_mode.as_deref().map(dec_u64),
                        )
                    })
                    .collect();
                if got != want {
                    failures.push(format!("{}: changes {got:?}, want {want:?}", c.name));
                }
            }
            (Ok(got), Some(want)) => failures.push(format!(
                "{}: {} changes, want error {want:?}",
                c.name,
                got.len()
            )),
            (Err(e), None) => failures.push(format!(
                "{}: {}",
                c.name,
                raw_err_text(&e, &root.root_str(), texts_of_walk_errors)
            )),
            (Err(e), Some(want)) => {
                let got = raw_err_text(&e, &root.root_str(), texts_of_walk_errors);
                if &got != want {
                    failures.push(format!("{}: {got:?}, want {want:?}", c.name));
                }
            }
        }
        st.close().expect("close store");
    }
    check("scan", v.scan.len(), &failures);
}

#[test]
fn scan() {
    scan_cases(false);
}

#[test]
fn scan_corefmt() {
    scan_cases(true);
}

// ---- worktree/merge.json ----

#[derive(Deserialize)]
struct MergeApply {
    path: Option<String>,
    path_hex: Option<String>,
    kind: String,
}

#[derive(Deserialize)]
struct MergeConflict {
    path: Option<String>,
    path_hex: Option<String>,
    local_path: Option<String>,
    local_path_hex: Option<String>,
    local_kind: String,
    incoming_kind: String,
}

#[derive(Deserialize)]
struct MergeCase {
    name: String,
    local: Option<Vec<ChangeJson>>,
    incoming: Option<Vec<ChangeJson>>,
    apply: Option<Vec<MergeApply>>,
    conflicts: Option<Vec<MergeConflict>>,
}

#[derive(Deserialize)]
struct MergeVectors {
    cases: Vec<MergeCase>,
}

#[test]
fn merge() {
    let v: MergeVectors = load_json("worktree/merge.json");
    let mut failures = Vec::new();
    for c in &v.cases {
        let local = changes_of(&c.local);
        let incoming = changes_of(&c.incoming);
        let (apply, conflicts) = wt::merge(&local, &incoming);
        let got_apply: Vec<(Vec<u8>, String)> = apply
            .iter()
            .map(|a| (a.path.clone(), a.kind.to_string()))
            .collect();
        let want_apply: Vec<(Vec<u8>, String)> = c
            .apply
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|a| (text(&a.path, &a.path_hex), a.kind.clone()))
            .collect();
        if got_apply != want_apply {
            failures.push(format!(
                "{}: apply {got_apply:?}, want {want_apply:?}",
                c.name
            ));
        }
        type Row = (Vec<u8>, Vec<u8>, String, String);
        let got_conflicts: Vec<Row> = conflicts
            .iter()
            .map(|k| {
                (
                    k.path.clone(),
                    k.local.path.clone(),
                    k.local.kind.to_string(),
                    k.incoming.kind.to_string(),
                )
            })
            .collect();
        let want_conflicts: Vec<Row> = c
            .conflicts
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|k| {
                (
                    text(&k.path, &k.path_hex),
                    text(&k.local_path, &k.local_path_hex),
                    k.local_kind.clone(),
                    k.incoming_kind.clone(),
                )
            })
            .collect();
        if got_conflicts != want_conflicts {
            failures.push(format!(
                "{}: conflicts {got_conflicts:?}, want {want_conflicts:?}",
                c.name
            ));
        }
    }
    check("merge", v.cases.len(), &failures);
}

// ---- worktree/unified.json ----

#[derive(Deserialize)]
struct UnifiedCase {
    name: String,
    a: String,
    b: String,
    unified: Option<String>,
    unified_hex: Option<String>,
    unified_error: Option<String>,
    stat: Option<String>,
    stat_hex: Option<String>,
    stat_error: Option<String>,
}

#[derive(Deserialize)]
struct DiskCase {
    name: String,
    setup: Vec<Op>,
    edit: Vec<Op>,
    after_scan: Vec<Op>,
    synced_at_unix: String,
    synced_at_nsec: u32,
    jobs: usize,
    changes: Vec<String>,
    unified: Option<String>,
    unified_hex: Option<String>,
    unified_error: Option<String>,
    stat: Option<String>,
    stat_hex: Option<String>,
    stat_error: Option<String>,
}

#[derive(Deserialize)]
struct UnifiedVectors {
    cases: Vec<UnifiedCase>,
    disk: Vec<DiskCase>,
}

/// Cases whose rendering embeds a walk error (through `corefmt`).
const UNIFIED_COREFMT: &[&str] = &["missing-content"];

/// Renders `unified` and `stat` into fresh buffers and compares them with the vector.
#[allow(clippy::too_many_arguments)]
fn compare_render(
    name: &str,
    changes: &[wt::Change],
    old: &dyn wt::Source,
    new: &dyn wt::Source,
    root: &str,
    want_unified: (Vec<u8>, &Option<String>),
    want_stat: (Vec<u8>, &Option<String>),
    failures: &mut Vec<String>,
) {
    let mut buf = Vec::new();
    let err = wt::unified(&mut buf, changes, old, new)
        .err()
        .map(|e| err_text(&e, root));
    if buf != want_unified.0 {
        failures.push(format!(
            "{name}: unified {:?}, want {:?}",
            lossy(&buf),
            lossy(&want_unified.0)
        ));
    }
    if err.as_deref() != want_unified.1.as_deref() {
        failures.push(format!(
            "{name}: unified error {err:?}, want {:?}",
            want_unified.1
        ));
    }
    let mut buf = Vec::new();
    let err = wt::stat(&mut buf, changes, old, new)
        .err()
        .map(|e| err_text(&e, root));
    if buf != want_stat.0 {
        failures.push(format!(
            "{name}: stat {:?}, want {:?}",
            lossy(&buf),
            lossy(&want_stat.0)
        ));
    }
    if err.as_deref() != want_stat.1.as_deref() {
        failures.push(format!(
            "{name}: stat error {err:?}, want {:?}",
            want_stat.1
        ));
    }
}

fn unified_tree_cases(corefmt_cases: bool) {
    let v: UnifiedVectors = load_json("worktree/unified.json");
    let fx = Fixtures::load();
    let get = |k: Key| fx.get(k);
    let mut failures = Vec::new();
    let mut total = 0;
    for c in &v.cases {
        if UNIFIED_COREFMT.contains(&c.name.as_str()) != corefmt_cases {
            continue;
        }
        total += 1;
        let changes = match wt::diff_trees(&get, fx.root(&c.a), fx.root(&c.b)) {
            Ok(ch) => ch,
            Err(e) => {
                failures.push(format!("{}: diff_trees: {e}", c.name));
                continue;
            }
        };
        let src = wt::TreeSource { get: &get };
        compare_render(
            &c.name,
            &changes,
            &src,
            &src,
            "",
            (text(&c.unified, &c.unified_hex), &c.unified_error),
            (text(&c.stat, &c.stat_hex), &c.stat_error),
            &mut failures,
        );
    }
    check("unified", total, &failures);
}

#[test]
fn unified_tree_to_tree() {
    unified_tree_cases(false);
}

#[test]
fn unified_tree_to_tree_corefmt() {
    unified_tree_cases(true);
}

#[test]
fn unified_tree_to_disk() {
    let v: UnifiedVectors = load_json("worktree/unified.json");
    let mut failures = Vec::new();
    for c in &v.disk {
        let root = Temp::new();
        let store_dir = Temp::new();
        let st = open_temp_store(&store_dir);
        run_ops(root.path(), &c.setup);
        let base = ingest_base(&st, root.path());
        run_ops(root.path(), &c.edit);
        let get = |k: Key| st.get(k);
        let synced_at = GoTime {
            unix_secs: dec_i64(&c.synced_at_unix),
            nanos: c.synced_at_nsec,
        };
        let changes = match wt::scan(&root.bytes(), base, &get, synced_at, c.jobs) {
            Ok(ch) => ch,
            Err(e) => {
                failures.push(format!("{}: scan: {e}", c.name));
                continue;
            }
        };
        let got: Vec<String> = changes
            .iter()
            .map(|ch| format!("{} {}", ch.kind, lossy(&ch.path)))
            .collect();
        if got != c.changes {
            failures.push(format!("{}: changes {got:?}, want {:?}", c.name, c.changes));
        }
        run_ops(root.path(), &c.after_scan);
        let disk = wt::DiskSource { root: root.bytes() };
        compare_render(
            &c.name,
            &changes,
            &wt::TreeSource { get: &get },
            &disk,
            &root.root_str(),
            (text(&c.unified, &c.unified_hex), &c.unified_error),
            (text(&c.stat, &c.stat_hex), &c.stat_error),
            &mut failures,
        );
        st.close().expect("close store");
    }
    check("unified disk", v.disk.len(), &failures);
}

// ---- worktree/cli.json ----

#[derive(Deserialize)]
struct KindString {
    kind: i64,
    out: String,
}

#[derive(Deserialize)]
struct TypeNameCase {
    mode: String,
    out: String,
}

#[derive(Deserialize)]
struct CliVectors {
    kind_string: Vec<KindString>,
    type_name: Vec<TypeNameCase>,
}

#[test]
fn kind_string_and_type_name() {
    let v: CliVectors = load_json("worktree/cli.json");
    let kinds = [
        wt::Kind::Added,
        wt::Kind::Deleted,
        wt::Kind::Modified,
        wt::Kind::TypeChanged,
        wt::Kind::ModeChanged,
        wt::Kind::MetaChanged,
    ];
    let mut failures = Vec::new();
    let mut total = 0;
    for c in &v.kind_string {
        // The Go values outside 0-5 have no Rust Kind.
        let Some(k) = usize::try_from(c.kind).ok().and_then(|i| kinds.get(i)) else {
            continue;
        };
        total += 1;
        if k.to_string() != c.out {
            failures.push(format!("Kind({}): {k}, want {}", c.kind, c.out));
        }
    }
    assert_eq!(total, 6);
    for c in &v.type_name {
        total += 1;
        let got = wt::type_name(dec_u64(&c.mode));
        if got != c.out {
            failures.push(format!("type_name({}): {got:?}, want {:?}", c.mode, c.out));
        }
    }
    check("cli", total, &failures);
}

// ---- errors/worktree_text.json ----

#[derive(Deserialize)]
struct ErrorCase {
    name: String,
    out: Option<String>,
    out_hex: Option<String>,
}

#[derive(Deserialize)]
struct ErrorVectors {
    cases: Vec<ErrorCase>,
}

fn error_vectors() -> HashMap<String, String> {
    let v: ErrorVectors = load_json("errors/worktree_text.json");
    v.cases
        .iter()
        .map(|c| (c.name.clone(), lossy(&text(&c.out, &c.out_hex))))
        .collect()
}

fn want_error<'a>(vectors: &'a HashMap<String, String>, name: &str) -> &'a str {
    vectors
        .get(name)
        .unwrap_or_else(|| panic!("errors/worktree_text.json has no case {name}"))
}

#[test]
fn sentinel_error_texts() {
    let v = error_vectors();
    let cases = [
        ("worktree/ErrNotWorkingCopy", wt::Error::NotWorkingCopy),
        ("worktree/ErrIncomplete", wt::Error::Incomplete),
        ("worktree/ErrNoRemote", wt::Error::NoRemote),
        ("worktree/ErrRemoteMoved", wt::Error::RemoteMoved),
        ("worktree/ErrRemoteDeleted", wt::Error::RemoteDeleted),
        ("worktree/ErrConflict", wt::Error::Conflict),
        ("worktree/ErrTooLarge", wt::Error::TooLarge),
    ];
    let mut failures = Vec::new();
    for (name, e) in &cases {
        if e.to_string() != want_error(&v, name) {
            failures.push(format!("{name}: {e}"));
        }
        let line = format!("dstore: {e}\n");
        if line != want_error(&v, &format!("cli-line/{name}")) {
            failures.push(format!("cli-line/{name}: {line:?}"));
        }
    }
    check("sentinels", cases.len(), &failures);
}

type Step = fn(&Temp) -> Result<(), Box<wt::Error>>;

fn create(root: &Temp) -> Result<(), Box<wt::Error>> {
    wt::Tree::create(&root.bytes(), small_config())
        .map_err(Box::new)?
        .close()
        .map_err(Box::new)
}

fn open(root: &Temp) -> Result<(), Box<wt::Error>> {
    wt::Tree::open(&root.bytes())
        .map_err(Box::new)?
        .close()
        .map_err(Box::new)
}

fn mkdir_dstore(root: &Temp) -> Result<(), Box<wt::Error>> {
    std::fs::create_dir(root.join(".dstore")).expect("mkdir .dstore");
    Ok(())
}

#[test]
fn tree_error_texts() {
    let v = error_vectors();
    let cases: Vec<(&str, Option<Step>, Step)> = vec![
        ("tree/find-not-a-working-copy", None, |r| {
            wt::find(&r.bytes()).map(|_| ()).map_err(Box::new)
        }),
        ("tree/open-not-a-working-copy", None, open),
        (
            "tree/open-dstore-is-a-file",
            Some(|r| {
                std::fs::write(r.join(".dstore"), b"").expect("write");
                Ok(())
            }),
            open,
        ),
        ("tree/open-no-config", Some(mkdir_dstore), open),
        (
            "tree/open-config-is-a-directory",
            Some(|r| {
                std::fs::create_dir_all(r.join(".dstore/config")).expect("mkdir");
                Ok(())
            }),
            open,
        ),
        (
            "tree/open-bad-config",
            Some(|r| {
                mkdir_dstore(r)?;
                std::fs::write(r.join(".dstore/config"), b"x").expect("write");
                Ok(())
            }),
            open,
        ),
        (
            "tree/open-bad-config-type",
            Some(|r| {
                mkdir_dstore(r)?;
                std::fs::write(r.join(".dstore/config"), br#"{"name":1}"#).expect("write");
                Ok(())
            }),
            open,
        ),
        (
            "tree/open-packstore-is-a-file",
            Some(|r| {
                create(r)?;
                dstore_gocompat::os::remove_all(r.join(".dstore/packstore").as_os_str().as_bytes())
                    .expect("remove packstore");
                std::fs::write(r.join(".dstore/packstore"), b"").expect("write");
                Ok(())
            }),
            open,
        ),
        ("tree/open-incomplete", Some(create), open),
        (
            "tree/open-locked",
            Some(|r| {
                create(r)?;
                std::fs::write(r.join(".dstore/state"), VALID_STATE).expect("write state");
                Ok(())
            }),
            |r| {
                let t = wt::Tree::open(&r.bytes()).map_err(Box::new)?;
                let res = open(r);
                t.close().map_err(Box::new)?;
                res
            },
        ),
        ("tree/open-locked-before-state-check", Some(create), |r| {
            let st = packstore::Store::open(r.join(".dstore/packstore")).expect("open store");
            let res = open(r);
            st.close().expect("close store");
            res
        }),
        ("tree/create-over-existing", Some(create), create),
        (
            "tree/create-inside",
            Some(|r| {
                create(r)?;
                std::fs::create_dir(r.join("sub")).expect("mkdir sub");
                Ok(())
            }),
            |r| {
                wt::Tree::create(r.join("sub").as_os_str().as_bytes(), small_config())
                    .map_err(Box::new)?
                    .close()
                    .map_err(Box::new)
            },
        ),
    ];
    let mut failures = Vec::new();
    for (name, prepare, act) in &cases {
        let root = Temp::new();
        if let Some(prepare) = prepare {
            prepare(&root).unwrap_or_else(|e| panic!("{name}: prepare: {e}"));
        }
        match act(&root) {
            Ok(()) => failures.push(format!("{name}: no error")),
            Err(e) => {
                let got = err_text(&e, &root.root_str());
                if got != want_error(&v, name) {
                    failures.push(format!("{name}: {got:?}, want {:?}", want_error(&v, name)));
                }
            }
        }
    }
    check("tree errors", cases.len(), &failures);
}

/// vectorgen `wtApplyErrors`.
fn apply_error_cases(corefmt_cases: bool) {
    const COREFMT: &[&str] = &[
        "apply/missing-content",
        "apply/bad-inline-xattrs",
        "apply/inline-xattrs-not-a-map",
    ];
    let v = error_vectors();
    let blob = fstree::encode_blob(b"content\n");
    let missing = fstree::encode_blob(b"missing content\n").key;
    let mut reserved = [0u8; 32];
    reserved[0] = 0x08;
    let blob_key = blob.key;
    let blob_bytes = blob.bytes.clone();
    let get = move |k: Key| {
        if k == blob_key {
            Ok(blob_bytes.clone())
        } else {
            Err(packstore::Error::NotFound)
        }
    };
    let file = |mode: u64| Entry {
        name: b"x".to_vec(),
        mode: 0o100000 | mode,
        content_key: blob_key.0.to_vec(),
        mtime: 1_600_000_000_000_000_000,
        ..Default::default()
    };
    let add = |path: &[u8], e: Entry| wt::Change {
        path: path.to_vec(),
        kind: wt::Kind::Added,
        old: None,
        new: Some(Arc::new(e)),
    };
    let del = |path: &[u8], e: Entry| wt::Change {
        path: path.to_vec(),
        kind: wt::Kind::Deleted,
        old: Some(Arc::new(e)),
        new: None,
    };
    let with = |f: fn(&mut Entry)| {
        let mut e = file(0o644);
        f(&mut e);
        e
    };
    type Prepare = fn(&Temp);
    let cases: Vec<(&str, Option<Prepare>, Vec<wt::Change>)> = vec![
        ("apply/empty-path", None, vec![add(b"", file(0o644))]),
        ("apply/unsafe-parent", None, vec![add(b"../x", file(0o644))]),
        (
            "apply/unsafe-inner-parent",
            None,
            vec![add(b"a/../x", file(0o644))],
        ),
        (
            "apply/unsafe-empty-component",
            None,
            vec![add(b"a//x", file(0o644))],
        ),
        ("apply/unsafe-dot", None, vec![add(b"./x", file(0o644))]),
        (
            "apply/unsafe-trailing-slash",
            None,
            vec![add(b"x/", file(0o644))],
        ),
        (
            "apply/unsafe-leading-slash",
            None,
            vec![add(b"/x", file(0o644))],
        ),
        (
            "apply/unsafe-trailing-dot",
            None,
            vec![del(b"a/.", file(0o644))],
        ),
        (
            "apply/unsafe-quoted-bytes",
            None,
            vec![add(
                &[b"\xff\t\"\\".as_slice(), "\u{a0}\u{1f600}/../x".as_bytes()].concat(),
                file(0o644),
            )],
        ),
        (
            "apply/unsafe-checked-before-writing",
            None,
            vec![add(b"ok", file(0o644)), del(b"..", file(0o644))],
        ),
        (
            "apply/symlinked-ancestor",
            Some(|r| {
                std::os::unix::fs::symlink(std::env::temp_dir(), r.join("lnk")).expect("symlink")
            }),
            vec![add(b"lnk/f", file(0o644))],
        ),
        (
            "apply/file-ancestor",
            Some(|r| std::fs::write(r.join("f"), b"x").expect("write")),
            vec![add(b"f/g", file(0o644))],
        ),
        (
            "apply/unsupported-type",
            None,
            vec![add(
                b"x",
                Entry {
                    name: b"x".to_vec(),
                    mode: 0o160644,
                    ..Default::default()
                },
            )],
        ),
        (
            "apply/unsupported-type-zero",
            None,
            vec![add(
                b"x",
                Entry {
                    name: b"x".to_vec(),
                    ..Default::default()
                },
            )],
        ),
        (
            "apply/bad-content-key",
            None,
            vec![add(
                b"x",
                Entry {
                    name: b"x".to_vec(),
                    mode: 0o100644,
                    content_key: vec![1, 2, 3],
                    ..Default::default()
                },
            )],
        ),
        (
            "apply/missing-content",
            None,
            vec![add(
                b"x",
                Entry {
                    name: b"x".to_vec(),
                    mode: 0o100644,
                    content_key: missing.0.to_vec(),
                    ..Default::default()
                },
            )],
        ),
        (
            "apply/bad-inline-xattrs",
            None,
            vec![add(b"x", with(|e| e.xattrs_in = vec![0xa1]))],
        ),
        (
            "apply/inline-xattrs-not-a-map",
            None,
            vec![add(b"x", with(|e| e.xattrs_in = vec![0x80]))],
        ),
        (
            "apply/missing-xattr-set",
            None,
            vec![add(b"x", {
                let mut e = file(0o644);
                e.xattrs_key = missing.0.to_vec();
                e
            })],
        ),
        (
            "apply/bad-xattr-set-key",
            None,
            vec![add(b"x", {
                let mut e = file(0o644);
                e.xattrs_key = reserved.to_vec();
                e
            })],
        ),
    ];
    let mut failures = Vec::new();
    let mut total = 0;
    for (name, prepare, changes) in &cases {
        if COREFMT.contains(name) != corefmt_cases {
            continue;
        }
        total += 1;
        let root = Temp::new();
        if let Some(prepare) = prepare {
            prepare(&root);
        }
        match wt::apply(&root.bytes(), changes, &get) {
            Ok(()) => failures.push(format!("{name}: no error")),
            Err(e) => {
                let got = err_text(&e, &root.root_str());
                if got != want_error(&v, name) {
                    failures.push(format!("{name}: {got:?}, want {:?}", want_error(&v, name)));
                }
            }
        }
        if *name == "apply/unsafe-checked-before-writing" && root.join("ok").exists() {
            failures.push(format!("{name}: wrote before checking every path"));
        }
    }
    check("apply errors", total, &failures);
}

#[test]
fn apply_error_texts() {
    apply_error_cases(false);
}

#[test]
fn apply_error_texts_corefmt() {
    apply_error_cases(true);
}
