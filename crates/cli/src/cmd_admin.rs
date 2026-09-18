//! Actions of `cluster`, `serve`, `token`, `node`, `voter`, `transition`, `gc` and `catalog`
//! (`cmd/dstore/main.go:279-698`, `client.go:98-132`).
//!
//! `args` holds what needs neither the network nor stdio: the argument checks, the requests, the prompt's
//! scanner and the output texts. This file adds Go's steps around them, in Go's order: `signalCtx`, dialing,
//! the admin call, printing and closing. The node-side paths follow PORTING.md §2.2.

mod args;

use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::ffi::OsStrExt;
use std::time::Duration;

use dstore_client::Cluster;
use dstore_gocli::{CliError, Context};
use dstore_gocompat::ctx::Ctx;
use futures::StreamExt;

use crate::common::{self, Session};
use crate::{nodeside, size};

use args::{AdminCommand, RestoreSource};

// ---- node-side commands (PORTING.md §2.2 A) ----

/// `cluster init` (`main.go:294-316`): node-side.
pub(crate) async fn cluster_init(c: &Context) -> Result<(), CliError> {
    let _ctx = common::signal_ctx();
    open_node(c, "cluster init")
}

/// `serve` (`main.go:414-432`): node-side.
pub(crate) async fn serve(c: &Context) -> Result<(), CliError> {
    let _ctx = common::signal_ctx();
    open_node(c, "serve")
}

/// `node join` (`main.go:486-533`): node-side, after the seed and token checks.
pub(crate) async fn node_join(c: &Context) -> Result<(), CliError> {
    let _ctx = common::signal_ctx();
    args::join_seed_and_token(c)?;
    open_node(c, "node join")
}

/// `openNode` (`main.go:229-237`) up to its first filesystem work: the store directory, then `--pack-size`,
/// then the node-side error. Nothing is created.
fn open_node(c: &Context, cmd: &str) -> Result<(), CliError> {
    if c.string("store").is_empty() {
        return Err(CliError::Msg(
            "no store directory: set --store or $DSTORE_STORE".to_string(),
        ));
    }
    size::pack_size(c).map_err(CliError::Msg)?;
    Err(nodeside::node_side_error(cmd))
}

// ---- cluster ----

/// `cluster status` (`main.go:322-331`).
pub(crate) async fn cluster_status(c: &Context) -> Result<(), CliError> {
    let ctx = common::signal_ctx();
    let session = dial(&ctx, c).await?;
    let mut out = std::io::stdout();
    let res = common::print_status(&ctx, &session.cluster, &mut out).await;
    let _ = out.flush();
    session.close().await;
    res
}

/// `cluster ticket` (`main.go:337-366`): without `--ticket` but with `--store`, the store's own ticket
/// (PORTING.md §2.2 B, no network); else `cluster-ticket` from the cluster, parsed and re-encoded.
pub(crate) async fn cluster_ticket(c: &Context) -> Result<(), CliError> {
    let ctx = common::signal_ctx();
    let store = c.os_string("store");
    if c.string("ticket").is_empty() && !store.is_empty() {
        let t = nodeside::local_ticket(store.as_bytes())?;
        print_stdout(&args::ticket_line(&t, c.bool("ids")));
        return Ok(());
    }
    let req = AdminCommand::ClusterTicket.request(c)?;
    let session = dial(&ctx, c).await?;
    let res = match common::admin(&ctx, &session.cluster, &req).await {
        Ok(r) => args::reply_ticket(&r),
        Err(e) => Err(e),
    };
    if let Ok(t) = &res {
        print_stdout(&args::ticket_line(t, c.bool("ids")));
    }
    session.close().await;
    res.map(|_| ())
}

/// `cluster replicas R` (`main.go:373-387`): R is checked, then the question is asked before dialing
/// unless `--yes`; only an answer starting with y or Y goes on.
pub(crate) async fn cluster_replicas(c: &Context) -> Result<(), CliError> {
    let req = AdminCommand::ClusterReplicas.request(c)?;
    if !c.bool("yes") {
        print_stdout(&args::replicas_prompt(req.replicas));
        let ans = tokio::task::spawn_blocking(read_answer)
            .await
            .unwrap_or_default();
        if !args::confirmed(&ans) {
            return Err(CliError::Msg("aborted".to_string()));
        }
    }
    common::admin_action(c, req).await
}

