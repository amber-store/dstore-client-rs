//! The command table: every definition of cli.md §2.8, in Go order (definitions only).
//!
//! Sources: `cmd/dstore/main.go:34-52` (the app), the flag groups of `main.go:73-121`, `client.go:204-206`
//! (`localStoreFlag`), `tui.go:34-36` (`noTUIFlag`), `wc.go:25-46` (`wcFlags`, `jobsFlag`), and the
//! `*Cmd()` functions of `main.go`, `client.go` and `wc.go`.
//!
//! The framework adds what urfave adds itself (cli.md §2.2.1-§2.2.2): the root `--help, -h` and
//! `--version, -v` flags, every command's `--help, -h`, and the shared `help, h` command.

use std::future::Future;
use std::pin::Pin;

use dstore_gocli::{Action, AppDef, CliError, CommandDef, Context, FlagDef, FlagKind};

use crate::size::DEFAULT_PACK_SIZE;

/// `time.Hour` in nanoseconds.
const HOUR_NS: i64 = 3_600_000_000_000;

/// The `dstore` app.
pub fn app() -> AppDef {
    AppDef {
        name: "dstore",
        usage: "a distributed amber store: cluster nodes and the client",
        version: crate::VERSION.to_string(),
        flags: vec![
            string(
                "log-level",
                "debug|info|warn|error (a global flag: give it before the command)",
            )
            .default_text("info")
            .env(&["DSTORE_LOG_LEVEL"]),
        ],
        commands: vec![
            cluster_cmd(),
            serve_cmd(),
            token_cmd(),
            node_cmd(),
            voter_cmd(),
            transition_cmd(),
            gc_cmd(),
            catalog_cmd(),
            store_cmd(),
            clone_cmd(),
            init_cmd(),
            fetch_cmd(),
            pull_cmd(),
            push_cmd(),
            status_cmd(),
            diff_cmd(),
            refs_cmd(),
            watch_cmd(),
            ref_cmd(),
            ls_cmd(),
            cat_cmd(),
        ],
    }
}

// ---- flag and command constructors ----

fn flag(name: &'static str, kind: FlagKind, usage: &'static str) -> FlagDef {
    FlagDef {
        name,
        aliases: &[],
        kind,
        usage,
        env: &[],
        required: false,
        disable_default_text: false,
    }
}

/// `cli.StringFlag`, default "".
fn string(name: &'static str, usage: &'static str) -> FlagDef {
    flag(name, FlagKind::String { default: "" }, usage)
}

/// `cli.BoolFlag`, default false.
fn boolean(name: &'static str, usage: &'static str) -> FlagDef {
    flag(name, FlagKind::Bool { default: false }, usage)
}

/// `cli.IntFlag`, default 0.
fn int(name: &'static str, usage: &'static str) -> FlagDef {
    flag(name, FlagKind::Int { default: 0 }, usage)
}

/// `cli.Int64Flag`, default 0.
fn int64(name: &'static str, usage: &'static str) -> FlagDef {
    flag(name, FlagKind::Int64 { default: 0 }, usage)
}

/// `cli.UintFlag`.
fn uint(name: &'static str, default: u64, usage: &'static str) -> FlagDef {
    flag(name, FlagKind::Uint { default }, usage)
}

/// `cli.Float64Flag`, default 0.
fn float64(name: &'static str, usage: &'static str) -> FlagDef {
    flag(name, FlagKind::Float64 { default: 0.0 }, usage)
}

/// `cli.DurationFlag` without usage.
fn duration(name: &'static str, default_ns: i64) -> FlagDef {
    flag(name, FlagKind::Duration { default_ns }, "")
}

/// `cli.StringSliceFlag` without a default.
fn string_slice(name: &'static str, usage: &'static str) -> FlagDef {
    flag(name, FlagKind::StringSlice, usage)
}

/// `Value`, `EnvVars` and `Required` of a flag literal.
trait FlagDefExt {
    /// The `Value` of a string flag.
    fn default_text(self, default: &'static str) -> Self;
    fn env(self, vars: &'static [&'static str]) -> Self;
    fn required(self) -> Self;
}

impl FlagDefExt for FlagDef {
    fn default_text(mut self, default: &'static str) -> FlagDef {
        self.kind = FlagKind::String { default };
        self
    }

    fn env(mut self, vars: &'static [&'static str]) -> FlagDef {
        self.env = vars;
        self
    }

    fn required(mut self) -> FlagDef {
        self.required = true;
        self
    }
}

