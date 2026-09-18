//! Help output: urfave's templates (`template.go`) executed over command data, the template functions
//! and row stringification (`help.go`, `flag.go:237-363`), and a port of Go `text/tabwriter`
//! (go1.26.5), cli.md §2.2.7.
//!
//! urfave prints help through `tabwriter.NewWriter(out, 1, 8, 2, ' ', 0)` and flushes only after the
//! template executed successfully. A template error loses whatever the writer still buffers; that is
//! how `help <parent>` comes out truncated (`Execution::complete`).

use std::io::{self, Write};

use dstore_gocompat::quote::is_space;
use dstore_gocompat::strings::{fields_func, trim_space};

use crate::{FlagDef, FlagKind};

/// `printHelpCustom`'s `maxLineLength`, the `wrapAt` of the default templates.
pub const WRAP_AT: usize = 10000;

/// urfave `wrap(input, offset, wrapAt)`.
pub fn wrap(input: &str, offset: usize, wrap_at: usize) -> String {
    let padding = " ".repeat(offset);
    let mut ss: Vec<String> = Vec::new();
    for (i, line) in input.split('\n').enumerate() {
        if line.is_empty() {
            ss.push(String::new());
        } else {
            let wrapped = wrap_line(line, offset, wrap_at, &padding);
            if i == 0 {
                ss.push(wrapped);
            } else {
                ss.push(format!("{padding}{wrapped}"));
            }
        }
    }
    ss.join("\n")
}

/// urfave `wrapLine`.
fn wrap_line(input: &str, offset: usize, wrap_at: usize, padding: &str) -> String {
    if wrap_at <= offset || input.len() <= wrap_at - offset {
        return input.to_owned();
    }
    let line_width = to_i64(wrap_at - offset);
    let words = fields_func(input.as_bytes(), is_space);
    let Some((first, rest)) = words.split_first() else {
        return input.to_owned();
    };
    let mut wrapped: Vec<u8> = first.to_vec();
    let mut space_left = line_width - to_i64(first.len());
    for word in rest {
        if to_i64(word.len()) + 1 > space_left {
            wrapped.push(b'\n');
            wrapped.extend_from_slice(padding.as_bytes());
            wrapped.extend_from_slice(word);
            space_left = line_width - to_i64(word.len());
        } else {
            wrapped.push(b' ');
            wrapped.extend_from_slice(word);
            space_left -= 1 + to_i64(word.len());
        }
    }
    String::from_utf8_lossy(&wrapped).into_owned()
}

fn to_i64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// urfave `unquoteUsage`: the placeholder named by the first backquoted word, and the usage without
/// the backquotes.
pub fn unquote_usage(usage: &str) -> (String, String) {
    let b = usage.as_bytes();
    if let Some(i) = b.iter().position(|&c| c == b'`')
        && let Some(len) = b[i + 1..].iter().position(|&c| c == b'`')
    {
        let j = i + 1 + len;
        let name = usage.get(i + 1..j).unwrap_or_default();
        let unquoted = format!(
            "{}{}{}",
            usage.get(..i).unwrap_or_default(),
            name,
            usage.get(j + 1..).unwrap_or_default()
        );
        return (name.to_owned(), unquoted);
    }
    (String::new(), usage.to_owned())
}

/// urfave `prefixedNames`: `-` for one-byte names, `--` otherwise, each followed by the placeholder.
pub fn prefixed_names(names: &[String], placeholder: &str) -> String {
    let mut prefixed = String::new();
    for (i, name) in names.iter().enumerate() {
        if name.is_empty() {
            continue;
        }
        prefixed.push_str(if name.len() == 1 { "-" } else { "--" });
        prefixed.push_str(name);
        if !placeholder.is_empty() {
            prefixed.push(' ');
            prefixed.push_str(placeholder);
        }
        if i + 1 < names.len() {
            prefixed.push_str(", ");
        }
    }
    prefixed
}

