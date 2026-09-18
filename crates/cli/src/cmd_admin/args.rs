//! The part of the admin actions (`cmd/dstore/main.go`) that needs neither the network nor the process's
//! stdio: the argument checks, the `node.AdminRequest` each subcommand builds, the `cluster replicas`
//! prompt and its `fmt.Scanln` answer, the `catalog restore` argument, the `node join` checks, and the stdout
//! texts of `token create` and `cluster ticket`.
//!
//! `tests/cli_admin.rs` compiles this file as a module of its own (`#[path]`), so it names external crates
//! only, never `crate::` or `super::`.

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::io::{ErrorKind, Read};
use std::os::unix::ffi::OsStrExt;

use dstore_gocli::{CliError, Context};
use dstore_gocompat::{hex, strconv, strings};
use dstore_ticket::Ticket;
use dstore_view::parse_node_id;
use dstore_wire::{AdminReply, AdminRequest};

/// `errors.New(text)`.
fn msg(text: &str) -> CliError {
    CliError::Msg(text.to_string())
}

/// `node.AdminRequest{Op: name}`.
fn op(name: &str) -> AdminRequest {
    AdminRequest {
        op: name.to_string(),
        ..AdminRequest::default()
    }
}

/// `idArg` (`main.go:464-470`) and the voter `act` (`main.go:586`): `view.ParseNodeID(c.Args().First())`, its
/// error returned verbatim.
fn id_arg(c: &Context) -> Result<Vec<u8>, CliError> {
    parse_node_id(c.first().as_bytes())
        .map(|id| id.as_bytes().to_vec())
        .map_err(CliError::msg)
}

/// `strconv.ParseUint(s, 10, bits)` of a Go string. Bytes that are not UTF-8 are no digits either, so they
/// fail as Go's parse does.
fn parse_uint_arg(s: &OsStr, bits: u32) -> Option<u64> {
    s.to_str()
        .and_then(|s| strconv::parse_uint(s, 10, bits).ok())
}

/// `k, err := hex.DecodeString(s); err == nil && len(k) == 32`.
fn key32(s: &[u8]) -> Option<[u8; 32]> {
    hex::decode_string(s)
        .ok()
        .and_then(|k| <[u8; 32]>::try_from(k.as_slice()).ok())
}

/// The subcommands whose `node.AdminRequest` comes from their arguments alone, in `main.go` order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AdminCommand {
    ClusterTicket,
    ClusterReplicas,
    TokenCreate,
    NodeRemove,
    NodeDrain,
    NodeWeight,
    NodeZone,
    NodeRepair,
    VoterAdd,
    VoterRemove,
    TransitionStatus,
    TransitionAbort,
    TransitionRefreeze,
    TransitionPause,
    TransitionResume,
    GcRun,
    GcStatus,
    GcHold,
    GcRelease,
    GcWhy,
    CatalogBackup,
    CatalogBackups,
}