/// A command without an action (urfave runs the help action).
fn parent(name: &'static str, usage: &'static str, subcommands: Vec<CommandDef>) -> CommandDef {
    CommandDef {
        name,
        aliases: &[],
        usage,
        args_usage: "",
        description: "",
        flags: Vec::new(),
        subcommands,
        action: None,
    }
}

/// A command with an action.
fn leaf(
    name: &'static str,
    usage: &'static str,
    args_usage: &'static str,
    flags: Vec<FlagDef>,
    action: Action,
) -> CommandDef {
    CommandDef {
        name,
        aliases: &[],
        usage,
        args_usage,
        description: "",
        flags,
        subcommands: Vec::new(),
        action: Some(action),
    }
}

/// `append(group, extra...)`.
fn with(mut group: Vec<FlagDef>, extra: impl IntoIterator<Item = FlagDef>) -> Vec<FlagDef> {
    group.extend(extra);
    group
}

// ---- shared flags (main.go:73-121, client.go:204-206, tui.go:34-36, wc.go:25-46) ----

fn store_flag() -> FlagDef {
    string("store", "store directory").env(&["DSTORE_STORE"])
}

fn ticket_flag() -> FlagDef {
    string(
        "ticket",
        "cluster ticket (dstore1…) or comma-separated node ids, found by discovery",
    )
    .env(&["DSTORE_TICKET"])
}

fn relay_flag() -> FlagDef {
    string(
        "relay",
        "relay URL for the fallback path (default: the built-in relay map)",
    )
}

fn no_relay_flag() -> FlagDef {
    boolean("no-relay", "direct addresses only, no relay")
}

/// `clientFlags`: the flags of every command that dials the cluster.
fn client_flags() -> Vec<FlagDef> {
    vec![
        ticket_flag(),
        relay_flag(),
        no_relay_flag(),
        no_discovery_flag(),
    ]
}

fn no_discovery_flag() -> FlagDef {
    boolean(
        "no-discovery",
        "neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS)",
    )
    .env(&["DSTORE_NO_DISCOVERY"])
}

fn net_flags() -> Vec<FlagDef> {
    vec![
        relay_flag(),
        no_relay_flag(),
        string_slice(
            "advertise-addr",
            "direct address to advertise, ip or ip:port (repeatable)",
        ),
        boolean(
            "loopback",
            "advertise 127.0.0.1 only (single-machine tests)",
        ),
        string("bind", "UDP address to bind, ip:port"),
        no_discovery_flag(),
    ]
}

fn node_flags() -> Vec<FlagDef> {
    with(
        vec![
            store_flag(),
            string(
                "paxos-dir",
                "acceptor state directory (default <store>/paxos; put it on its own device)",
            ),
            int64("rate", "reconcile copy rate in bytes/s (0 = unlimited)"),
            int("jobs", "parallelism (0 = cores)"),
            int64(
                "min-free",
                "free bytes below which uploads are refused (0 = 5% or 100 GiB)",
            ),
            string(
                "pack-size",
                "size at which the active pack is sealed (bytes or Ki/Mi/Gi/Ti); applies to packs written from now on",
            )
            .default_text(DEFAULT_PACK_SIZE)
            .env(&["DSTORE_PACK_SIZE"]),
            boolean(
                "gateway",
                "also serve the transport-iroh ALPN (not implemented in this version)",
            ),
            duration("gc-interval", 4 * HOUR_NS),
            duration("put-ttl", HOUR_NS),
        ],
        net_flags(),
    )
}

fn local_store_flag() -> FlagDef {
    string(
        "local",
        "local store directory (layout: <dir>/packstore, <dir>/refs)",
    )
    .env(&["AMBER_STORE"])
    .required()
}

fn no_tui_flag() -> FlagDef {
    boolean("no-tui", "plain log lines instead of the progress display").env(&["DSTORE_NO_TUI"])
}

/// `wcFlags`: no env vars; the stored config comes before `$DSTORE_TICKET`.
fn wc_flags() -> Vec<FlagDef> {
    vec![
        string(
            "ticket",
            "cluster ticket (dstore1…) or comma-separated node ids; overrides the stored one for this run",
        ),
        relay_flag(),
        no_relay_flag(),
        boolean(
            "no-discovery",
            "neither announce this endpoint nor resolve node ids by discovery",
        ),
    ]
}

fn jobs_flag() -> FlagDef {
    int("jobs", "parallelism (0 = cores)")
}

// ---- commands ----

