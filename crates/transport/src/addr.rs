//! Go address strings: go-iroh `netaddr` (`endpointaddr.go`, `relayurl.go`) parse and format rules,
//! `netip.ParseAddrPort`, and dstore `transport.ParseAddrs`.
//!
//! Everything is ported by hand from go1.26.5 (`net/netip/netip.go`, `net/url/url.go`) because the Rust
//! parsers differ: `SocketAddr` rejects zones and brackets differently, and `url::Url` is stricter than Go
//! and drops default ports. The parsers work over bytes, so no input can make them panic.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use dstore_gocompat::quote::quote;

/// A relay URL in its normalised Go `url.String()` form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoRelayUrl(pub String);

/// go-iroh `netaddr.TransportAddr`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GoTransportAddr {
    Relay(GoRelayUrl),
    /// An IPv4-mapped IPv6 address is kept as IPv6.
    Ip {
        ip: IpAddr,
        zone: Option<String>,
        port: u16,
    },
    Custom {
        id: u64,
        data: Vec<u8>,
    },
}

/// "relay:<url>", "ip:1.2.3.4:5", "ip:[fe80::1%en0]:7", "<id:x>_<hex>" (no "custom:").
impl fmt::Display for GoTransportAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GoTransportAddr::Relay(u) => write!(f, "relay:{}", u.0),
            GoTransportAddr::Ip { ip, zone, port } => {
                write!(f, "ip:{}", addr_port_string(ip, zone.as_deref(), *port))
            }
            // CustomAddr.customString: strconv.FormatUint(id, 16) + "_" + hex.EncodeToString(data).
            GoTransportAddr::Custom { id, data } => {
                write!(f, "{id:x}_{}", dstore_gocompat::hex::encode(data))
            }
        }
    }
}

impl GoTransportAddr {
    pub fn is_relay(&self) -> bool {
        matches!(self, GoTransportAddr::Relay(_))
    }
}

/// `netaddr.ParseRelayURL`: "failed to parse relay URL: parse \"…\": …".
///
/// Go `url.Parse`, then `normalizeURL` (lower-case host; "/" for an empty path of a special scheme), then
/// `url.String()`.
pub fn parse_relay_url(s: &str) -> Result<GoRelayUrl, String> {
    let mut u = url_parse(s.as_bytes()).map_err(|e| format!("failed to parse relay URL: {e}"))?;
    u.host = dstore_gocompat::strings::to_lower(&u.host);
    if u.path.is_empty() && is_special_scheme(&u.scheme) {
        u.path = b"/".to_vec();
    }
    // The result is valid UTF-8: every part is either escaped to ASCII or a sub-slice of `s` cut at ASCII
    // delimiters, and the host went through `to_lower`, which replaces invalid bytes with U+FFFD.
    Ok(GoRelayUrl(
        String::from_utf8_lossy(&u.to_go_string()).into_owned(),
    ))
}

/// `netip.ParseAddrPort` with Go's texts.
pub fn parse_addr_port(s: &str) -> Result<(IpAddr, Option<String>, u16), String> {
    let b = s.as_bytes();
    // splitAddrPort
    let Some(i) = b.iter().rposition(|&c| c == b':') else {
        return Err("not an ip:port".to_string());
    };
    let mut ip = b.get(..i).unwrap_or_default();
    let port = b.get(i + 1..).unwrap_or_default();
    if ip.is_empty() {
        return Err("no IP".to_string());
    }
    if port.is_empty() {
        return Err("no port".to_string());
    }
    let mut v6 = false;
    if ip.first() == Some(&b'[') {
        if ip.len() < 2 || ip.last() != Some(&b']') {
            return Err("missing ]".to_string());
        }
        ip = ip.get(1..ip.len() - 1).unwrap_or_default();
        v6 = true;
    }
    // ParseAddrPort
    let invalid_port = || format!("invalid port {} parsing {}", quote(port), quote(b));
    let port = std::str::from_utf8(port).map_err(|_| invalid_port())?;
    let port16 = dstore_gocompat::strconv::parse_uint(port, 10, 16)
        .ok()
        .and_then(|p| u16::try_from(p).ok())
        .ok_or_else(invalid_port)?;
    let (addr, zone) = parse_addr(ip)?;
    match addr {
        IpAddr::V4(_) if v6 => Err(format!(
            "invalid ip:port {}, square brackets can only be used with IPv6 addresses",
            quote(b)
        )),
        IpAddr::V6(_) if !v6 => Err(format!(
            "invalid ip:port {}, IPv6 addresses must be surrounded by square brackets",
            quote(b)
        )),
        _ => Ok((addr, zone, port16)),
    }
}

/// `netaddr.ParseTransportAddr` with Go's texts.
pub fn parse_transport_addr(s: &str) -> Result<GoTransportAddr, String> {
    let Some((kind, value)) = s.split_once(':') else {
        return parse_custom_addr(s);
    };
    match kind {
        "relay" => parse_relay_url(value).map(GoTransportAddr::Relay),
        "ip" => match parse_addr_port(value) {
            Ok((ip, zone, port)) => Ok(GoTransportAddr::Ip { ip, zone, port }),
            Err(e) => Err(format!("transport address {}: {e}", quote(s.as_bytes()))),
        },
        "custom" => parse_custom_addr(value),
        _ => Err(format!(
            "transport address {}: unknown kind {}",
            quote(s.as_bytes()),
            quote(kind.as_bytes())
        )),
    }
}

/// Go `transport.ParseAddrs`: bare ip:port fallback, silent skip.
pub fn parse_addrs(addrs: &[String]) -> Vec<GoTransportAddr> {
    let mut out = Vec::new();
    for s in addrs {
        match parse_transport_addr(s) {
            Ok(a) => out.push(a),
            Err(_) => {
                // Accept bare ip:port too.
                if let Ok((ip, zone, port)) = parse_addr_port(s) {
                    out.push(GoTransportAddr::Ip { ip, zone, port });
                }
            }
        }
    }
    out
}

/// `netaddr.ParseCustomAddr`: "<id hex>_<data hex>", with an optional "custom:" prefix.
fn parse_custom_addr(s: &str) -> Result<GoTransportAddr, String> {
    let s = s.strip_prefix("custom:").unwrap_or(s);
    let Some((id, data)) = s.split_once('_') else {
        return Err("missing '_' separator".to_string());
    };
    let id =
        dstore_gocompat::strconv::parse_uint(id, 16, 64).map_err(|_| "invalid ID".to_string())?;
    let data = dstore_gocompat::hex::decode_string(data.as_bytes())
        .map_err(|_| "invalid data".to_string())?;
    Ok(GoTransportAddr::Custom { id, data })
}

// ---- net/netip ----

/// `parseAddrError.Error`.
fn addr_error(input: &[u8], msg: &str, at: &[u8]) -> String {
    if at.is_empty() {
        format!("ParseAddr({}): {msg}", quote(input))
    } else {
        format!("ParseAddr({}): {msg} (at {})", quote(input), quote(at))
    }
}

/// `netip.ParseAddr`: the address and its zone (IPv6 only, never empty).
fn parse_addr(s: &[u8]) -> Result<(IpAddr, Option<String>), String> {
    for &c in s {
        match c {
            b'.' => return parse_ipv4(s).map(|a| (IpAddr::V4(a), None)),
            b':' => return parse_ipv6(s).map(|(a, zone)| (IpAddr::V6(a), zone)),
            // Assume that this was trying to be an IPv6 address with a zone specifier, but the address is
            // missing.
            b'%' => return Err(addr_error(s, "missing IPv6 address", b"")),
            _ => {}
        }
    }
    Err(addr_error(s, "unable to parse IP", b""))
}