/// urfave `withEnvHint` on Unix: ` [$A, $B]`.
pub fn with_env_hint(env_vars: &[&str], s: &str) -> String {
    if env_vars.is_empty() {
        return s.to_owned();
    }
    format!("{s} [${}]", env_vars.join(", $"))
}

/// urfave `stringifyFlag`: `names<TAB>usage (default: …) [$ENV]`.
pub fn stringify_flag(f: &FlagDef) -> String {
    let (mut placeholder, usage) = unquote_usage(f.usage);
    if f.takes_value() && placeholder.is_empty() {
        placeholder = String::from("value");
    }
    let mut default_value_string = String::new();
    if !(matches!(f.kind, FlagKind::Bool { .. }) && f.disable_default_text) {
        let s = f.default_text();
        if !s.is_empty() {
            default_value_string = format!(" (default: {s})");
        }
    }
    let joined = format!("{usage}{default_value_string}");
    let usage_with_default = String::from_utf8_lossy(trim_space(joined.as_bytes())).into_owned();
    let mut pn = prefixed_names(&f.names(), &placeholder);
    if matches!(f.kind, FlagKind::StringSlice) {
        pn = format!("{pn} [ {pn} ]");
    }
    with_env_hint(f.env, &format!("{pn}\t{usage_with_default}"))
}

/// One row of a command list: `.Names` and `.Usage`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandRow {
    pub names: Vec<String>,
    pub usage: String,
}

/// A `CommandCategory`: its name and the commands added to it at setup.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandCategory {
    pub name: String,
    pub commands: Vec<CommandRow>,
}

/// What the templates read from a `*cli.Command`, or from the `*cli.App` for the app template.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HelpData {
    pub help_name: String,
    pub usage: String,
    pub usage_text: String,
    pub args_usage: String,
    pub args: bool,
    pub category: String,
    pub description: String,
    /// App template only.
    pub version: String,
    /// App template only.
    pub hide_version: bool,
    /// App template only.
    pub copyright: String,
    /// `.VisibleCommands`.
    pub visible_commands: Vec<CommandRow>,
    /// `.VisibleCategories`; `None` when the command's categories were never set up, where urfave's
    /// `visibleCommandCategoryTemplate` panics and the execution fails.
    pub categories: Option<Vec<CommandCategory>>,
    /// `String()` of each of `.VisibleFlags`.
    pub visible_flags: Vec<String>,
}

/// The three urfave templates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Template {
    /// `AppHelpTemplate`.
    App,
    /// `CommandHelpTemplate`.
    Command,
    /// `SubcommandHelpTemplate`.
    Subcommand,
}

/// What a template execution wrote to the tabwriter, and whether it completed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Execution {
    pub text: String,
    pub complete: bool,
}

/// Executes a template (`template.go`, verbatim structure) over `d`.
pub fn execute(template: Template, d: &HelpData) -> Execution {
    let mut t = String::from("NAME:\n   ");
    t.push_str(&help_name_template(d));
    t.push_str("\n\nUSAGE:\n   ");
    match template {
        Template::App => {
            if !d.usage_text.is_empty() {
                t.push_str(&wrap(&d.usage_text, 3, WRAP_AT));
            } else {
                t.push_str(&d.help_name);
                t.push(' ');
                if !d.visible_flags.is_empty() {
                    t.push_str("[global options]");
                }
                if !d.visible_commands.is_empty() {
                    t.push_str(" command [command options]");
                }
                push_args_usage(&mut t, d);
            }
            if !d.version.is_empty() && !d.hide_version {
                t.push_str("\n\nVERSION:\n   ");
                t.push_str(&d.version);
            }
            if !d.description.is_empty() {
                t.push_str("\n\nDESCRIPTION:\n   ");
                t.push_str(&wrap(&d.description, 3, WRAP_AT));
            }
            if !d.visible_commands.is_empty() {
                t.push_str("\n\nCOMMANDS:");
                match command_category_template(d) {
                    Some(s) => t.push_str(&s),
                    None => {
                        return Execution {
                            text: t,
                            complete: false,
                        };
                    }
                }
            }
            if !d.visible_flags.is_empty() {
                t.push_str("\n\nGLOBAL OPTIONS:");
                t.push_str(&flag_template(d));
            }
            if !d.copyright.is_empty() {
                t.push_str("\n\nCOPYRIGHT:\n   ");
                t.push_str(&wrap(&d.copyright, 3, WRAP_AT));
            }
        }
        Template::Command | Template::Subcommand => {
            t.push_str(&usage_template(d));
            if !d.category.is_empty() {
                t.push_str("\n\nCATEGORY:\n   ");
                t.push_str(&d.category);
            }
            if !d.description.is_empty() {
                t.push_str("\n\nDESCRIPTION:\n   ");
                t.push_str(&wrap(&d.description, 3, WRAP_AT));
            }
            if template == Template::Subcommand && !d.visible_commands.is_empty() {
                t.push_str("\n\nCOMMANDS:");
                match command_category_template(d) {
                    Some(s) => t.push_str(&s),
                    None => {
                        return Execution {
                            text: t,
                            complete: false,
                        };
                    }
                }
            }
            if !d.visible_flags.is_empty() {
                t.push_str("\n\nOPTIONS:");
                t.push_str(&flag_template(d));
            }
        }
    }
    t.push('\n');
    Execution {
        text: t,
        complete: true,
    }
}

