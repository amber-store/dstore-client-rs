//! `worktree/tree.go`: the working copy on disk (blocking).

use std::sync::Arc;

use amber_store_core::fstree;
use amber_store_core::key::{Key, Type};
use amber_store_core::packstore;
use dstore_gocompat::errno::{PathError, rewrite_os_errors};
use dstore_gocompat::json::{self, JsonField, JsonFieldSpec, JsonKind, JsonValue};
use dstore_gocompat::path::{self, to_path};
use dstore_gocompat::time::GoTime;
use dstore_gocompat::{hex, os};

use crate::error::{io_error, lossy, wrap};
use crate::{Change, DIR, Error, Kind};

const CONFIG_FILE: &[u8] = b"config";
const STATE_FILE: &[u8] = b"state";
const STORE_DIR: &[u8] = b"packstore";

/// The empty tree's key (`fstree.EncodeDirLeaf(nil)` in core v0.0.9).
const EMPTY_TREE_KEY: [u8; 32] = [
    0x20, 0x01, 0xbb, 0xe6, 0xa9, 0xf5, 0xa0, 0x14, 0x6a, 0x1f, 0x4d, 0x03, 0x81, 0xe9, 0xb0, 0xed,
    0x1a, 0xc2, 0xf1, 0xa9, 0x79, 0xce, 0x9d, 0x5a, 0xd8, 0x4e, 0x46, 0xff, 0x0b, 0x58, 0xf3, 0x6b,
];

/// Go's zero `time.Time` (0001-01-01T00:00:00Z).
pub(crate) const GO_ZERO_TIME: GoTime = GoTime {
    unix_secs: -62_135_596_800,
    nanos: 0,
};

/// `worktree.Config` (`.dstore/config`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    pub ticket: Vec<u8>,
    pub name: Vec<u8>,
    pub relay: Vec<u8>,
    pub no_relay: bool,
    pub no_discovery: bool,
    pub user: Vec<u8>,
}

/// `worktree.State` (`.dstore/state`): base is the tree the directory was last synced to; remote the
/// reference's tree as of the last fetch, with its cluster version (`has_remote` false: the reference does
/// not exist). When the reference names a commit (a branch), `remote_commit` is that commit and `remote` its
/// tree; otherwise `remote_commit` is the zero key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub base: Key,
    pub remote: Key,
    pub remote_commit: Key,
    pub has_remote: bool,
    pub remote_version: Option<Vec<u8>>,
    pub synced_at: GoTime,
}

impl State {
    /// `State.IsBranch`: whether the fetched reference names a commit.
    pub fn is_branch(&self) -> bool {
        self.has_remote && is_commit(&self.remote_commit)
    }

    /// `State.RemoteKey`: the key the reference named at the last fetch: the commit on a branch, else the
    /// tree.
    pub fn remote_key(&self) -> Key {
        if self.is_branch() {
            self.remote_commit
        } else {
            self.remote
        }
    }
}

/// Go `k.Type() == key.Commit`, read from the type nibble: core-rs `Key::type_` panics on a reserved one,
/// and the zero key is a Blob key.
pub(crate) fn is_commit(k: &Key) -> bool {
    Type::from_u8(k.0[0] >> 4) == Some(Type::Commit)
}

/// `*worktree.Tree`.
pub struct Tree {
    pub root: Vec<u8>,
    pub config: Config,
    pub state: State,
    pub store: Arc<packstore::Store>,
}

/// Where the reference stands on the cluster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteState {
    UpToDate,
    Moved,
    Absent,
}

/// `worktree.Status`.
pub struct Status {
    pub changes: Vec<Change>,
    pub meta_only: usize,
    pub remote: RemoteState,
    pub incoming: Vec<Change>,
}

/// Reads an object by key.
pub type Getter<'a> = &'a (dyn Fn(Key) -> Result<Vec<u8>, packstore::Error> + Sync);

/// The empty tree: key 2001bbe6…, bytes 80.
pub fn empty_tree() -> (Key, Vec<u8>) {
    match fstree::encode_dir_leaf(&[]) {
        Ok(obj) => (obj.key, obj.bytes),
        // A constant encoding that cannot fail (Go panics here).
        Err(_) => (Key(EMPTY_TREE_KEY), vec![0x80]),
    }
}