/// `netip.parseIPv4Fields` over `input[off..end]`.
fn parse_ipv4_fields(input: &[u8], off: usize, end: usize) -> Result<[u8; 4], String> {
    let s = input.get(off..end).unwrap_or_default();
    let mut fields = [0u8; 4];
    let mut val: u32 = 0;
    let mut pos = 0usize;
    let mut dig_len = 0usize; // number of digits in current octet
    for (i, &c) in s.iter().enumerate() {
        if c.is_ascii_digit() {
            if dig_len == 1 && val == 0 {
                return Err(addr_error(
                    input,
                    "IPv4 field has octet with leading zero",
                    b"",
                ));
            }
            val = val * 10 + u32::from(c - b'0');
            dig_len += 1;
            if val > 255 {
                return Err(addr_error(input, "IPv4 field has value >255", b""));
            }
        } else if c == b'.' {
            // .1.2.3
            // 1.2.3.
            // 1..2.3
            if i == 0 || i == s.len() - 1 || s.get(i - 1) == Some(&b'.') {
                return Err(addr_error(
                    input,
                    "IPv4 field must have at least one digit",
                    s.get(i..).unwrap_or_default(),
                ));
            }
            // 1.2.3.4.5
            if pos == 3 {
                return Err(addr_error(input, "IPv4 address too long", b""));
            }
            if let Some(f) = fields.get_mut(pos) {
                *f = val as u8;
            }
            pos += 1;
            val = 0;
            dig_len = 0;
        } else {
            return Err(addr_error(
                input,
                "unexpected character",
                s.get(i..).unwrap_or_default(),
            ));
        }
    }
    if pos < 3 {
        return Err(addr_error(input, "IPv4 address too short", b""));
    }
    fields[3] = val as u8;
    Ok(fields)
}

/// `netip.parseIPv4`.
fn parse_ipv4(s: &[u8]) -> Result<Ipv4Addr, String> {
    parse_ipv4_fields(s, 0, s.len()).map(Ipv4Addr::from)
}

/// `netip.parseIPv6`: the address and its zone.
fn parse_ipv6(input: &[u8]) -> Result<(Ipv6Addr, Option<String>), String> {
    let mut s = input;

    // Split off the zone right from the start.
    let mut zone: &[u8] = &[];
    if let Some(i) = s.iter().position(|&c| c == b'%') {
        zone = s.get(i + 1..).unwrap_or_default();
        s = s.get(..i).unwrap_or_default();
        if zone.is_empty() {
            // Not allowed to have an empty zone if explicitly specified.
            return Err(addr_error(input, "zone must be a non-empty string", b""));
        }
    }

    let mut ip = [0u8; 16];
    let mut ellipsis: Option<usize> = None; // position of ellipsis in ip

    // Might have leading ellipsis
    if s.starts_with(b"::") {
        ellipsis = Some(0);
        s = s.get(2..).unwrap_or_default();
        // Might be only ellipsis
        if s.is_empty() {
            return Ok((Ipv6Addr::UNSPECIFIED, zone_string(zone)));
        }
    }

    // Loop, parsing hex numbers followed by colon.
    let mut i = 0usize;
    while i < 16 {
        // Hex number.
        let mut off = 0usize;
        let mut acc: u32 = 0;
        while let Some(&c) = s.get(off) {
            let d = match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                _ => break,
            };
            acc = (acc << 4) + u32::from(d);
            if off > 3 {
                // more than 4 digits in group, fail.
                return Err(addr_error(
                    input,
                    "each group must have 4 or less digits",
                    s,
                ));
            }
            if acc > u32::from(u16::MAX) {
                // Overflow, fail.
                return Err(addr_error(input, "IPv6 field has value >=2^16", s));
            }
            off += 1;
        }
        if off == 0 {
            // No digits found, fail.
            return Err(addr_error(
                input,
                "each colon-separated field must have at least one digit",
                s,
            ));
        }

        // If followed by dot, might be in trailing IPv4.
        if s.get(off) == Some(&b'.') {
            if ellipsis.is_none() && i != 12 {
                // Not the right place.
                return Err(addr_error(
                    input,
                    "embedded IPv4 address must replace the final 2 fields of the address",
                    s,
                ));
            }
            if i + 4 > 16 {
                // Not enough room.
                return Err(addr_error(
                    input,
                    "too many hex fields to fit an embedded IPv4 at the end of the address",
                    s,
                ));
            }
            let mut end = input.len();
            if !zone.is_empty() {
                end -= zone.len() + 1;
            }
            let v4 = parse_ipv4_fields(input, end - s.len(), end)?;
            if let Some(dst) = ip.get_mut(i..i + 4) {
                dst.copy_from_slice(&v4);
            }
            s = &[];
            i += 4;
            break;
        }

        // Save this 16-bit chunk.
        ip[i] = (acc >> 8) as u8;
        ip[i + 1] = acc as u8;
        i += 2;

        // Stop at end of string.
        s = s.get(off..).unwrap_or_default();
        if s.is_empty() {
            break;
        }

        // Otherwise must be followed by colon and more.
        if s.first() != Some(&b':') {
            return Err(addr_error(input, "unexpected character, want colon", s));
        } else if s.len() == 1 {
            return Err(addr_error(
                input,
                "colon must be followed by more characters",
                s,
            ));
        }
        s = s.get(1..).unwrap_or_default();

        // Look for ellipsis.
        if s.first() == Some(&b':') {
            if ellipsis.is_some() {
                // already have one
                return Err(addr_error(input, "multiple :: in address", s));
            }
            ellipsis = Some(i);
            s = s.get(1..).unwrap_or_default();
            if s.is_empty() {
                // can be at end
                break;
            }
        }
    }

    // Must have used entire string.
    if !s.is_empty() {
        return Err(addr_error(input, "trailing garbage after address", s));
    }

    // If didn't parse enough, expand ellipsis.
    if i < 16 {
        let Some(e) = ellipsis else {
            return Err(addr_error(input, "address string too short", b""));
        };
        let n = 16 - i;
        for j in (e..i).rev() {
            ip[j + n] = ip[j];
        }
        for b in ip.iter_mut().skip(e).take(n) {
            *b = 0;
        }
    } else if ellipsis.is_some() {
        // Ellipsis must represent at least one 0 group.
        return Err(addr_error(
            input,
            "the :: must expand to at least one field of zeros",
            b"",
        ));
    }
    Ok((Ipv6Addr::from(ip), zone_string(zone)))
}

fn zone_string(zone: &[u8]) -> Option<String> {
    if zone.is_empty() {
        None
    } else {
        Some(String::from_utf8_lossy(zone).into_owned())
    }
}

/// `netip.AddrPort.String`.
fn addr_port_string(ip: &IpAddr, zone: Option<&str>, port: u16) -> String {
    let mut b = String::with_capacity(48);
    match ip {
        IpAddr::V4(a) => append_to4(&mut b, a.octets()),
        IpAddr::V6(a) => {
            b.push('[');
            append_to6_or_4in6(&mut b, a, zone);
            b.push(']');
        }
    }
    b.push(':');
    b.push_str(&port.to_string());
    b
}

/// `Addr.appendTo4`.
fn append_to4(b: &mut String, o: [u8; 4]) {
    for (i, v) in o.iter().enumerate() {
        if i > 0 {
            b.push('.');
        }
        b.push_str(&v.to_string());
    }
}

/// `Addr.appendTo4In6` for IPv4-mapped addresses, else `Addr.appendTo6` (RFC 5952: the first longest
/// run of at least two zero groups becomes "::"), each followed by "%zone" when there is a zone.
fn append_to6_or_4in6(b: &mut String, a: &Ipv6Addr, zone: Option<&str>) {
    let o = a.octets();
    if o[..10].iter().all(|&x| x == 0) && o[10] == 0xff && o[11] == 0xff {
        b.push_str("::ffff:");
        append_to4(b, [o[12], o[13], o[14], o[15]]);
    } else {
        let g = a.segments();
        let (mut zero_start, mut zero_end) = (usize::MAX, usize::MAX);
        let mut best = 0usize;
        for i in 0..8 {
            let mut j = i;
            while j < 8 && g[j] == 0 {
                j += 1;
            }
            let l = j - i;
            if l >= 2 && l > best {
                zero_start = i;
                zero_end = j;
                best = l;
            }
        }
        let mut i = 0usize;
        while i < 8 {
            if i == zero_start {
                b.push_str("::");
                i = zero_end;
                if i >= 8 {
                    break;
                }
            } else if i > 0 {
                b.push(':');
            }
            b.push_str(&format!("{:x}", g[i]));
            i += 1;
        }
    }
    if let Some(z) = zone
        && !z.is_empty()
    {
        b.push('%');
        b.push_str(z);
    }
}

// ---- net/url ----

/// `url.encoding`: the section of a URL being escaped or unescaped. Go's `encodePathSegment` is left out:
/// only `url.PathEscape`/`PathUnescape` use it, and relay URL parsing never calls them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Path,
    Host,
    Zone,
    UserPassword,
    QueryComponent,
    Fragment,
}