/// `printHelpCustom`'s output step: the text through `tabwriter.NewWriter(out, 1, 8, 2, ' ', 0)`,
/// flushed only when the execution completed.
pub fn print_help(out: &mut dyn Write, exec: &Execution) -> io::Result<()> {
    let mut w = TabWriter::new(Vec::new(), 1, 8, 2, b' ', 0);
    w.write_all(exec.text.as_bytes())?;
    if exec.complete {
        w.flush()?;
    }
    out.write_all(w.get_ref())
}

/// `helpNameTemplate`.
fn help_name_template(d: &HelpData) -> String {
    let mut s = wrap(&d.help_name, 3, WRAP_AT);
    if !d.usage.is_empty() {
        s.push_str(" - ");
        s.push_str(&wrap(&d.usage, d.help_name.len() + 6, WRAP_AT));
    }
    s
}

/// `usageTemplate`.
fn usage_template(d: &HelpData) -> String {
    if !d.usage_text.is_empty() {
        return wrap(&d.usage_text, 3, WRAP_AT);
    }
    let mut s = d.help_name.clone();
    if !d.visible_flags.is_empty() {
        s.push_str(" [command options]");
    }
    push_args_usage(&mut s, d);
    s
}

fn push_args_usage(s: &mut String, d: &HelpData) {
    if !d.args_usage.is_empty() {
        s.push(' ');
        s.push_str(&d.args_usage);
    } else if d.args {
        s.push_str(" [arguments...]");
    }
}

/// `visibleCommandCategoryTemplate`; `None` where the template panics.
fn command_category_template(d: &HelpData) -> Option<String> {
    let categories = d.categories.as_ref()?;
    let mut s = String::new();
    for category in categories.iter().filter(|c| !c.commands.is_empty()) {
        if !category.name.is_empty() {
            s.push_str("\n   ");
            s.push_str(&category.name);
            s.push(':');
            for command in &category.commands {
                s.push_str("\n     ");
                s.push_str(&command.names.join(", "));
                s.push('\t');
                s.push_str(&command.usage);
            }
        } else {
            s.push_str(&command_template(&category.commands));
        }
    }
    Some(s)
}

/// `visibleCommandTemplate` over a category's commands.
fn command_template(commands: &[CommandRow]) -> String {
    // offsetCommands .VisibleCommands 5
    let cv = commands
        .iter()
        .map(|c| c.names.join(", ").len())
        .max()
        .unwrap_or(0)
        + 5;
    let mut s = String::new();
    for command in commands {
        let names = command.names.join(", ");
        s.push_str("\n   ");
        s.push_str(&names);
        // indent (subtract $cv (offset $s 3)) ""
        s.push_str(&" ".repeat(cv.saturating_sub(names.len() + 3)));
        s.push_str(&wrap(&command.usage, cv, WRAP_AT));
    }
    s
}