/// `worktree.Find`: the nearest ancestor of `dir` (included) holding a `.dstore` directory. `os.Stat`
/// follows symlinks, and any Stat error moves the walk up.
pub fn find(dir: &[u8]) -> Result<Vec<u8>, Error> {
    let mut abs = path::abs(dir).map_err(io_error)?;
    loop {
        let meta = path::join(&[&abs, DIR.as_bytes()]);
        if !meta.contains(&0) && std::fs::metadata(to_path(&meta)).is_ok_and(|md| md.is_dir()) {
            return Ok(abs);
        }
        let parent = path::dir(&abs);
        if parent == abs {
            return Err(Error::NotWorkingCopy);
        }
        abs = parent;
    }
}

/// `worktree.Remove`: `os.RemoveAll(dir/.dstore)`.
pub fn remove(dir: &[u8]) -> Result<(), Error> {
    os::remove_all(&path::join(&[dir, DIR.as_bytes()])).map_err(Error::Path)
}

impl Tree {
    /// `worktree.Open`: lock before state (a locked copy reports the lock error).
    pub fn open(dir: &[u8]) -> Result<Tree, Error> {
        let mut t = Tree::open_raw(dir)?;
        match load_state(&t.root) {
            Ok(st) => {
                t.state = st;
                Ok(t)
            }
            Err(e) => {
                let _ = t.close();
                Err(e)
            }
        }
    }

    /// `openRaw`: a working copy without its state file (a clone in progress). `state` holds the empty tree
    /// as base and Go's zero time.
    pub(crate) fn open_raw(dir: &[u8]) -> Result<Tree, Error> {
        let root = find(dir)?;
        let meta = path::join(&[&root, DIR.as_bytes()]);
        let b = os::read_file(&path::join(&[&meta, CONFIG_FILE]))
            .map_err(|e| wrap(format!("working copy {}: {e}", lossy(&root)), e))?;
        let config = decode_config(&b)
            .map_err(|e| wrap(format!("working copy {}: bad config: {e}", lossy(&root)), e))?;
        let store = open_store(&path::join(&[&meta, STORE_DIR]))?;
        let (empty, _) = empty_tree();
        Ok(Tree {
            root,
            config,
            state: State {
                base: empty,
                remote: Key([0; 32]),
                remote_commit: Key([0; 32]),
                has_remote: false,
                remote_version: None,
                synced_at: GO_ZERO_TIME,
            },
            store: Arc::new(store),
        })
    }

    /// `worktree.Create`: a fresh `.dstore` in `dir` holding the config and the empty tree. No state file.
    pub fn create(dir: &[u8], cfg: Config) -> Result<Tree, Error> {
        let abs = path::abs(dir).map_err(io_error)?;
        if let Ok(root) = find(&abs) {
            return Err(Error::Msg(format!(
                "{} is inside the working copy at {}",
                lossy(&abs),
                lossy(&root)
            )));
        }
        let meta = path::join(&[&abs, DIR.as_bytes()]);
        os::mkdir_all(&meta, 0o755).map_err(Error::Path)?;
        write_json(&path::join(&[&meta, CONFIG_FILE]), config_json(&cfg))?;
        let store = open_store(&path::join(&[&meta, STORE_DIR]))?;
        let (empty, bytes) = empty_tree();
        if let Err(e) = store.put(empty, &bytes) {
            let _ = store.close();
            return Err(Error::Packstore(e));
        }
        Ok(Tree {
            root: abs,
            config: cfg,
            state: State {
                base: empty,
                remote: Key([0; 32]),
                remote_commit: Key([0; 32]),
                has_remote: false,
                remote_version: None,
                synced_at: GoTime::now(),
            },
            store: Arc::new(store),
        })
    }

    pub fn close(self) -> Result<(), Error> {
        self.store.close().map_err(Error::Packstore)
    }

    /// `Tree.Get`: an object's payload from the local packstore.
    pub fn get(&self, k: Key) -> Result<Vec<u8>, packstore::Error> {
        self.store.get(k)
    }

    pub fn save_config(&self) -> Result<(), Error> {
        let p = path::join(&[&self.root, DIR.as_bytes(), CONFIG_FILE]);
        write_json(&p, config_json(&self.config))
    }