const UPPERHEX: &[u8; 16] = b"0123456789ABCDEF";

fn is_hex(c: u8) -> bool {
    c.is_ascii_hexdigit()
}

/// Precondition: `is_hex(c)`.
fn unhex(c: u8) -> u8 {
    9 * (c >> 6) + (c & 15)
}

/// `url.EscapeError`.
fn escape_error(s: &[u8]) -> String {
    format!("invalid URL escape {}", quote(s))
}

/// `url.shouldEscape` (the reference implementation `encoding_table.go` is generated from).
fn should_escape(c: u8, mode: Encoding) -> bool {
    // §2.3 Unreserved characters (alphanum)
    if c.is_ascii_alphanumeric() {
        return false;
    }
    if mode == Encoding::Host || mode == Encoding::Zone {
        // §3.2.2 Host allows sub-delims as part of reg-name, plus ':', '[', ']', '<', '>' and '"'.
        if matches!(
            c,
            b'!' | b'$'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b';'
                | b'='
                | b':'
                | b'['
                | b']'
                | b'<'
                | b'>'
                | b'"'
        ) {
            return false;
        }
    }
    match c {
        // §2.3 Unreserved characters (mark)
        b'-' | b'_' | b'.' | b'~' => return false,
        // §2.2 Reserved characters (reserved)
        b'$' | b'&' | b'+' | b',' | b'/' | b':' | b';' | b'=' | b'?' | b'@' => match mode {
            Encoding::Path => return c == b'?',
            Encoding::UserPassword => return matches!(c, b'@' | b'/' | b'?' | b':'),
            Encoding::QueryComponent => return true,
            Encoding::Fragment => return false,
            Encoding::Host | Encoding::Zone => {}
        },
        _ => {}
    }
    if mode == Encoding::Fragment && matches!(c, b'!' | b'(' | b')' | b'*') {
        return false;
    }
    // Everything else must be escaped.
    true
}

/// `url.unescape`.
fn unescape(s: &[u8], mode: Encoding) -> Result<Vec<u8>, String> {
    // Count %, check that they're well-formed.
    let mut n = 0usize;
    let mut has_plus = false;
    let mut i = 0usize;
    while let Some(&c) = s.get(i) {
        match c {
            b'%' => {
                n += 1;
                let (h1, h2) = match (s.get(i + 1), s.get(i + 2)) {
                    (Some(&h1), Some(&h2)) if is_hex(h1) && is_hex(h2) => (h1, h2),
                    _ => {
                        let rest = s.get(i..).unwrap_or_default();
                        return Err(escape_error(rest.get(..3).unwrap_or(rest)));
                    }
                };
                let esc = s.get(i..i + 3).unwrap_or_default();
                // In the host component %-encoding can only be used for non-ASCII bytes, except %25 in
                // IPv6 scoped-address literals (RFC 6874).
                if mode == Encoding::Host && unhex(h1) < 8 && esc != b"%25" {
                    return Err(escape_error(esc));
                }
                if mode == Encoding::Zone {
                    let v = (unhex(h1) << 4) | unhex(h2);
                    if esc != b"%25" && v != b' ' && should_escape(v, Encoding::Host) {
                        return Err(escape_error(esc));
                    }
                }
                i += 3;
            }
            b'+' => {
                has_plus = mode == Encoding::QueryComponent;
                i += 1;
            }
            _ => {
                if (mode == Encoding::Host || mode == Encoding::Zone)
                    && c < 0x80
                    && should_escape(c, mode)
                {
                    return Err(format!("invalid character {} in host name", quote(&[c])));
                }
                i += 1;
            }
        }
    }

    if n == 0 && !has_plus {
        return Ok(s.to_vec());
    }

    let plus = if mode == Encoding::QueryComponent {
        b' '
    } else {
        b'+'
    };
    let mut t = Vec::with_capacity(s.len().saturating_sub(2 * n));
    let mut i = 0usize;
    while let Some(&c) = s.get(i) {
        match c {
            b'%' => {
                // The loop above established that both bytes are hex digits.
                let h1 = s.get(i + 1).copied().unwrap_or(b'0');
                let h2 = s.get(i + 2).copied().unwrap_or(b'0');
                t.push((unhex(h1) << 4) | unhex(h2));
                i += 3;
            }
            b'+' => {
                t.push(plus);
                i += 1;
            }
            _ => {
                t.push(c);
                i += 1;
            }
        }
    }
    Ok(t)
}

/// `url.escape`.
fn escape(s: &[u8], mode: Encoding) -> Vec<u8> {
    let mut t = Vec::with_capacity(s.len());
    for &c in s {
        if c == b' ' && mode == Encoding::QueryComponent {
            t.push(b'+');
        } else if should_escape(c, mode) {
            t.push(b'%');
            t.push(UPPERHEX[usize::from(c >> 4)]);
            t.push(UPPERHEX[usize::from(c & 15)]);
        } else {
            t.push(c);
        }
    }
    t
}

/// `url.URL`, with Go strings as bytes.
#[derive(Default)]
struct GoUrl {
    scheme: Vec<u8>,
    opaque: Vec<u8>,
    /// (username, password if set).
    user: Option<(Vec<u8>, Option<Vec<u8>>)>,
    host: Vec<u8>,
    path: Vec<u8>,
    fragment: Vec<u8>,
    raw_query: Vec<u8>,
    raw_path: Vec<u8>,
    raw_fragment: Vec<u8>,
    force_query: bool,
    omit_host: bool,
}

/// `url.getScheme`: (scheme, rest).
fn get_scheme(raw: &[u8]) -> Result<(&[u8], &[u8]), String> {
    for (i, &c) in raw.iter().enumerate() {
        if c.is_ascii_alphabetic() {
            // do nothing
        } else if c.is_ascii_digit() || c == b'+' || c == b'-' || c == b'.' {
            if i == 0 {
                return Ok((&[], raw));
            }
        } else if c == b':' {
            if i == 0 {
                return Err("missing protocol scheme".to_string());
            }
            return Ok((
                raw.get(..i).unwrap_or_default(),
                raw.get(i + 1..).unwrap_or_default(),
            ));
        } else {
            // we have encountered an invalid character, so there is no valid scheme
            return Ok((&[], raw));
        }
    }
    Ok((&[], raw))
}

/// `url.Parse`: `parse %q: <err>`.
fn url_parse(raw: &[u8]) -> Result<GoUrl, String> {
    // Cut off #frag
    let (u, frag) = match raw.iter().position(|&c| c == b'#') {
        Some(i) => (
            raw.get(..i).unwrap_or_default(),
            raw.get(i + 1..).unwrap_or_default(),
        ),
        None => (raw, &[][..]),
    };
    let mut url = parse_url(u).map_err(|e| format!("parse {}: {e}", quote(u)))?;
    if frag.is_empty() {
        return Ok(url);
    }
    url.set_fragment(frag)
        .map_err(|e| format!("parse {}: {e}", quote(raw)))?;
    Ok(url)
}

/// `url.parse(rawURL, viaRequest = false)`.
fn parse_url(raw: &[u8]) -> Result<GoUrl, String> {
    if raw.iter().any(|&b| b < b' ' || b == 0x7f) {
        return Err("net/url: invalid control character in URL".to_string());
    }
    let mut url = GoUrl::default();

    if raw == b"*" {
        url.path = b"*".to_vec();
        return Ok(url);
    }

    // Split off possible leading "http:", "mailto:", etc. Cannot contain escaped characters.
    let (scheme, mut rest) = get_scheme(raw)?;
    url.scheme = scheme.to_ascii_lowercase();

    if rest.last() == Some(&b'?') && rest.iter().filter(|&&c| c == b'?').count() == 1 {
        url.force_query = true;
        rest = rest.get(..rest.len() - 1).unwrap_or_default();
    } else if let Some(i) = rest.iter().position(|&c| c == b'?') {
        url.raw_query = rest.get(i + 1..).unwrap_or_default().to_vec();
        rest = rest.get(..i).unwrap_or_default();
    }

    if !rest.starts_with(b"/") {
        if !url.scheme.is_empty() {
            // We consider rootless paths per RFC 3986 as opaque.
            url.opaque = rest.to_vec();
            return Ok(url);
        }
        // Avoid confusion with malformed schemes, like cache_object:foo/bar.
        if first_segment(rest).contains(&b':') {
            // First path segment has colon. Not allowed in relative URL.
            return Err("first path segment in URL cannot contain colon".to_string());
        }
    }

    if (!url.scheme.is_empty() || !rest.starts_with(b"///")) && rest.starts_with(b"//") {
        let mut authority = rest.get(2..).unwrap_or_default();
        rest = &[];
        if let Some(i) = authority.iter().position(|&c| c == b'/') {
            rest = authority.get(i..).unwrap_or_default();
            authority = authority.get(..i).unwrap_or_default();
        }
        let (user, host) = parse_authority(&url.scheme, authority)?;
        url.user = user;
        url.host = host;
    } else if !url.scheme.is_empty() && rest.starts_with(b"/") {
        // OmitHost is set to true when rawURL has an empty host (authority).
        url.omit_host = true;
    }

    // Set Path and, optionally, RawPath.
    url.set_path(rest)?;
    Ok(url)
}