/// `visibleFlagTemplate`.
fn flag_template(d: &HelpData) -> String {
    let mut s = String::new();
    for f in &d.visible_flags {
        s.push_str("\n   ");
        s.push_str(&wrap(f, 6, WRAP_AT));
    }
    s
}

/// `tabwriter.FilterHTML`.
pub const FILTER_HTML: u32 = 1 << 0;
/// `tabwriter.StripEscape`.
pub const STRIP_ESCAPE: u32 = 1 << 1;
/// `tabwriter.AlignRight`.
pub const ALIGN_RIGHT: u32 = 1 << 2;
/// `tabwriter.DiscardEmptyColumns`.
pub const DISCARD_EMPTY_COLUMNS: u32 = 1 << 3;
/// `tabwriter.TabIndent`.
pub const TAB_INDENT: u32 = 1 << 4;
/// `tabwriter.Debug`.
pub const DEBUG: u32 = 1 << 5;
/// `tabwriter.Escape`.
pub const ESCAPE: u8 = 0xff;

const TABS: &[u8] = b"\t\t\t\t\t\t\t\t";

/// A cell: its size in bytes, width in runes, and whether a horizontal tab ends it.
#[derive(Clone, Copy, Debug, Default)]
struct Cell {
    size: usize,
    width: usize,
    htab: bool,
}

/// A port of go1.26.5 `text/tabwriter.Writer` (elastic tabstops). `Write::flush` is Go's `Flush`: it
/// formats and writes out the buffered text (it does not flush the underlying writer).
#[derive(Debug)]
pub struct TabWriter<W: Write> {
    output: W,
    minwidth: usize,
    tabwidth: usize,
    padding: usize,
    padbytes: [u8; 8],
    flags: u32,
    buf: Vec<u8>,
    pos: usize,
    cell: Cell,
    end_char: u8,
    lines: Vec<Vec<Cell>>,
    widths: Vec<usize>,
}

impl<W: Write> TabWriter<W> {
    /// `tabwriter.NewWriter(output, minwidth, tabwidth, padding, padchar, flags)`.
    pub fn new(
        output: W,
        minwidth: usize,
        tabwidth: usize,
        padding: usize,
        padchar: u8,
        flags: u32,
    ) -> TabWriter<W> {
        let mut flags = flags;
        if padchar == b'\t' {
            // tab padding enforces left-alignment
            flags &= !ALIGN_RIGHT;
        }
        let mut w = TabWriter {
            output,
            minwidth,
            tabwidth,
            padding,
            padbytes: [padchar; 8],
            flags,
            buf: Vec::new(),
            pos: 0,
            cell: Cell::default(),
            end_char: 0,
            lines: Vec::new(),
            widths: Vec::new(),
        };
        w.reset();
        w
    }

    /// The underlying writer.
    pub fn get_ref(&self) -> &W {
        &self.output
    }

    /// The underlying writer; buffered text that was not flushed is dropped.
    pub fn into_inner(self) -> W {
        self.output
    }

    fn reset(&mut self) {
        self.buf.clear();
        self.pos = 0;
        self.cell = Cell::default();
        self.end_char = 0;
        self.lines.clear();
        self.widths.clear();
        self.lines.push(Vec::new());
    }

    fn write_n(&mut self, src: &[u8], mut n: usize) -> io::Result<()> {
        while n > src.len() {
            self.output.write_all(src)?;
            n -= src.len();
        }
        self.output.write_all(&src[..n])
    }

    fn write_padding(&mut self, textw: usize, cellw: usize, use_tabs: bool) -> io::Result<()> {
        if self.padbytes[0] == b'\t' || use_tabs {
            // padding is done with tabs
            if self.tabwidth == 0 {
                return Ok(()); // tabs have no width - can't do any padding
            }
            // make cellw the smallest multiple of b.tabwidth
            let cellw = cellw.div_ceil(self.tabwidth) * self.tabwidth;
            let n = cellw.saturating_sub(textw); // amount of padding
            return self.write_n(TABS, n.div_ceil(self.tabwidth));
        }
        // padding is done with non-tab characters
        let pad = self.padbytes;
        self.write_n(&pad, cellw.saturating_sub(textw))
    }