    pub fn save_state(&self) -> Result<(), Error> {
        let s = &self.state;
        let base = hex::encode(&s.base.0);
        let synced_at = dstore_gocompat::time::format_rfc3339nano_utc(s.synced_at);
        let (remote, version) = if s.has_remote {
            (
                hex::encode(&s.remote.0),
                hex::encode(s.remote_version.as_deref().unwrap_or_default()),
            )
        } else {
            (String::new(), String::new())
        };
        // `remote_commit,omitempty`: written only on a branch.
        let commit = if s.is_branch() {
            hex::encode(&s.remote_commit.0)
        } else {
            String::new()
        };
        let mut fields = vec![
            ("base", JsonField::Str(base.as_bytes())),
            ("remote", JsonField::Str(remote.as_bytes())),
        ];
        if !commit.is_empty() {
            fields.push(("remote_commit", JsonField::Str(commit.as_bytes())));
        }
        fields.push(("remote_version", JsonField::Str(version.as_bytes())));
        fields.push(("synced_at", JsonField::Str(synced_at.as_bytes())));
        let p = path::join(&[&self.root, DIR.as_bytes(), STATE_FILE]);
        write_json(&p, json::marshal_indent_object(&fields))
    }

    /// `Tree.Status`: the local changes (metadata-only ones counted), and how the fetched tree relates to
    /// base.
    pub fn status(&self, jobs: usize) -> Result<Status, Error> {
        let get = |k: Key| self.store.get(k);
        let changes = crate::scan(
            &self.root,
            self.state.base,
            &get,
            self.state.synced_at,
            jobs,
        )?;
        let mut s = Status {
            changes: Vec::new(),
            meta_only: 0,
            remote: RemoteState::Absent,
            incoming: Vec::new(),
        };
        for c in changes {
            if c.kind == Kind::MetaChanged {
                s.meta_only += 1;
            } else {
                s.changes.push(c);
            }
        }
        if !self.state.has_remote {
            s.remote = RemoteState::Absent;
        } else if self.state.remote == self.state.base {
            s.remote = RemoteState::UpToDate;
        } else {
            s.remote = RemoteState::Moved;
            s.incoming = crate::diff_trees(&get, self.state.base, self.state.remote)?;
        }
        Ok(s)
    }
}

/// Go `packstore.Open(dir, WithSync(true))` with Go's texts: `MkdirAll(dir, 0o755)` wrapped
/// `packstore: creating <dir>: …`, then `os.Open(dir)` raw, then the flock (`… is already open: <errno>`).
/// core-rs creates directories 0777 & ~umask and renders Rust errno texts, so the first two steps run here
/// (core-rs-gaps G3, G5, G6).
pub(crate) fn open_store(dir: &[u8]) -> Result<packstore::Store, Error> {
    if let Err(e) = os::mkdir_all(dir, 0o755) {
        return Err(wrap(format!("packstore: creating {}: {e}", lossy(dir)), e));
    }
    if let Err(e) = std::fs::File::open(to_path(dir)) {
        return Err(Error::Path(PathError {
            op: "open",
            path: dir.to_vec(),
            err: e,
        }));
    }
    match packstore::Store::open_with(to_path(dir), packstore::Options::new().sync(true)) {
        Ok(st) => Ok(st),
        Err(packstore::Error::Other(msg)) => Err(Error::Msg(rewrite_os_errors(&msg))),
        Err(e) => Err(Error::Packstore(e)),
    }
}

const CONFIG_FIELDS: &[JsonFieldSpec] = &[
    JsonFieldSpec {
        name: "ticket",
        kind: JsonKind::String,
    },
    JsonFieldSpec {
        name: "name",
        kind: JsonKind::String,
    },
    JsonFieldSpec {
        name: "relay",
        kind: JsonKind::String,
    },
    JsonFieldSpec {
        name: "no_relay",
        kind: JsonKind::Bool,
    },
    JsonFieldSpec {
        name: "no_discovery",
        kind: JsonKind::Bool,
    },
    JsonFieldSpec {
        name: "user",
        kind: JsonKind::String,
    },
];

