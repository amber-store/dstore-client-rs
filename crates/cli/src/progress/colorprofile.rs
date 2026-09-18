//! The colour profile of the TUI and the downsampling of its frames: charmbracelet/colorprofile v0.4.3
//! (`Detect`, `Env`, `Profile.Convert`, `Writer`), x/ansi v0.11.8 (`Convert256`, `Convert16`,
//! `ReadStyleColor`, `Strip`), go-colorful v1.4.1 (`DistanceHSLuv`) and xo/terminfo (`Load`, `Decode`).
//!
//! Bubble Tea v2.0.9 picks the profile once in `Program.Run` with `colorprofile.Detect(os.Stderr,
//! os.Environ())` and its renderer converts every cell's style with `Profile.Convert`: truecolor as is,
//! 256 colours, 16 colours, no colours (`NO_COLOR`: bold and faint stay), or no styling at all (`NoTTY`:
//! `TERM` unset or `dumb`, or stderr not a terminal). The Rust renderer rewrites each frame the way
//! colorprofile's `Writer` does, with the same conversions, so the colours and attributes on the terminal
//! are Go's; the bytes are not (PORTING.md DD-6).

use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::{Command, Stdio};

use dstore_gocompat::path::join;
use dstore_gocompat::strconv::parse_bool;

use super::{Colorful, Segment, clamp01, fma, segments, xyz};

/// `colorprofile.Profile` (`Unknown` is never detected).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Profile {
    /// No terminal: no escape sequences at all.
    NoTty = 1,
    /// No colours; attributes such as bold and faint stay.
    Ascii = 2,
    /// 16 colours.
    Ansi = 3,
    /// 256 colours.
    Ansi256 = 4,
    /// 24-bit colours.
    TrueColor = 5,
}

impl Profile {
    /// `Profile.String()`.
    pub fn name(self) -> &'static str {
        match self {
            Profile::NoTty => "NoTTY",
            Profile::Ascii => "Ascii",
            Profile::Ansi => "ANSI",
            Profile::Ansi256 => "ANSI256",
            Profile::TrueColor => "TrueColor",
        }
    }
}

impl std::fmt::Display for Profile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// colorprofile's `environ`: the variables of `KEY=VALUE` entries, split at the first `=` (an entry
/// without one has the empty value); a later entry wins.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Environ(HashMap<String, String>);

impl Environ {
    /// `newEnviron(entries)`.
    pub fn new<S: AsRef<str>>(entries: &[S]) -> Environ {
        let mut m = HashMap::with_capacity(entries.len());
        for e in entries {
            let e = e.as_ref();
            let (k, v) = e.split_once('=').unwrap_or((e, ""));
            m.insert(k.to_owned(), v.to_owned());
        }
        Environ(m)
    }

    /// `os.Environ()`, as Bubble Tea reads it; names and values that are not UTF-8 are taken lossily.
    pub fn from_process() -> Environ {
        Environ(
            std::env::vars_os()
                .map(|(k, v)| {
                    (
                        k.to_string_lossy().into_owned(),
                        v.to_string_lossy().into_owned(),
                    )
                })
                .collect(),
        )
    }

    fn lookup(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    fn get(&self, key: &str) -> &str {
        self.lookup(key).unwrap_or("")
    }

    /// `strconv.ParseBool(env.get(key))`, false when it does not parse.
    fn flag(&self, key: &str) -> bool {
        parse_bool(self.get(key)).unwrap_or(false)
    }
}

const DUMB_TERM: &str = "dumb";

/// `colorprofile.Detect(output, env)` for an output that is a terminal or not: the environment's profile,
/// raised by the terminfo entry of `$TERM` (`Tc`/`RGB`) and by tmux (`tmux info`) when the output is a
/// terminal that is not dumb.
pub fn detect(output_is_terminal: bool, env: &Environ) -> Profile {
    detect_with(output_is_terminal, env, terminfo_profile, tmux_profile)
}

fn detect_with(
    output_is_terminal: bool,
    env: &Environ,
    terminfo: impl Fn(&str) -> Profile,
    tmux: impl Fn(&Environ) -> Profile,
) -> Profile {
    let isatty = env.flag("TTY_FORCE") || output_is_terminal;
    let term = env.lookup("TERM");
    let is_dumb = term.is_none_or(|t| t == DUMB_TERM);
    let envp = color_profile(isatty, env);
    if envp == Profile::TrueColor || env.flag("NO_COLOR") {
        return envp;
    }
    if isatty && !is_dumb {
        let tip = terminfo(term.unwrap_or(""));
        let tmuxp = tmux(env);
        return envp.max(tip.max(tmuxp));
    }
    envp
}

/// `colorprofile.Env(env)`: the profile of the environment alone, for a terminal output.
pub fn env_profile(env: &Environ) -> Profile {
    color_profile(true, env)
}

/// colorprofile's `colorProfile(isatty, env)`: `NO_COLOR`, `CLICOLOR_FORCE` and `CLICOLOR` over the
/// profile `TERM`, `COLORTERM` and friends give (not on Windows).
pub fn color_profile(isatty: bool, env: &Environ) -> Profile {
    let is_dumb = env.lookup("TERM").is_none_or(|t| t == DUMB_TERM);
    let envp = env_color_profile(env);
    let mut p = if !isatty || is_dumb {
        Profile::NoTty
    } else {
        envp
    };
    if env.flag("NO_COLOR") && isatty {
        return p.min(Profile::Ascii);
    }
    if env.flag("CLICOLOR_FORCE") {
        return p.max(Profile::Ansi).max(envp);
    }
    if env.flag("CLICOLOR") && isatty && !is_dumb {
        p = p.max(Profile::Ansi);
    }
    p
}

/// colorprofile's `envColorProfile(env)`.
fn env_color_profile(env: &Environ) -> Profile {
    let term = env.lookup("TERM");
    let mut p = match term {
        None | Some("") | Some(DUMB_TERM) => Profile::NoTty,
        Some(_) => Profile::Ansi,
    };
    let term = term.unwrap_or("");
    const TRUECOLOR_TERMS: [&str; 8] = [
        "alacritty",
        "contour",
        "foot",
        "ghostty",
        "kitty",
        "rio",
        "st",
        "wezterm",
    ];
    if TRUECOLOR_TERMS.iter().any(|t| term.contains(t)) {
        return Profile::TrueColor;
    }
    if term.starts_with("tmux") || term.starts_with("screen") {
        p = p.max(Profile::Ansi256);
    } else if term.starts_with("xterm") {
        p = p.max(Profile::Ansi);
    }
    if !env.get("WT_SESSION").is_empty() || env.flag("GOOGLE_CLOUD_SHELL") {
        return Profile::TrueColor;
    }
    let colorterm = env.get("COLORTERM").to_lowercase();
    if matches!(colorterm.as_str(), "truecolor" | "24bit" | "yes" | "true")
        && !term.starts_with("screen")
        && !term.starts_with("tmux")
    {
        return Profile::TrueColor;
    }
    if term.ends_with("256color") {
        p = p.max(Profile::Ansi256);
    }
    if term.ends_with("direct") {
        return Profile::TrueColor;
    }
    p
}

/// `colorprofile.Terminfo(term)`: `TrueColor` when the terminfo entry has the `Tc` or `RGB` extended
/// capability, else `ANSI` (also when there is no readable entry); `NoTTY` for an empty or dumb `term`.
pub fn terminfo_profile(term: &str) -> Profile {
    if term.is_empty() || term == DUMB_TERM {
        return Profile::NoTty;
    }
    match load_ext_bool_names(term) {
        Some(names) if names.iter().any(|n| n == b"Tc" || n == b"RGB") => Profile::TrueColor,
        _ => Profile::Ansi,
    }
}

/// colorprofile's `tmux(env)`: inside tmux (`$TMUX` set and not empty) at least 256 colours, and
/// `TrueColor` when a line of `tmux info` names `Tc` or `RGB` and says `true`.
fn tmux_profile(env: &Environ) -> Profile {
    if env.get("TMUX").is_empty() {
        return Profile::NoTty;
    }
    let out = Command::new("tmux")
        .arg("info")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() && tmux_info_has_truecolor(&o.stdout) => Profile::TrueColor,
        _ => Profile::Ansi256,
    }
}