fn cluster_cmd() -> CommandDef {
    parent(
        "cluster",
        "init, status, ticket, replicas",
        vec![
            leaf(
                "init",
                "create a cluster on this store and print its ticket; then run serve",
                "",
                with(
                    node_flags(),
                    [
                        uint("replicas", 3, "R: owners per object"),
                        uint(
                            "min-replicas",
                            0,
                            "owners that must hold an object before a write succeeds (default max(R\u{2212}1, 2))",
                        ),
                        string("weight", "capacity in GiB, or auto").default_text("auto"),
                        string("zone", "failure domain (default: the node id)"),
                        boolean("allow-unsafe", "allow min-replicas 1"),
                    ],
                ),
                act::cluster_init,
            ),
            leaf(
                "status",
                "view, epoch, reachability, disk, transition, gc, voters",
                "",
                with(client_flags(), [store_flag()]),
                act::cluster_status,
            ),
            leaf(
                "ticket",
                "print the bootstrap ticket, or with --ids the member ids to hand out instead",
                "",
                with(
                    client_flags(),
                    [
                        store_flag(),
                        boolean(
                            "ids",
                            "print comma-separated node ids (the short form, found by discovery)",
                        ),
                    ],
                ),
                act::cluster_ticket,
            ),
            leaf(
                "replicas",
                "change R (a transition that copies 1/R of the store)",
                "R",
                with(client_flags(), [boolean("yes", "do not ask")]),
                act::cluster_replicas,
            ),
        ],
    )
}

fn serve_cmd() -> CommandDef {
    leaf("serve", "run a node", "", node_flags(), act::serve)
}

fn token_cmd() -> CommandDef {
    parent(
        "token",
        "join tokens",
        vec![leaf(
            "create",
            "create a single-use join token",
            "",
            with(
                client_flags(),
                [uint(
                    "weight",
                    0,
                    "weight the token imposes on the joiner (GiB)",
                )],
            ),
            act::token_create,
        )],
    )
}

fn node_cmd() -> CommandDef {
    parent(
        "node",
        "join, remove, drain, weight, zone, repair",
        vec![
            leaf(
                "join",
                "join a cluster with this store and keep serving",
                "",
                with(
                    node_flags(),
                    [
                        string(
                            "seed",
                            "cluster ticket (dstore1…) or comma-separated node ids, found by discovery",
                        )
                        .required(),
                        string("token", "join token (hex)").required(),
                        string("weight", "capacity in GiB, or auto").default_text("auto"),
                        string("zone", ""),
                        boolean("no-ramp", "join at full weight in one step"),
                        boolean("no-vote", "hold no catalog (a relay-only or archive box)"),
                    ],
                ),
                act::node_join,
            ),
            leaf(
                "remove",
                "",
                "ID",
                with(
                    client_flags(),
                    [
                        boolean("dead", "the node is gone: remove its vote first"),
                        boolean("allow-unsafe", ""),
                    ],
                ),
                act::node_remove,
            ),
            leaf("drain", "", "ID", client_flags(), act::node_drain),
            leaf("weight", "", "ID GiB", client_flags(), act::node_weight),
            leaf("zone", "", "ID ZONE", client_flags(), act::node_zone),
            leaf("repair", "", "ID", client_flags(), act::node_repair),
        ],
    )
}

fn voter_cmd() -> CommandDef {
    parent(
        "voter",
        "change a node's vote after join",
        vec![
            leaf("add", "", "ID", client_flags(), act::voter_add),
            leaf(
                "remove",
                "",
                "ID",
                with(client_flags(), [boolean("allow-unsafe", "")]),
                act::voter_remove,
            ),
        ],
    )
}

fn transition_cmd() -> CommandDef {
    parent(
        "transition",
        "status, abort, refreeze, pause, resume",
        vec![
            leaf("status", "", "", client_flags(), act::transition_status),
            leaf("abort", "", "", client_flags(), act::transition_abort),
            leaf("refreeze", "", "", client_flags(), act::transition_refreeze),
            leaf("pause", "", "", client_flags(), act::transition_pause),
            leaf("resume", "", "", client_flags(), act::transition_resume),
        ],
    )
}

fn gc_cmd() -> CommandDef {
    parent(
        "gc",
        "run, status, why, hold, release",
        vec![
            leaf(
                "run",
                "",
                "",
                with(
                    client_flags(),
                    [
                        boolean("tolerate-missing", ""),
                        float64("garbage", "re-sweep only, at this dead ratio"),
                    ],
                ),
                act::gc_run,
            ),
            leaf("status", "", "", client_flags(), act::gc_status),
            leaf("hold", "", "", client_flags(), act::gc_hold),
            leaf("release", "", "", client_flags(), act::gc_release),
            leaf("why", "", "KEY", client_flags(), act::gc_why),
        ],
    )
}