/// `fmt.Scanln(&ans)` on stdin, unbuffered (PORTING.md §5.9): it reads through a duplicate of fd 0, which
/// shares the file offset and bypasses std's buffer. A stdin that cannot be duplicated fails the scan,
/// which leaves `ans` empty as a failed read does in Go.
fn read_answer() -> String {
    match std::io::stdin().as_fd().try_clone_to_owned() {
        Ok(fd) => args::scanln_word(&mut WaitReadable(std::fs::File::from(fd))),
        Err(_) => String::new(),
    }
}

/// The pause before a read that would block is retried.
const WOULD_BLOCK_PAUSE: Duration = Duration::from_millis(10);

/// Go's `os.Stdin` (`os.NewFile`) registers a non-blocking fd 0 with the poller, so a read waits for input
/// (`os/file_unix.go` `newFile`: `pollable` when `nonBlocking`). A pipe or terminal left in O_NONBLOCK by
/// another process would make a plain `read(2)` fail with EAGAIN and the scan read as `aborted`; here such a
/// read is retried after a pause instead. This runs on a blocking thread.
struct WaitReadable<R>(R);

impl<R: Read> Read for WaitReadable<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            match self.0.read(buf) {
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    std::thread::sleep(WOULD_BLOCK_PAUSE)
                }
                r => return r,
            }
        }
    }
}

// ---- token ----

/// `token create` (`main.go:444-458`): prints the reply's text, a newline even when it is empty.
pub(crate) async fn token_create(c: &Context) -> Result<(), CliError> {
    let ctx = common::signal_ctx();
    let req = AdminCommand::TokenCreate.request(c)?;
    let session = dial(&ctx, c).await?;
    let res = common::admin(&ctx, &session.cluster, &req).await;
    if let Ok(r) = &res {
        print_stdout(&args::token_line(r));
    }
    session.close().await;
    res.map(|_| ())
}

// ---- node, voter, transition, gc, catalog backup(s): adminAction ----

/// `node remove ID [--dead] [--allow-unsafe]`.
pub(crate) async fn node_remove(c: &Context) -> Result<(), CliError> {
    admin_command(c, AdminCommand::NodeRemove).await
}

/// `node drain ID`.
pub(crate) async fn node_drain(c: &Context) -> Result<(), CliError> {
    admin_command(c, AdminCommand::NodeDrain).await
}

/// `node weight ID GiB`.
pub(crate) async fn node_weight(c: &Context) -> Result<(), CliError> {
    admin_command(c, AdminCommand::NodeWeight).await
}

/// `node zone ID ZONE`.
pub(crate) async fn node_zone(c: &Context) -> Result<(), CliError> {
    admin_command(c, AdminCommand::NodeZone).await
}

/// `node repair ID`.
pub(crate) async fn node_repair(c: &Context) -> Result<(), CliError> {
    admin_command(c, AdminCommand::NodeRepair).await
}

/// `voter add ID` (`act("voter-add")`).
pub(crate) async fn voter_add(c: &Context) -> Result<(), CliError> {
    admin_command(c, AdminCommand::VoterAdd).await
}

/// `voter remove ID [--allow-unsafe]` (`act("voter-remove")`).
pub(crate) async fn voter_remove(c: &Context) -> Result<(), CliError> {
    admin_command(c, AdminCommand::VoterRemove).await
}

/// `transition status`.
pub(crate) async fn transition_status(c: &Context) -> Result<(), CliError> {
    admin_command(c, AdminCommand::TransitionStatus).await
}

/// `transition abort`.
pub(crate) async fn transition_abort(c: &Context) -> Result<(), CliError> {
    admin_command(c, AdminCommand::TransitionAbort).await
}

/// `transition refreeze`.
pub(crate) async fn transition_refreeze(c: &Context) -> Result<(), CliError> {
    admin_command(c, AdminCommand::TransitionRefreeze).await
}

/// `transition pause`.
pub(crate) async fn transition_pause(c: &Context) -> Result<(), CliError> {
    admin_command(c, AdminCommand::TransitionPause).await
}

/// `transition resume`.
pub(crate) async fn transition_resume(c: &Context) -> Result<(), CliError> {
    admin_command(c, AdminCommand::TransitionResume).await
}

