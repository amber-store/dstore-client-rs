//! Commands, the app, errors and `run` (urfave `app.go`, `command.go`, `help.go`, `errors.go`).
//!
//! urfave mutates its command objects while it runs: `setup` appends the shared `helpCommand` and the
//! help flag and names the subcommands, `ShowCommandHelp` appends `helpCommandDontUse`, and the help
//! command keeps the `HelpName` of the first parent that set it up. A run models that state in an
//! arena of command nodes, one per `run`, as one process of the Go CLI does.

use std::collections::HashSet;
use std::ffi::OsString;
use std::future::Future;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::pin::Pin;

use dstore_gocompat::path;

use crate::context::LevelState;
use crate::flag::{ApplyError, HELP_FLAG, VERSION_FLAG};
use crate::goflag::FlagSet;
use crate::help::{self, CommandCategory, CommandRow, HelpData, Template};
use crate::{Context, FlagDef};

/// A command action.
pub type Action =
    for<'a> fn(&'a Context) -> Pin<Box<dyn Future<Output = Result<(), CliError>> + Send + 'a>>;

/// A command definition.
pub struct CommandDef {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub usage: &'static str,
    pub args_usage: &'static str,
    pub description: &'static str,
    pub flags: Vec<FlagDef>,
    pub subcommands: Vec<CommandDef>,
    /// None → the help action.
    pub action: Option<Action>,
}

/// The app definition.
pub struct AppDef {
    pub name: &'static str,
    pub usage: &'static str,
    pub version: String,
    pub flags: Vec<FlagDef>,
    pub commands: Vec<CommandDef>,
}

/// A command failure.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// Printed "dstore: <msg>", exit 1.
    #[error("{0}")]
    Msg(String),
    /// Printed "<msg>", exit `code`.
    #[error("{msg}")]
    Exit { msg: String, code: i32 },
}

impl CliError {
    pub fn msg(e: impl std::fmt::Display) -> CliError {
        CliError::Msg(e.to_string())
    }
}

/// cli.md §2.2.2: setup, env application, Go flag parse, Incorrect Usage + help, help/version flags,
/// required flags, subcommand dispatch, action or help action.
///
/// `args` is the whole argument vector with the program name first, as Go's `App.Run(os.Args)`; the
/// program name is not interpreted. Help, version and `Incorrect Usage` output go to `stdout`. The
/// environment is read with `std::env::var_os`.
pub async fn run(
    app: &AppDef,
    args: Vec<OsString>,
    stdout: &mut (dyn std::io::Write + Send),
) -> Result<(), CliError> {
    let found = dispatch(app, args, stdout, &|name: &str| std::env::var_os(name))?;
    match found {
        Some((action, ctx)) => action(&ctx).await,
        None => Ok(()),
    }
}

/// The synchronous part of `run`: everything up to the action. Returns the action at the end of the
/// command path with the `Context` it receives, or `None` when the path ended in help or version
/// output. `getenv` is `syscall.Getenv` (`run` passes `std::env::var_os`), so tests can supply an
/// environment.
pub fn dispatch(
    app: &AppDef,
    args: Vec<OsString>,
    stdout: &mut dyn Write,
    getenv: &dyn Fn(&str) -> Option<OsString>,
) -> Result<Option<(Action, Context)>, CliError> {
    let mut rt = Runtime::setup(app);
    rt.check_duplicated_cmds(rt.root)?;
    let mut ctx = Context::default();
    let mut path: Vec<usize> = Vec::new();
    let mut node = rt.root;
    let mut arguments = args;
    loop {
        let is_root = path.is_empty();
        if !is_root {
            rt.setup_command(node);
            rt.check_duplicated_cmds(node)?;
        }
        // args.Tail(): the arguments after this level's name.
        let tail = arguments.get(1..).unwrap_or_default();
        match rt.parse_flags(node, tail, getenv) {
            Ok(level) => ctx.push(level),
            Err(ApplyError::Define(msg)) => return Err(CliError::Msg(msg)),
            Err(ApplyError::Usage(text)) => {
                let line = [&b"Incorrect Usage: "[..], &text, b"\n\n"].concat();
                let _ = stdout.write_all(&line);
                match path.last() {
                    None => rt.show_app_help(stdout),
                    Some(&parent) => {
                        let name = rt.nodes[node].name.clone();
                        let _ = rt.show_command_help(parent, name.as_bytes(), stdout);
                    }
                }
                return Err(CliError::Msg(String::from_utf8_lossy(&text).into_owned()));
            }
        }
        path.push(node);

        // checkHelp: an error of the help action is returned, not handled as an exit code.
        if ctx.bool("help") || ctx.bool("h") {
            return rt
                .help_action(&ctx, &path, stdout)
                .map(|()| None)
                .map_err(CliError::Msg);
        }
        if is_root && !rt.hide_version && (ctx.bool("version") || ctx.bool("v")) {
            let _ = writeln!(stdout, "{} version {}", rt.app_name, app.version);
            return Ok(None);
        }
        if let Some(err) = ctx.check_required_flags() {
            let _ = rt.help_action(&ctx, &path, stdout);
            return Err(CliError::Msg(err));
        }
        let first = ctx.args().first().map(|a| a.as_bytes().to_vec());
        if let Some(sub) = first.and_then(|name| rt.command(node, &name)) {
            arguments = ctx.args().to_vec();
            node = sub;
            continue;
        }
        return match rt.nodes[node].action {
            NodeAction::User(action) => Ok(Some((action, ctx))),
            // HandleExitCoder: "No help topic for …" exits 3.
            NodeAction::Help => rt
                .help_action(&ctx, &path, stdout)
                .map(|()| None)
                .map_err(|msg| CliError::Exit { msg, code: 3 }),
        };
    }
}