fn tmux_info_has_truecolor(out: &[u8]) -> bool {
    out.split(|&b| b == b'\n')
        .any(|line| (contains(line, b"Tc") || contains(line, b"RGB")) && contains(line, b"true"))
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

// ---- xo/terminfo: Load, Open, Decode (only as far as the extended boolean names) ----

/// `terminfo.Load(name)`, then the names of the entry's extended boolean capabilities; None when the
/// entry cannot be loaded. The search stops at the first file found, valid or not, as Go's does.
///
/// Go takes the home directory from `user.Current()`; this uses `$HOME`.
fn load_ext_bool_names(name: &str) -> Option<Vec<Vec<u8>>> {
    if name.is_empty() {
        return None;
    }
    let mut dirs: Vec<Vec<u8>> = Vec::new();
    if let Some(dir) = std::env::var_os("TERMINFO").filter(|d| !d.is_empty()) {
        dirs.push(dir.as_bytes().to_vec());
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    dirs.push(join(&[home.as_bytes(), b".terminfo"]));
    if let Some(list) = std::env::var_os("TERMINFO_DIRS").filter(|d| !d.is_empty()) {
        dirs.extend(list.as_bytes().split(|&b| b == b':').map(<[u8]>::to_vec));
    }
    for dir in ["/etc/terminfo", "/lib/terminfo", "/usr/share/terminfo"] {
        dirs.push(dir.as_bytes().to_vec());
    }
    for dir in &dirs {
        match open_terminfo(dir, name.as_bytes()) {
            Opened::NotFound => continue,
            Opened::Invalid => return None,
            Opened::Found(names) => return Some(names),
        }
    }
    None
}

enum Opened {
    NotFound,
    Invalid,
    Found(Vec<Vec<u8>>),
}

/// `terminfo.Open(dir, name)`: `dir/<first byte>/name`, else `dir/<first byte in hex>/name`.
fn open_terminfo(dir: &[u8], name: &[u8]) -> Opened {
    let Some(&first) = name.first() else {
        return Opened::NotFound;
    };
    let hex = format!("{first:x}");
    let candidates = [
        join(&[dir, &name[..1], name]),
        join(&[dir, hex.as_bytes(), name]),
    ];
    for f in &candidates {
        if let Ok(buf) = std::fs::read(Path::new(OsStr::from_bytes(f))) {
            return match decode_ext_bool_names(&buf) {
                Some(names) => Opened::Found(names),
                None => Opened::Invalid,
            };
        }
    }
    Opened::NotFound
}

/// xo/terminfo's decoder position over a compiled entry.
struct Decoder<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Decoder<'a> {
    /// `readBytes(n)`.
    fn read_bytes(&mut self, n: i64) -> Option<&'a [u8]> {
        let n = usize::try_from(n).ok()?;
        let end = self.pos.checked_add(n)?;
        let b = self.buf.get(self.pos..end)?;
        self.pos = end;
        Some(b)
    }

    /// `readInts(n, w)`: n little-endian integers of w bits (8 and 16 signed, 32 as Go's `int` of the
    /// four bytes), then the position is aligned to an even offset.
    fn read_ints(&mut self, n: i64, w: usize) -> Option<Vec<i64>> {
        let size = w / 8;
        let len = n.checked_mul(i64::try_from(size).ok()?)?;
        let b = self.read_bytes(len)?;
        self.pos += self.pos % 2;
        Some(
            b.chunks_exact(size)
                .map(|c| match c {
                    [a] => i64::from(*a),
                    [a, b] => i64::from(i16::from_le_bytes([*a, *b])),
                    [a, b, c, d] => i64::from(u32::from_le_bytes([*a, *b, *c, *d])),
                    _ => 0,
                })
                .collect(),
        )
    }
}

/// The offset of the first NUL in `buf` at or after `start` (`findNull`).
fn find_nul(buf: &[u8], start: i64) -> Option<usize> {
    let start = usize::try_from(start).ok()?;
    buf.get(start..)?
        .iter()
        .position(|&b| b == 0)
        .map(|i| start + i)
}

/// xo/terminfo's `readStrings(idx, buf, n)`: the strings of the first n offsets (a negative offset is
/// absent) and the offset after the last one read.
fn read_strings<'a>(idx: &[i64], buf: &'a [u8], n: i64) -> Option<(Vec<&'a [u8]>, usize)> {
    let n = usize::try_from(n).ok()?;
    let mut out = vec![&[][..]; n];
    let mut last = 0;
    for (i, &start) in idx.get(..n)?.iter().enumerate() {
        if start < 0 {
            continue;
        }
        let end = find_nul(buf, start)?;
        out[i] = buf.get(usize::try_from(start).ok()?..end)?;
        last = end + 1;
    }
    Some((out, last))
}

