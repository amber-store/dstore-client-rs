//! Golden tests of `dstore_transport_iroh::mdns` (owner transport-iroh-mdns): `transport/mdns.json` (query
//! packets, announcements and parse cases).
//!
//! The vectors come from go-iroh v0.2.0 `iroh/mdns` through `tools/vectorgen` (family `transport`, VECTORS.md
//! "`transport/mdns.json`"). `questions` covers `parseQuestions`, the responder side, which is node-side and
//! not ported in v1, so it is not read here.
//!
//! A packet that carries a `relay=` TXT entry reaches `dstore_transport::addr::parse_relay_url`; those cases
//! are kept in their own tests (`*_with_relays`), so a regression there is told apart from one in the parser.
//!
//! A `parse` case that yields an announcement also carries `addrs`, go-iroh's `EndpointData` strings; the result
//! rendered as `ip:<addr>` then `relay:<url>` must equal them.

use std::net::SocketAddr;

use dstore_testkit::golden::{hex, load_json};
use dstore_transport_iroh::mdns::{self, Announcement};
use serde::Deserialize;

const FILE: &str = "transport/mdns.json";

#[derive(Deserialize)]
struct MdnsFile {
    service_name: String,
    names: Vec<NamesCase>,
    queries: Vec<QueryCase>,
    announcements: Vec<AnnouncementCase>,
    parse: Vec<ParseCase>,
}

#[derive(Deserialize)]
struct NamesCase {
    id: String,
    label: String,
    service: String,
    instance: String,
    host: String,
}

#[derive(Deserialize)]
struct QueryCase {
    name: String,
    id: String,
    packet: String,
}

#[derive(Deserialize)]
struct AnnouncementCase {
    name: String,
    id: String,
    service: String,
    info: Option<AnnouncementInfo>,
    error: Option<String>,
    packet: Option<String>,
}

#[derive(Deserialize)]
struct AnnouncementInfo {
    port: u16,
    ips: Vec<String>,
    relay: String,
    user_data: String,
}

#[derive(Deserialize)]
struct ParseCase {
    name: String,
    service: String,
    packet: String,
    ok: bool,
    id: Option<String>,
    ips: Option<Vec<String>>,
    relay: Option<String>,
    user_data: Option<String>,
    /// go-iroh's `EndpointData` strings: `ip:` addresses in record order, then `relay:` URLs.
    addrs: Option<Vec<String>>,
}

fn load() -> MdnsFile {
    load_json(FILE)
}