const STATE_FIELDS: &[JsonFieldSpec] = &[
    JsonFieldSpec {
        name: "base",
        kind: JsonKind::String,
    },
    JsonFieldSpec {
        name: "remote",
        kind: JsonKind::String,
    },
    JsonFieldSpec {
        name: "remote_commit",
        kind: JsonKind::String,
    },
    JsonFieldSpec {
        name: "remote_version",
        kind: JsonKind::String,
    },
    JsonFieldSpec {
        name: "synced_at",
        kind: JsonKind::String,
    },
];

fn slot_str(slots: &[Option<JsonValue>], i: usize) -> Vec<u8> {
    match slots.get(i) {
        Some(Some(JsonValue::String(s))) => s.clone(),
        _ => Vec::new(),
    }
}

fn slot_bool(slots: &[Option<JsonValue>], i: usize) -> bool {
    matches!(slots.get(i), Some(Some(JsonValue::Bool(true))))
}

/// `json.Unmarshal(b, &cfg)` from the zero Config.
fn decode_config(b: &[u8]) -> Result<Config, json::JsonError> {
    let (slots, res) = json::unmarshal_object(b, "worktree.Config", CONFIG_FIELDS);
    res?;
    Ok(Config {
        ticket: slot_str(&slots, 0),
        name: slot_str(&slots, 1),
        relay: slot_str(&slots, 2),
        no_relay: slot_bool(&slots, 3),
        no_discovery: slot_bool(&slots, 4),
        user: slot_str(&slots, 5),
    })
}

/// `json.MarshalIndent(cfg, "", "  ")`, the omitempty fields dropped.
fn config_json(cfg: &Config) -> Vec<u8> {
    let mut fields = vec![
        ("ticket", JsonField::Str(&cfg.ticket)),
        ("name", JsonField::Str(&cfg.name)),
    ];
    if !cfg.relay.is_empty() {
        fields.push(("relay", JsonField::Str(&cfg.relay)));
    }
    if cfg.no_relay {
        fields.push(("no_relay", JsonField::Bool(true)));
    }
    if cfg.no_discovery {
        fields.push(("no_discovery", JsonField::Bool(true)));
    }
    if !cfg.user.is_empty() {
        fields.push(("user", JsonField::Str(&cfg.user)));
    }
    json::marshal_indent_object(&fields)
}

/// `writeJSON`: the bytes plus "\n" to `<path>.tmp` (`os.WriteFile`, 0o644), then `os.Rename`. No fsync.
fn write_json(p: &[u8], mut b: Vec<u8>) -> Result<(), Error> {
    b.push(b'\n');
    let mut tmp = p.to_vec();
    tmp.extend_from_slice(b".tmp");
    os::write_file(&tmp, &b, 0o644).map_err(Error::Path)?;
    crate::sys::rename(&tmp, p).map_err(|e| {
        let text = e.to_string();
        wrap(text, e)
    })
}

/// `hex.DecodeString`, then `key.Parse`.
fn parse_key(s: &[u8], what: &str) -> Result<Key, Error> {
    let b = hex::decode_string(s).map_err(|e| wrap(format!("bad state file: {what}: {e}"), e))?;
    Key::parse(&b).map_err(|e| wrap(format!("bad state file: {what}: {e}"), e))
}