/// xo/terminfo `Decode` with its checks, returning the extended boolean capability names (absent names
/// are empty), or None where `Decode` fails.
fn decode_ext_bool_names(buf: &[u8]) -> Option<Vec<Vec<u8>>> {
    const MAX_FILE_LENGTH: usize = 4096;
    const MAGIC: i64 = 0o432;
    const MAGIC_EXTENDED: i64 = 0o1036;
    const CAP_COUNT_BOOL: i64 = 44;
    const CAP_COUNT_NUM: i64 = 39;
    const CAP_COUNT_STRING: i64 = 414;
    if buf.len() >= MAX_FILE_LENGTH {
        return None;
    }
    let n = i64::try_from(buf.len()).ok()?;
    let mut d = Decoder { buf, pos: 0 };
    let h = d.read_ints(6, 16)?;
    let (name_size, bool_count, num_count, string_count, table_size) =
        (h[1], h[2], h[3], h[4], h[5]);
    let num_width = match h[0] {
        MAGIC => 16,
        MAGIC_EXTENDED => 32,
        _ => return None,
    };
    if bool_count > CAP_COUNT_BOOL || num_count > CAP_COUNT_NUM || string_count > CAP_COUNT_STRING {
        return None;
    }
    // capLength counts two bytes per number even in the 32-bit format.
    let cap_length = name_size
        + bool_count
        + (name_size + bool_count) % 2
        + num_count * 2
        + string_count * 2
        + table_size;
    if n - i64::try_from(d.pos).ok()? < cap_length {
        return None;
    }
    let names = d.read_bytes(name_size)?;
    if !names.contains(&0) {
        return None;
    }
    d.read_ints(bool_count, 8)?;
    d.read_ints(num_count, num_width)?;
    let offsets = d.read_ints(string_count, 16)?;
    let table = d.read_bytes(table_size)?;
    d.pos += d.pos % 2;
    for &start in &offsets {
        if start >= 0 {
            find_nul(table, start)?;
        }
    }
    if d.pos >= buf.len() {
        return Some(Vec::new());
    }
    let eh = d.read_ints(5, 16)?;
    let (ext_bools, ext_nums, ext_strings, ext_offsets, ext_table) =
        (eh[0], eh[1], eh[2], eh[3], eh[4]);
    if ext_bools + ext_nums + ext_strings * 2 != ext_offsets {
        return None;
    }
    let ext_length = ext_bools
        + ext_bools % 2
        + ext_nums * i64::try_from(num_width / 8).ok()?
        + ext_offsets * 2
        + ext_table;
    if n - i64::try_from(d.pos).ok()? != ext_length {
        return None;
    }
    d.read_ints(ext_bools, 8)?;
    d.read_ints(ext_nums, num_width)?;
    let idx = d.read_ints(ext_offsets, 16)?;
    let data = d.read_bytes(ext_table)?;
    if d.pos != buf.len() {
        return None;
    }
    let (_, last) = read_strings(&idx, data, ext_strings)?;
    let idx = idx.get(usize::try_from(ext_strings).ok()?..)?;
    let data = data.get(last..)?;
    let (bool_names, _) = read_strings(idx, data, ext_bools)?;
    let idx = idx.get(usize::try_from(ext_bools).ok()?..)?;
    read_strings(idx, data, ext_nums)?;
    let idx = idx.get(usize::try_from(ext_nums).ok()?..)?;
    read_strings(idx, data, ext_strings)?;
    Some(bool_names.into_iter().map(<[u8]>::to_vec).collect())
}

// ---- x/ansi colours and go-colorful HSLuv ----

/// A colour of an SGR sequence, as x/ansi reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SgrColor {
    /// `ansi.BasicColor` 0..15.
    Basic(u8),
    /// `ansi.IndexedColor` 0..255.
    Indexed(u8),
    /// Any other `color.Color`, as its `RGBA()` (16-bit, alpha-premultiplied) values.
    Rgba([u32; 4]),
}

/// `color.RGBA{r, g, b, a}.RGBA()`.
fn rgba8(r: u8, g: u8, b: u8, a: u8) -> SgrColor {
    let w = |v: u8| u32::from(v) * 0x101;
    SgrColor::Rgba([w(r), w(g), w(b), w(a)])
}

/// `color.CMYK{c, m, y, k}.RGBA()`.
fn cmyk(c: u8, m: u8, y: u8, k: u8) -> SgrColor {
    let w = 0xffff - u32::from(k) * 0x101;
    let ch = |v: u8| (0xffff - u32::from(v) * 0x101) * w / 0xffff;
    SgrColor::Rgba([ch(c), ch(m), ch(y), 0xffff])
}

/// x/ansi `ansi256To16`.
const ANSI256_TO_16: [u8; 256] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, //
    0, 4, 4, 4, 12, 12, 2, 6, 4, 4, 12, 12, 2, 2, 6, 4, //
    12, 12, 2, 2, 2, 6, 12, 12, 10, 10, 10, 10, 14, 12, 10, 10, //
    10, 10, 10, 14, 1, 5, 4, 4, 12, 12, 3, 8, 4, 4, 12, 12, //
    2, 2, 6, 4, 12, 12, 2, 2, 2, 6, 12, 12, 10, 10, 10, 10, //
    14, 12, 10, 10, 10, 10, 10, 14, 1, 1, 5, 4, 12, 12, 1, 1, //
    5, 4, 12, 12, 3, 3, 8, 4, 12, 12, 2, 2, 2, 6, 12, 12, //
    10, 10, 10, 10, 14, 12, 10, 10, 10, 10, 10, 14, 1, 1, 1, 5, //
    12, 12, 1, 1, 1, 5, 12, 12, 1, 1, 1, 5, 12, 12, 3, 3, //
    3, 7, 12, 12, 10, 10, 10, 10, 14, 12, 10, 10, 10, 10, 10, 14, //
    9, 9, 9, 9, 13, 12, 9, 9, 9, 9, 13, 12, 9, 9, 9, 9, //
    13, 12, 9, 9, 9, 9, 13, 12, 11, 11, 11, 11, 7, 12, 10, 10, //
    10, 10, 10, 14, 9, 9, 9, 9, 9, 13, 9, 9, 9, 9, 9, 13, //
    9, 9, 9, 9, 9, 13, 9, 9, 9, 9, 9, 13, 9, 9, 9, 9, //
    9, 13, 11, 11, 11, 11, 11, 15, 0, 0, 0, 0, 0, 0, 8, 8, //
    8, 8, 8, 8, 7, 7, 7, 7, 7, 7, 15, 15, 15, 15, 15, 15, //
];