/// `gc run [--tolerate-missing] [--garbage F]`.
pub(crate) async fn gc_run(c: &Context) -> Result<(), CliError> {
    admin_command(c, AdminCommand::GcRun).await
}

/// `gc status`.
pub(crate) async fn gc_status(c: &Context) -> Result<(), CliError> {
    admin_command(c, AdminCommand::GcStatus).await
}

/// `gc hold`.
pub(crate) async fn gc_hold(c: &Context) -> Result<(), CliError> {
    admin_command(c, AdminCommand::GcHold).await
}

/// `gc release`.
pub(crate) async fn gc_release(c: &Context) -> Result<(), CliError> {
    admin_command(c, AdminCommand::GcRelease).await
}

/// `gc why KEY`.
pub(crate) async fn gc_why(c: &Context) -> Result<(), CliError> {
    admin_command(c, AdminCommand::GcWhy).await
}

/// `catalog backup`.
pub(crate) async fn catalog_backup(c: &Context) -> Result<(), CliError> {
    admin_command(c, AdminCommand::CatalogBackup).await
}

/// `catalog backups`.
pub(crate) async fn catalog_backups(c: &Context) -> Result<(), CliError> {
    admin_command(c, AdminCommand::CatalogBackups).await
}

/// `adminAction(c, req)` (`client.go:110-132`) with the request built, and its arguments checked, first.
async fn admin_command(c: &Context, cmd: AdminCommand) -> Result<(), CliError> {
    let req = cmd.request(c)?;
    common::admin_action(c, req).await
}

// ---- catalog restore (PORTING.md §2.2 C) ----

/// `catalog restore KEY|FILE` (`main.go:652-695`): the argument, the fetch of a key (before `--store` is
/// required), then the restore step.
pub(crate) async fn catalog_restore(c: &Context) -> Result<(), CliError> {
    let ctx = common::signal_ctx();
    let (_data, session) = match args::restore_source(c.first().as_bytes())? {
        RestoreSource::File(data) => (data, None),
        RestoreSource::Key(k) => {
            let session = dial(&ctx, c).await?;
            match fetch_backup(&ctx, &session.cluster, k).await {
                Ok(data) => (data, Some(session)),
                Err(e) => {
                    session.close().await;
                    return Err(e);
                }
            }
        }
    };
    let res = restore_step(c);
    if let Some(session) = session {
        session.close().await;
    }
    res
}

/// The fetch of `catalog restore KEY` (`main.go:669-681`): an error result is returned; each record's
/// payload replaces the data (a v0.1.9 get may deliver a record more than once); no record at all is
/// `backup object not found in the cluster`.
async fn fetch_backup(ctx: &Ctx, cl: &Cluster, k: [u8; 32]) -> Result<Vec<u8>, CliError> {
    let mut records = cl.get(ctx, vec![k]);
    let mut data = None;
    while let Some(r) = records.next().await {
        let r = r.map_err(CliError::msg)?;
        data = Some(common::record_payload(&r.record).map_err(CliError::msg)?);
    }
    data.ok_or_else(|| CliError::Msg("backup object not found in the cluster".to_string()))
}

/// The restore step (`main.go:683-694`): `--store` is required, then the write needs the node's paxos
/// acceptor (PORTING.md §2.2 C.4-5).
fn restore_step(c: &Context) -> Result<(), CliError> {
    if c.string("store").is_empty() {
        return Err(CliError::Msg(
            "restore runs on a voter: give --store".to_string(),
        ));
    }
    Err(CliError::Msg(
        nodeside::CATALOG_RESTORE_UNSUPPORTED.to_string(),
    ))
}

// ---- helpers ----

/// `dialCluster(ctx, c)` = `dialClusterLog(ctx, c, logger(c))` (`client.go:41-43`).
async fn dial(ctx: &Ctx, c: &Context) -> Result<Session, CliError> {
    common::dial_cluster(ctx, c, &common::logger(c)).await
}

/// One `fmt.Print*` to stdout, which Go does not buffer. Write errors are ignored, as Go's are.
fn print_stdout(text: &str) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
}