fn catalog_cmd() -> CommandDef {
    parent(
        "catalog",
        "backup, restore, backups",
        vec![
            leaf("backup", "", "", client_flags(), act::catalog_backup),
            leaf(
                "backups",
                "list the backup object keys a node remembers",
                "",
                client_flags(),
                act::catalog_backups,
            ),
            leaf(
                "restore",
                "force-write every reference of a backup object",
                "KEY|FILE",
                with(client_flags(), [store_flag()]),
                act::catalog_restore,
            ),
        ],
    )
}

fn store_cmd() -> CommandDef {
    parent(
        "store",
        "push and pull between a standalone local store and the cluster",
        vec![
            leaf(
                "push",
                "build a tree from PATH into the local store and push it under NAME",
                "PATH NAME",
                with(
                    client_flags(),
                    [
                        local_store_flag(),
                        string("user", "user identity recorded in the reference"),
                        boolean("force", "replace unconditionally"),
                        string(
                            "expected-version",
                            "CAS: the version ref get printed (hex); omit to require the name to be new",
                        ),
                        int("jobs", ""),
                        no_tui_flag(),
                    ],
                ),
                act::store_push,
            ),
            leaf(
                "pull",
                "pull the tree under NAME into the local store",
                "NAME",
                with(
                    client_flags(),
                    [local_store_flag(), int("jobs", ""), no_tui_flag()],
                ),
                act::store_pull,
            ),
        ],
    )
}

fn clone_cmd() -> CommandDef {
    leaf(
        "clone",
        "clone the tree under NAME into DIR (default: the last segment of NAME) as a working copy",
        "NAME [DIR]",
        with(
            wc_flags(),
            [
                string("user", "user identity stored for pushes"),
                jobs_flag(),
                no_tui_flag(),
            ],
        ),
        act::clone,
    )
}

fn init_cmd() -> CommandDef {
    leaf(
        "init",
        "make the current directory a working copy of NAME, with nothing synced yet",
        "NAME",
        with(
            wc_flags(),
            [
                string("user", "user identity stored for pushes"),
                no_tui_flag(),
            ],
        ),
        act::init,
    )
}

fn fetch_cmd() -> CommandDef {
    leaf(
        "fetch",
        "record the reference's current tree as the remote and fetch its objects",
        "",
        with(wc_flags(), [no_tui_flag()]),
        act::fetch,
    )
}

fn pull_cmd() -> CommandDef {
    leaf(
        "pull",
        "fetch and apply the cluster's changes over the working directory",
        "",
        with(
            wc_flags(),
            [
                boolean("force", "take the cluster's side on conflicting paths"),
                jobs_flag(),
                no_tui_flag(),
            ],
        ),
        act::pull,
    )
}

fn push_cmd() -> CommandDef {
    leaf(
        "push",
        "build the working directory's tree, upload it and write the reference",
        "",
        with(
            wc_flags(),
            [
                string(
                    "user",
                    "user identity recorded in the reference (default: the stored one, then the OS user)",
                ),
                boolean("force", "replace the reference unconditionally"),
                jobs_flag(),
                no_tui_flag(),
            ],
        ),
        act::push,
    )
}

fn status_cmd() -> CommandDef {
    leaf(
        "status",
        "list the working directory's changes since the last sync, and whether the cluster moved",
        "",
        vec![jobs_flag()],
        act::status,
    )
}

fn diff_cmd() -> CommandDef {
    leaf(
        "diff",
        "unified diffs of the working directory against the last synced tree",
        "[PATH...]",
        vec![
            boolean("remote", "against the tree last fetched from the cluster"),
            boolean(
                "incoming",
                "the last synced tree against the fetched one (what pull would apply)",
            ),
            boolean("stat", "one line per changed path with line counts"),
            jobs_flag(),
        ],
        act::diff,
    )
}

fn refs_cmd() -> CommandDef {
    leaf(
        "refs",
        "list references",
        "[PREFIX]",
        client_flags(),
        act::refs,
    )
}