/// x/ansi `Convert256` of an opaque 24-bit colour: the nearest xterm 256-colour index.
pub fn convert256(rgb: [u8; 3]) -> u8 {
    convert256_rgba(rgba_of(rgb))
}

/// x/ansi `Convert16` of an opaque 24-bit colour: `ansi256To16[Convert256(c)]`.
pub fn convert16(rgb: [u8; 3]) -> u8 {
    ANSI256_TO_16[usize::from(convert256(rgb))]
}

fn rgba_of(rgb: [u8; 3]) -> [u32; 4] {
    match rgba8(rgb[0], rgb[1], rgb[2], 0xff) {
        SgrColor::Rgba(v) => v,
        _ => [0, 0, 0, 0xffff],
    }
}

/// x/ansi `to6Cube(c*255)` of a colorful channel `c`: 0, 1, then `int((v - 35) / 40)`, at most 5 (Go
/// indexes out of range for channels above 255, which only a non-premultiplied RGBA input gives). The gc
/// compiler fuses `c*255 - 35` into one FMA on arm64, so `v - 35` is not the rounded product minus 35.
fn to6_cube(c: f64) -> usize {
    let v = c * 255.0;
    if v < 48.0 {
        0
    } else if v < 115.0 {
        1
    } else {
        ((fma(c, 255.0, -35.0) / 40.0) as usize).min(5)
    }
}

/// x/ansi `Convert256` over `RGBA()` values (after `colorful.MakeColor`).
fn convert256_rgba([r, g, b, a]: [u32; 4]) -> u8 {
    const Q2C: [i64; 6] = [0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff];
    if a == 0 {
        return 0;
    }
    // MakeColor undoes the premultiplication in uint32 arithmetic.
    let un = |v: u32| f64::from(v.wrapping_mul(0xffff) / a) / 65535.0;
    let col = Colorful {
        r: un(r),
        g: un(g),
        b: un(b),
    };
    let (r, g, b) = (col.r * 255.0, col.g * 255.0, col.b * 255.0);
    let (qr, qg, qb) = (to6_cube(col.r), to6_cube(col.g), to6_cube(col.b));
    let (cr, cg, cb) = (Q2C[qr], Q2C[qg], Q2C[qb]);
    let ci = 36 * qr + 6 * qg + qb;
    let cube = u8::try_from(16 + ci).unwrap_or(u8::MAX);
    if cr == r as i64 && cg == g as i64 && cb == b as i64 {
        return cube;
    }
    // `int(r+g+b)`: both additions fuse the products of g and b (FMADDD).
    let grey_avg = fma(col.b, 255.0, fma(col.g, 255.0, r)) as i64 / 3;
    let grey_idx = if grey_avg > 238 {
        23
    } else {
        (grey_avg - 3) / 10
    };
    let grey = (8 + 10 * grey_idx) as f64;
    let c2 = Colorful {
        r: cr as f64 / 255.0,
        g: cg as f64 / 255.0,
        b: cb as f64 / 255.0,
    };
    let g2 = Colorful {
        r: grey / 255.0,
        g: grey / 255.0,
        b: grey / 255.0,
    };
    if distance_hsluv(col, c2) <= distance_hsluv(col, g2) {
        cube
    } else {
        u8::try_from(232 + grey_idx).unwrap_or(u8::MAX)
    }
}

/// go-colorful `hSLuvD65`.
const HSLUV_D65: [f64; 3] = [0.95045592705167, 1.0, 1.089057750759878];

/// go-colorful's hsluv `m`.
#[allow(clippy::excessive_precision)] // go-colorful's literals, verbatim; they parse to the same f64
const HSLUV_M: [[f64; 3]; 3] = [
    [
        3.2409699419045214,
        -1.5373831775700935,
        -0.49861076029300328,
    ],
    [
        -0.96924363628087983,
        1.8759675015077207,
        0.041555057407175613,
    ],
    [
        0.055630079696993609,
        -0.20397695888897657,
        1.0569715142428786,
    ],
];

const KAPPA: f64 = 903.2962962962963;
#[allow(clippy::excessive_precision)] // go-colorful's literal, verbatim
const EPSILON: f64 = 0.0088564516790356308;

/// `xyz_to_uv`.
fn xyz_to_uv(x: f64, y: f64, z: f64) -> (f64, f64) {
    let denom = fma(3.0, z, fma(15.0, y, x));
    if denom == 0.0 {
        (0.0, 0.0)
    } else {
        (4.0 * x / denom, 9.0 * y / denom)
    }
}

/// `Color.LuvWhiteRef(wref)`; the constants `6.0/29.0*6.0/29.0*6.0/29.0` and
/// `29.0/3.0*29.0/3.0*29.0/3.0` are the exact 216/24389 and 24389/27.
fn luv_white_ref(c: Colorful, wref: [f64; 3]) -> [f64; 3] {
    let [x, y, z] = xyz(c);
    let l = if y / wref[1] <= 216.0 / 24389.0 {
        y / wref[1] * (24389.0 / 27.0) / 100.0
    } else {
        fma(1.16, (y / wref[1]).cbrt(), -0.16)
    };
    let (ubis, vbis) = xyz_to_uv(x, y, z);
    let (un, vn) = xyz_to_uv(wref[0], wref[1], wref[2]);
    [l, 13.0 * l * (ubis - un), 13.0 * l * (vbis - vn)]
}

/// `LuvToLuvLCh`.
#[allow(clippy::excessive_precision)] // go-colorful's literal, verbatim
fn luv_to_lch([l, u, v]: [f64; 3]) -> [f64; 3] {
    let h = if (v - u).abs() > 1e-4 && u.abs() > 1e-4 {
        fma(57.29577951308232087721, v.atan2(u), 360.0) % 360.0
    } else {
        0.0
    };
    [l, fma(v, v, u * u).sqrt(), h]
}