/// The bytes before the first '/' (`strings.Cut(s, "/")`).
fn first_segment(s: &[u8]) -> &[u8] {
    match s.iter().position(|&c| c == b'/') {
        Some(i) => s.get(..i).unwrap_or_default(),
        None => s,
    }
}

/// `url.parseAuthority`.
#[allow(clippy::type_complexity)]
fn parse_authority(
    scheme: &[u8],
    authority: &[u8],
) -> Result<(Option<(Vec<u8>, Option<Vec<u8>>)>, Vec<u8>), String> {
    let at = authority.iter().rposition(|&c| c == b'@');
    let host = match at {
        None => parse_host(scheme, authority)?,
        Some(i) => parse_host(scheme, authority.get(i + 1..).unwrap_or_default())?,
    };
    let Some(i) = at else {
        return Ok((None, host));
    };
    let userinfo = authority.get(..i).unwrap_or_default();
    if !valid_userinfo(userinfo) {
        return Err("net/url: invalid userinfo".to_string());
    }
    let user = match userinfo.iter().position(|&c| c == b':') {
        None => (unescape(userinfo, Encoding::UserPassword)?, None),
        Some(j) => {
            let username = unescape(
                userinfo.get(..j).unwrap_or_default(),
                Encoding::UserPassword,
            )?;
            let password = unescape(
                userinfo.get(j + 1..).unwrap_or_default(),
                Encoding::UserPassword,
            )?;
            (username, Some(password))
        }
    };
    Ok((Some(user), host))
}