fn watch_cmd() -> CommandDef {
    let mut cmd = leaf(
        "watch",
        "watch references matching a glob and print each change until interrupted",
        "PATTERN",
        client_flags(),
        act::watch,
    );
    cmd.description = "PATTERN is path-style: * and ? match within one /-separated segment, ** as a whole segment matches any number of segments, [...] is a character class.\nEvery matching reference is printed first, then each change as it happens: NAME<TAB>KEY<TAB>CREATED<TAB>USER, or NAME<TAB>deleted.";
    cmd
}

fn ref_cmd() -> CommandDef {
    parent(
        "ref",
        "get or delete a reference",
        vec![
            leaf("get", "", "NAME", client_flags(), act::ref_get),
            leaf(
                "delete",
                "",
                "NAME",
                with(
                    client_flags(),
                    [boolean("force", ""), string("expected-version", "")],
                ),
                act::ref_delete,
            ),
        ],
    )
}

fn ls_cmd() -> CommandDef {
    leaf(
        "ls",
        "list a directory of a pushed tree",
        "NAME [PATH]",
        client_flags(),
        act::ls,
    )
}

fn cat_cmd() -> CommandDef {
    leaf(
        "cat",
        "write a file of a pushed tree to stdout",
        "NAME PATH",
        client_flags(),
        act::cat,
    )
}

/// The boxed future of an action.
type ActionFuture<'a> = Pin<Box<dyn Future<Output = Result<(), CliError>> + Send + 'a>>;

/// `Action` wrappers over the async actions of `cmd_admin`, `cmd_client` and `cmd_wc` (layer L5).
mod act {
    use super::{ActionFuture, Context};
    use crate::{cmd_admin, cmd_client, cmd_wc};