/// Fails with every collected failure.
fn report(what: &str, failures: &[String], total: usize) {
    assert!(
        failures.is_empty(),
        "{what}: {} of {total} cases failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn id32(case: &str, s: &str) -> Result<[u8; 32], String> {
    <[u8; 32]>::try_from(hex(s)).map_err(|_| format!("{case}: id {s:?} is not 32 bytes"))
}

fn socket_addrs(case: &str, ips: &[String]) -> Result<Vec<SocketAddr>, String> {
    ips.iter()
        .map(|s| {
            s.parse::<SocketAddr>()
                .map_err(|e| format!("{case}: address {s:?} does not parse: {e}"))
        })
        .collect()
}

/// Go `netip.AddrPort.String()` of an address an announcement carries: the zone is not sent over DNS.
fn without_zone(s: &str) -> String {
    match (s.find('%'), s.rfind(']')) {
        (Some(pct), Some(close)) if pct < close => format!("{}{}", &s[..pct], &s[close..]),
        _ => s.to_string(),
    }
}

/// Whether parsing the packet reaches `ParseRelayURL`: it carries a `relay=` TXT entry.
fn needs_relay(packet: &[u8]) -> bool {
    packet.windows(6).any(|w| w == b"relay=")
}

fn check_parse(c: &ParseCase) -> Result<(), String> {
    if c.service != mdns::SERVICE_NAME {
        return Err(format!(
            "{}: service {:?} has no Rust counterpart",
            c.name, c.service
        ));
    }
    let got = mdns::parse_announcement(&hex(&c.packet));
    if !c.ok {
        return match got {
            None => Ok(()),
            Some(a) => Err(format!("{}: want no announcement, got {a:?}", c.name)),
        };
    }
    let (Some(id), Some(ips), Some(go_addrs)) = (&c.id, &c.ips, &c.addrs) else {
        return Err(format!("{}: an ok case without id, ips or addrs", c.name));
    };
    let want = Announcement {
        id: id32(&c.name, id)?,
        addrs: socket_addrs(&c.name, ips)?,
        relay: c.relay.clone(),
        user_data: c.user_data.clone(),
    };
    let got = match got {
        Some(a) if a == want => a,
        Some(a) => return Err(format!("{}: got {a:?}, want {want:?}", c.name)),
        None => return Err(format!("{}: no announcement, want {want:?}", c.name)),
    };
    // The Go address strings of what was parsed, in go-iroh's EndpointData order: the IPs, then the relay.
    let mut strings: Vec<String> = got.addrs.iter().map(|a| format!("ip:{a}")).collect();
    strings.extend(got.relay.iter().map(|r| format!("relay:{r}")));
    if strings != *go_addrs {
        return Err(format!("{}: addrs {strings:?}, want {go_addrs:?}", c.name));
    }
    Ok(())
}

fn run_parse_cases(what: &str, relays: bool) {
    let f = load();
    let cases: Vec<&ParseCase> = f
        .parse
        .iter()
        .filter(|c| needs_relay(&hex(&c.packet)) == relays)
        .collect();
    assert!(!cases.is_empty(), "{what}: no cases");
    let failures: Vec<String> = cases.iter().filter_map(|c| check_parse(c).err()).collect();
    report(what, &failures, cases.len());
}

#[test]
fn service_name_and_names() {
    let f = load();
    assert_eq!(f.service_name, mdns::SERVICE_NAME);
    assert!(!f.names.is_empty());
    let mut failures = Vec::new();
    for c in &f.names {
        let id = match id32("names", &c.id) {
            Ok(id) => id,
            Err(e) => {
                failures.push(e);
                continue;
            }
        };
        let label = mdns::endpoint_label(&id);
        let service = format!("_{}._udp.local", mdns::SERVICE_NAME);
        if label != c.label {
            failures.push(format!("{}: label {label:?}, want {:?}", c.id, c.label));
        }
        if service != c.service {
            failures.push(format!(
                "{}: service {service:?}, want {:?}",
                c.id, c.service
            ));
        }
        if format!("{label}.{service}") != c.instance {
            failures.push(format!("{}: instance, want {:?}", c.id, c.instance));
        }
        if format!("{label}.local") != c.host {
            failures.push(format!("{}: host, want {:?}", c.id, c.host));
        }
    }
    report("names", &failures, f.names.len());
}

#[test]
fn query_packets() {
    let f = load();
    assert!(!f.queries.is_empty());
    let mut failures = Vec::new();
    for c in &f.queries {
        match id32(&c.name, &c.id) {
            Ok(id) => {
                let got = hex_string(&mdns::build_query(&id));
                if got != c.packet {
                    failures.push(format!("{}: got {got}, want {}", c.name, c.packet));
                }
            }
            Err(e) => failures.push(e),
        }
    }
    report("queries", &failures, f.queries.len());
}

fn hex_string(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn parse_cases_without_relays() {
    run_parse_cases("parse without relays", false);
}

#[test]
fn parse_cases_with_relays() {
    run_parse_cases("parse with relays", true);
}

/// Every packet go-iroh's `buildAnnouncement` produced parses back to what `announcementInfo` put in it: the
/// id, the addresses with their zones removed and duplicates skipped, the relay and the user data. Packets
/// of another service or with port 0 give no announcement.
fn run_announcements(what: &str, relays: bool) {
    let f = load();
    let mut failures = Vec::new();
    let mut total = 0usize;
    for c in &f.announcements {
        let (Some(info), Some(packet)) = (&c.info, &c.packet) else {
            if c.error.is_none() {
                failures.push(format!("{}: neither info and packet nor error", c.name));
            }
            continue;
        };
        let packet = hex(packet);
        if needs_relay(&packet) != relays {
            continue;
        }
        total += 1;
        let got = mdns::parse_announcement(&packet);
        if c.service != mdns::SERVICE_NAME || info.port == 0 {
            if let Some(a) = got {
                failures.push(format!("{}: want no announcement, got {a:?}", c.name));
            }
            continue;
        }
        let id = match id32(&c.name, &c.id) {
            Ok(id) => id,
            Err(e) => {
                failures.push(e);
                continue;
            }
        };
        let unzoned: Vec<String> = info.ips.iter().map(|s| without_zone(s)).collect();
        let mut addrs = match socket_addrs(&c.name, &unzoned) {
            Ok(a) => a,
            Err(e) => {
                failures.push(e);
                continue;
            }
        };
        let mut seen = Vec::new();
        addrs.retain(|a| {
            let fresh = !seen.contains(a);
            seen.push(*a);
            fresh
        });
        let want = Announcement {
            id,
            addrs,
            relay: Some(info.relay.clone()).filter(|r| !r.is_empty()),
            user_data: Some(info.user_data.clone()).filter(|u| !u.is_empty()),
        };
        match got {
            Some(a) if a == want => {}
            Some(a) => failures.push(format!("{}: got {a:?}, want {want:?}", c.name)),
            None => failures.push(format!("{}: no announcement, want {want:?}", c.name)),
        }
    }
    assert!(total > 0, "{what}: no cases");
    report(what, &failures, total);
}

#[test]
fn announcement_packets_without_relays() {
    run_announcements("announcements without relays", false);
}

#[test]
fn announcement_packets_with_relays() {
    run_announcements("announcements with relays", true);
}

#[test]
fn announcement_errors_carry_no_packet() {
    let f = load();
    let errors: Vec<&AnnouncementCase> = f
        .announcements
        .iter()
        .filter(|c| c.error.is_some())
        .collect();
    assert!(!errors.is_empty());
    for c in errors {
        assert!(c.packet.is_none() && c.info.is_none(), "{}", c.name);
        assert_eq!(
            c.error.as_deref(),
            Some("mdns: endpoint data has no IP addresses"),
            "{}",
            c.name
        );
    }
}

#[test]
fn zone_removal_helper() {
    assert_eq!(without_zone("[fe80::1%en0]:7777"), "[fe80::1]:7777");
    assert_eq!(without_zone("192.0.2.1:7777"), "192.0.2.1:7777");
    assert_eq!(without_zone("[2001:db8::1]:7777"), "[2001:db8::1]:7777");
}