const HELP_NAME: &str = "help";
const HELP_ALIASES: &[&str] = &["h"];
const HELP_USAGE: &str = "Shows a list of commands or help for one command";

/// What `Run` calls when no subcommand matches.
#[derive(Clone, Copy)]
enum NodeAction {
    /// `helpCommand.Action` (a command without an action, and the help commands).
    Help,
    User(Action),
}

/// A `*cli.Command` (the root command stands for the app).
struct Node<'a> {
    name: String,
    aliases: &'a [&'a str],
    usage: &'a str,
    args_usage: &'a str,
    description: &'a str,
    help_name: String,
    subcommands: Vec<usize>,
    flags: Vec<&'a FlagDef>,
    /// The commands of the "" category, as of `setup`; `None` until then.
    categories: Option<Vec<usize>>,
    action: NodeAction,
}

/// The mutable command state of one run.
struct Runtime<'a> {
    nodes: Vec<Node<'a>>,
    root: usize,
    /// `helpCommand`.
    help: usize,
    /// `helpCommandDontUse`.
    help_dont_use: usize,
    app_name: String,
    hide_version: bool,
    version: &'a str,
}

impl<'a> Runtime<'a> {
    /// `App.Setup` and `newRootCommand`.
    fn setup(app: &'a AppDef) -> Runtime<'a> {
        let app_name = if app.name.is_empty() {
            // filepath.Base(os.Args[0])
            let argv0 = std::env::args_os().next().unwrap_or_default();
            String::from_utf8_lossy(&path::base(argv0.as_bytes())).into_owned()
        } else {
            app.name.to_owned()
        };
        let usage = if app.usage.is_empty() {
            "A new cli application"
        } else {
            app.usage
        };
        let mut rt = Runtime {
            nodes: Vec::new(),
            root: 0,
            help: 0,
            help_dont_use: 0,
            app_name: app_name.clone(),
            hide_version: app.version.is_empty(),
            version: app.version.as_str(),
        };
        rt.nodes.push(Node {
            name: app_name.clone(),
            aliases: &[],
            usage,
            args_usage: "",
            description: "",
            help_name: app_name.clone(),
            subcommands: Vec::new(),
            flags: app.flags.iter().collect(),
            categories: None,
            action: NodeAction::Help,
        });
        let mut commands: Vec<usize> = app.commands.iter().map(|c| rt.add_command(c)).collect();
        for &c in &commands {
            let cname = if rt.nodes[c].help_name.is_empty() {
                rt.nodes[c].name.clone()
            } else {
                rt.nodes[c].help_name.clone()
            };
            rt.nodes[c].help_name = format!("{app_name} {cname}");
        }
        rt.help = rt.push_help_node();
        rt.help_dont_use = rt.push_help_node();
        if !commands
            .iter()
            .any(|&c| rt.has_name(c, HELP_NAME.as_bytes()))
        {
            commands.push(rt.help);
            rt.append_flag(rt.root, &HELP_FLAG);
        }
        if !rt.hide_version {
            rt.append_flag(rt.root, &VERSION_FLAG);
        }
        let root = rt.root;
        rt.nodes[root].categories = Some(commands.clone());
        rt.nodes[root].subcommands = commands;
        rt
    }

    fn add_command(&mut self, def: &'a CommandDef) -> usize {
        let id = self.nodes.len();
        self.nodes.push(Node {
            name: def.name.to_owned(),
            aliases: def.aliases,
            usage: def.usage,
            args_usage: def.args_usage,
            description: def.description,
            help_name: String::new(),
            subcommands: Vec::new(),
            flags: def.flags.iter().collect(),
            categories: None,
            action: def.action.map_or(NodeAction::Help, NodeAction::User),
        });
        let subcommands = def
            .subcommands
            .iter()
            .map(|s| self.add_command(s))
            .collect();
        self.nodes[id].subcommands = subcommands;
        id
    }

    fn push_help_node(&mut self) -> usize {
        self.nodes.push(Node {
            name: HELP_NAME.to_owned(),
            aliases: HELP_ALIASES,
            usage: HELP_USAGE,
            args_usage: "[command]",
            description: "",
            help_name: String::new(),
            subcommands: Vec::new(),
            flags: Vec::new(),
            categories: None,
            action: NodeAction::Help,
        });
        self.nodes.len() - 1
    }

    /// `HasName`.
    fn has_name(&self, node: usize, name: &[u8]) -> bool {
        self.nodes.get(node).is_some_and(|n| {
            n.name.as_bytes() == name || n.aliases.iter().any(|a| a.as_bytes() == name)
        })
    }

    /// `Command(name)`: the first subcommand with that name or alias.
    fn command(&self, node: usize, name: &[u8]) -> Option<usize> {
        self.nodes
            .get(node)?
            .subcommands
            .iter()
            .copied()
            .find(|&c| self.has_name(c, name))
    }

    /// `appendFlag`: unless that very flag is already there.
    fn append_flag(&mut self, node: usize, flag: &'a FlagDef) {
        if let Some(n) = self.nodes.get_mut(node)
            && !n.flags.iter().any(|f| std::ptr::eq(*f, flag))
        {
            n.flags.push(flag);
        }
    }

    /// `Command.setup`.
    fn setup_command(&mut self, node: usize) {
        if self.command(node, HELP_NAME.as_bytes()).is_none() {
            let help = self.help;
            self.nodes[node].subcommands.push(help);
        }
        self.append_flag(node, &HELP_FLAG);
        let subcommands = self.nodes[node].subcommands.clone();
        self.nodes[node].categories = Some(subcommands.clone());
        let parent_help_name = self.nodes[node].help_name.clone();
        for s in subcommands {
            if self.nodes[s].help_name.is_empty() {
                self.nodes[s].help_name = format!("{} {}", parent_help_name, self.nodes[s].name);
            }
        }
    }

    /// `checkDuplicatedCmds`.
    fn check_duplicated_cmds(&self, node: usize) -> Result<(), CliError> {
        let n = &self.nodes[node];
        let mut seen: HashSet<&str> = HashSet::new();
        for &c in &n.subcommands {
            let sub = &self.nodes[c];
            for name in std::iter::once(sub.name.as_str()).chain(sub.aliases.iter().copied()) {
                if !seen.insert(name) {
                    return Err(CliError::Msg(format!(
                        "parent command [{}] has duplicated subcommand name or alias: {name}",
                        n.name
                    )));
                }
            }
        }
        Ok(())
    }

    /// `parseFlags`: `flagSet` (every flag's `Apply`), `Parse(args.Tail())`, `normalizeFlags`.
    fn parse_flags(
        &self,
        node: usize,
        tail: &[OsString],
        getenv: &dyn Fn(&str) -> Option<OsString>,
    ) -> Result<LevelState, ApplyError> {
        let n = &self.nodes[node];
        let mut set = FlagSet::new(&n.name);
        let mut flags = Vec::with_capacity(n.flags.len());
        for def in &n.flags {
            flags.push(def.apply(&mut set, getenv)?);
        }
        set.parse(tail).map_err(|e| ApplyError::Usage(e.0))?;
        normalize_flags(&flags, &mut set)?;
        Ok(LevelState { flags, set })
    }

    /// `helpCommand.Action` on the context at the end of `path`. The error is the text of the
    /// `Exit("No help topic for …", 3)` error.
    fn help_action(
        &mut self,
        ctx: &Context,
        path: &[usize],
        stdout: &mut dyn Write,
    ) -> Result<(), String> {
        let Some(&current) = path.last() else {
            return Ok(());
        };
        let first = ctx
            .args()
            .first()
            .map(|a| a.as_bytes().to_vec())
            .unwrap_or_default();
        let mut level = path.len() - 1;
        // Invoked as the help command: switch to the parent context. (A root named "help" would make
        // Go dereference the parent of the root context; it stays on the root here.)
        let name = self.nodes[current].name.as_str();
        if (name == HELP_NAME || name == "h") && level > 0 {
            level -= 1;
        }
        let node = path[level];
        if !first.is_empty() {
            return self.show_command_help(node, &first, stdout);
        }
        if level == 0 {
            self.show_app_help(stdout);
            return Ok(());
        }
        let template = if self.nodes[node].subcommands.len() == 1 {
            Template::Command
        } else {
            // ShowSubcommandHelp
            Template::Subcommand
        };
        self.print(template, node, stdout);
        Ok(())
    }

    /// `ShowCommandHelp(ctx, name)` where `ctx.Command` is `node`.
    fn show_command_help(
        &mut self,
        node: usize,
        name: &[u8],
        stdout: &mut dyn Write,
    ) -> Result<(), String> {
        let commands = if self.nodes[node].subcommands.is_empty() {
            self.nodes[self.root].subcommands.clone()
        } else {
            self.nodes[node].subcommands.clone()
        };
        for c in commands {
            if !self.has_name(c, name) {
                continue;
            }
            if !self.has_name(c, HELP_NAME.as_bytes())
                && !self.nodes[c].subcommands.is_empty()
                && self.command(c, HELP_NAME.as_bytes()).is_none()
            {
                let dont_use = self.help_dont_use;
                self.nodes[c].subcommands.push(dont_use);
            }
            self.append_flag(c, &HELP_FLAG);
            let template = if self.nodes[c].subcommands.is_empty() {
                Template::Command
            } else {
                Template::Subcommand
            };
            self.print(template, c, stdout);
            return Ok(());
        }
        Err(format!(
            "No help topic for '{}'",
            String::from_utf8_lossy(name)
        ))
    }

    /// `ShowAppHelp`.
    fn show_app_help(&self, stdout: &mut dyn Write) {
        self.print(Template::App, self.root, stdout);
    }

    fn print(&self, template: Template, node: usize, stdout: &mut dyn Write) {
        let exec = help::execute(template, &self.help_data(node, template));
        let _ = help::print_help(stdout, &exec);
    }

    fn row(&self, node: usize) -> CommandRow {
        let n = &self.nodes[node];
        CommandRow {
            names: std::iter::once(n.name.clone())
                .chain(n.aliases.iter().map(|a| (*a).to_owned()))
                .collect(),
            usage: n.usage.to_owned(),
        }
    }

    fn help_data(&self, node: usize, template: Template) -> HelpData {
        let n = &self.nodes[node];
        let app = template == Template::App;
        HelpData {
            help_name: n.help_name.clone(),
            usage: n.usage.to_owned(),
            usage_text: String::new(),
            args_usage: n.args_usage.to_owned(),
            args: false,
            category: String::new(),
            description: n.description.to_owned(),
            version: if app {
                self.version.to_owned()
            } else {
                String::new()
            },
            hide_version: !app || self.hide_version,
            copyright: String::new(),
            visible_commands: n.subcommands.iter().map(|&s| self.row(s)).collect(),
            categories: n.categories.as_ref().map(|commands| {
                if commands.is_empty() {
                    Vec::new()
                } else {
                    vec![CommandCategory {
                        name: String::new(),
                        commands: commands.iter().map(|&s| self.row(s)).collect(),
                    }]
                }
            }),
            visible_flags: n.flags.iter().map(|f| help::stringify_flag(f)).collect(),
        }
    }
}

/// `normalizeFlags`: two names of one flag on the command line are an error; otherwise every other
/// name of a flag that was set counts as set too.
fn normalize_flags(flags: &[crate::flag::FlagState], set: &mut FlagSet) -> Result<(), ApplyError> {
    let visited: HashSet<Vec<u8>> = set.visit().map(<[u8]>::to_vec).collect();
    for f in flags {
        if f.names.len() == 1 {
            continue;
        }
        let mut ff: Option<&str> = None;
        for name in &f.names {
            let name = name.trim_matches(' ');
            if visited.contains(name.as_bytes()) {
                if let Some(first) = ff {
                    return Err(ApplyError::Usage(
                        format!("Cannot use two forms of the same flag: {name} {first}")
                            .into_bytes(),
                    ));
                }
                ff = Some(name);
            }
        }
        if ff.is_none() {
            continue;
        }
        for name in &f.names {
            let name = name.trim_matches(' ');
            if !visited.contains(name.as_bytes()) {
                set.mark_set(name.as_bytes());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