/// `math.Pow(x, 3)`: Go multiplies by repeated squaring, which rounds as `x * (x * x)`.
fn pow3(x: f64) -> f64 {
    x * (x * x)
}

/// `getBounds(l)`.
fn get_bounds(l: f64) -> [[f64; 2]; 6] {
    let sub1 = pow3(l + 16.0) / 1560896.0;
    let sub2 = if sub1 > EPSILON { sub1 } else { l / KAPPA };
    let mut ret = [[0.0; 2]; 6];
    for (i, m) in HSLUV_M.iter().enumerate() {
        for k in 0..2u8 {
            let kf = f64::from(k);
            let top1 = fma(-94839.0, m[2], 284517.0 * m[0]) * sub2;
            let sum = fma(731718.0, m[0], fma(769860.0, m[1], 838422.0 * m[2]));
            let top2 = fma(-(769860.0 * kf), l, sum * l * sub2);
            let bottom = fma(126452.0, kf, fma(-126452.0, m[1], 632260.0 * m[2]) * sub2);
            ret[i * 2 + usize::from(k)] = [top1 / bottom, top2 / bottom];
        }
    }
    ret
}

/// `maxChromaForLH(l, h)`.
fn max_chroma_for_lh(l: f64, h: f64) -> f64 {
    let h_rad = h / 360.0 * std::f64::consts::PI * 2.0;
    let (sin, cos) = (h_rad.sin(), h_rad.cos());
    let mut min_length = f64::MAX;
    for [x, y] in get_bounds(l) {
        let length = y / fma(-x, cos, sin);
        if length > 0.0 && length < min_length {
            min_length = length;
        }
    }
    min_length
}

/// `Color.HSLuv()`: hue, saturation and lightness.
#[allow(clippy::manual_range_contains)] // Go's comparison; a range test would differ for NaN
fn hsluv(c: Colorful) -> [f64; 3] {
    let [l, chroma, h] = luv_to_lch(luv_white_ref(c, HSLUV_D65));
    let (chroma, l) = (chroma * 100.0, l * 100.0);
    let s = if l > 99.9999999 || l < 0.00000001 {
        0.0
    } else {
        chroma / max_chroma_for_lh(l, h) * 100.0
    };
    [h, clamp01(s / 100.0), clamp01(l / 100.0)]
}

/// `Color.DistanceHSLuv`.
fn distance_hsluv(c1: Colorful, c2: Colorful) -> f64 {
    let [h1, s1, l1] = hsluv(c1);
    let [h2, s2, l2] = hsluv(c2);
    let (dh, ds, dl) = ((h1 - h2) / 100.0, s1 - s2, l1 - l2);
    fma(dl, dl, fma(ds, ds, dh * dh)).sqrt()
}

/// `Profile.Convert(c)` for `ANSI` and `ANSI256`.
fn convert(p: Profile, c: SgrColor) -> SgrColor {
    match (p, c) {
        (_, SgrColor::Basic(_)) => c,
        (Profile::Ansi, SgrColor::Indexed(i)) => SgrColor::Basic(ANSI256_TO_16[usize::from(i)]),
        (Profile::Ansi256, SgrColor::Rgba(v)) => SgrColor::Indexed(convert256_rgba(v)),
        (Profile::Ansi, SgrColor::Rgba(v)) => {
            SgrColor::Basic(ANSI256_TO_16[usize::from(convert256_rgba(v))])
        }
        _ => c,
    }
}

// ---- colorprofile Writer ----

/// `colorprofile.Writer{Profile: p}.Write(s)`: truecolor unchanged, `NoTTY` without any escape sequence
/// (`ansi.Strip`), otherwise each SGR sequence rewritten for the profile and everything else kept.
pub fn downsample(s: &str, p: Profile) -> Cow<'_, str> {
    if p == Profile::TrueColor || !s.contains('\x1b') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    for seg in segments(s) {
        match seg {
            Segment::Char(c) => out.push(c),
            Segment::Escape(_) if p == Profile::NoTty => {}
            Segment::Escape(e) => match sgr_params(e) {
                Some(params) => handle_sgr(p, &params, &mut out),
                None => out.push_str(e),
            },
        }
    }
    Cow::Owned(out)
}

/// One parameter of a CSI sequence: its value (None when missing) and whether a `:` sub-parameter
/// follows (x/ansi `Param.HasMore`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Param {
    value: Option<i64>,
    more: bool,
}

impl Param {
    /// `Param.Param(0)`.
    fn get(self) -> i64 {
        self.value.unwrap_or(0)
    }
}

/// The parameters of an SGR sequence (`ESC [ params m`, no private marker or intermediate byte), or
/// None for any other sequence.
fn sgr_params(e: &str) -> Option<Vec<Param>> {
    let body = e.strip_prefix("\x1b[")?.strip_suffix('m')?;
    if !body
        .bytes()
        .all(|b| b.is_ascii_digit() || b == b';' || b == b':')
    {
        return None;
    }
    let mut params = Vec::new();
    if body.is_empty() {
        return Some(params);
    }
    let mut value: Option<i64> = None;
    for b in body.bytes() {
        match b {
            b';' | b':' => {
                params.push(Param {
                    value: value.take(),
                    more: b == b':',
                });
            }
            d => {
                let digit = i64::from(d - b'0');
                value = Some(value.unwrap_or(0).saturating_mul(10).saturating_add(digit));
            }
        }
    }
    params.push(Param { value, more: false });
    Some(params)
}