/// `url.parseHost`: host[:port], with GODEBUG `urlstrictcolons` at its go1.26 default (strict for http
/// and https).
fn parse_host(scheme: &[u8], host: &[u8]) -> Result<Vec<u8>, String> {
    match host.iter().rposition(|&c| c == b'[') {
        Some(idx) if idx > 0 => return Err("invalid IP-literal".to_string()),
        Some(_) => {
            // Parse an IP-Literal in RFC 3986 and RFC 6874.
            let Some(close) = host.iter().rposition(|&c| c == b']') else {
                return Err("missing ']' in host".to_string());
            };
            let colon_port = host.get(close + 1..).unwrap_or_default();
            if !valid_optional_port(colon_port) {
                return Err(format!("invalid port {} after host", quote(colon_port)));
            }
            let unescaped_colon_port = unescape(colon_port, Encoding::Host)?;

            let hostname = host.get(1..close).unwrap_or_default();
            // RFC 6874 defines that %25 (%-encoded percent) introduces the zone identifier.
            let unescaped_hostname = match find(hostname, b"%25") {
                Some(z) => {
                    let mut h = unescape(hostname.get(..z).unwrap_or_default(), Encoding::Host)?;
                    h.extend(unescape(
                        hostname.get(z..).unwrap_or_default(),
                        Encoding::Zone,
                    )?);
                    h
                }
                None => unescape(hostname, Encoding::Host)?,
            };

            // Only a host identified by a valid IPv6 address can be enclosed by square brackets.
            let (addr, _) =
                parse_addr(&unescaped_hostname).map_err(|e| format!("invalid host: {e}"))?;
            if addr.is_ipv4() {
                return Err("invalid IP-literal".to_string());
            }
            let mut out = Vec::with_capacity(unescaped_hostname.len() + 2 + colon_port.len());
            out.push(b'[');
            out.extend(&unescaped_hostname);
            out.push(b']');
            out.extend(unescaped_colon_port);
            return Ok(out);
        }
        None => {}
    }
    if let Some(first) = host.iter().position(|&c| c == b':') {
        let last = host.iter().rposition(|&c| c == b':').unwrap_or(first);
        let mut i = first;
        // RFC 3986 does not allow colons in the host; strict colons are enforced only for http and
        // https (go.dev/issue/75223, 78077).
        if last != first && scheme != b"http" && scheme != b"https" {
            i = last;
        }
        let colon_port = host.get(i..).unwrap_or_default();
        if !valid_optional_port(colon_port) {
            return Err(format!("invalid port {} after host", quote(colon_port)));
        }
    }
    unescape(host, Encoding::Host)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// `url.validOptionalPort`: empty, or ":" followed by digits.
fn valid_optional_port(port: &[u8]) -> bool {
    match port.split_first() {
        None => true,
        Some((&b':', digits)) => digits.iter().all(u8::is_ascii_digit),
        Some(_) => false,
    }
}

/// `url.validUserinfo` (RFC 3986 §3.2.1, plus '@').
fn valid_userinfo(s: &[u8]) -> bool {
    s.iter().all(|&c| {
        c.is_ascii_alphanumeric()
            || matches!(
                c,
                b'-' | b'.'
                    | b'_'
                    | b':'
                    | b'~'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
                    | b'%'
                    | b'@'
            )
    })
}

/// `url.validEncoded`.
fn valid_encoded(s: &[u8], mode: Encoding) -> bool {
    s.iter().all(|&c| match c {
        b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'=' | b':'
        | b'@' => true,
        b'[' | b']' => true,
        b'%' => true,
        _ => !should_escape(c, mode),
    })
}

/// `relayurl.go` `isSpecialScheme`.
fn is_special_scheme(scheme: &[u8]) -> bool {
    matches!(
        scheme.to_ascii_lowercase().as_slice(),
        b"http" | b"https" | b"ws" | b"wss" | b"ftp" | b"file"
    )
}

impl GoUrl {
    /// `URL.setPath`.
    fn set_path(&mut self, p: &[u8]) -> Result<(), String> {
        let path = unescape(p, Encoding::Path)?;
        self.raw_path = if escape(&path, Encoding::Path) == p {
            // Default encoding is fine.
            Vec::new()
        } else {
            p.to_vec()
        };
        self.path = path;
        Ok(())
    }

    /// `URL.setFragment`.
    fn set_fragment(&mut self, f: &[u8]) -> Result<(), String> {
        let frag = unescape(f, Encoding::Fragment)?;
        self.raw_fragment = if escape(&frag, Encoding::Fragment) == f {
            Vec::new()
        } else {
            f.to_vec()
        };
        self.fragment = frag;
        Ok(())
    }

    /// `URL.EscapedPath`.
    fn escaped_path(&self) -> Vec<u8> {
        if !self.raw_path.is_empty()
            && valid_encoded(&self.raw_path, Encoding::Path)
            && unescape(&self.raw_path, Encoding::Path).is_ok_and(|p| p == self.path)
        {
            return self.raw_path.clone();
        }
        if self.path == b"*" {
            return b"*".to_vec(); // don't escape (Issue 11202)
        }
        escape(&self.path, Encoding::Path)
    }

    /// `URL.EscapedFragment`.
    fn escaped_fragment(&self) -> Vec<u8> {
        if !self.raw_fragment.is_empty()
            && valid_encoded(&self.raw_fragment, Encoding::Fragment)
            && unescape(&self.raw_fragment, Encoding::Fragment).is_ok_and(|f| f == self.fragment)
        {
            return self.raw_fragment.clone();
        }
        escape(&self.fragment, Encoding::Fragment)
    }

    /// `URL.String`.
    fn to_go_string(&self) -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        if !self.scheme.is_empty() {
            buf.extend(&self.scheme);
            buf.push(b':');
        }
        if !self.opaque.is_empty() {
            buf.extend(&self.opaque);
        } else {
            let has_authority =
                !self.scheme.is_empty() || !self.host.is_empty() || self.user.is_some();
            let omit_empty_host = self.omit_host && self.host.is_empty() && self.user.is_none();
            if has_authority && !omit_empty_host {
                if !self.host.is_empty() || !self.path.is_empty() || self.user.is_some() {
                    buf.extend(b"//");
                }
                if let Some((username, password)) = &self.user {
                    buf.extend(escape(username, Encoding::UserPassword));
                    if let Some(password) = password {
                        buf.push(b':');
                        buf.extend(escape(password, Encoding::UserPassword));
                    }
                    buf.push(b'@');
                }
                if !self.host.is_empty() {
                    buf.extend(escape(&self.host, Encoding::Host));
                }
            }
            let path = self.escaped_path();
            if path.first().is_some_and(|&c| c != b'/') && !self.host.is_empty() {
                buf.push(b'/');
            }
            if buf.is_empty() && first_segment(&path).contains(&b':') {
                // RFC 3986 §4.2: a first segment with a colon must be preceded by a dot-segment.
                buf.extend(b"./");
            }
            buf.extend(path);
        }
        if self.force_query || !self.raw_query.is_empty() {
            buf.push(b'?');
            buf.extend(&self.raw_query);
        }
        if !self.fragment.is_empty() {
            buf.push(b'#');
            buf.extend(self.escaped_fragment());
        }
        buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        match s.parse() {
            Ok(a) => a,
            Err(e) => panic!("bad test address {s}: {e}"),
        }
    }

    fn relay(s: &str) -> GoRelayUrl {
        match parse_relay_url(s) {
            Ok(u) => u,
            Err(e) => panic!("parse_relay_url({s:?}): {e}"),
        }
    }

    fn addr(s: &str) -> GoTransportAddr {
        match parse_transport_addr(s) {
            Ok(a) => a,
            Err(e) => panic!("parse_transport_addr({s:?}): {e}"),
        }
    }

    // go-iroh netaddr TestRelayURLNormalization.
    #[test]
    fn relay_url_normalization() {
        assert_eq!(relay("https://example.com").0, "https://example.com/");
    }

    // go-iroh netaddr TestTransportAddrStringRoundTrip.
    #[test]
    fn transport_addr_string_round_trip() {
        let cases = [
            GoTransportAddr::Relay(relay("https://relay.example.com")),
            GoTransportAddr::Ip {
                ip: ip("127.0.0.1"),
                zone: None,
                port: 9,
            },
            GoTransportAddr::Custom {
                id: 7,
                data: vec![0xde, 0xad],
            },
        ];
        for a in cases {
            let s = a.to_string();
            let parsed = addr(&s);
            assert_eq!(parsed.to_string(), s);
            assert_eq!(parsed, a);
        }
    }

    // go-iroh netaddr TestCustomAddrParseErrors.
    #[test]
    fn custom_addr_parse_errors() {
        for s in ["abc123", "xyz_0102", "1_ghij", "1_abc"] {
            assert!(parse_custom_addr(s).is_err(), "{s}");
        }
        assert_eq!(
            parse_custom_addr("abc123"),
            Err("missing '_' separator".to_string())
        );
        assert_eq!(parse_custom_addr("xyz_0102"), Err("invalid ID".to_string()));
        assert_eq!(parse_custom_addr("1_ghij"), Err("invalid data".to_string()));
        assert_eq!(parse_custom_addr("1_abc"), Err("invalid data".to_string()));
    }

    // go-iroh netaddr TestCustomAddrStringPrefix.
    #[test]
    fn custom_addr_string_prefix() {
        let a = GoTransportAddr::Custom {
            id: 7,
            data: vec![0xde, 0xad],
        };
        assert_eq!(a.to_string(), "7_dead");
        assert!(!a.is_relay());
        for s in ["7_dead", "custom:7_dead"] {
            assert_eq!(addr(s), a, "{s}");
        }
    }

    // go-iroh netaddr TestCustomAddrRoundTrip (iroh-base test_custom_addr_roundtrip).
    #[test]
    fn custom_addr_round_trip() {
        let cases: [(u64, Vec<u8>, String); 4] = [
            (
                1,
                vec![0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6],
                "1_a1b2c3d4e5f6".to_string(),
            ),
            (42, vec![0xab; 32], format!("2a_{}", "ab".repeat(32))),
            (0, Vec::new(), "0_".to_string()),
            (0xdead_beef, vec![0x01, 0x02], "deadbeef_0102".to_string()),
        ];
        for (id, data, want) in cases {
            let a = GoTransportAddr::Custom { id, data };
            assert_eq!(a.to_string(), want);
            assert_eq!(parse_custom_addr(&want), Ok(a.clone()), "{want}");
            assert_eq!(
                parse_custom_addr(&format!("custom:{want}")),
                Ok(a),
                "custom:{want}"
            );
        }
    }

    // go-iroh netaddr TestRelayURLEqualCompare: equal after normalisation, ordered by the normalised string.
    #[test]
    fn relay_url_equal_compare() {
        let a = relay("https://a.example.com");
        let b = relay("https://b.example.com");
        let a2 = relay("https://a.example.com/");
        assert_eq!(a, a2);
        assert!(a.0 < b.0);
    }

    // go-iroh netaddr TestTransportAddrTextRoundTrip: the text form of each kind parses back to the same
    // kind, never another.
    #[test]
    fn transport_addr_text_round_trip() {
        let r = GoTransportAddr::Relay(relay("https://relay.example.com"));
        assert_eq!(r.to_string(), "relay:https://relay.example.com/");
        assert!(addr(&r.to_string()).is_relay());
        assert!(!addr("ip:127.0.0.1:9").is_relay());
        let i = addr("ip:127.0.0.1:9");
        assert_eq!(i.to_string(), "ip:127.0.0.1:9");
        assert!(matches!(
            addr("relay:https://relay.example.com/"),
            GoTransportAddr::Relay(_)
        ));
    }

    // transport §5.4: netaddr.IPAddr{}.String().
    #[test]
    fn ip_addr_strings() {
        let cases: [(IpAddr, Option<&str>, u16, &str); 4] = [
            (ip("192.168.1.2"), None, 4242, "ip:192.168.1.2:4242"),
            (ip("2001:db8::1"), None, 4242, "ip:[2001:db8::1]:4242"),
            (ip("::ffff:10.0.0.1"), None, 1, "ip:[::ffff:10.0.0.1]:1"),
            (ip("fe80::1"), Some("en0"), 7, "ip:[fe80::1%en0]:7"),
        ];
        for (a, zone, port, want) in cases {
            let got = GoTransportAddr::Ip {
                ip: a,
                zone: zone.map(str::to_string),
                port,
            };
            assert_eq!(got.to_string(), want);
        }
    }

    // netip string6: the first longest run of two or more zero groups is elided; a single zero group is
    // not; hex digits are lower case.
    #[test]
    fn ipv6_rfc5952_layout() {
        let cases = [
            ("0:0:0:0:0:0:0:0", "::"),
            ("0:0:0:0:0:0:0:1", "::1"),
            ("1:0:0:0:0:0:0:0", "1::"),
            ("1:0:1:0:1:0:1:0", "1:0:1:0:1:0:1:0"),
            ("1:0:0:1:0:0:1:1", "1::1:0:0:1:1"),
            ("1:0:0:1:0:0:0:1", "1:0:0:1::1"),
            ("ABCD:EF01:0:0:0:0:0:2", "abcd:ef01::2"),
            ("0:0:0:0:0:1:0:0", "::1:0:0"),
            ("::102:304", "::102:304"),
            ("::ffff:0:0", "::ffff:0.0.0.0"),
            ("0:0:0:0:0:fffe:a00:1", "::fffe:a00:1"),
            // Review rows, printed by a go1.26.5 probe (`netip.ParseAddrPort("[in]:1").String()`).
            ("0:0:1:0:0:0:1:0", "0:0:1::1:0"),
            ("1:0:0:0:0:1:0:0", "1::1:0:0"),
            ("0:0:0:1:0:0:0:1", "::1:0:0:0:1"),
            ("::ffff:1.2.3.4", "::ffff:1.2.3.4"),
            ("::fffe:1.2.3.4", "::fffe:102:304"),
            ("0:0:0:0:0:ffff:0:1", "::ffff:0.0.0.1"),
            ("ffff::", "ffff::"),
            ("1:2:3:4:5:6:7:8", "1:2:3:4:5:6:7:8"),
            ("0:1:0:0:1:0:0:0", "0:1:0:0:1::"),
        ];
        for (input, want) in cases {
            let (a, zone, port) = match parse_addr_port(&format!("[{input}]:1")) {
                Ok(v) => v,
                Err(e) => panic!("{input}: {e}"),
            };
            assert_eq!(zone, None);
            assert_eq!(port, 1);
            assert_eq!(
                GoTransportAddr::Ip { ip: a, zone, port }.to_string(),
                format!("ip:[{want}]:1"),
                "{input}"
            );
        }
    }

    // netip.ParseAddr texts the vectors do not reach through ParseAddrPort. Expected values were printed by
    // go1.26.5 `netip.ParseAddr` (a throwaway probe): "ok:<Addr.String()> zone=<Zone()>" or "err:<text>".
    #[test]
    fn parse_addr_matches_go() {
        let cases = [
            (
                r#"1.2.3.4."#,
                r#"err:ParseAddr("1.2.3.4."): IPv4 field must have at least one digit (at ".")"#,
            ),
            (
                r#".1.2.3"#,
                r#"err:ParseAddr(".1.2.3"): IPv4 field must have at least one digit (at ".1.2.3")"#,
            ),
            (
                r#"1..2.3"#,
                r#"err:ParseAddr("1..2.3"): IPv4 field must have at least one digit (at ".2.3")"#,
            ),
            (
                r#"12345::"#,
                r#"err:ParseAddr("12345::"): each group must have 4 or less digits (at "12345::")"#,
            ),
            (
                r#":1"#,
                r#"err:ParseAddr(":1"): each colon-separated field must have at least one digit (at ":1")"#,
            ),
            (
                r#"1.2.3.4:"#,
                r#"err:ParseAddr("1.2.3.4:"): unexpected character (at ":")"#,
            ),
            (
                r#"1:1.2.3.4"#,
                r#"err:ParseAddr("1:1.2.3.4"): embedded IPv4 address must replace the final 2 fields of the address (at "1.2.3.4")"#,
            ),
            (
                r#"1:2:3:4:5:6:7:1.2.3.4"#,
                r#"err:ParseAddr("1:2:3:4:5:6:7:1.2.3.4"): embedded IPv4 address must replace the final 2 fields of the address (at "1.2.3.4")"#,
            ),
            (
                r#"1:2:3:4:5:6::1.2.3.4"#,
                r#"err:ParseAddr("1:2:3:4:5:6::1.2.3.4"): the :: must expand to at least one field of zeros"#,
            ),
            (r#"1x"#, r#"err:ParseAddr("1x"): unable to parse IP"#),
            (
                r#"1:x"#,
                r#"err:ParseAddr("1:x"): each colon-separated field must have at least one digit (at "x")"#,
            ),
            (
                r#"1:2x"#,
                r#"err:ParseAddr("1:2x"): unexpected character, want colon (at "x")"#,
            ),
            (
                r#"1:"#,
                r#"err:ParseAddr("1:"): colon must be followed by more characters (at ":")"#,
            ),
            (
                r#"1:2:3"#,
                r#"err:ParseAddr("1:2:3"): address string too short"#,
            ),
            (
                r#"1:2:3:4:5:6:7::8"#,
                r#"err:ParseAddr("1:2:3:4:5:6:7::8"): the :: must expand to at least one field of zeros"#,
            ),
            (
                r#"1:2:3:4::5:6:7:8"#,
                r#"err:ParseAddr("1:2:3:4::5:6:7:8"): the :: must expand to at least one field of zeros"#,
            ),
            (r#"%en0"#, r#"err:ParseAddr("%en0"): missing IPv6 address"#),
            (
                r#"::1:1.2.3.04"#,
                r#"err:ParseAddr("::1:1.2.3.04"): IPv4 field has octet with leading zero"#,
            ),
            (r#"::%eth0"#, r#"ok::: zone=eth0"#),
            (r#"::1.2.3.4"#, r#"ok:::102:304 zone="#),
            (r#"1:2:3:4:5:6:1.2.3.4"#, r#"ok:1:2:3:4:5:6:102:304 zone="#),
            (
                r#"fffff::"#,
                r#"err:ParseAddr("fffff::"): each group must have 4 or less digits (at "fffff::")"#,
            ),
            (
                r#"1:2:3:4:5:6:7:8:"#,
                r#"err:ParseAddr("1:2:3:4:5:6:7:8:"): colon must be followed by more characters (at ":")"#,
            ),
            (r#"::ffff:1.2.3.4%x"#, r#"ok:::ffff:1.2.3.4 zone=x"#),
            (
                r#"1.2.3.4%x"#,
                r#"err:ParseAddr("1.2.3.4%x"): unexpected character (at "%x")"#,
            ),
            (r#""#, r#"err:ParseAddr(""): unable to parse IP"#),
            (r#"::"#, r#"ok::: zone="#),
            (r#"1:2:3:4:5:6:7:8"#, r#"ok:1:2:3:4:5:6:7:8 zone="#),
            (
                r#"::1.2.3"#,
                r#"err:ParseAddr("::1.2.3"): IPv4 address too short"#,
            ),
            (
                r#"1::1.2.3.4.5"#,
                r#"err:ParseAddr("1::1.2.3.4.5"): IPv4 address too long"#,
            ),
            (r#"ä"#, r#"err:ParseAddr("ä"): unable to parse IP"#),
            (
                r#"1.2.3.ä"#,
                r#"err:ParseAddr("1.2.3.ä"): unexpected character (at "ä")"#,
            ),
            // Review rows, printed by the same go1.26.5 probe.
            (r#"::ffff:1.2.3.4"#, r#"ok:::ffff:1.2.3.4 zone="#),
            (r#"1:2:3:4:5:6:7::"#, r#"ok:1:2:3:4:5:6:7:0 zone="#),
            (r#"::1:2:3:4:5:6:7"#, r#"ok:0:1:2:3:4:5:6:7 zone="#),
            (
                r#"1::2::3"#,
                r#"err:ParseAddr("1::2::3"): multiple :: in address (at ":3")"#,
            ),
            (r#"fe80::1%en0"#, r#"ok:fe80::1 zone=en0"#),
            (r#"0000:0000::"#, r#"ok::: zone="#),
            (
                r#"00000::"#,
                r#"err:ParseAddr("00000::"): each group must have 4 or less digits (at "00000::")"#,
            ),
            (
                r#"1.2.3.4.5"#,
                r#"err:ParseAddr("1.2.3.4.5"): IPv4 address too long"#,
            ),
            (
                r#"1.2.3"#,
                r#"err:ParseAddr("1.2.3"): IPv4 address too short"#,
            ),
            (
                r#"256.1.1.1"#,
                r#"err:ParseAddr("256.1.1.1"): IPv4 field has value >255"#,
            ),
            (
                r#"1:2:3:4:5:6:7:8%"#,
                r#"err:ParseAddr("1:2:3:4:5:6:7:8%"): zone must be a non-empty string"#,
            ),
            (
                r#"1:2:3:4:5:6:7:8:9"#,
                r#"err:ParseAddr("1:2:3:4:5:6:7:8:9"): trailing garbage after address (at "9")"#,
            ),
            (
                r#"1::1.2.3.4%"#,
                r#"err:ParseAddr("1::1.2.3.4%"): zone must be a non-empty string"#,
            ),
            (
                r#"::ffff:256.1.1.1"#,
                r#"err:ParseAddr("::ffff:256.1.1.1"): IPv4 field has value >255"#,
            ),
            (r#"::1.2.3.4%zone"#, r#"ok:::102:304 zone=zone"#),
            (r#"1:2:3:4:5::1.2.3.4"#, r#"ok:1:2:3:4:5:0:102:304 zone="#),
            (
                r#"1:2:3:4:5:6:77777"#,
                r#"err:ParseAddr("1:2:3:4:5:6:77777"): each group must have 4 or less digits (at "77777")"#,
            ),
            (
                r#"g::"#,
                r#"err:ParseAddr("g::"): each colon-separated field must have at least one digit (at "g::")"#,
            ),
            (
                r#"::g"#,
                r#"err:ParseAddr("::g"): each colon-separated field must have at least one digit (at "g")"#,
            ),
            (
                r#" 1.2.3.4"#,
                r#"err:ParseAddr(" 1.2.3.4"): unexpected character (at " 1.2.3.4")"#,
            ),
            (
                r#"1.2.3.4 "#,
                r#"err:ParseAddr("1.2.3.4 "): unexpected character (at " ")"#,
            ),
            (
                r#":::"#,
                r#"err:ParseAddr(":::"): each colon-separated field must have at least one digit (at ":")"#,
            ),
            (
                r#"1:::2"#,
                r#"err:ParseAddr("1:::2"): each colon-separated field must have at least one digit (at ":2")"#,
            ),
            (
                r#"FFFF:FFFF:FFFF:FFFF:FFFF:FFFF:FFFF:FFFF"#,
                r#"ok:ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff zone="#,
            ),
            (
                r#"1::%"#,
                r#"err:ParseAddr("1::%"): zone must be a non-empty string"#,
            ),
            (
                r#"1:2:3:4:5:6:7:8.1"#,
                r#"err:ParseAddr("1:2:3:4:5:6:7:8.1"): embedded IPv4 address must replace the final 2 fields of the address (at "8.1")"#,
            ),
            (
                r#"::1.2.3.4:5"#,
                r#"err:ParseAddr("::1.2.3.4:5"): unexpected character (at ":5")"#,
            ),
            (
                r#"1:2:3:4:5:6:7:8::"#,
                r#"err:ParseAddr("1:2:3:4:5:6:7:8::"): the :: must expand to at least one field of zeros"#,
            ),
        ];
        for (input, want) in cases {
            let got = match parse_addr(input.as_bytes()) {
                // Go's Addr.String() of an address with a zone appends "%zone"; the probe printed the zone
                // separately too, so compare the zone-less form here.
                Ok((a, zone)) => {
                    let s = GoTransportAddr::Ip {
                        ip: a,
                        zone: None,
                        port: 1,
                    }
                    .to_string();
                    let s = s.strip_prefix("ip:").unwrap_or(&s);
                    let s = s.strip_suffix(":1").unwrap_or(s);
                    let s = s
                        .strip_prefix('[')
                        .and_then(|x| x.strip_suffix(']'))
                        .unwrap_or(s);
                    format!("ok:{s} zone={}", zone.unwrap_or_default())
                }
                Err(e) => format!("err:{e}"),
            };
            assert_eq!(got, want, "{input}");
        }
    }

    // go1.26.5 net/url.Parse + go-iroh normalizeURL + URL.String for cases beyond relay_urls.json. Expected
    // values were printed by `netaddr.ParseRelayURL` of go-iroh v0.2.0 (a throwaway probe).
    #[test]
    fn parse_relay_url_matches_go() {
        let cases = [
            (r#"https:"#, r#"ok:https:///"#),
            (r#"*"#, r#"ok:*"#),
            (
                r#"http://a:b:c"#,
                r#"err:failed to parse relay URL: parse "http://a:b:c": invalid port ":b:c" after host"#,
            ),
            (
                r#"custom://a:b:c"#,
                r#"err:failed to parse relay URL: parse "custom://a:b:c": invalid port ":c" after host"#,
            ),
            (r#"mailto:ä"#, r#"ok:mailto:ä"#),
            (r#"https://x?ä"#, r#"ok:https://x/?ä"#),
            (r#"https://%C3.com"#, r#"ok:https://%EF%BF%BD.com/"#),
            (r#"https://ÄB.com"#, r#"ok:https://%C3%A4b.com/"#),
            (r#"https://İ.com"#, r#"ok:https://i.com/"#),
            (r#"a:b"#, r#"ok:a:b"#),
            (r#"/a:b"#, r#"ok:/a:b"#),
            (r#"./x:y"#, r#"ok:./x:y"#),
            (r#"?x"#, r#"ok:?x"#),
            (r#"#"#, r#"ok:"#),
            (r#"https://example.com#"#, r#"ok:https://example.com/"#),
            (r#"https://[::1%25en0]"#, r#"ok:https://[::1%25en0]/"#),
            (
                r#"https://[1.2.3.4]"#,
                r#"err:failed to parse relay URL: parse "https://[1.2.3.4]": invalid IP-literal"#,
            ),
            (r#"https://x@y@z"#, r#"ok:https://x%40y@z/"#),
            (
                r#"https://a%zz@b"#,
                r#"err:failed to parse relay URL: parse "https://a%zz@b": invalid URL escape "%zz""#,
            ),
            (
                r#"https://a b@c"#,
                r#"err:failed to parse relay URL: parse "https://a b@c": net/url: invalid userinfo"#,
            ),
            (
                r#"http://[fe80::1%25%41]:1"#,
                r#"ok:http://[fe80::1%25a]:1/"#,
            ),
            (
                r#"https://[::1]x"#,
                r#"err:failed to parse relay URL: parse "https://[::1]x": invalid port "x" after host"#,
            ),
            (r#"https://a/b%2fc"#, r#"ok:https://a/b%2fc"#),
            (
                r#"https://a/%zz"#,
                r#"err:failed to parse relay URL: parse "https://a/%zz": invalid URL escape "%zz""#,
            ),
            (
                r#"https://a#%zz"#,
                r#"err:failed to parse relay URL: parse "https://a#%zz": invalid URL escape "%zz""#,
            ),
            (r#"https://a#a b"#, r#"ok:https://a/#a%20b"#),
            (
                r#"https://user:p%40ss@host"#,
                r#"ok:https://user:p%40ss@host/"#,
            ),
            (
                r#"https://user:pa ss@host"#,
                r#"err:failed to parse relay URL: parse "https://user:pa ss@host": net/url: invalid userinfo"#,
            ),
            (r#"relay.example.com:443"#, r#"ok:relay.example.com:443"#),
            (
                r#"1:x"#,
                r#"err:failed to parse relay URL: parse "1:x": first path segment in URL cannot contain colon"#,
            ),
            (r#"x/y:z"#, r#"ok:x/y:z"#),
            (r#"//a:b@"#, r#"ok://a:b@"#),
            (
                r#"https://[::1]:80/?#frag"#,
                r#"ok:https://[::1]:80/?#frag"#,
            ),
            (r#"HTTP://"#, r#"ok:http:///"#),
            (r#"http:/x"#, r#"ok:http:/x"#),
            (r#"http:///x"#, r#"ok:http:///x"#),
            (r#"https://a/b?c#d'e"#, r#"ok:https://a/b?c#d'e"#),
            (r#"https://a/?q=a b"#, r#"ok:https://a/?q=a b"#),
            (r#"https://a/b;c,d"#, r#"ok:https://a/b;c,d"#),
            (
                r#"https://[fe80::1%25en0%2541]"#,
                r#"ok:https://[fe80::1%25en0%2541]/"#,
            ),
            (
                r#"https://[x]"#,
                r#"err:failed to parse relay URL: parse "https://[x]": invalid host: ParseAddr("x"): unable to parse IP"#,
            ),
            (
                r#"https://a[b]"#,
                r#"err:failed to parse relay URL: parse "https://a[b]": invalid IP-literal"#,
            ),
            (
                r#"https://[::1]:x"#,
                r#"err:failed to parse relay URL: parse "https://[::1]:x": invalid port ":x" after host"#,
            ),
            (
                r#"https://a:1:2"#,
                r#"err:failed to parse relay URL: parse "https://a:1:2": invalid port ":1:2" after host"#,
            ),
            (r#"ftp://a:1:2"#, r#"ok:ftp://a:1:2/"#),
            (r#"https://%E2%82%AC.com"#, r#"ok:https://%E2%82%AC.com/"#),
            (r#"https://a/%E2%82%AC"#, r#"ok:https://a/%E2%82%AC"#),
            (r#"https://a/€"#, r#"ok:https://a/%E2%82%AC"#),
            (r#"wss://ÄÖ"#, r#"ok:wss://%C3%A4%C3%B6/"#),
            (r#"//"#, r#"ok:"#),
            (r#"///a"#, r#"ok:///a"#),
            (r#"https:///a"#, r#"ok:https:///a"#),
            (r#"https://a#frag%20x"#, r#"ok:https://a/#frag%20x"#),
            (r#"https://a#!()*"#, r#"ok:https://a/#!()*"#),
            (r#"https://u%41:p@h"#, r#"ok:https://uA:p@h/"#),
            (r#"file:"#, r#"ok:file:///"#),
            (r#"HTTPS:"#, r#"ok:https:///"#),
            // Review rows, printed by the same go1.26.5 / go-iroh v0.2.0 probe.
            (
                r#"https://example.com/a%2Fb"#,
                r#"ok:https://example.com/a%2Fb"#,
            ),
            (r#"https://a/b%20c"#, r#"ok:https://a/b%20c"#),
            (r#"https://a/b c"#, r#"ok:https://a/b%20c"#),
            (r#"HTTP://EXAMPLE.COM:80/"#, r#"ok:http://example.com:80/"#),
            (
                r#"https://[::ffff:1.2.3.4]"#,
                r#"ok:https://[::ffff:1.2.3.4]/"#,
            ),
            (
                r#"https://[::1%25]"#,
                r#"err:failed to parse relay URL: parse "https://[::1%25]": invalid host: ParseAddr("::1%"): zone must be a non-empty string"#,
            ),
            (
                r#"https://a:80:90"#,
                r#"err:failed to parse relay URL: parse "https://a:80:90": invalid port ":80:90" after host"#,
            ),
            (r#"ws://a:1:2"#, r#"ok:ws://a:1:2/"#),
            (r#"https://a/?"#, r#"ok:https://a/?"#),
            (r#"https://a?b?c"#, r#"ok:https://a/?b?c"#),
            (r#"http://a/b#c#d"#, r#"ok:http://a/b#c%23d"#),
            (r#"https://a/%7e"#, r#"ok:https://a/%7e"#),
            (
                r#"https://a/%2"#,
                r#"err:failed to parse relay URL: parse "https://a/%2": invalid URL escape "%2""#,
            ),
            (r#"mailto:"#, r#"ok:mailto:"#),
            (r#"file:///x"#, r#"ok:file:///x"#),
            (r#"https://user@"#, r#"ok:https://user@/"#),
            (r#"https://@a"#, r#"ok:https://@a/"#),
            (
                "\u{9}",
                r#"err:failed to parse relay URL: parse "\t": net/url: invalid control character in URL"#,
            ),
            (
                r#"https://a b"#,
                r#"err:failed to parse relay URL: parse "https://a b": invalid character " " in host name"#,
            ),
            (r#"https://a/b?c d#e f"#, r#"ok:https://a/b?c d#e%20f"#),
            (
                r#"https://[fe80::1%25en0"#,
                r#"err:failed to parse relay URL: parse "https://[fe80::1%25en0": missing ']' in host"#,
            ),
            (r#"https://a]"#, r#"ok:https://a]/"#),
            (
                r#"https://a%41"#,
                r#"err:failed to parse relay URL: parse "https://a%41": invalid URL escape "%41""#,
            ),
            (r#"https://a%C3%A9"#, r#"ok:https://a%C3%A9/"#),
            (r#"https://a:"#, r#"ok:https://a:/"#),
            (r#"https://a:0080"#, r#"ok:https://a:0080/"#),
            (r#"https:/a"#, r#"ok:https:/a"#),
            (r#"https:a"#, r#"ok:https:a"#),
            (r#"https://a/./b/../c"#, r#"ok:https://a/./b/../c"#),
            (
                r#"https://a/%zz%"#,
                r#"err:failed to parse relay URL: parse "https://a/%zz%": invalid URL escape "%zz""#,
            ),
            (r#"https://A.EXAMPLE/B"#, r#"ok:https://a.example/B"#),
            (r#"relay+custom://X"#, r#"ok:relay+custom://x"#),
            (
                r#"1https://x"#,
                r#"err:failed to parse relay URL: parse "1https://x": first path segment in URL cannot contain colon"#,
            ),
            ("https://a\u{a0}b", r#"ok:https://a%C2%A0b/"#),
            ("https://x/\u{80}", r#"ok:https://x/%C2%80"#),
            (r#"https://a/b#%41"#, r#"ok:https://a/b#%41"#),
            (r#"https://a/b#c%2Fd"#, r#"ok:https://a/b#c%2Fd"#),
        ];
        for (input, want) in cases {
            let got = match parse_relay_url(input) {
                Ok(u) => format!("ok:{}", u.0),
                Err(e) => format!("err:{e}"),
            };
            assert_eq!(got, want, "{input}");
        }
        // Through ParseTransportAddr.
        let cases = [
            (r#"relay:https:"#, r#"ok:relay:https:///"#),
            (r#"ip:[::1%25en0]:1"#, r#"ok:ip:[::1%25en0]:1"#),
            (r#"custom:1_"#, r#"ok:1_"#),
            (r#"custom:_"#, r#"err:invalid ID"#),
            (r#"ip:[fe80::1%en0]:0"#, r#"ok:ip:[fe80::1%en0]:0"#),
            // Review rows, printed by the same probe.
            (
                r#"relay:https://a:80:90"#,
                r#"err:failed to parse relay URL: parse "https://a:80:90": invalid port ":80:90" after host"#,
            ),
            (
                r#"ip:[::ffff:1.2.3.4%eth0]:5"#,
                r#"ok:ip:[::ffff:1.2.3.4%eth0]:5"#,
            ),
            (r#"custom:1_AbCd"#, r#"ok:1_abcd"#),
            (r#"custom:FFFFFFFFFFFFFFFF_"#, r#"ok:ffffffffffffffff_"#),
            (r#"custom:1_0g"#, r#"err:invalid data"#),
            (r#"custom:_1"#, r#"err:invalid ID"#),
            (r#"x:"#, r#"err:transport address "x:": unknown kind "x""#),
            (r#":"#, r#"err:transport address ":": unknown kind """#),
            (r#"relay"#, r#"err:missing '_' separator"#),
            (r#"custom:1_2_3"#, r#"err:invalid data"#),
            (r#"custom:1__00"#, r#"err:invalid data"#),
        ];
        for (input, want) in cases {
            let got = match parse_transport_addr(input) {
                Ok(a) => format!("ok:{a}"),
                Err(e) => format!("err:{e}"),
            };
            assert_eq!(got, want, "{input}");
        }
    }

    #[test]
    fn parse_addrs_keeps_order_and_skips_silently() {
        let input: Vec<String> = [
            "ip:127.0.0.1:9",
            "bogus",
            "relay:https://r.example./",
            "127.0.0.1:9",
            "custom:1_ab",
            "mem:01020304",
            "ip:127.0.0.1:9",
            "[::1]:9",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let got: Vec<String> = parse_addrs(&input).iter().map(|a| a.to_string()).collect();
        assert_eq!(
            got,
            [
                "ip:127.0.0.1:9",
                "relay:https://r.example./",
                "ip:127.0.0.1:9",
                "1_ab",
                "ip:127.0.0.1:9",
                "ip:[::1]:9"
            ]
        );
        assert!(parse_addrs(&[]).is_empty());
    }

    #[test]
    fn url_should_escape_matches_go_table_spots() {
        // Spot checks of go1.26.5 net/url encoding_table.go.
        assert!(!should_escape(b'a', Encoding::Host));
        assert!(!should_escape(b'"', Encoding::Host));
        assert!(should_escape(b'"', Encoding::Path));
        assert!(should_escape(b'%', Encoding::Host));
        assert!(!should_escape(b'/', Encoding::Path));
        assert!(should_escape(b'?', Encoding::Path));
        assert!(!should_escape(b'?', Encoding::Fragment));
        assert!(!should_escape(b'!', Encoding::Fragment));
        assert!(should_escape(b'!', Encoding::Path));
        assert!(should_escape(b'\'', Encoding::Fragment));
        assert!(should_escape(b':', Encoding::UserPassword));
        assert!(!should_escape(b';', Encoding::UserPassword));
        assert!(should_escape(b'@', Encoding::Host));
        assert!(should_escape(0x80, Encoding::Host));
        assert!(should_escape(b' ', Encoding::QueryComponent));
        assert_eq!(escape(b"a b", Encoding::QueryComponent), b"a+b");
        assert_eq!(
            unescape(b"a+b%20", Encoding::QueryComponent),
            Ok(b"a b ".to_vec())
        );
        assert_eq!(unescape(b"a+b", Encoding::Path), Ok(b"a+b".to_vec()));
    }
}