impl AdminCommand {
    /// The request, after the argument checks Go makes before dialing, in Go's order (cli.md §2.8).
    pub(crate) fn request(self, c: &Context) -> Result<AdminRequest, CliError> {
        Ok(match self {
            // main.go:352
            AdminCommand::ClusterTicket => op("cluster-ticket"),
            // main.go:374-377, 386: ParseUint(First(), 10, 8) ("" and "+3" fail; "0" does not).
            AdminCommand::ClusterReplicas => {
                let replicas = parse_uint_arg(c.first(), 8)
                    .and_then(|r| u8::try_from(r).ok())
                    .ok_or_else(|| msg("replicas R"))?;
                AdminRequest {
                    replicas,
                    ..op("replicas")
                }
            }
            // main.go:452: uint32(c.Uint("weight")) keeps the low 32 bits.
            AdminCommand::TokenCreate => AdminRequest {
                weight: c.uint("weight") as u32,
                ..op("token-create")
            },
            // main.go:537-541
            AdminCommand::NodeRemove => AdminRequest {
                node: id_arg(c)?,
                dead: c.bool("dead"),
                allow_unsafe: c.bool("allow-unsafe"),
                ..op("node-remove")
            },
            // main.go:545-549
            AdminCommand::NodeDrain => AdminRequest {
                node: id_arg(c)?,
                ..op("node-drain")
            },
            // main.go:553-561: the id first, then ParseUint(Get(1), 10, 32).
            AdminCommand::NodeWeight => {
                let node = id_arg(c)?;
                let weight = parse_uint_arg(c.arg(1), 32)
                    .and_then(|w| u32::try_from(w).ok())
                    .ok_or_else(|| msg("weight ID GiB"))?;
                AdminRequest {
                    node,
                    weight,
                    ..op("node-weight")
                }
            }
            // main.go:565-569: any zone, "" included (lossy for non-UTF-8 argv, PORTING.md DD-8).
            AdminCommand::NodeZone => AdminRequest {
                node: id_arg(c)?,
                zone: c.arg(1).to_string_lossy().into_owned(),
                ..op("node-zone")
            },
            // main.go:573-577
            AdminCommand::NodeRepair => AdminRequest {
                node: id_arg(c)?,
                ..op("node-repair")
            },
            // main.go:584-592: `voter add` has no --allow-unsafe, so it reads false.
            AdminCommand::VoterAdd => voter(c, "voter-add")?,
            AdminCommand::VoterRemove => voter(c, "voter-remove")?,
            // main.go:604-606
            AdminCommand::TransitionStatus => op("transition-status"),
            AdminCommand::TransitionAbort => op("transition-abort"),
            AdminCommand::TransitionRefreeze => op("transition-refreeze"),
            AdminCommand::TransitionPause => op("transition-pause"),
            AdminCommand::TransitionResume => op("transition-resume"),
            // main.go:626-628
            AdminCommand::GcRun => AdminRequest {
                tolerate: c.bool("tolerate-missing"),
                garbage: c.float64("garbage"),
                ..op("gc-run")
            },
            // main.go:629-631
            AdminCommand::GcStatus => op("gc-status"),
            AdminCommand::GcHold => AdminRequest {
                pause: true,
                ..op("gc-hold")
            },
            AdminCommand::GcRelease => op("gc-hold"),
            // main.go:633-638: strict hex of 32 bytes.
            AdminCommand::GcWhy => AdminRequest {
                key: key32(c.first().as_bytes())
                    .ok_or_else(|| msg("why KEY (64 hex chars)"))?
                    .to_vec(),
                ..op("gc-why")
            },
            // main.go:649-650
            AdminCommand::CatalogBackup => op("catalog-backup"),
            AdminCommand::CatalogBackups => op("catalog-backups"),
        })
    }
}

/// The voter `act(op)` (`main.go:584-592`).
fn voter(c: &Context, name: &str) -> Result<AdminRequest, CliError> {
    Ok(AdminRequest {
        node: id_arg(c)?,
        allow_unsafe: c.bool("allow-unsafe"),
        ..op(name)
    })
}

// ---- cluster replicas: the prompt ----

/// The question of `cluster replicas` without `--yes` (`main.go:379`), printed without a newline.
pub(crate) fn replicas_prompt(r: u8) -> String {
    format!("changing R to {r} moves about 1/{r} of every node's data; continue? [y/N] ")
}

/// `strings.HasPrefix(strings.ToLower(ans), "y")` (`main.go:382`).
pub(crate) fn confirmed(ans: &str) -> bool {
    strings::to_lower(ans.as_bytes()).starts_with(b"y")
}

/// `var ans string; fmt.Scanln(&ans)` (`fmt/scan.go`: `Fscanln` → `doScan` → `scanOne` →
/// `convertString('v')`) over a reader that is not an `io.RuneScanner`, as `os.Stdin` is not. Runes are read
/// one byte at a time (`readRune`), so no more input is consumed than Go consumes, and a reader that stays
/// open blocks where Go's does.
///
/// Returns `ans` afterwards: the first space-separated word of the line, or "" when the scan fails before
/// the word is stored (a newline or EOF before any word, or a read error). The trailing check (only spaces
/// until the newline) runs after the word is stored, so its failure keeps the word.
pub(crate) fn scanln_word(src: &mut dyn Read) -> String {
    let mut s = Scanner::new(src);
    match s.convert_string() {
        Ok(word) => {
            let _ = s.check_newline();
            word
        }
        Err(Abort) => String::new(),
    }
}

/// A scan error (Go panics with `scanError` and `errorHandler` recovers it).
struct Abort;