/// x/ansi `ReadStyleColor(params, &c)`: the number of parameters read (0 when invalid) and the colour
/// (None for the implementation-defined type 0).
fn read_style_color(params: &[Param]) -> (usize, Option<SgrColor>) {
    let (Some(&s), Some(&p)) = (params.first(), params.get(1)) else {
        return (0, None);
    };
    let more = |i: usize| params.get(i).is_some_and(|x| x.more);
    let val = |i: usize| params.get(i).map_or(0, |x| x.get());
    let len = params.len();
    let mut n = 2;
    let mut values = || -> [i64; 4] {
        let (sm, pm) = (s.more, p.more);
        if sm && pm && len > 8 && (2..=7).all(more) {
            n += 7;
            [val(3), val(4), val(5), val(6)]
        } else if sm && pm && len > 7 && (2..=6).all(more) {
            n += 6;
            [val(3), val(4), val(5), val(6)]
        } else if sm && pm && len > 6 && (2..=5).all(more) {
            n += 5;
            [val(3), val(4), val(5), val(6)]
        } else if sm && pm && len > 5 && (2..=4).all(more) && !more(5) {
            n += 4;
            [val(3), val(4), val(5), -1]
        } else if (sm && pm && p.get() == 2 && more(2) && more(3) && !more(4))
            || (!sm && !pm && p.get() == 2 && !more(2) && !more(3) && !more(4))
        {
            n += 3;
            [val(2), val(3), val(4), -1]
        } else {
            [-1; 4]
        }
    };
    // uint8(v) keeps the low byte.
    let byte = |v: i64| v as u8;
    match p.get() {
        0 => (2, None),
        1 => (2, Some(SgrColor::Rgba([0, 0, 0, 0]))),
        2 | 3 => {
            if len < 5 {
                return (0, None);
            }
            let [a, b, c, _] = values();
            if a == -1 || b == -1 || c == -1 {
                return (0, None);
            }
            let color = if p.get() == 2 {
                rgba8(byte(a), byte(b), byte(c), 0xff)
            } else {
                cmyk(byte(a), byte(b), byte(c), 0)
            };
            (n, Some(color))
        }
        4 | 6 => {
            if len < 6 {
                return (0, None);
            }
            let [a, b, c, d] = values();
            if a == -1 || b == -1 || c == -1 || d == -1 {
                return (0, None);
            }
            let color = if p.get() == 4 {
                cmyk(byte(a), byte(b), byte(c), byte(d))
            } else {
                rgba8(byte(a), byte(b), byte(c), byte(d))
            };
            (n, Some(color))
        }
        5 => {
            if len < 3 {
                return (0, None);
            }
            if !((s.more && p.more && !more(2)) || (!s.more && !p.more && !more(2))) {
                return (0, None);
            }
            (3, Some(SgrColor::Indexed(byte(val(2)))))
        }
        _ => (0, None),
    }
}

/// x/ansi `shift`: a 16-bit channel as 8 bits.
fn shift(v: u32) -> u32 {
    if v > 0xff { v >> 8 } else { v }
}

/// The SGR attribute of a colour: `foregroundColorString` (base 30), `backgroundColorString` (40) and
/// `underlineColorString` (58, which has no basic form).
fn color_attr(c: SgrColor, base: u8) -> String {
    let ext = if base == 30 {
        38
    } else if base == 40 {
        48
    } else {
        58
    };
    match c {
        SgrColor::Basic(n) if base != 58 && n < 8 => format!("{}", base + n),
        SgrColor::Basic(n) if base != 58 => format!("{}", base + 60 + (n - 8)),
        SgrColor::Basic(n) | SgrColor::Indexed(n) => format!("{ext};5;{n}"),
        SgrColor::Rgba([r, g, b, _]) => format!("{ext};2;{};{};{}", shift(r), shift(g), shift(b)),
    }
}