// The argument handling, the requests, the prompt and the texts (`args`) are tested in `tests/cli_admin.rs`
// over the golden vectors and Go's `fmt.Scanln`; the pre-dial outcomes of every action in
// `tests/cli_snapshots.rs` `admin_cases`. Here: the fetch, which needs the client's stream.
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use amber_store_core::key::{Key, Type};
    use dstore_client::Config;
    use dstore_gocompat::slog::{Attr, Handler, Level, Logger, Record};
    use dstore_testkit::fake::{FakeCluster, FakeClusterConfig};
    use dstore_transport::Endpoint;
    use dstore_transport::mem::Network;
    use dstore_view::NodeId;
    use dstore_wire::AdminRequest;

    use super::*;

    /// A log handler that drops everything.
    struct Discard;

    impl Handler for Discard {
        fn enabled(&self, _level: Level) -> bool {
            false
        }
        fn handle(&self, _handler_attrs: &[Attr], _r: &Record) {}
    }

    async fn dial_fake(net: &Arc<Network>, fc: &FakeCluster) -> Cluster {
        let endpoint: Arc<dyn Endpoint> = net.bind(NodeId([0xc1; 32]), &[]);
        let cfg = Config {
            endpoint: Some(endpoint),
            ticket: fc.ticket(),
            logger: Some(Logger::new(Arc::new(Discard))),
            ..Config::default()
        };
        match Cluster::dial(&Ctx::background(), cfg).await {
            Ok(c) => c,
            Err(e) => panic!("dial: {e}"),
        }
    }

    fn text(e: CliError) -> String {
        match e {
            CliError::Msg(m) => m,
            CliError::Exit { msg, code } => panic!("unexpected exit error {code}: {msg}"),
        }
    }

    /// A reader that would block twice before each byte, as a non-blocking pipe with a slow writer does.
    struct WouldBlockTwice {
        data: &'static [u8],
        pos: usize,
        blocks: u32,
    }

    impl Read for WouldBlockTwice {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.blocks > 0 {
                self.blocks -= 1;
                return Err(ErrorKind::WouldBlock.into());
            }
            self.blocks = 2;
            let (Some(slot), Some(&b)) = (buf.first_mut(), self.data.get(self.pos)) else {
                return Ok(0);
            };
            *slot = b;
            self.pos += 1;
            Ok(1)
        }
    }

    /// The prompt's answer on a non-blocking stdin: Go waits in the poller, so the word is read, not lost.
    #[test]
    fn a_non_blocking_stdin_is_waited_for() {
        let mut r = WaitReadable(WouldBlockTwice {
            data: b"yes please\n",
            pos: 0,
            blocks: 2,
        });
        assert_eq!(args::scanln_word(&mut r), "yes");
        // Other errors still fail the scan.
        let mut r = WaitReadable(FailingReader);
        assert_eq!(args::scanln_word(&mut r), "");
    }

    struct FailingReader;

    impl Read for FailingReader {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("boom"))
        }
    }

    /// `catalog restore KEY`'s fetch: the object `catalog backup` wrote, then a key no node holds.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fetch_backup_over_a_fake_cluster() {
        let net = Network::new();
        let fc = FakeCluster::start(&net, FakeClusterConfig::default()).await;
        let cl = dial_fake(&net, &fc).await;
        let ctx = Ctx::background();
        let req = AdminRequest {
            op: "catalog-backup".to_string(),
            ..AdminRequest::default()
        };
        let r = match common::admin(&ctx, &cl, &req).await {
            Ok(r) => r,
            Err(e) => panic!("catalog-backup: {}", text(e)),
        };
        let Ok(k) = <[u8; 32]>::try_from(r.key.as_slice()) else {
            panic!("backup key {:?}", r.key);
        };
        let data = match fetch_backup(&ctx, &cl, k).await {
            Ok(d) => d,
            Err(e) => panic!("fetch: {}", text(e)),
        };
        assert_eq!(
            Key::new(Type::Blob, data.len() as u64, &data).0,
            k,
            "the payload is the backup object's"
        );
        let absent = Key::new(Type::Blob, 12, b"never stored").0;
        match fetch_backup(&ctx, &cl, absent).await {
            Ok(d) => panic!("an absent key fetched {d:?}"),
            Err(e) => assert_eq!(text(e), "backup object not found in the cluster"),
        }
        cl.close();
        fc.close().await;
    }
}