/// fmt's `space` table, a copy of the `unicode.White_Space` ranges.
const SPACE: [(u32, u32); 10] = [
    (0x0009, 0x000d),
    (0x0020, 0x0020),
    (0x0085, 0x0085),
    (0x00a0, 0x00a0),
    (0x1680, 0x1680),
    (0x2000, 0x200a),
    (0x2028, 0x2029),
    (0x202f, 0x202f),
    (0x205f, 0x205f),
    (0x3000, 0x3000),
];

/// fmt's `isSpace`.
fn is_space(r: char) -> bool {
    let r = u32::from(r);
    SPACE.iter().any(|&(lo, hi)| (lo..=hi).contains(&r))
}

/// `utf8.FullRune`: `p` holds a whole encoding, or an invalid one (which decodes as one error rune).
fn full_rune(p: &[u8]) -> bool {
    match std::str::from_utf8(p) {
        Ok(_) => true,
        Err(e) => e.valid_up_to() > 0 || e.error_len().is_some(),
    }
}

/// `utf8.DecodeRune`: the first rune and its length; an invalid or short encoding is (U+FFFD, 1).
fn decode_rune(p: &[u8]) -> (char, usize) {
    let valid = match std::str::from_utf8(p) {
        Ok(s) => s,
        Err(e) => std::str::from_utf8(&p[..e.valid_up_to()]).unwrap_or_default(),
    };
    match valid.chars().next() {
        Some(r) => (r, r.len_utf8()),
        None => (char::REPLACEMENT_CHARACTER, 1),
    }
}

/// fmt's `ss` for `Fscanln` (`nlIsSpace` false, `nlIsEnd` true) over its `readRune`.
struct Scanner<'a> {
    src: &'a mut dyn Read,
    /// `readRune.pendBuf`: the bytes after an ill-formed sequence, read again first.
    pending: VecDeque<u8>,
    /// `readRune.peekRune`: the last rune read, and whether it was unread.
    last: char,
    unread: bool,
    /// `ss.atEOF`: set by EOF, and (as `nlIsEnd`) by reading a newline.
    at_eof: bool,
}