    macro_rules! wire {
        ($($name:ident => $target:path),* $(,)?) => {
            $(
                pub(super) fn $name(c: &Context) -> ActionFuture<'_> {
                    Box::pin($target(c))
                }
            )*
        };
    }

    wire! {
        cluster_init => cmd_admin::cluster_init,
        cluster_status => cmd_admin::cluster_status,
        cluster_ticket => cmd_admin::cluster_ticket,
        cluster_replicas => cmd_admin::cluster_replicas,
        serve => cmd_admin::serve,
        token_create => cmd_admin::token_create,
        node_join => cmd_admin::node_join,
        node_remove => cmd_admin::node_remove,
        node_drain => cmd_admin::node_drain,
        node_weight => cmd_admin::node_weight,
        node_zone => cmd_admin::node_zone,
        node_repair => cmd_admin::node_repair,
        voter_add => cmd_admin::voter_add,
        voter_remove => cmd_admin::voter_remove,
        transition_status => cmd_admin::transition_status,
        transition_abort => cmd_admin::transition_abort,
        transition_refreeze => cmd_admin::transition_refreeze,
        transition_pause => cmd_admin::transition_pause,
        transition_resume => cmd_admin::transition_resume,
        gc_run => cmd_admin::gc_run,
        gc_status => cmd_admin::gc_status,
        gc_hold => cmd_admin::gc_hold,
        gc_release => cmd_admin::gc_release,
        gc_why => cmd_admin::gc_why,
        catalog_backup => cmd_admin::catalog_backup,
        catalog_backups => cmd_admin::catalog_backups,
        catalog_restore => cmd_admin::catalog_restore,
        store_push => cmd_client::store_push,
        store_pull => cmd_client::store_pull,
        refs => cmd_client::refs,
        watch => cmd_client::watch,
        ref_get => cmd_client::ref_get,
        ref_delete => cmd_client::ref_delete,
        ls => cmd_client::ls,
        cat => cmd_client::cat,
        clone => cmd_wc::clone,
        init => cmd_wc::init,
        fetch => cmd_wc::fetch,
        pull => cmd_wc::pull,
        push => cmd_wc::push,
        status => cmd_wc::status,
        diff => cmd_wc::diff,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn names(flags: &[FlagDef]) -> Vec<&'static str> {
        flags.iter().map(|f| f.name).collect()
    }

    fn find<'a>(cmds: &'a [CommandDef], path: &[&str]) -> &'a CommandDef {
        let (first, rest) = path.split_first().expect("non-empty path");
        let cmd = cmds
            .iter()
            .find(|c| c.name == *first)
            .unwrap_or_else(|| panic!("no command {first}"));
        if rest.is_empty() {
            cmd
        } else {
            find(&cmd.subcommands, rest)
        }
    }

    /// Every command path with its subcommands, depth first, in definition order.
    fn walk<'a>(prefix: &str, cmds: &'a [CommandDef], out: &mut Vec<(String, &'a CommandDef)>) {
        for c in cmds {
            let path = if prefix.is_empty() {
                c.name.to_string()
            } else {
                format!("{prefix} {}", c.name)
            };
            out.push((path.clone(), c));
            walk(&path, &c.subcommands, out);
        }
    }

    const CLIENT: &[&str] = &["ticket", "relay", "no-relay", "no-discovery"];
    const NODE: &[&str] = &[
        "store",
        "paxos-dir",
        "rate",
        "jobs",
        "min-free",
        "pack-size",
        "gateway",
        "gc-interval",
        "put-ttl",
        "relay",
        "no-relay",
        "advertise-addr",
        "loopback",
        "bind",
        "no-discovery",
    ];
    const WC: &[&str] = &["ticket", "relay", "no-relay", "no-discovery"];

    fn cat(a: &[&'static str], b: &[&'static str]) -> Vec<&'static str> {
        a.iter().chain(b).copied().collect()
    }

    #[test]
    fn root() {
        let app = app();
        assert_eq!(app.name, "dstore");
        assert_eq!(
            app.usage,
            "a distributed amber store: cluster nodes and the client"
        );
        assert_eq!(app.version, crate::VERSION);
        assert_eq!(names(&app.flags), ["log-level"]);
        let top: Vec<_> = app.commands.iter().map(|c| c.name).collect();
        assert_eq!(
            top,
            [
                "cluster",
                "serve",
                "token",
                "node",
                "voter",
                "transition",
                "gc",
                "catalog",
                "store",
                "clone",
                "init",
                "fetch",
                "pull",
                "push",
                "status",
                "diff",
                "refs",
                "watch",
                "ref",
                "ls",
                "cat"
            ]
        );
    }

    /// cli.md §2.8: command paths, Go order, flag names in definition order, args usage.
    #[test]
    fn commands_and_flags() {
        let app = app();
        let mut all = Vec::new();
        walk("", &app.commands, &mut all);
        let got: Vec<(String, String, Vec<&'static str>)> = all
            .iter()
            .map(|(path, c)| (path.clone(), c.args_usage.to_string(), names(&c.flags)))
            .collect();
        let none: Vec<&'static str> = Vec::new();
        let want: Vec<(&str, &str, Vec<&'static str>)> = vec![
            ("cluster", "", none.clone()),
            (
                "cluster init",
                "",
                cat(
                    NODE,
                    &["replicas", "min-replicas", "weight", "zone", "allow-unsafe"],
                ),
            ),
            ("cluster status", "", cat(CLIENT, &["store"])),
            ("cluster ticket", "", cat(CLIENT, &["store", "ids"])),
            ("cluster replicas", "R", cat(CLIENT, &["yes"])),
            ("serve", "", NODE.to_vec()),
            ("token", "", none.clone()),
            ("token create", "", cat(CLIENT, &["weight"])),
            ("node", "", none.clone()),
            (
                "node join",
                "",
                cat(
                    NODE,
                    &["seed", "token", "weight", "zone", "no-ramp", "no-vote"],
                ),
            ),
            ("node remove", "ID", cat(CLIENT, &["dead", "allow-unsafe"])),
            ("node drain", "ID", CLIENT.to_vec()),
            ("node weight", "ID GiB", CLIENT.to_vec()),
            ("node zone", "ID ZONE", CLIENT.to_vec()),
            ("node repair", "ID", CLIENT.to_vec()),
            ("voter", "", none.clone()),
            ("voter add", "ID", CLIENT.to_vec()),
            ("voter remove", "ID", cat(CLIENT, &["allow-unsafe"])),
            ("transition", "", none.clone()),
            ("transition status", "", CLIENT.to_vec()),
            ("transition abort", "", CLIENT.to_vec()),
            ("transition refreeze", "", CLIENT.to_vec()),
            ("transition pause", "", CLIENT.to_vec()),
            ("transition resume", "", CLIENT.to_vec()),
            ("gc", "", none.clone()),
            ("gc run", "", cat(CLIENT, &["tolerate-missing", "garbage"])),
            ("gc status", "", CLIENT.to_vec()),
            ("gc hold", "", CLIENT.to_vec()),
            ("gc release", "", CLIENT.to_vec()),
            ("gc why", "KEY", CLIENT.to_vec()),
            ("catalog", "", none.clone()),
            ("catalog backup", "", CLIENT.to_vec()),
            ("catalog backups", "", CLIENT.to_vec()),
            ("catalog restore", "KEY|FILE", cat(CLIENT, &["store"])),
            ("store", "", none.clone()),
            (
                "store push",
                "PATH NAME",
                cat(
                    CLIENT,
                    &[
                        "local",
                        "user",
                        "force",
                        "expected-version",
                        "jobs",
                        "no-tui",
                    ],
                ),
            ),
            (
                "store pull",
                "NAME",
                cat(CLIENT, &["local", "jobs", "no-tui"]),
            ),
            ("clone", "NAME [DIR]", cat(WC, &["user", "jobs", "no-tui"])),
            ("init", "NAME", cat(WC, &["user", "no-tui"])),
            ("fetch", "", cat(WC, &["no-tui"])),
            ("pull", "", cat(WC, &["force", "jobs", "no-tui"])),
            ("push", "", cat(WC, &["user", "force", "jobs", "no-tui"])),
            ("status", "", vec!["jobs"]),
            (
                "diff",
                "[PATH...]",
                vec!["remote", "incoming", "stat", "jobs"],
            ),
            ("refs", "[PREFIX]", CLIENT.to_vec()),
            ("watch", "PATTERN", CLIENT.to_vec()),
            ("ref", "", none.clone()),
            ("ref get", "NAME", CLIENT.to_vec()),
            (
                "ref delete",
                "NAME",
                cat(CLIENT, &["force", "expected-version"]),
            ),
            ("ls", "NAME [PATH]", CLIENT.to_vec()),
            ("cat", "NAME PATH", CLIENT.to_vec()),
        ];
        let want: Vec<(String, String, Vec<&'static str>)> = want
            .into_iter()
            .map(|(p, a, f)| (p.to_string(), a.to_string(), f))
            .collect();
        assert_eq!(got, want);
        // 21 top-level commands and 30 subcommands: cluster 4, token 1, node 6, voter 2, transition 5,
        // gc 5, catalog 3, store 2, ref 2 (PORTING.md §2.1 says 32).
        assert_eq!(app.commands.len(), 21);
        assert_eq!(all.len() - app.commands.len(), 30);
    }

    /// Parents have no action (urfave runs the help action); every leaf has one.
    #[test]
    fn actions() {
        let app = app();
        let mut all = Vec::new();
        walk("", &app.commands, &mut all);
        for (path, c) in all {
            assert_eq!(
                c.action.is_some(),
                c.subcommands.is_empty(),
                "{path}: action presence"
            );
            assert!(c.aliases.is_empty(), "{path}: no command aliases");
        }
    }

    /// The 7 env vars (PORTING.md §11) and the required flags.
    #[test]
    fn env_and_required() {
        let app = app();
        let mut all = Vec::new();
        walk("", &app.commands, &mut all);
        let mut env = BTreeSet::new();
        let mut required = BTreeSet::new();
        for f in &app.flags {
            env.extend(f.env.iter().map(|e| (f.name, *e)));
        }
        for (path, c) in &all {
            let mut seen = BTreeSet::new();
            for f in &c.flags {
                assert!(seen.insert(f.name), "{path}: flag {} defined twice", f.name);
                assert!(f.aliases.is_empty(), "{path}: --{} has aliases", f.name);
                assert!(!f.disable_default_text, "{path}: --{}", f.name);
                env.extend(f.env.iter().map(|e| (f.name, *e)));
                if f.required {
                    required.insert((path.clone(), f.name));
                }
            }
        }
        let env: BTreeSet<_> = env.into_iter().collect();
        let want_env: BTreeSet<(&str, &str)> = [
            ("local", "AMBER_STORE"),
            ("log-level", "DSTORE_LOG_LEVEL"),
            ("no-discovery", "DSTORE_NO_DISCOVERY"),
            ("no-tui", "DSTORE_NO_TUI"),
            ("pack-size", "DSTORE_PACK_SIZE"),
            ("store", "DSTORE_STORE"),
            ("ticket", "DSTORE_TICKET"),
        ]
        .into_iter()
        .collect();
        assert_eq!(env, want_env);
        let required: Vec<_> = required.iter().map(|(p, n)| format!("{p} --{n}")).collect();
        assert_eq!(
            required,
            [
                "node join --seed",
                "node join --token",
                "store pull --local",
                "store push --local"
            ]
        );
        // wcFlags bind no env var.
        for path in [["clone"], ["init"], ["fetch"], ["pull"], ["push"]] {
            let c = find(&app.commands, &path);
            let ticket = c.flags.iter().find(|f| f.name == "ticket").expect("ticket");
            assert!(ticket.env.is_empty(), "{path:?}");
            let nd = c
                .flags
                .iter()
                .find(|f| f.name == "no-discovery")
                .expect("nd");
            assert!(nd.env.is_empty(), "{path:?}");
        }
    }

    /// Kinds and defaults that the help output shows.
    #[test]
    fn kinds_and_defaults() {
        let app = app();
        let kind = |path: &[&str], name: &str| -> String {
            let c = find(&app.commands, path);
            let f = c
                .flags
                .iter()
                .find(|f| f.name == name)
                .unwrap_or_else(|| panic!("{path:?} --{name}"));
            match f.kind {
                FlagKind::String { default } => format!("string {default:?}"),
                FlagKind::Bool { default } => format!("bool {default}"),
                FlagKind::Int { default } => format!("int {default}"),
                FlagKind::Int64 { default } => format!("int64 {default}"),
                FlagKind::Uint { default } => format!("uint {default}"),
                FlagKind::Float64 { default } => format!("float64 {default}"),
                FlagKind::Duration { default_ns } => format!("duration {default_ns}"),
                FlagKind::StringSlice => "slice".to_string(),
            }
        };
        match &app.flags[0].kind {
            FlagKind::String { default } => assert_eq!(*default, "info"),
            _ => panic!("log-level kind"),
        }
        let cases: &[(&[&str], &str, &str)] = &[
            (&["serve"], "rate", "int64 0"),
            (&["serve"], "jobs", "int 0"),
            (&["serve"], "min-free", "int64 0"),
            (&["serve"], "pack-size", "string \"2Gi\""),
            (&["serve"], "gateway", "bool false"),
            (&["serve"], "gc-interval", "duration 14400000000000"),
            (&["serve"], "put-ttl", "duration 3600000000000"),
            (&["serve"], "advertise-addr", "slice"),
            (&["serve"], "bind", "string \"\""),
            (&["cluster", "init"], "replicas", "uint 3"),
            (&["cluster", "init"], "min-replicas", "uint 0"),
            (&["cluster", "init"], "weight", "string \"auto\""),
            (&["node", "join"], "weight", "string \"auto\""),
            (&["node", "join"], "zone", "string \"\""),
            (&["token", "create"], "weight", "uint 0"),
            (&["gc", "run"], "garbage", "float64 0"),
            (&["gc", "run"], "tolerate-missing", "bool false"),
            (&["store", "push"], "jobs", "int 0"),
            (&["store", "push"], "local", "string \"\""),
            (&["diff"], "jobs", "int 0"),
            (&["ref", "delete"], "expected-version", "string \"\""),
        ];
        for &(path, name, want) in cases {
            assert_eq!(kind(path, name), want, "{path:?} --{name}");
        }
    }

    /// Usage texts with non-ASCII runes and the only description.
    #[test]
    fn texts() {
        let app = app();
        let init = find(&app.commands, &["cluster", "init"]);
        let min = init
            .flags
            .iter()
            .find(|f| f.name == "min-replicas")
            .expect("min-replicas");
        assert_eq!(
            min.usage,
            "owners that must hold an object before a write succeeds (default max(R\u{2212}1, 2))"
        );
        let refs = find(&app.commands, &["refs"]);
        assert_eq!(
            refs.flags[0].usage,
            "cluster ticket (dstore1\u{2026}) or comma-separated node ids, found by discovery"
        );
        let mut all = Vec::new();
        walk("", &app.commands, &mut all);
        let described: Vec<_> = all
            .iter()
            .filter(|(_, c)| !c.description.is_empty())
            .map(|(p, _)| p.as_str())
            .collect();
        assert_eq!(described, ["watch"]);
        let watch = find(&app.commands, &["watch"]);
        assert_eq!(watch.description.lines().count(), 2);
        // Commands without usage (help rows keep trailing spaces).
        let no_usage: Vec<_> = all
            .iter()
            .filter(|(_, c)| c.usage.is_empty())
            .map(|(p, _)| p.as_str())
            .collect();
        assert_eq!(
            no_usage,
            [
                "node remove",
                "node drain",
                "node weight",
                "node zone",
                "node repair",
                "voter add",
                "voter remove",
                "transition status",
                "transition abort",
                "transition refreeze",
                "transition pause",
                "transition resume",
                "gc run",
                "gc status",
                "gc hold",
                "gc release",
                "gc why",
                "catalog backup",
                "ref get",
                "ref delete"
            ]
        );
    }
}