/// `loadState`.
fn load_state(root: &[u8]) -> Result<State, Error> {
    let p = path::join(&[root, DIR.as_bytes(), STATE_FILE]);
    let b = match os::read_file(&p) {
        Ok(b) => b,
        Err(e) if e.err.raw_os_error() == Some(libc::ENOENT) => return Err(Error::Incomplete),
        Err(e) => return Err(Error::Path(e)),
    };
    let (slots, res) = json::unmarshal_object(&b, "worktree.stateJSON", STATE_FIELDS);
    if let Err(e) = res {
        return Err(wrap(format!("bad state file: {e}"), e));
    }
    let base = parse_key(&slot_str(&slots, 0), "base")?;
    let remote_hex = slot_str(&slots, 1);
    let mut s = State {
        base,
        remote: Key([0; 32]),
        remote_commit: Key([0; 32]),
        has_remote: false,
        remote_version: None,
        synced_at: GO_ZERO_TIME,
    };
    if !remote_hex.is_empty() {
        s.has_remote = true;
        s.remote = parse_key(&remote_hex, "remote")?;
        let commit_hex = slot_str(&slots, 2);
        if !commit_hex.is_empty() {
            // parse_key validated the key, so `type_` cannot panic.
            let rc = parse_key(&commit_hex, "remote_commit")?;
            if !is_commit(&rc) {
                return Err(Error::Msg(format!(
                    "bad state file: remote_commit {rc} is a {}",
                    rc.type_()
                )));
            }
            s.remote_commit = rc;
        }
        let version = hex::decode_string(&slot_str(&slots, 3))
            .map_err(|e| wrap(format!("bad state file: remote_version: {e}"), e))?;
        s.remote_version = Some(version);
    }
    let synced = slot_str(&slots, 4);
    // JSON strings decode to valid UTF-8 (invalid bytes become U+FFFD).
    let synced = String::from_utf8_lossy(&synced);
    s.synced_at = dstore_gocompat::time::parse_rfc3339nano(&synced)
        .map_err(|e| Error::Msg(format!("bad state file: synced_at: {e}")))?;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Scratch;

    fn cfg() -> Config {
        Config {
            ticket: b"dstore1abc".to_vec(),
            name: b"trees/demo".to_vec(),
            no_relay: true,
            user: b"me".to_vec(),
            ..Default::default()
        }
    }

    #[test]
    fn empty_tree_key() {
        let (k, b) = empty_tree();
        assert_eq!(k.0, EMPTY_TREE_KEY);
        assert_eq!(b, [0x80]);
        assert_eq!(k.to_string()[..16].to_string(), "2001bbe6a9f5a014");
    }

    // Port of TestCreateOpenRoundTrip.
    #[test]
    fn create_open_round_trip() {
        let dir = Scratch::new();
        let root = dir.bytes();
        let tr = Tree::create(&root, cfg()).expect("create");
        let (empty, _) = empty_tree();
        assert_eq!(tr.state.base, empty);
        tr.get(empty).expect("empty tree stored");
        tr.close().expect("close");
        assert!(matches!(Tree::open(&root), Err(Error::Incomplete)));
        match Tree::create(&root, cfg()) {
            Ok(t) => {
                let _ = t.close();
                panic!("Create over an existing .dstore succeeded");
            }
            Err(e) => {
                let r = lossy(&root).into_owned();
                assert_eq!(
                    e.to_string(),
                    format!("{r} is inside the working copy at {r}")
                );
            }
        }
        let mut tr = Tree::open_raw(&root).expect("open raw");
        tr.state.has_remote = true;
        tr.state.remote = empty;
        let branch = test_commit(&tr, empty);
        tr.state.remote_commit = branch;
        tr.state.remote_version = Some(vec![1, 2, 3]);
        tr.state.synced_at = GoTime {
            unix_secs: 1_700_000_000,
            nanos: 5,
        };
        tr.save_state().expect("save state");
        tr.close().expect("close");
        let state = std::fs::read(dir.join(".dstore/state")).expect("state file");
        assert_eq!(
            String::from_utf8_lossy(&state),
            format!(
                "{{\n  \"base\": \"2001bbe6a9f5a0146a1f4d0381e9b0ed1ac2f1a979ce9d5ad84e46ff0b58f36b\",\n  \"remote\": \"2001bbe6a9f5a0146a1f4d0381e9b0ed1ac2f1a979ce9d5ad84e46ff0b58f36b\",\n  \"remote_commit\": \"{branch}\",\n  \"remote_version\": \"010203\",\n  \"synced_at\": \"2023-11-14T22:13:20.000000005Z\"\n}}\n"
            )
        );
        let config = std::fs::read(dir.join(".dstore/config")).expect("config file");
        assert_eq!(
            String::from_utf8_lossy(&config),
            "{\n  \"ticket\": \"dstore1abc\",\n  \"name\": \"trees/demo\",\n  \"no_relay\": true,\n  \"user\": \"me\"\n}\n"
        );
        assert!(!dir.join(".dstore/state.tmp").exists());
        assert!(!dir.join(".dstore/config.tmp").exists());

        std::fs::create_dir_all(dir.join("a/b")).expect("mkdir");
        let got = Tree::open(&dir.join_bytes("a/b")).expect("open from a subdirectory");
        assert_eq!(got.root, root);
        assert_eq!(got.config, cfg());
        assert!(got.state.is_branch());
        assert_eq!(got.state.remote_commit, branch);
        assert_eq!(got.state.remote_key(), branch);
        assert!(got.state.has_remote);
        assert_eq!(got.state.remote, empty);
        assert_eq!(got.state.remote_version.as_deref(), Some(&[1u8, 2, 3][..]));
        assert_eq!(
            got.state.synced_at,
            GoTime {
                unix_secs: 1_700_000_000,
                nanos: 5
            }
        );
        got.close().expect("close");
    }

    /// A commit of `tree` by "tester" at 1 ns, stored in the working copy (the Go tests' commit).
    fn test_commit(tr: &Tree, tree: Key) -> Key {
        use amber_store_core::commit::{Commit, Identity};
        let id = Identity {
            name: "tester".into(),
            when: 1,
            ..Identity::default()
        };
        let (k, raw) = Commit {
            tree,
            parents: Vec::new(),
            author: id.clone(),
            committer: id,
            message: String::new(),
            signature: Vec::new(),
            public_key: Vec::new(),
        }
        .object()
        .expect("commit");
        tr.store.put(k, &raw).expect("put commit");
        k
    }

    /// `remote_commit` in the state file: written only on a branch, read only beside a remote, and it
    /// must name a commit.
    #[test]
    fn remote_commit_in_the_state_file() {
        let dir = Scratch::new();
        let root = dir.bytes();
        let mut tr = Tree::create(&root, cfg()).expect("create");
        let (empty, _) = empty_tree();
        let branch = test_commit(&tr, empty);
        let state_path = dir.join(".dstore/state");
        let read =
            || String::from_utf8(std::fs::read(&state_path).expect("state file")).expect("utf-8");

        // Not a branch: a zero remote_commit, a tree key in remote_commit, or no remote at all.
        tr.state.has_remote = true;
        tr.state.remote = empty;
        tr.state.remote_version = Some(vec![1]);
        assert!(!tr.state.is_branch());
        assert_eq!(tr.state.remote_key(), empty);
        tr.save_state().expect("save");
        assert!(!read().contains("remote_commit"), "{}", read());
        tr.state.remote_commit = empty;
        assert!(!tr.state.is_branch());
        tr.save_state().expect("save");
        assert!(!read().contains("remote_commit"), "{}", read());
        tr.state.remote_commit = branch;
        tr.state.has_remote = false;
        assert!(!tr.state.is_branch());
        assert_eq!(tr.state.remote_key(), empty);
        tr.save_state().expect("save");
        assert!(!read().contains("remote_commit"), "{}", read());
        tr.close().expect("close");

        let e = empty.to_string();
        let file = |remote: &str, commit: &str| {
            format!(
                r#"{{"base":"{e}","remote":"{remote}","remote_commit":"{commit}","remote_version":"01","synced_at":"2023-11-14T22:13:20Z"}}"#
            )
        };
        let open = |text: String| {
            std::fs::write(&state_path, text).expect("write state");
            Tree::open(&root).map(|t| {
                let s = t.state.clone();
                t.close().expect("close");
                s
            })
        };
        let s = open(file(&e, &branch.to_string())).expect("branch state");
        assert!(s.is_branch());
        assert_eq!(
            (s.remote, s.remote_commit, s.remote_key()),
            (empty, branch, branch)
        );
        // Without a remote the commit is not read, not even a malformed one.
        let s = open(file("", "zz")).expect("no remote");
        assert!(!s.has_remote && !s.is_branch());
        assert_eq!(s.remote_commit, Key([0; 32]));
        let err = |text: String| match open(text) {
            Ok(s) => panic!("opened: {s:?}"),
            Err(e) => e.to_string(),
        };
        assert_eq!(
            err(file(&e, "zz")),
            "bad state file: remote_commit: encoding/hex: invalid byte: U+007A 'z'"
        );
        assert_eq!(
            err(file(&e, &e)),
            format!("bad state file: remote_commit {e} is a DirLeaf")
        );
        // remote is parsed before remote_commit, remote_commit before remote_version.
        assert_eq!(
            err(file("yy", "zz")),
            "bad state file: remote: encoding/hex: invalid byte: U+0079 'y'"
        );
        assert_eq!(
            err(file(&e, &e).replace(r#""remote_version":"01""#, r#""remote_version":"xx""#)),
            format!("bad state file: remote_commit {e} is a DirLeaf")
        );
    }

    // Port of TestFindOutsideWorkingCopy.
    #[test]
    fn find_outside_working_copy() {
        let dir = Scratch::new();
        assert!(matches!(find(&dir.bytes()), Err(Error::NotWorkingCopy)));
    }

    // Port of TestRemove.
    #[test]
    fn remove_deletes_dstore() {
        let dir = Scratch::new();
        let tr = Tree::create(
            &dir.bytes(),
            Config {
                name: b"x".to_vec(),
                ..Default::default()
            },
        )
        .expect("create");
        tr.close().expect("close");
        remove(&dir.bytes()).expect("remove");
        assert!(!dir.join(".dstore").exists());
    }

    #[test]
    fn locked_copy_reports_the_lock_before_the_state() {
        let dir = Scratch::new();
        let root = dir.bytes();
        let tr = Tree::create(&root, cfg()).expect("create");
        let e = Tree::open(&root).err().expect("second open must fail");
        assert_eq!(
            e.to_string(),
            format!(
                "packstore: {}/.dstore/packstore is already open: resource temporarily unavailable",
                lossy(&root)
            )
        );
        tr.close().expect("close");
    }

    #[test]
    fn state_without_remote_ignores_version() {
        let dir = Scratch::new();
        let root = dir.bytes();
        Tree::create(&root, cfg())
            .expect("create")
            .close()
            .expect("close");
        std::fs::write(
            dir.join(".dstore/state"),
            r#"{"base":"2001BBE6A9F5A0146A1F4D0381E9B0ED1AC2F1A979CE9D5AD84E46FF0B58F36B","remote_version":"zz","synced_at":"2023-11-14T22:13:20+02:00"}"#,
        )
        .expect("write state");
        let t = Tree::open(&root).expect("open");
        assert!(!t.state.has_remote);
        assert_eq!(t.state.remote_version, None);
        assert_eq!(t.state.synced_at.unix_secs, 1_700_000_000 - 7200);
        t.close().expect("close");
    }

    #[test]
    fn bad_state_texts() {
        let dir = Scratch::new();
        let root = dir.bytes();
        Tree::create(&root, cfg())
            .expect("create")
            .close()
            .expect("close");
        for (file, want) in [
            (
                r#"{"base":"","synced_at":""}"#,
                "bad state file: base: key: data is not 32 bytes: got 0",
            ),
            (
                r#"{"base":"2001bbe6a9f5a0146a1f4d0381e9b0ed1ac2f1a979ce9d5ad84e46ff0b58f36b","synced_at":""}"#,
                "bad state file: synced_at: parsing time \"\" as \"2006-01-02T15:04:05.999999999Z07:00\": cannot parse \"\" as \"2006\"",
            ),
            ("[", "bad state file: unexpected end of JSON input"),
        ] {
            std::fs::write(dir.join(".dstore/state"), file).expect("write state");
            let e = Tree::open(&root).err().expect("open must fail");
            assert_eq!(e.to_string(), want, "{file}");
        }
    }

    #[test]
    fn status_counts_meta_and_remote() {
        let dir = Scratch::new();
        let root = dir.bytes();
        let mut tr = Tree::create(&root, cfg()).expect("create");
        tr.state.synced_at = GoTime::now();
        let s = tr.status(2).expect("status");
        assert!(s.changes.is_empty());
        assert_eq!(s.meta_only, 0);
        assert_eq!(s.remote, RemoteState::Absent);
        tr.state.has_remote = true;
        tr.state.remote = tr.state.base;
        crate::testutil::write_file(&dir.join("new.txt"), "n", 0o644);
        let s = tr.status(2).expect("status");
        assert_eq!(s.remote, RemoteState::UpToDate);
        assert_eq!(s.changes.len(), 1);
        assert_eq!(s.changes[0].path, b"new.txt");
        assert_eq!(s.changes[0].kind, Kind::Added);
        tr.close().expect("close");
    }
}