impl<'a> Scanner<'a> {
    fn new(src: &'a mut dyn Read) -> Scanner<'a> {
        Scanner {
            src,
            pending: VecDeque::new(),
            last: '\0',
            unread: false,
            at_eof: false,
        }
    }

    /// `readRune.readByte`: a pending byte, else a one-byte read. `Ok(None)` at EOF.
    fn read_byte(&mut self) -> Result<Option<u8>, Abort> {
        if let Some(b) = self.pending.pop_front() {
            return Ok(Some(b));
        }
        let mut b = [0u8; 1];
        loop {
            match self.src.read(&mut b) {
                Ok(0) => return Ok(None),
                Ok(_) => return Ok(Some(b[0])),
                // os.File.Read retries EINTR.
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => return Err(Abort),
            }
        }
    }

    /// `readRune.ReadRune`: the unread rune, else one ASCII byte, else bytes until `utf8.FullRune`. EOF inside
    /// a sequence ends it; the bytes after an ill-formed start are kept for the next reads.
    fn read_rune(&mut self) -> Result<Option<char>, Abort> {
        if self.unread {
            self.unread = false;
            return Ok(Some(self.last));
        }
        let Some(first) = self.read_byte()? else {
            return Ok(None);
        };
        let mut buf = vec![first];
        if first >= 0x80 {
            while !full_rune(&buf) {
                match self.read_byte()? {
                    Some(b) => buf.push(b),
                    None => break,
                }
            }
        }
        let (r, size) = decode_rune(&buf);
        self.pending
            .extend(buf.get(size..).unwrap_or_default().iter().copied());
        self.last = r;
        Ok(Some(r))
    }

    /// `ss.getRune` over `ss.ReadRune`: `None` at EOF, and after a newline has been read.
    fn get_rune(&mut self) -> Result<Option<char>, Abort> {
        if self.at_eof {
            return Ok(None);
        }
        match self.read_rune()? {
            Some(r) => {
                if r == '\n' {
                    self.at_eof = true;
                }
                Ok(Some(r))
            }
            None => {
                self.at_eof = true;
                Ok(None)
            }
        }
    }

    /// `ss.UnreadRune`.
    fn unread_rune(&mut self) {
        self.unread = true;
        self.at_eof = false;
    }

    /// `ss.peek(string(want))`.
    fn peek(&mut self, want: char) -> Result<bool, Abort> {
        let r = self.get_rune()?;
        if r.is_some() {
            self.unread_rune();
        }
        Ok(r == Some(want))
    }

    /// `ss.SkipSpace`: a newline (also as "\r\n") is the error "unexpected newline".
    fn skip_space(&mut self) -> Result<(), Abort> {
        loop {
            let Some(r) = self.get_rune()? else {
                return Ok(());
            };
            if r == '\r' && self.peek('\n')? {
                continue;
            }
            if r == '\n' {
                return Err(Abort);
            }
            if !is_space(r) {
                self.unread_rune();
                return Ok(());
            }
        }
    }

    /// `ss.notEOF`: EOF is an error (`io.EOF`).
    fn not_eof(&mut self) -> Result<(), Abort> {
        match self.get_rune()? {
            Some(_) => {
                self.unread_rune();
                Ok(())
            }
            None => Err(Abort),
        }
    }

    /// `ss.token(true, notSpace)`: the runes up to a space or EOF.
    fn token(&mut self) -> Result<String, Abort> {
        self.skip_space()?;
        let mut word = String::new();
        while let Some(r) = self.get_rune()? {
            if is_space(r) {
                self.unread_rune();
                break;
            }
            word.push(r);
        }
        Ok(word)
    }

    /// `convertString('v')`: `SkipSpace`, `notEOF`, then the token.
    fn convert_string(&mut self) -> Result<String, Abort> {
        self.skip_space()?;
        self.not_eof()?;
        self.token()
    }

    /// `doScan`'s `nlIsEnd` check: spaces up to a newline or EOF; anything else is "expected newline".
    fn check_newline(&mut self) -> Result<(), Abort> {
        loop {
            match self.get_rune()? {
                None | Some('\n') => return Ok(()),
                Some(r) if !is_space(r) => return Err(Abort),
                Some(_) => {}
            }
        }
    }
}

// ---- node join, catalog restore ----

/// `node join`'s checks before `openNode` (`main.go:489-496`): `ticket.Parse(--seed)`, its error returned
/// verbatim, then a token of 32 bytes in strict hex.
pub(crate) fn join_seed_and_token(c: &Context) -> Result<(), CliError> {
    dstore_ticket::parse(c.os_string("seed").as_bytes()).map_err(CliError::msg)?;
    match hex::decode_string(c.os_string("token").as_bytes()) {
        Ok(tok) if tok.len() == 32 => Ok(()),
        _ => Err(msg("token must be 32 bytes of hex")),
    }
}

/// What `catalog restore` restores from (`main.go:655-663`).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RestoreSource {
    /// `os.ReadFile(arg)` succeeded.
    File(Vec<u8>),
    /// Otherwise the argument must be a key: `hex.DecodeString` of 32 bytes.
    Key([u8; 32]),
}

/// The `catalog restore` argument (`main.go:655-663`): a readable file, else a key, else `restore KEY|FILE`
/// (also for "", and for a directory that is not a key).
pub(crate) fn restore_source(arg: &[u8]) -> Result<RestoreSource, CliError> {
    if let Ok(data) = dstore_gocompat::os::read_file(arg) {
        return Ok(RestoreSource::File(data));
    }
    key32(arg)
        .map(RestoreSource::Key)
        .ok_or_else(|| msg("restore KEY|FILE"))
}

// ---- stdout texts ----

/// `token create`: `fmt.Println(r.Text)` (`main.go:456`), a newline even for an empty text.
pub(crate) fn token_line(r: &AdminReply) -> String {
    format!("{}\n", r.text)
}

/// `cluster ticket` over the cluster: `ticket.Parse(r.Ticket)` (`main.go:356`), its error returned verbatim.
pub(crate) fn reply_ticket(r: &AdminReply) -> Result<Ticket, CliError> {
    dstore_ticket::parse(r.ticket.as_bytes()).map_err(CliError::msg)
}

/// `fmt.Println(t.IDs())` with `--ids`, else `fmt.Println(t.Encode())` (`main.go:360-364`).
pub(crate) fn ticket_line(t: &Ticket, ids: bool) -> String {
    if ids {
        format!("{}\n", t.ids())
    } else {
        format!("{}\n", t.encode())
    }
}