    fn write_lines(&mut self, pos0: usize, line0: usize, line1: usize) -> io::Result<usize> {
        let mut pos = pos0;
        for i in line0..line1 {
            // if TabIndent is set, use tabs to pad leading empty cells
            let mut use_tabs = self.flags & TAB_INDENT != 0;
            let ncells = self.lines.get(i).map_or(0, Vec::len);
            for j in 0..ncells {
                let Some(c) = self.lines.get(i).and_then(|l| l.get(j)).copied() else {
                    break;
                };
                if j > 0 && self.flags & DEBUG != 0 {
                    // indicate column break
                    self.output.write_all(b"|")?;
                }
                let width = self.widths.get(j).copied();
                if c.size == 0 {
                    // empty cell
                    if let Some(w) = width {
                        self.write_padding(c.width, w, use_tabs)?;
                    }
                } else {
                    // non-empty cell
                    use_tabs = false;
                    let end = (pos + c.size).min(self.buf.len());
                    if self.flags & ALIGN_RIGHT == 0 {
                        // align left
                        self.output.write_all(&self.buf[pos.min(end)..end])?;
                        pos += c.size;
                        if let Some(w) = width {
                            self.write_padding(c.width, w, false)?;
                        }
                    } else {
                        // align right
                        if let Some(w) = width {
                            self.write_padding(c.width, w, false)?;
                        }
                        self.output.write_all(&self.buf[pos.min(end)..end])?;
                        pos += c.size;
                    }
                }
            }
            if i + 1 == self.lines.len() {
                // last buffered line - we don't have a newline, so just write
                // any outstanding buffered data
                let end = (pos + self.cell.size).min(self.buf.len());
                self.output.write_all(&self.buf[pos.min(end)..end])?;
                pos += self.cell.size;
            } else {
                // not the last line - write newline
                self.output.write_all(b"\n")?;
            }
        }
        Ok(pos)
    }

    /// Formats the text between `line0` and `line1` (excluding `line1`); `pos0` is the buffer position of
    /// the start of `line0`. Returns the buffer position of the start of `line1`.
    fn format(&mut self, pos0: usize, line0: usize, line1: usize) -> io::Result<usize> {
        let mut pos = pos0;
        let mut line0 = line0;
        let column = self.widths.len();
        let mut this = line0;
        while this < line1 {
            if column + 1 >= self.lines.get(this).map_or(0, Vec::len) {
                this += 1;
                continue;
            }
            // cell exists in this column => this line
            // has more cells than the previous line
            // (the last cell per line is ignored because cells are
            // tab-terminated; the last cell per line describes the
            // text before the newline/formfeed and does not belong
            // to a column)

            // print unprinted lines until beginning of block
            pos = self.write_lines(pos, line0, this)?;
            line0 = this;

            // column block begin
            let mut width = self.minwidth; // minimal column width
            let mut discardable = true; // true if all cells in this column are empty and "soft"
            while this < line1 {
                let Some(line) = self.lines.get(this) else {
                    break;
                };
                if column + 1 >= line.len() {
                    break;
                }
                // cell exists in this column
                let c = line[column];
                // update width
                let w = c.width + self.padding;
                if w > width {
                    width = w;
                }
                // update discardable
                if c.width > 0 || c.htab {
                    discardable = false;
                }
                this += 1;
            }
            // column block end

            // discard empty columns if necessary
            if discardable && self.flags & DISCARD_EMPTY_COLUMNS != 0 {
                width = 0;
            }

            // format and print all columns to the right of this column
            // (we know the widths of this column and all columns to the left)
            self.widths.push(width);
            pos = self.format(pos, line0, this)?;
            self.widths.pop();
            line0 = this;
        }

        // print unprinted lines until end
        self.write_lines(pos, line0, line1)
    }

    fn append(&mut self, text: &[u8]) {
        self.buf.extend_from_slice(text);
        self.cell.size += text.len();
    }