/// colorprofile's `handleSgr`: the SGR sequence of `params` for `p` (`ASCII`, `ANSI` or `ANSI256`).
fn handle_sgr(p: Profile, params: &[Param], out: &mut String) {
    let colors = p >= Profile::Ansi;
    let mut style: Vec<String> = Vec::new();
    let mut i = 0;
    while i < params.len() {
        let param = params[i].get();
        match param {
            0 => style.push(String::new()),
            30..=37 | 40..=47 | 90..=97 | 100..=107 if colors => {
                let (base, n) = match param {
                    30..=37 => (30, param - 30),
                    40..=47 => (40, param - 40),
                    90..=97 => (30, param - 90 + 8),
                    _ => (40, param - 100 + 8),
                };
                let c = convert(p, SgrColor::Basic(u8::try_from(n).unwrap_or(0)));
                style.push(color_attr(c, base));
            }
            30..=37 | 40..=47 | 90..=97 | 100..=107 => {}
            38 | 48 | 58 => {
                let (n, c) = read_style_color(&params[i..]);
                if n > 0 {
                    i += n - 1;
                }
                // Go converts a missing colour (type 0, or an invalid sequence) too, and panics; the
                // attribute is dropped here.
                if let (true, Some(c)) = (colors, c) {
                    let base = match param {
                        38 => 30,
                        48 => 40,
                        _ => 58,
                    };
                    style.push(color_attr(convert(p, c), base));
                }
            }
            39 | 49 | 59 => {
                if colors {
                    style.push(param.to_string());
                }
            }
            _ => style.push(param.to_string()),
        }
        i += 1;
    }
    if style.is_empty() {
        out.push_str("\x1b[m");
    } else {
        out.push_str("\x1b[");
        out.push_str(&style.join(";"));
        out.push('m');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(entries: &[&str]) -> Environ {
        Environ::new(entries)
    }

    /// colorprofile v0.4.3 `env_test.go`, the expectations off Windows.
    #[test]
    fn env_profile_go_cases() {
        use Profile::*;
        let cases: &[(&[&str], Profile)] = &[
            (&[], NoTty),
            (&["TERM=dumb"], NoTty),
            (&["TERM=dumb", "COLORTERM=truecolor"], NoTty),
            (
                &["TERM=dumb", "COLORTERM=truecolor", "CLICOLOR_FORCE=1"],
                TrueColor,
            ),
            (&["TERM=dumb", "CLICOLOR_FORCE=1"], Ansi),
            (&["TERM=dumb", "CLICOLOR=1"], NoTty),
            (&["TERM=xterm-256color"], Ansi256),
            (&["TERM=xterm-256color", "CLICOLOR=1"], Ansi256),
            (&["TERM=xterm-256color", "COLORTERM=yes"], TrueColor),
            (&["TERM=xterm-256color", "NO_COLOR=1"], Ascii),
            (&["TERM=xterm"], Ansi),
            (&["TERM=xterm", "NO_COLOR=1"], Ascii),
            (&["TERM=xterm", "CLICOLOR=1"], Ansi),
            (&["TERM=xterm", "CLICOLOR_FORCE=1"], Ansi),
            (&["TERM=xterm-16color"], Ansi),
            (&["TERM=xterm-color"], Ansi),
            (
                &["TERM=xterm-256color", "NO_COLOR=1", "CLICOLOR_FORCE=1"],
                Ascii,
            ),
            (&["WT_SESSION=1"], NoTty),
            (&["TERM=xterm-256color", "WT_SESSION=1"], TrueColor),
            (&["TERM=screen"], Ansi256),
            (&["TERM=screen", "COLORTERM=truecolor"], Ansi256),
            (&["TERM=tmux", "COLORTERM=truecolor"], Ansi256),
            (&["TERM=tmux-256color"], Ansi256),
            (&["COLORTERM=truecolor"], NoTty),
            (&["TERM=xterm-direct"], TrueColor),
        ];
        for (entries, want) in cases {
            assert_eq!(env_profile(&env(entries)), *want, "{entries:?}");
        }
    }

    #[test]
    fn env_profile_known_truecolor_terminals() {
        for term in [
            "alacritty",
            "xterm-kitty",
            "xterm-ghostty",
            "foot",
            "wezterm",
            "contour",
            "rio",
            "st-256color",
        ] {
            let e = env(&[&format!("TERM={term}")]);
            assert_eq!(env_profile(&e), Profile::TrueColor, "{term}");
        }
        let e = env(&["TERM=xterm-256color", "COLORTERM=24BIT"]);
        assert_eq!(env_profile(&e), Profile::TrueColor);
        let e = env(&["TERM=xterm", "GOOGLE_CLOUD_SHELL=true"]);
        assert_eq!(env_profile(&e), Profile::TrueColor);
        // An empty TERM counts as set for `isDumb` but gives no colours.
        assert_eq!(env_profile(&env(&["TERM="])), Profile::NoTty);
        assert_eq!(
            env_profile(&env(&["TERM=", "CLICOLOR=1"])),
            Profile::Ansi,
            "CLICOLOR raises an empty TERM, which is not dumb"
        );
    }

    #[test]
    fn detect_consults_terminfo_and_tmux_only_for_a_terminal() {
        let ti = |_: &str| Profile::TrueColor;
        let no_ti = |_: &str| Profile::Ansi;
        let tmux = |_: &Environ| Profile::NoTty;
        let e = env(&["TERM=xterm-256color"]);
        assert_eq!(detect_with(true, &e, ti, tmux), Profile::TrueColor);
        assert_eq!(detect_with(true, &e, no_ti, tmux), Profile::Ansi256);
        // Not a terminal: NoTTY, whatever the terminfo says; TTY_FORCE makes it one.
        assert_eq!(detect_with(false, &e, ti, tmux), Profile::NoTty);
        let forced = env(&["TERM=xterm-256color", "TTY_FORCE=1"]);
        assert_eq!(detect_with(false, &forced, ti, tmux), Profile::TrueColor);
        // NO_COLOR and a dumb terminal return before terminfo.
        let nc = env(&["TERM=xterm-256color", "NO_COLOR=1"]);
        assert_eq!(detect_with(true, &nc, ti, tmux), Profile::Ascii);
        let dumb = env(&["TERM=dumb"]);
        assert_eq!(detect_with(true, &dumb, ti, tmux), Profile::NoTty);
        assert_eq!(detect_with(true, &env(&[]), ti, tmux), Profile::NoTty);
        // tmux raises the profile.
        let t = env(&["TERM=screen", "TMUX=/tmp/x,1,0"]);
        assert_eq!(
            detect_with(true, &t, no_ti, |_: &Environ| Profile::TrueColor),
            Profile::TrueColor
        );
        // CLICOLOR_FORCE colours a non-terminal.
        let f = env(&["TERM=xterm", "CLICOLOR_FORCE=1"]);
        assert_eq!(detect_with(false, &f, ti, tmux), Profile::Ansi);
    }

    #[test]
    fn tmux_needs_tmux_and_reads_tc_or_rgb() {
        assert_eq!(tmux_profile(&env(&[])), Profile::NoTty);
        assert_eq!(tmux_profile(&env(&["TMUX="])), Profile::NoTty);
        assert!(tmux_info_has_truecolor(
            b"x\n197: Tc: [missing]\n 42: RGB: (flag) true\n"
        ));
        assert!(!tmux_info_has_truecolor(b"197: Tc: [missing]\n"));
        assert!(!tmux_info_has_truecolor(b""));
    }

    /// A compiled terminfo entry: legacy (16-bit numbers) or extended-number format, with the given
    /// extended boolean names.
    fn terminfo_entry(extended_numbers: bool, ext_bools: &[&str]) -> Vec<u8> {
        let mut b = Vec::new();
        let push16 = |b: &mut Vec<u8>, v: i16| b.extend_from_slice(&v.to_le_bytes());
        let names = b"xterm-test|test entry\0";
        let magic: i16 = if extended_numbers { 0o1036 } else { 0o432 };
        for v in [magic, names.len() as i16, 1, 1, 1, 4] {
            push16(&mut b, v);
        }
        b.extend_from_slice(names);
        b.push(1); // one boolean
        if b.len() % 2 == 1 {
            b.push(0);
        }
        if extended_numbers {
            b.extend_from_slice(&80i32.to_le_bytes());
        } else {
            push16(&mut b, 80);
        }
        push16(&mut b, 0); // one string at offset 0
        b.extend_from_slice(b"\x1b[H\0");
        if ext_bools.is_empty() {
            return b;
        }
        let mut table = Vec::new();
        let mut offsets = Vec::new();
        for n in ext_bools {
            offsets.push(table.len() as i16);
            table.extend_from_slice(n.as_bytes());
            table.push(0);
        }
        for v in [
            ext_bools.len() as i16,
            0,
            0,
            ext_bools.len() as i16,
            table.len() as i16,
        ] {
            push16(&mut b, v);
        }
        b.extend(std::iter::repeat_n(1, ext_bools.len()));
        if ext_bools.len() % 2 == 1 {
            b.push(0);
        }
        for o in offsets {
            push16(&mut b, o);
        }
        b.extend_from_slice(&table);
        b
    }

    #[test]
    fn terminfo_decode_extended_booleans() {
        let names = |b: &[u8]| decode_ext_bool_names(b);
        assert_eq!(
            names(&terminfo_entry(false, &["AX", "Tc"])),
            Some(vec![b"AX".to_vec(), b"Tc".to_vec()])
        );
        assert_eq!(
            names(&terminfo_entry(true, &["RGB"])),
            Some(vec![b"RGB".to_vec()])
        );
        assert_eq!(names(&terminfo_entry(false, &[])), Some(vec![]));
        // Bad magic, a truncated file, a file of 4096 bytes and a bad extended header are invalid.
        let mut bad = terminfo_entry(false, &["Tc"]);
        bad[0] = 0;
        assert_eq!(names(&bad), None);
        let full = terminfo_entry(false, &["Tc"]);
        assert_eq!(names(&full[..full.len() - 1]), None);
        let mut big = terminfo_entry(false, &[]);
        big.resize(4096, 0);
        assert_eq!(names(&big), None);
        let mut ext = terminfo_entry(false, &["Tc"]);
        let at = ext.len() - 3 - 2 - 2 - 10; // the extended header's offset count
        ext[at + 6] = 9;
        assert_eq!(names(&ext), None);
    }

    #[test]
    fn terminfo_open_finds_letter_and_hex_directories() {
        let dir = std::env::temp_dir().join(format!("dstore-terminfo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let letter = dir.join("letter");
        let hex = dir.join("hex");
        std::fs::create_dir_all(letter.join("x")).unwrap();
        std::fs::create_dir_all(hex.join("78")).unwrap();
        std::fs::write(letter.join("x/xterm-test"), terminfo_entry(false, &["Tc"])).unwrap();
        std::fs::write(hex.join("78/xterm-test"), terminfo_entry(true, &["RGB"])).unwrap();
        std::fs::write(letter.join("x/xterm-bad"), b"junk").unwrap();
        let open = |d: &Path, name: &str| open_terminfo(d.as_os_str().as_bytes(), name.as_bytes());
        assert!(
            matches!(open(&letter, "xterm-test"), Opened::Found(n) if n == vec![b"Tc".to_vec()])
        );
        assert!(matches!(open(&hex, "xterm-test"), Opened::Found(n) if n == vec![b"RGB".to_vec()]));
        assert!(matches!(open(&letter, "xterm-bad"), Opened::Invalid));
        assert!(matches!(open(&letter, "vt100"), Opened::NotFound));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(terminfo_profile(""), Profile::NoTty);
        assert_eq!(terminfo_profile("dumb"), Profile::NoTty);
    }

    /// Values from x/ansi v0.11.8 (go1.26.5, darwin/arm64), and the 256-colour frame of a live Go pull.
    /// All 2^24 colours were compared once with a Go dump (impl-interop-fixes.md).
    #[test]
    fn convert256_go_values() {
        for (rgb, c256, c16) in [
            // arm64 fuses c*255-35 in to6Cube: 155 falls into the cube level below (amd64 Go: 19, 4).
            ([0, 0, 155], 18, 4),
            ([115, 115, 115], 243, 8),
            ([96, 96, 96], 59, 8),
            ([90, 86, 224], 62, 12),
            ([92, 86, 225], 62, 12),
            ([0, 0, 0], 16, 0),
            ([255, 255, 255], 231, 15),
            ([0x5f, 0x87, 0xaf], 67, 4),
            ([128, 128, 128], 244, 7),
            ([238, 111, 248], 207, 13),
        ] {
            assert_eq!(convert256(rgb), c256, "{rgb:?}");
            assert_eq!(convert16(rgb), c16, "{rgb:?}");
        }
    }

    /// colorprofile v0.4.3 `Writer` outputs for one input, per profile (Go probe).
    #[test]
    fn downsample_matches_colorprofile_writer() {
        let input = "\x1b[1mt\x1b[m \x1b[38;2;90;86;224;48;2;92;86;225m\u{258c}\x1b[m\x1b[38;2;96;96;96m\u{2591}\x1b[m \x1b[32mdone\x1b[m \x1b[0;1;38:2::1:2:3m x\x1b[?25l\x1b[2A";
        let want = [
            (Profile::NoTty, "t \u{258c}\u{2591} done  x"),
            (
                Profile::Ascii,
                "\x1b[1mt\x1b[m \x1b[m\u{258c}\x1b[m\x1b[m\u{2591}\x1b[m \x1b[mdone\x1b[m \x1b[;1m x\x1b[?25l\x1b[2A",
            ),
            (
                Profile::Ansi,
                "\x1b[1mt\x1b[m \x1b[94;104m\u{258c}\x1b[m\x1b[90m\u{2591}\x1b[m \x1b[32mdone\x1b[m \x1b[;1;30m x\x1b[?25l\x1b[2A",
            ),
            (
                Profile::Ansi256,
                "\x1b[1mt\x1b[m \x1b[38;5;62;48;5;62m\u{258c}\x1b[m\x1b[38;5;59m\u{2591}\x1b[m \x1b[32mdone\x1b[m \x1b[;1;38;5;16m x\x1b[?25l\x1b[2A",
            ),
            (Profile::TrueColor, input),
        ];
        for (p, out) in want {
            assert_eq!(downsample(input, p), out, "{p}");
        }
    }

    #[test]
    fn sgr_forms() {
        let d = |s: &str, p| downsample(s, p).into_owned();
        // Indexed colours pass under 256 colours and map to 16; bright and default colours.
        assert_eq!(d("\x1b[38;5;200m", Profile::Ansi256), "\x1b[38;5;200m");
        assert_eq!(d("\x1b[38;5;200m", Profile::Ansi), "\x1b[91m");
        assert_eq!(d("\x1b[48;5;232m", Profile::Ansi), "\x1b[40m");
        assert_eq!(
            d("\x1b[91;101;39;49m", Profile::Ansi256),
            "\x1b[91;101;39;49m"
        );
        assert_eq!(d("\x1b[91;101;39;49m", Profile::Ascii), "\x1b[m");
        // Underline colours have no basic form.
        assert_eq!(d("\x1b[58;2;255;0;0m", Profile::Ansi), "\x1b[58;5;9m");
        // Transparent, and an invalid colour (dropped).
        assert_eq!(d("\x1b[38;1m", Profile::Ansi256), "\x1b[38;5;0m");
        assert_eq!(d("\x1b[1;38;2;1m", Profile::Ansi256), "\x1b[1;2;1m");
        // Other sequences and text are untouched; NoTTY strips them.
        assert_eq!(
            d("a\x1b]8;;x\x07b\x1b[?25h", Profile::Ansi),
            "a\x1b]8;;x\x07b\x1b[?25h"
        );
        assert_eq!(d("a\x1b]8;;x\x07b\x1b[?25h", Profile::NoTty), "ab");
        assert!(matches!(
            downsample("plain", Profile::Ascii),
            Cow::Borrowed(_)
        ));
    }
}