    fn update_width(&mut self) {
        self.cell.width += rune_count(self.buf.get(self.pos..).unwrap_or_default());
        self.pos = self.buf.len();
    }

    fn end_escape(&mut self) {
        match self.end_char {
            ESCAPE => {
                self.update_width();
                if self.flags & STRIP_ESCAPE == 0 {
                    // don't count the Escape chars
                    self.cell.width = self.cell.width.saturating_sub(2);
                }
            }
            b'>' => {}                    // tag of zero width
            b';' => self.cell.width += 1, // entity, count as one rune
            _ => {}
        }
        self.pos = self.buf.len();
        self.end_char = 0;
    }

    fn terminate_cell(&mut self, htab: bool) -> usize {
        self.cell.htab = htab;
        let cell = self.cell;
        self.cell = Cell::default();
        match self.lines.last_mut() {
            Some(line) => {
                line.push(cell);
                line.len()
            }
            None => {
                self.lines.push(vec![cell]);
                1
            }
        }
    }

    fn flush_no_defers(&mut self) -> io::Result<()> {
        // add current cell if not empty
        if self.cell.size > 0 {
            if self.end_char != 0 {
                // inside escape - terminate it even if incomplete
                self.end_escape();
            }
            self.terminate_cell(false);
        }
        // format contents of buffer
        let nlines = self.lines.len();
        self.format(0, 0, nlines)?;
        self.reset();
        Ok(())
    }
}

impl<W: Write> Write for TabWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // split text into cells
        let mut n = 0;
        for (i, &ch) in buf.iter().enumerate() {
            if self.end_char == 0 {
                // outside escape
                match ch {
                    b'\t' | b'\x0b' | b'\n' | b'\x0c' => {
                        // end of cell
                        self.append(&buf[n..i]);
                        self.update_width();
                        n = i + 1; // ch consumed
                        let ncells = self.terminate_cell(ch == b'\t');
                        if ch == b'\n' || ch == b'\x0c' {
                            // terminate line
                            self.lines.push(Vec::new());
                            if ch == b'\x0c' || ncells == 1 {
                                // A '\f' always forces a flush. Otherwise, if the previous
                                // line has only one cell which does not have an impact on
                                // the formatting of the following lines (the last cell per
                                // line is ignored by format()), thus we can flush the
                                // Writer contents.
                                self.flush_no_defers()?;
                                if ch == b'\x0c' && self.flags & DEBUG != 0 {
                                    // indicate section break
                                    self.output.write_all(b"---\n")?;
                                }
                            }
                        }
                    }
                    ESCAPE => {
                        // start of escaped sequence
                        self.append(&buf[n..i]);
                        self.update_width();
                        n = i;
                        if self.flags & STRIP_ESCAPE != 0 {
                            n += 1; // strip Escape
                        }
                        self.end_char = ESCAPE;
                    }
                    b'<' | b'&' if self.flags & FILTER_HTML != 0 => {
                        // begin of tag/entity
                        self.append(&buf[n..i]);
                        self.update_width();
                        n = i;
                        self.end_char = if ch == b'<' { b'>' } else { b';' };
                    }
                    _ => {}
                }
            } else if ch == self.end_char {
                // end of tag/entity
                let mut j = i + 1;
                if ch == ESCAPE && self.flags & STRIP_ESCAPE != 0 {
                    j = i; // strip Escape
                }
                self.append(&buf[n..j]);
                n = i + 1; // ch consumed
                self.end_escape();
            }
        }
        // append leftover text
        self.append(&buf[n..]);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let result = self.flush_no_defers();
        if result.is_err() {
            // If Flush ran into a panic, we still need to reset.
            self.reset();
        }
        result
    }
}

/// `utf8.RuneCount`: each byte that is not part of a valid encoding counts as one rune.
fn rune_count(b: &[u8]) -> usize {
    b.utf8_chunks()
        .map(|c| c.valid().chars().count() + c.invalid().len())
        .sum()
}

#[cfg(test)]
mod tests;
