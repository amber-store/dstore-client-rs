package main

// Family "transport": transport/addrs.json, transport/relay_urls.json,
// transport/ids.json, transport/mdns.json and transport/pool_scripts.json
// (port-notes/transport.md §5), produced by dstore v0.1.11 transport and wire,
// go-iroh v0.2.0 netaddr, relay, key, iroh and iroh/mdns, and Go net/netip.
// Schemas: docs/vectorgen-view.md.
//
// go-iroh builds and parses mDNS packets in unexported functions
// (iroh/mdns/dnsmsg.go). The generator reaches them through go:linkname, with
// mirror types of the unexported structs they take; a version check refuses
// any go-iroh other than v0.2.0, and every announcement is parsed back as a
// layout check.

import (
	"context"
	"crypto/ed25519"
	"encoding/binary"
	"encoding/hex"
	"errors"
	"fmt"
	"log/slog"
	"net/netip"
	"path/filepath"
	"runtime/debug"
	"strings"
	"sync"
	"time"
	_ "unsafe" // go:linkname

	"github.com/amber-store/dstore/transport"
	"github.com/amber-store/dstore/view"
	"github.com/amber-store/dstore/wire"
	"github.com/tmc/go-iroh/dns"
	"github.com/tmc/go-iroh/iroh"
	"github.com/tmc/go-iroh/iroh/mdns"
	"github.com/tmc/go-iroh/key"
	"github.com/tmc/go-iroh/netaddr"
	"github.com/tmc/go-iroh/relay"
)

func init() {
	register("transport", []string{
		"transport/addrs.json",
		"transport/ids.json",
		"transport/mdns.json",
		"transport/pool_scripts.json",
		"transport/relay_urls.json",
	}, genTransport)
}

func genTransport(out string) error {
	if err := tspCheckVersions(); err != nil {
		return err
	}
	for _, f := range []struct {
		name string
		gen  func() (any, error)
	}{
		{"addrs.json", tspAddrs},
		{"relay_urls.json", tspRelayURLs},
		{"ids.json", tspIDs},
		{"mdns.json", tspMDNS},
		{"pool_scripts.json", tspPoolScripts},
	} {
		v, err := f.gen()
		if err != nil {
			return fmt.Errorf("transport/%s: %w", f.name, err)
		}
		if err := writeJSON(filepath.Join(out, "transport", f.name), v); err != nil {
			return err
		}
	}
	return nil
}

// tspCheckVersions refuses module versions the linkname mirrors and the
// vectors were not written against.
func tspCheckVersions() error {
	bi, ok := debug.ReadBuildInfo()
	if !ok {
		return errors.New("no build info: cannot check module versions")
	}
	want := map[string]string{"github.com/tmc/go-iroh": "v0.2.0", "github.com/amber-store/dstore": "v0.1.11"}
	for _, dep := range bi.Deps {
		v, ok := want[dep.Path]
		if !ok {
			continue
		}
		got := dep.Version
		if dep.Replace != nil {
			got = dep.Replace.Path + "@" + dep.Replace.Version
		}
		if got != v {
			return fmt.Errorf("module %s is %s, the transport family needs %s", dep.Path, got, v)
		}
		delete(want, dep.Path)
	}
	for p, v := range want {
		return fmt.Errorf("module %s %s not in the build", p, v)
	}
	return nil
}

func tspStr(s string) *string { return &s }

func tspErrText(err error) *string {
	if err == nil {
		return nil
	}
	return tspStr(err.Error())
}

// ---- go-iroh iroh/mdns internals (dnsmsg.go, mdns.go at v0.2.0) ----

// tspAnnouncementData mirrors mdns.announcementData field for field.
type tspAnnouncementData struct {
	id       key.EndpointID
	port     uint16
	ips      []netip.AddrPort
	relay    string
	userData string
}

// tspDNSQuestion mirrors mdns.dnsQuestion field for field.
type tspDNSQuestion struct {
	name string
	typ  uint16
}

//go:linkname tspMDNSBuildQuery github.com/tmc/go-iroh/iroh/mdns.buildQuery
func tspMDNSBuildQuery(names ...string) ([]byte, error)

//go:linkname tspMDNSBuildAnnouncement github.com/tmc/go-iroh/iroh/mdns.buildAnnouncement
func tspMDNSBuildAnnouncement(service string, data tspAnnouncementData) ([]byte, error)

//go:linkname tspMDNSAnnouncementInfo github.com/tmc/go-iroh/iroh/mdns.(*Discovery).announcementInfo
func tspMDNSAnnouncementInfo(d *mdns.Discovery, data dns.EndpointData) (tspAnnouncementData, error)

//go:linkname tspMDNSParseAnnouncement github.com/tmc/go-iroh/iroh/mdns.parseAnnouncement
func tspMDNSParseAnnouncement(packet []byte, service string) (dns.EndpointInfo, bool)

//go:linkname tspMDNSParseQuestions github.com/tmc/go-iroh/iroh/mdns.parseQuestions
func tspMDNSParseQuestions(packet []byte) ([]tspDNSQuestion, bool)

//go:linkname tspMDNSEndpointLabel github.com/tmc/go-iroh/iroh/mdns.endpointLabel
func tspMDNSEndpointLabel(id key.EndpointID) string

//go:linkname tspMDNSServiceName github.com/tmc/go-iroh/iroh/mdns.serviceName
func tspMDNSServiceName(service string) string

//go:linkname tspMDNSInstanceName github.com/tmc/go-iroh/iroh/mdns.instanceName
func tspMDNSInstanceName(service string, id key.EndpointID) string

//go:linkname tspMDNSHostName github.com/tmc/go-iroh/iroh/mdns.hostName
func tspMDNSHostName(id key.EndpointID) string

// ---- shared fixtures ----

// tspSeedKey is the ed25519 public key of NewKeyFromSeed(data(seed, 32)).
func tspSeedKey(seed uint64) [32]byte {
	var out [32]byte
	copy(out[:], ed25519.NewKeyFromSeed(smData(seed, 32)).Public().(ed25519.PublicKey))
	return out
}

// tspSeed0120 is the seed 01 02 … 20 of port-notes/transport.md §5.6.
func tspSeed0120() [32]byte {
	var seed [32]byte
	for i := range seed {
		seed[i] = byte(i + 1)
	}
	return seed
}

func tspEndpointID(b [32]byte) (key.EndpointID, error) {
	return key.NewEndpointID(b)
}

// ---- transport/addrs.json ----

type tspAddr struct {
	Kind       string  `json:"kind"`
	String     string  `json:"string"`
	RelayURL   *string `json:"relay_url"`
	IP         *string `json:"ip"`
	Zone       *string `json:"zone"`
	Port       *int    `json:"port"`
	CustomID   *U64    `json:"custom_id"`
	CustomData Hex     `json:"custom_data"`
}

type tspParseAddr struct {
	Input      string    `json:"input"`
	OK         bool      `json:"ok"`
	Addr       *tspAddr  `json:"addr"`
	Error      *string   `json:"error"`
	ParseAddrs []tspAddr `json:"parse_addrs"`
}

type tspParseAddrs struct {
	Name   string    `json:"name"`
	Input  []string  `json:"input"`
	Output []tspAddr `json:"output"`
}

type tspAddrPort struct {
	Input  string  `json:"input"`
	OK     bool    `json:"ok"`
	IP     *string `json:"ip"`
	Zone   *string `json:"zone"`
	Port   *int    `json:"port"`
	String *string `json:"string"`
	Error  *string `json:"error"`
}

type tspAddrsFile struct {
	ParseTransportAddr []tspParseAddr  `json:"parse_transport_addr"`
	ParseAddrs         []tspParseAddrs `json:"parse_addrs"`
	ParseAddrPort      []tspAddrPort   `json:"parse_addr_port"`
}

func tspAddrJSON(ta netaddr.TransportAddr) (tspAddr, error) {
	a := tspAddr{Kind: ta.Network(), String: ta.String()}
	switch t := ta.(type) {
	case netaddr.RelayAddr:
		a.RelayURL = tspStr(t.URL.String())
	case netaddr.IPAddr:
		a.IP = tspStr(t.Addr.Addr().WithZone("").String())
		if z := t.Addr.Addr().Zone(); z != "" {
			a.Zone = tspStr(z)
		}
		p := int(t.Addr.Port())
		a.Port = &p
	case netaddr.CustomAddr:
		id := U64(t.ID())
		a.CustomID = &id
		a.CustomData = append(Hex{}, t.Data()...)
	default:
		return a, fmt.Errorf("unknown transport address type %T", ta)
	}
	return a, nil
}

func tspParseAddrsOf(in []string) ([]tspAddr, error) {
	out := []tspAddr{}
	for _, ta := range transport.ParseAddrs(in) {
		a, err := tspAddrJSON(ta)
		if err != nil {
			return nil, err
		}
		out = append(out, a)
	}
	return out, nil
}

var tspAddrInputs = []string{
	// port-notes/transport.md §5.1
	"ip:127.0.0.1:9", "ip:[::1]:9", "ip:[fe80::1%en0]:9", "127.0.0.1:9", "[::1]:9", "[::ffff:1.2.3.4]:5", "ip:[::ffff:1.2.3.4]:5",
	"ip:1.2.3.4", "ip:999.1.1.1:5", "ip:01.2.3.4:5", "ip:1.2.3.4:0", "ip:1.2.3.4:65536",
	"relay:https://use1-1.relay.n0.iroh-canary.iroh.link./", "relay:https://Example.COM", "relay:http://example.com:80", "relay:example.com", "relay:",
	"relay:https://ex ample.com", "relay:https://example.com:443/x?y=1", "mem:0102abcd", "custom:1_abcd", "1_abcd", "abc", "", "http://x", "custom:zz_00", "IP:1.2.3.4:5",
	// published node addresses
	"ip:192.168.1.10:51820", "ip:[2001:db8::1]:4433", "relay:https://euw1-1.relay.n0.iroh-canary.iroh.link./",
	// netip.ParseAddrPort edges
	"ip:[2001:DB8:0:0:0:0:0:1]:1", "ip:[fe80::1%2]:4433", "ip:[fe80::1%]:9", "ip:[::1]", "ip:::1:9", "ip:1.2.3.4:", "ip:1.2.3.4:+1", "ip:1.2.3.4:-1",
	"ip:1.2.3.4:0x10", "ip:1.2.3.4:080", "ip:[1.2.3.4]:5", "ip:1.2.3:5", "ip:[::ffff:1.2.3.4%eth0]:5", "ip: 1.2.3.4:5", "ip:1.2.3.4:5 ", "ip:[::1]:9:9",
	"ip:", "ip", "[fe80::1%en0]:9", "1.2.3.4:5:6", "ip:[::1.2.3.4]:5", "ip:[fe80::1%en0%x]:9", "ip:0.0.0.0:0", "ip:255.255.255.255:65535", "ip:[::]:0",
	"ip:[0:0:0:0:0:ffff:a00:1]:7", "ip:1.2.3.4:5%x", "ip:[::1]:65535", "ip:[::1]:99999",
	// ParseCustomAddr edges
	"custom:ff_", "custom:_00", "custom:1_0", "custom:1_ABCD", "custom:0x1_ab", "custom:10000000000000000_ab", "custom:ffffffffffffffff_00",
	"custom:1_2_3", "custom:custom:1_ab", "1_", "_", "custom:", "custom", "0_", "DEADbeef_CAFE", "custom:+1_ab",
	// ParseRelayURL through ParseTransportAddr
	"relay:https://Example.COM./", "relay:%zz", "relay:https://example.com:abc", "relay:ftp://Host", "relay:mailto:x@y", "relay://host",
	"relay:relay:https://x", "relay:HTTPS://EX.example", "relay:https://[::1]:443", "mem:", "RELAY:https://x",
}

var tspAddrPortInputs = []string{
	"127.0.0.1:9", "[::1]:9", "[fe80::1%en0]:9", "1.2.3.4", "999.1.1.1:5", "01.2.3.4:5", "1.2.3.4:65536", "[::ffff:1.2.3.4]:5", "::1:9", "[::1]",
	"1.2.3.4:", ":9", "[]:9", "[::1%]:9", "1.2.3.4:+1", "1.2.3.4:0x10", "localhost:9", "[1.2.3.4]:5", "1.2.3:5", "[2001:DB8::1]:1", "[::1.2.3.4]:5",
	"[fe80::1%en0%x]:9", "0.0.0.0:0", "255.255.255.255:65535", "[::]:0", "1.2.3.4:5 ", " 1.2.3.4:5", "192.168.1.2:4242", "[2001:db8::1]:4242",
	"[::ffff:10.0.0.1]:1", "[fe80::1%en0]:7", "[fe80::1%2]:4433", "", "1.2.3.4:080", "[0:0:0:0:0:0:0:1]:1", "[::ffff:0:0]:1", "1.2.3.256:1",
	"1.2.3.4.5:1", "[1:2:3:4:5:6:7:8:9]:1", "[1::2::3]:1", "[fe80::1%]:9", "1.2.3.4:-0",
}

func tspAddrs() (any, error) {
	f := tspAddrsFile{}
	for _, s := range tspAddrInputs {
		c := tspParseAddr{Input: s}
		ta, err := netaddr.ParseTransportAddr(s)
		if err != nil {
			c.Error = tspErrText(err)
		} else {
			a, err := tspAddrJSON(ta)
			if err != nil {
				return nil, err
			}
			c.OK, c.Addr = true, &a
		}
		pa, err := tspParseAddrsOf([]string{s})
		if err != nil {
			return nil, err
		}
		c.ParseAddrs = pa
		f.ParseTransportAddr = append(f.ParseTransportAddr, c)
	}
	for _, tc := range []struct {
		name  string
		input []string
	}{
		{"empty", []string{}},
		{"nil", nil},
		{"order_kept_skips_unparseable_no_dedup", []string{"ip:127.0.0.1:9", "bogus", "relay:https://r.example./", "127.0.0.1:9", "custom:1_ab", "mem:01020304", "ip:127.0.0.1:9", "[::1]:9"}},
		{"init_view_node", []string{"ip:192.168.1.10:51820", "ip:[fe80::1]:51820", "relay:https://euw1-1.relay.n0.iroh-canary.iroh.link./"}},
		{"mem_endpoint_addrs", []string{"mem:0102abcd"}},
		{"relay_between_direct", []string{"ip:10.0.0.1:1", "relay:https://a.example./", "ip:10.0.0.2:2", "relay:b.example", "1_ff"}},
	} {
		out, err := tspParseAddrsOf(tc.input)
		if err != nil {
			return nil, err
		}
		in := tc.input
		if in == nil {
			in = []string{}
		}
		f.ParseAddrs = append(f.ParseAddrs, tspParseAddrs{Name: tc.name, Input: in, Output: out})
	}
	for _, s := range tspAddrPortInputs {
		c := tspAddrPort{Input: s}
		ap, err := netip.ParseAddrPort(s)
		if err != nil {
			c.Error = tspErrText(err)
		} else {
			c.OK = true
			c.IP = tspStr(ap.Addr().WithZone("").String())
			if z := ap.Addr().Zone(); z != "" {
				c.Zone = tspStr(z)
			}
			p := int(ap.Port())
			c.Port = &p
			c.String = tspStr(ap.String())
		}
		f.ParseAddrPort = append(f.ParseAddrPort, c)
	}
	return f, nil
}

// ---- transport/relay_urls.json ----

type tspRelayURL struct {
	Input  string  `json:"input"`
	OK     bool    `json:"ok"`
	String *string `json:"string"`
	Error  *string `json:"error"`
}

type tspRelayConfig struct {
	URL      string `json:"url"`
	QUICPort *int   `json:"quic_port"`
}

type tspRelayFile struct {
	ParseRelayURL     []tspRelayURL    `json:"parse_relay_url"`
	DefaultQUICPort   int              `json:"default_quic_port"`
	DefaultMap        []tspRelayConfig `json:"default_map"`
	DefaultRelayAddrs []string         `json:"default_relay_addrs"`
	CustomURLMap      []tspRelayConfig `json:"custom_url_map"`
}

func tspRelayConfigs(m *relay.Map) []tspRelayConfig {
	out := []tspRelayConfig{}
	for _, c := range m.Configs() {
		rc := tspRelayConfig{URL: c.URL.String()}
		if c.QUIC != nil {
			p := int(c.QUIC.Port)
			rc.QUICPort = &p
		}
		out = append(out, rc)
	}
	return out
}

func tspRelayURLs() (any, error) {
	f := tspRelayFile{DefaultQUICPort: relay.DefaultQUICPort}
	for _, s := range []string{
		// port-notes/transport.md §5.2
		"https://Example.COM", "https://example.com:443", "https://example.com/path", "HTTPS://example.com", "relay.example.com",
		"https://example.com?x=1", "https://user@example.com", "https://ex ample.com", ":bad",
		// more net/url and normalisation edges
		"https://use1-1.relay.n0.iroh-canary.iroh.link.", "https://use1-1.relay.n0.iroh-canary.iroh.link./", "http://example.com:80", "ws://Example.com",
		"wss://EX.com:1", "ftp://h", "file:///tmp/x", "mailto:x@y.z", "//host", "", "%zz", "https://example.com:abc", "https://[::1]:443",
		"https://[fe80::1%25en0]:443", "https://Example.COM./", "https://example.com/a b", "https://example.com#frag", "HTTPS://EXAMPLE.COM/PATH",
		"https://user:pass@Host.Example", "http://a b.com", "https://host:99999", "https://example.com/", "https://example.com//", "HtTpS://x.y",
		"custom://Host", "https://example.com?", "https://example.com/%41", "https://%41.com", "https://example.com:", "https://ex%zzample.com",
		"relay:https://x", "https://exämple.com", "http://[::1", "\x7f",
	} {
		c := tspRelayURL{Input: s}
		u, err := netaddr.ParseRelayURL(s)
		if err != nil {
			c.Error = tspErrText(err)
		} else {
			c.OK, c.String = true, tspStr(u.String())
		}
		f.ParseRelayURL = append(f.ParseRelayURL, c)
	}
	f.DefaultMap = tspRelayConfigs(relay.DefaultMap())
	f.DefaultRelayAddrs = []string{}
	for _, u := range relay.DefaultMap().URLs() {
		f.DefaultRelayAddrs = append(f.DefaultRelayAddrs, netaddr.RelayAddr{URL: u}.String())
	}
	var custom []netaddr.RelayURL
	for _, s := range []string{"https://relay.example./", "http://Relay.Example:8080"} {
		u, err := netaddr.ParseRelayURL(s)
		if err != nil {
			return nil, err
		}
		custom = append(custom, u)
	}
	f.CustomURLMap = tspRelayConfigs(relay.ModeCustomURLs(custom...).Map())
	return f, nil
}

// ---- transport/ids.json ----

type tspIDValidation struct {
	Name  string  `json:"name"`
	Bytes Hex     `json:"bytes"`
	Valid bool    `json:"valid"`
	Error *string `json:"error"`
}

type tspIdentity struct {
	Name              string `json:"name"`
	Seed              Hex    `json:"seed"`
	ID                Hex    `json:"id"`
	String            string `json:"string"`
	Short             string `json:"short"`
	Z32               string `json:"z32"`
	MDNSLabel         string `json:"mdns_label"`
	NoCandidatesError string `json:"no_candidates_error"`
}

type tspIDsFile struct {
	EndpointIDs []tspIDValidation `json:"endpoint_ids"`
	Identities  []tspIdentity     `json:"identities"`
}

func tspHex32(h string) ([32]byte, error) {
	var out [32]byte
	b, err := hex.DecodeString(h)
	if err != nil || len(b) != 32 {
		return out, fmt.Errorf("fixture %q is not 32 bytes of hex", h)
	}
	copy(out[:], b)
	return out, nil
}

func tspIDs() (any, error) {
	f := tspIDsFile{}
	add := func(name string, b [32]byte) {
		c := tspIDValidation{Name: name, Bytes: append(Hex{}, b[:]...)}
		if _, err := key.NewEndpointID(b); err != nil {
			c.Error = tspErrText(err)
		} else {
			c.Valid = true
		}
		f.EndpointIDs = append(f.EndpointIDs, c)
	}
	for i := 0; i < 10; i++ {
		var b [32]byte
		b[0] = byte(i)
		add(fmt.Sprintf("first_byte_%d", i), b)
	}
	{
		var b [32]byte
		for i := range b {
			b[i] = 0xff
		}
		add("all_ff", b)
	}
	for _, tc := range []struct{ name, hex string }{
		{"small_order_identity", "0100000000000000000000000000000000000000000000000000000000000000"},
		{"small_order_2", "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f"},
		{"small_order_4_zero", "0000000000000000000000000000000000000000000000000000000000000000"},
		{"small_order_4_sign", "0000000000000000000000000000000000000000000000000000000000000080"},
		{"small_order_8_a", "26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc05"},
		{"small_order_8_b", "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac037a"},
		{"small_order_8_a_sign", "26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc85"},
		{"small_order_8_b_sign", "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac03fa"},
		{"noncanonical_y1_sign", "0100000000000000000000000000000000000000000000000000000000000080"},
		{"noncanonical_y_p", "edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f"},
		{"noncanonical_y_p_plus_1", "eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f"},
		{"noncanonical_y_p_minus_1_sign", "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"},
		{"basepoint", "5866666666666666666666666666666666666666666666666666666666666666"},
		{"basepoint_sign", "58666666666666666666666666666666666666666666666666666666666666e6"},
	} {
		b, err := tspHex32(tc.hex)
		if err != nil {
			return nil, err
		}
		add(tc.name, b)
	}
	for seed := uint64(1); seed <= 4; seed++ {
		k := tspSeedKey(seed)
		add(fmt.Sprintf("ed25519_seed_%d", seed), k)
		k[31] ^= 0x80
		add(fmt.Sprintf("ed25519_seed_%d_sign_flipped", seed), k)
	}
	for i := uint64(0); i < 48; i++ {
		var b [32]byte
		copy(b[:], smData(7000+i, 32))
		add(fmt.Sprintf("splitmix_%d", 7000+i), b)
	}

	seeds := []struct {
		name string
		seed [32]byte
	}{{"seed_01_to_20", tspSeed0120()}}
	for s := uint64(1); s <= 3; s++ {
		var seed [32]byte
		copy(seed[:], smData(s, 32))
		seeds = append(seeds, struct {
			name string
			seed [32]byte
		}{fmt.Sprintf("splitmix_seed_%d", s), seed})
	}
	for _, s := range seeds {
		id := key.NewSecretKey(s.seed).Public().EndpointID()
		b := id.Bytes()
		f.Identities = append(f.Identities, tspIdentity{
			Name: s.name, Seed: append(Hex{}, s.seed[:]...), ID: append(Hex{}, b[:]...), String: id.String(), Short: id.Short(), Z32: id.Z32(),
			MDNSLabel:         tspMDNSEndpointLabel(id),
			NoCandidatesError: fmt.Errorf("transport: no candidate addresses for %s", id.Short()).Error(),
		})
	}
	return f, nil
}

// ---- transport/mdns.json ----

type tspMDNSNames struct {
	ID       Hex    `json:"id"`
	Label    string `json:"label"`
	Service  string `json:"service"`
	Instance string `json:"instance"`
	Host     string `json:"host"`
}

type tspMDNSQuery struct {
	Name   string `json:"name"`
	ID     Hex    `json:"id"`
	Packet Hex    `json:"packet"`
}

type tspMDNSInfo struct {
	Port     int      `json:"port"`
	IPs      []string `json:"ips"`
	Relay    string   `json:"relay"`
	UserData string   `json:"user_data"`
}

type tspMDNSAnnouncement struct {
	Name     string       `json:"name"`
	ID       Hex          `json:"id"`
	Service  string       `json:"service"`
	Addrs    []string     `json:"addrs"`
	UserData *string      `json:"user_data"`
	Info     *tspMDNSInfo `json:"info"`
	Error    *string      `json:"error"`
	Packet   Hex          `json:"packet"`
}

type tspMDNSParse struct {
	Name     string   `json:"name"`
	Service  string   `json:"service"`
	Packet   Hex      `json:"packet"`
	OK       bool     `json:"ok"`
	ID       Hex      `json:"id"`
	IPs      []string `json:"ips"`
	Relay    *string  `json:"relay"`
	UserData *string  `json:"user_data"`
	Addrs    []string `json:"addrs"`
}

type tspMDNSQuestion struct {
	Name string `json:"name"`
	Type int    `json:"type"`
}

type tspMDNSQuestions struct {
	Name      string            `json:"name"`
	Packet    Hex               `json:"packet"`
	OK        bool              `json:"ok"`
	Questions []tspMDNSQuestion `json:"questions"`
}

type tspMDNSFile struct {
	ServiceName   string                `json:"service_name"`
	Names         []tspMDNSNames        `json:"names"`
	Queries       []tspMDNSQuery        `json:"queries"`
	Announcements []tspMDNSAnnouncement `json:"announcements"`
	Parse         []tspMDNSParse        `json:"parse"`
	Questions     []tspMDNSQuestions    `json:"questions"`
}

// tspDNS writes hand-built DNS packets: the swarm-discovery layout, name
// compression and malformed packets that go-iroh never builds.
type tspDNS struct{ b []byte }

func (w *tspDNS) u16(v uint16) { w.b = binary.BigEndian.AppendUint16(w.b, v) }
func (w *tspDNS) u32(v uint32) { w.b = binary.BigEndian.AppendUint32(w.b, v) }

func (w *tspDNS) header(flags, qd, an, ns, ar uint16) {
	w.u16(0)
	w.u16(flags)
	w.u16(qd)
	w.u16(an)
	w.u16(ns)
	w.u16(ar)
}

func (w *tspDNS) labels(n string) {
	for _, l := range strings.Split(strings.TrimSuffix(n, "."), ".") {
		if l == "" {
			continue
		}
		w.b = append(w.b, byte(len(l)))
		w.b = append(w.b, l...)
	}
}

// name writes n uncompressed and returns its offset.
func (w *tspDNS) name(n string) int {
	off := len(w.b)
	w.labels(n)
	w.b = append(w.b, 0)
	return off
}

// namePtr writes the labels of prefix, then a compression pointer to ptr, and
// returns the name's offset.
func (w *tspDNS) namePtr(prefix string, ptr int) int {
	off := len(w.b)
	w.labels(prefix)
	w.u16(0xc000 | uint16(ptr))
	return off
}

func (w *tspDNS) rrHead(typ uint16, ttl uint32) {
	w.u16(typ)
	w.u16(1)
	w.u32(ttl)
}

// rdata writes a length-prefixed rdata region filled by f.
func (w *tspDNS) rdata(f func()) {
	start := len(w.b)
	w.u16(0)
	f()
	binary.BigEndian.PutUint16(w.b[start:], uint16(len(w.b)-start-2))
}

func (w *tspDNS) txt(values ...string) {
	for _, v := range values {
		w.b = append(w.b, byte(len(v)))
		w.b = append(w.b, v...)
	}
}

func (w *tspDNS) ip(a netip.Addr) {
	if a.Is4() {
		b := a.As4()
		w.b = append(w.b, b[:]...)
		return
	}
	b := a.As16()
	w.b = append(w.b, b[:]...)
}

func tspIPType(a netip.Addr) uint16 {
	if a.Is4() {
		return 1
	}
	return 28
}

// tspRec is one resource record of a hand-built packet.
type tspRec struct {
	section int // 0 answer, 1 authority, 2 additional
	name    string
	typ     uint16
	ttl     uint32
	rdata   func(w *tspDNS)
}

func tspSRV(port uint16, target string) func(w *tspDNS) {
	return func(w *tspDNS) {
		w.u16(0)
		w.u16(0)
		w.u16(port)
		w.name(target)
	}
}

func tspTXT(values ...string) func(w *tspDNS) { return func(w *tspDNS) { w.txt(values...) } }
func tspPTR(target string) func(w *tspDNS)    { return func(w *tspDNS) { w.name(target) } }
func tspIPRData(a netip.Addr) func(w *tspDNS) { return func(w *tspDNS) { w.ip(a) } }

// tspPacket builds an uncompressed packet with the given questions (PTR, IN)
// and records, adding ancountDelta to ANCOUNT and appending trailer.
func tspPacket(flags uint16, questions []tspDNSQuestion, recs []tspRec, ancountDelta int, trailer []byte) []byte {
	var counts [3]int
	for _, r := range recs {
		counts[r.section]++
	}
	w := &tspDNS{}
	w.header(flags, uint16(len(questions)), uint16(counts[0]+ancountDelta), uint16(counts[1]), uint16(counts[2]))
	for _, q := range questions {
		w.name(q.name)
		w.u16(q.typ)
		w.u16(1)
	}
	for sec := 0; sec < 3; sec++ {
		for _, r := range recs {
			if r.section != sec {
				continue
			}
			w.name(r.name)
			w.rrHead(r.typ, r.ttl)
			w.rdata(func() { r.rdata(w) })
		}
	}
	return append(w.b, trailer...)
}

func tspParseCase(name, service string, packet []byte) tspMDNSParse {
	c := tspMDNSParse{Name: name, Service: service, Packet: append(Hex{}, packet...)}
	info, ok := tspMDNSParseAnnouncement(packet, service)
	if !ok {
		return c
	}
	c.OK = true
	b := info.ID.Bytes()
	c.ID = append(Hex{}, b[:]...)
	c.IPs = []string{}
	for _, ap := range info.Data.IPAddrs() {
		c.IPs = append(c.IPs, ap.String())
	}
	if rs := info.Data.RelayURLs(); len(rs) > 0 {
		c.Relay = tspStr(rs[0].String())
	}
	if u := info.Data.UserData(); u != nil {
		c.UserData = tspStr(u.String())
	}
	c.Addrs = []string{}
	for _, a := range info.Data.Addrs() {
		c.Addrs = append(c.Addrs, a.String())
	}
	return c
}

func tspMDNS() (any, error) {
	f := tspMDNSFile{ServiceName: mdns.DefaultServiceName}
	service := mdns.DefaultServiceName
	discard := slog.New(slog.DiscardHandler)

	id0, err := tspEndpointID(key.NewSecretKey(tspSeed0120()).Public().Bytes())
	if err != nil {
		return nil, err
	}
	id1, err := tspEndpointID(tspSeedKey(1))
	if err != nil {
		return nil, err
	}
	id2, err := tspEndpointID(tspSeedKey(2))
	if err != nil {
		return nil, err
	}
	for _, id := range []key.EndpointID{id0, id1, id2} {
		b := id.Bytes()
		f.Names = append(f.Names, tspMDNSNames{
			ID: append(Hex{}, b[:]...), Label: tspMDNSEndpointLabel(id), Service: tspMDNSServiceName(service),
			Instance: tspMDNSInstanceName(service, id), Host: tspMDNSHostName(id),
		})
		q, err := tspMDNSBuildQuery(tspMDNSServiceName(service), tspMDNSInstanceName(service, id))
		if err != nil {
			return nil, err
		}
		f.Queries = append(f.Queries, tspMDNSQuery{Name: "query_" + id.Short(), ID: append(Hex{}, b[:]...), Packet: q})
	}

	ip := func(s string) netaddr.TransportAddr { return netaddr.IPAddr{Addr: netip.MustParseAddrPort(s)} }
	rl := func(s string) (netaddr.TransportAddr, error) {
		u, err := netaddr.ParseRelayURL(s)
		if err != nil {
			return nil, err
		}
		return netaddr.RelayAddr{URL: u}, nil
	}
	use1, err := rl("https://use1-1.relay.n0.iroh-canary.iroh.link./")
	if err != nil {
		return nil, err
	}
	relay249, err := rl("https://relay.example/" + strings.Repeat("p", 249-len("https://relay.example/")))
	if err != nil {
		return nil, err
	}
	relay250, err := rl("https://relay.example/" + strings.Repeat("p", 250-len("https://relay.example/")))
	if err != nil {
		return nil, err
	}
	relayB, err := rl("https://relay.example./")
	if err != nil {
		return nil, err
	}
	if len(relay249.(netaddr.RelayAddr).URL.String()) != 249 || len(relay250.(netaddr.RelayAddr).URL.String()) != 250 {
		return nil, errors.New("relay URL fixtures do not have 249 and 250 bytes")
	}
	custom, err := netaddr.ParseCustomAddr("1_ab")
	if err != nil {
		return nil, err
	}
	ud := func(s string) *string { return &s }

	type annCase struct {
		name     string
		id       key.EndpointID
		service  string
		addrs    []netaddr.TransportAddr
		userData *string
	}
	annCases := []annCase{
		{"go_example", id0, service, []netaddr.TransportAddr{ip("192.168.1.2:4242"), ip("[2001:db8::1]:4242"), ip("10.0.0.1:5555"), use1}, nil},
		{"go_example_user_data", id0, service, []netaddr.TransportAddr{ip("192.168.1.2:4242"), ip("[2001:db8::1]:4242"), ip("10.0.0.1:5555"), use1}, ud("dstore")},
		{"single_ipv4", id1, service, []netaddr.TransportAddr{ip("192.0.2.1:7777")}, nil},
		{"ipv4_mapped_unmapped", id1, service, []netaddr.TransportAddr{ip("[::ffff:10.0.0.1]:7777"), ip("10.0.0.1:7777")}, nil},
		{"ipv6_zone", id1, service, []netaddr.TransportAddr{ip("[fe80::1%en0]:7777")}, nil},
		{"lowest_port_breaks_tie", id1, service, []netaddr.TransportAddr{ip("192.0.2.1:9999"), ip("192.0.2.2:7777")}, nil},
		{"majority_port", id1, service, []netaddr.TransportAddr{ip("192.0.2.1:1111"), ip("192.0.2.2:7777"), ip("[2001:db8::1]:7777")}, nil},
		{"relay_249_bytes_kept", id2, service, []netaddr.TransportAddr{ip("192.0.2.1:7777"), relay249}, nil},
		{"relay_250_bytes_dropped", id2, service, []netaddr.TransportAddr{ip("192.0.2.1:7777"), relay250}, nil},
		{"user_data_245_bytes", id2, service, []netaddr.TransportAddr{ip("192.0.2.1:7777")}, ud(strings.Repeat("u", 245))},
		{"relay_first_then_ip", id2, service, []netaddr.TransportAddr{relayB, ip("192.0.2.9:4433")}, ud("lan")},
		{"custom_addr_ignored", id2, service, []netaddr.TransportAddr{custom, ip("192.0.2.1:7777")}, nil},
		{"many_ips", id2, service, []netaddr.TransportAddr{ip("10.0.0.1:4242"), ip("10.0.0.2:4242"), ip("[2001:db8::2]:4242"), ip("172.16.0.1:4242"), ip("[2001:db8::3]:4242"), ip("192.168.0.1:4242")}, nil},
		{"port_zero", id2, service, []netaddr.TransportAddr{ip("192.0.2.1:0")}, nil},
		{"relay_only", id2, service, []netaddr.TransportAddr{use1}, nil},
		{"no_addrs", id2, service, nil, ud("x")},
		{"other_service", id1, "other", []netaddr.TransportAddr{ip("192.0.2.1:7777")}, nil},
	}
	var goExample []byte
	for _, ac := range annCases {
		b := ac.id.Bytes()
		a := tspMDNSAnnouncement{Name: ac.name, ID: append(Hex{}, b[:]...), Service: ac.service, Addrs: []string{}, UserData: ac.userData}
		data := dns.NewEndpointData(ac.addrs...)
		for _, ta := range data.Addrs() {
			a.Addrs = append(a.Addrs, ta.String())
		}
		if ac.userData != nil {
			u, err := dns.NewUserData(*ac.userData)
			if err != nil {
				return nil, err
			}
			data.SetUserData(&u)
		}
		d := mdns.New(ac.id, mdns.WithServiceName(ac.service), mdns.WithLogger(discard))
		info, err := tspMDNSAnnouncementInfo(d, data)
		if err != nil {
			a.Error = tspErrText(err)
			f.Announcements = append(f.Announcements, a)
			continue
		}
		mi := &tspMDNSInfo{Port: int(info.port), IPs: []string{}, Relay: info.relay, UserData: info.userData}
		for _, ap := range info.ips {
			mi.IPs = append(mi.IPs, ap.String())
		}
		a.Info = mi
		packet, err := tspMDNSBuildAnnouncement(ac.service, info)
		if err != nil {
			a.Error = tspErrText(err)
			f.Announcements = append(f.Announcements, a)
			continue
		}
		a.Packet = packet
		f.Announcements = append(f.Announcements, a)
		// Layout check of the mirror types: a packet with a port parses back to its id and addresses.
		if back, ok := tspMDNSParseAnnouncement(packet, ac.service); info.port != 0 && (!ok || back.ID != ac.id || len(back.Data.IPAddrs()) == 0) {
			return nil, fmt.Errorf("announcement %s does not parse back", ac.name)
		}
		f.Parse = append(f.Parse, tspParseCase("announcement_"+ac.name, service, packet))
		if ac.name == "go_example" {
			goExample = packet
		}
	}
	if goExample == nil {
		return nil, errors.New("no go_example announcement")
	}

	label0 := tspMDNSEndpointLabel(id0)
	label1 := tspMDNSEndpointLabel(id1)
	svc := tspMDNSServiceName(service)
	inst0 := label0 + "." + svc
	inst1 := label1 + "." + svc
	host0 := label0 + ".local"
	a4 := netip.MustParseAddr("192.168.1.2")
	a6 := netip.MustParseAddr("2001:db8::1")
	answers := func(inst, host string, port uint16, txt []string, ips ...netip.Addr) []tspRec {
		recs := []tspRec{
			{0, svc, 12, 120, tspPTR(inst)},
			{0, inst, 33, 120, tspSRV(port, host)},
			{0, inst, 16, 120, tspTXT(txt...)},
		}
		for _, a := range ips {
			recs = append(recs, tspRec{0, host, tspIPType(a), 120, tspIPRData(a)})
		}
		return recs
	}
	for _, n := range []int{11, 12, len(goExample) / 2, len(goExample) - 1} {
		f.Parse = append(f.Parse, tspParseCase(fmt.Sprintf("truncated_%d_of_%d", n, len(goExample)), service, goExample[:n]))
	}
	f.Parse = append(f.Parse, tspParseCase("trailing_garbage", service, append(append([]byte{}, goExample...), 0xde, 0xad)))

	// swarm-discovery layout (swarm-discovery-0.6.3/src/sender.rs:165-205): SRV and TXT in answers,
	// A/AAAA in additionals, target <label>-<port>.local, TTL 0.
	swarmTarget := fmt.Sprintf("%s-%d.local", label0, 4242)
	f.Parse = append(f.Parse, tspParseCase("swarm_discovery_layout", service, tspPacket(0x8400, nil, []tspRec{
		{0, inst0, 33, 0, tspSRV(4242, swarmTarget)},
		{0, inst0, 16, 0, tspTXT("relay=https://relay.example./", "user-data=swarm")},
		{2, swarmTarget, 1, 0, tspIPRData(a4)},
		{2, swarmTarget, 28, 0, tspIPRData(a6)},
	}, 0, nil)))
	{
		// The same with name compression, as hickory-proto encodes it.
		w := &tspDNS{}
		w.header(0x8400, 0, 2, 0, 2)
		instOff := w.name(inst0)
		localOff := instOff + 1 + len(label0) + 1 + 7 + 1 + 4
		w.rrHead(33, 0)
		var targetOff int
		w.rdata(func() {
			w.u16(0)
			w.u16(0)
			w.u16(4242)
			targetOff = w.namePtr(fmt.Sprintf("%s-%d", label0, 4242), localOff)
		})
		w.namePtr("", instOff)
		w.rrHead(16, 0)
		w.rdata(func() { w.txt("relay=https://relay.example./") })
		w.namePtr("", targetOff)
		w.rrHead(1, 0)
		w.rdata(func() { w.ip(a4) })
		w.namePtr("", targetOff)
		w.rrHead(28, 0)
		w.rdata(func() { w.ip(a6) })
		f.Parse = append(f.Parse, tspParseCase("swarm_discovery_layout_compressed", service, w.b))
	}
	{
		// go-iroh's layout with name compression.
		w := &tspDNS{}
		w.header(0x8400, 0, 4, 0, 0)
		svcOff := w.name(svc)
		localOff := svcOff + 1 + 7 + 1 + 4
		w.rrHead(12, 120)
		var instOff int
		w.rdata(func() { instOff = w.namePtr(label0, svcOff) })
		w.namePtr("", instOff)
		w.rrHead(33, 120)
		var hostOff int
		w.rdata(func() {
			w.u16(0)
			w.u16(0)
			w.u16(4433)
			hostOff = w.namePtr(label0, localOff)
		})
		w.namePtr("", instOff)
		w.rrHead(16, 120)
		w.rdata(func() { w.txt("user-data=compressed") })
		w.namePtr("", hostOff)
		w.rrHead(1, 120)
		w.rdata(func() { w.ip(netip.MustParseAddr("10.1.2.3")) })
		f.Parse = append(f.Parse, tspParseCase("go_layout_compressed", service, w.b))
	}
	{
		// Pointer chains ahead of the SRV target: readName follows at most 32 steps (pointers and labels together).
		chain := func(hops int) []byte {
			w := &tspDNS{}
			w.header(0x8400, 0, 2, 0, 1)
			w.name(inst0)
			w.rrHead(33, 120)
			// The target is a pointer into a chain appended after the records.
			var ptrPos int
			w.rdata(func() {
				w.u16(0)
				w.u16(0)
				w.u16(4242)
				ptrPos = len(w.b)
				w.u16(0xc000)
			})
			w.name(inst0)
			w.rrHead(16, 120)
			w.rdata(func() { w.txt() })
			w.name(host0)
			w.rrHead(1, 120)
			w.rdata(func() { w.ip(a4) })
			start := len(w.b)
			for i := 0; i < hops; i++ {
				w.u16(0xc000 | uint16(start+2*(i+1)))
			}
			w.name(host0)
			binary.BigEndian.PutUint16(w.b[ptrPos:], 0xc000|uint16(start))
			return w.b
		}
		f.Parse = append(f.Parse,
			tspParseCase("pointer_chain_28", service, chain(28)),
			tspParseCase("pointer_chain_29", service, chain(29)),
			tspParseCase("pointer_chain_30", service, chain(30)),
		)
		w := &tspDNS{}
		w.header(0x8400, 0, 1, 0, 0)
		off := len(w.b)
		w.u16(0xc000 | uint16(off))
		w.rrHead(33, 120)
		w.rdata(func() { w.u16(0); w.u16(0); w.u16(1) })
		f.Parse = append(f.Parse, tspParseCase("compression_loop", service, w.b))
	}
	f.Parse = append(f.Parse,
		tspParseCase("srv_port_zero", service, tspPacket(0x8400, nil, answers(inst0, host0, 0, nil, a4), 0, nil)),
		tspParseCase("no_address_records", service, tspPacket(0x8400, nil, answers(inst0, host0, 4242, []string{"relay=https://relay.example./"}), 0, nil)),
		tspParseCase("ptr_only", service, tspPacket(0x8400, nil, []tspRec{{0, svc, 12, 120, tspPTR(inst0)}}, 0, nil)),
		tspParseCase("target_case_mismatch", service, tspPacket(0x8400, nil, append(answers(inst0, host0, 4242, nil)[:3], tspRec{0, strings.ToUpper(host0), 1, 120, tspIPRData(a4)}), 0, nil)),
		tspParseCase("service_suffix_uppercase", service, tspPacket(0x8400, nil, answers(label0+"."+strings.ToUpper(svc), host0, 4242, nil, a4), 0, nil)),
		tspParseCase("label_uppercase", service, tspPacket(0x8400, nil, answers(strings.ToUpper(label0)+"."+svc, host0, 4242, nil, a4), 0, nil)),
		tspParseCase("label_hex_id_64_bytes", service, tspPacket(0x8400, nil, answers(id0.String()+"."+svc, host0, 4242, nil, a4), 0, nil)),
		tspParseCase("label_z32_id", service, tspPacket(0x8400, nil, answers(id0.Z32()+"."+svc, host0, 4242, nil, a4), 0, nil)),
		tspParseCase("label_invalid_point", service, tspPacket(0x8400, nil, answers("a4aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa."+svc, host0, 4242, nil, a4), 0, nil)),
		tspParseCase("two_instances_one_valid", service, tspPacket(0x8400, nil, append(answers(inst1, label1+".local", 0, nil, a6), answers(inst0, host0, 4242, nil, a4)...), 0, nil)),
		tspParseCase("records_in_authority_and_additional", service, tspPacket(0x8400, nil, []tspRec{
			{1, inst0, 33, 120, tspSRV(4242, host0)}, {1, inst0, 16, 120, tspTXT("user-data=auth")}, {2, host0, 1, 120, tspIPRData(a4)},
		}, 0, nil)),
		tspParseCase("with_questions", service, tspPacket(0x8400, []tspDNSQuestion{{svc, 12}}, answers(inst0, host0, 4242, nil, a4), 0, nil)),
		tspParseCase("query_flags_with_answers", service, tspPacket(0x0000, nil, answers(inst0, host0, 4242, nil, a4), 0, nil)),
		tspParseCase("txt_relay_last_wins", service, tspPacket(0x8400, nil, answers(inst0, host0, 4242, []string{"relay=https://a.example./", "relay=https://b.example./"}, a4), 0, nil)),
		tspParseCase("txt_relay_unparseable", service, tspPacket(0x8400, nil, answers(inst0, host0, 4242, []string{"relay=%zz", "user-data=ok"}, a4), 0, nil)),
		tspParseCase("txt_no_equals_and_overrun", service, tspPacket(0x8400, nil, append(answers(inst0, host0, 4242, nil, a4)[:2], append([]tspRec{{0, inst0, 16, 120, func(w *tspDNS) {
			w.txt("noequals", "user-data=first")
			w.b = append(w.b, 40, 'u')
		}}}, answers(inst0, host0, 4242, nil, a4)[3:]...)...), 0, nil)),
		tspParseCase("txt_empty_value_and_extra_equals", service, tspPacket(0x8400, nil, answers(inst0, host0, 4242, []string{"relay=", "user-data=a=b"}, a4), 0, nil)),
		tspParseCase("duplicate_a_records", service, tspPacket(0x8400, nil, answers(inst0, host0, 4242, nil, a4, a4, a6), 0, nil)),
		tspParseCase("aaaa_ipv4_mapped", service, tspPacket(0x8400, nil, answers(inst0, host0, 4242, nil, netip.MustParseAddr("::ffff:1.2.3.4")), 0, nil)),
		tspParseCase("ancount_too_large", service, tspPacket(0x8400, nil, answers(inst0, host0, 4242, nil, a4), 1, nil)),
		tspParseCase("rdata_overrun", service, func() []byte {
			p := tspPacket(0x8400, nil, answers(inst0, host0, 4242, nil, a4), 0, nil)
			binary.BigEndian.PutUint16(p[len(p)-6:], 200)
			return p
		}()),
		tspParseCase("unsupported_label_type", service, tspPacket(0x8400, nil, []tspRec{{0, svc, 12, 120, func(w *tspDNS) { w.b = append(w.b, 0x40, 0x00) }}}, 0, nil)),
		tspParseCase("srv_rdata_short", service, tspPacket(0x8400, nil, []tspRec{{0, inst0, 33, 120, func(w *tspDNS) { w.u16(0); w.u16(0) }}}, 0, nil)),
		tspParseCase("empty_packet", service, []byte{}),
	)

	for _, qc := range []struct {
		name   string
		packet func() ([]byte, error)
	}{
		{"query_service_and_instance", func() ([]byte, error) { return tspMDNSBuildQuery(svc, inst0) }},
		{"query_service_only", func() ([]byte, error) { return tspMDNSBuildQuery(svc) }},
		{"typed_srv_service", func() ([]byte, error) { return tspPacket(0, []tspDNSQuestion{{svc, 33}}, nil, 0, nil), nil }},
		{"typed_any_instance", func() ([]byte, error) { return tspPacket(0, []tspDNSQuestion{{inst0, 255}}, nil, 0, nil), nil }},
		{"typed_txt_and_a", func() ([]byte, error) {
			return tspPacket(0, []tspDNSQuestion{{inst0, 16}, {host0, 1}}, nil, 0, nil), nil
		}},
		{"response_flag", func() ([]byte, error) { return tspPacket(0x8400, []tspDNSQuestion{{svc, 12}}, nil, 0, nil), nil }},
		{"no_questions", func() ([]byte, error) { return tspPacket(0, nil, nil, 0, nil), nil }},
		{"truncated_question", func() ([]byte, error) {
			p := tspPacket(0, []tspDNSQuestion{{svc, 12}}, nil, 0, nil)
			return p[:len(p)-2], nil
		}},
		{"compressed_question_name", func() ([]byte, error) {
			w := &tspDNS{}
			w.header(0, 2, 0, 0, 0)
			svcOff := w.name(svc)
			w.u16(12)
			w.u16(1)
			w.namePtr(label0, svcOff)
			w.u16(255)
			w.u16(1)
			return w.b, nil
		}},
		{"announcement_packet", func() ([]byte, error) { return goExample, nil }},
		{"short_header", func() ([]byte, error) { return []byte{0, 0, 0, 0, 0, 1}, nil }},
	} {
		p, err := qc.packet()
		if err != nil {
			return nil, err
		}
		c := tspMDNSQuestions{Name: qc.name, Packet: append(Hex{}, p...)}
		qs, ok := tspMDNSParseQuestions(p)
		c.OK = ok
		if ok {
			c.Questions = []tspMDNSQuestion{}
			for _, q := range qs {
				c.Questions = append(c.Questions, tspMDNSQuestion{Name: q.name, Type: int(q.typ)})
			}
		}
		f.Questions = append(f.Questions, c)
	}
	return f, nil
}

// ---- transport/pool_scripts.json ----

type tspPoolMsg struct {
	Type         int    `json:"type"`
	Epoch        U64    `json:"epoch"`
	Code         string `json:"code"`
	Text         string `json:"text"`
	RetryAfterMS I64    `json:"retry_after_ms"`
}

type tspPoolRemote struct {
	Code         string `json:"code"`
	Text         string `json:"text"`
	RetryAfterNS I64    `json:"retry_after_ns"`
}

type tspPoolPath struct {
	Direct bool `json:"direct"`
	RTTNS  I64  `json:"rtt_ns"`
}

type tspPoolServer struct {
	Mode         string `json:"mode"`
	Code         string `json:"code,omitempty"`
	Text         string `json:"text,omitempty"`
	RetryAfterMS I64    `json:"retry_after_ms,omitempty"`
}

type tspPoolNode struct {
	Name   string         `json:"name"`
	ID     Hex            `json:"id"`
	Bound  bool           `json:"bound"`
	ALPNs  []string       `json:"alpns"`
	Server *tspPoolServer `json:"server"`
}

type tspPoolStep struct {
	Op           string         `json:"op"`
	Repeat       int            `json:"repeat,omitempty"`
	Peer         string         `json:"peer,omitempty"`
	ALPN         string         `json:"alpn,omitempty"`
	TimeoutMS    int            `json:"timeout_ms,omitempty"`
	MS           int            `json:"ms,omitempty"`
	Down         *bool          `json:"down,omitempty"`
	A            string         `json:"a,omitempty"`
	B            string         `json:"b,omitempty"`
	Cut          *bool          `json:"cut,omitempty"`
	Text         string         `json:"text,omitempty"`
	Request      *tspPoolMsg    `json:"request,omitempty"`
	Stream       *int           `json:"stream,omitempty"`
	ServerConn   *int           `json:"server_conn,omitempty"`
	ServerStream *int           `json:"server_stream,omitempty"`
	Data         Hex            `json:"data,omitempty"`
	BufLen       int            `json:"buf_len,omitempty"`
	OK           bool           `json:"ok"`
	Error        *string        `json:"error,omitempty"`
	Conn         *int           `json:"conn,omitempty"`
	N            *int           `json:"n,omitempty"`
	Read         Hex            `json:"read,omitempty"`
	Reply        *tspPoolMsg    `json:"reply,omitempty"`
	Remote       *tspPoolRemote `json:"remote,omitempty"`
	Path         *tspPoolPath   `json:"path,omitempty"`
	DialAttempts *int           `json:"dial_attempts,omitempty"`
	Dials        *int           `json:"dials,omitempty"`
}

type tspPoolScript struct {
	Name        string        `json:"name"`
	Description string        `json:"description"`
	PerPeer     int           `json:"per_peer"`
	Nodes       []tspPoolNode `json:"nodes"`
	Steps       []tspPoolStep `json:"steps"`
}

type tspDuration struct {
	NS        I64    `json:"ns"`
	RoundedNS I64    `json:"rounded_ns"`
	String    string `json:"string"`
}

type tspErrorText struct {
	Name string `json:"name"`
	Text string `json:"text"`
}

type tspPoolFile struct {
	Scripts    []tspPoolScript `json:"scripts"`
	RTTRound   []tspDuration   `json:"rtt_round"`
	ErrorTexts []tspErrorText  `json:"error_texts"`
}

// tspScriptEndpoint wraps the client's MemEndpoint: it counts dial attempts,
// numbers successful dials, and can fail the next dial with a given text.
type tspScriptEndpoint struct {
	transport.Endpoint
	mu       sync.Mutex
	attempts int
	conns    []transport.Conn
	failNext string
}

func (e *tspScriptEndpoint) Dial(ctx context.Context, id view.NodeID, addrs []string, alpn string) (transport.Conn, error) {
	e.mu.Lock()
	e.attempts++
	fail := e.failNext
	e.failNext = ""
	e.mu.Unlock()
	if fail != "" {
		return nil, errors.New(fail)
	}
	c, err := e.Endpoint.Dial(ctx, id, addrs, alpn)
	if err != nil {
		return nil, err
	}
	e.mu.Lock()
	e.conns = append(e.conns, c)
	e.mu.Unlock()
	return c, nil
}

func (e *tspScriptEndpoint) index(c transport.Conn) int {
	e.mu.Lock()
	defer e.mu.Unlock()
	for i, x := range e.conns {
		if x == c {
			return i
		}
	}
	return -1
}

func (e *tspScriptEndpoint) counts() (int, int) {
	e.mu.Lock()
	defer e.mu.Unlock()
	return e.attempts, len(e.conns)
}

type tspHarness struct {
	net      *transport.Network
	ids      map[string]view.NodeID
	eps      map[string]*transport.MemEndpoint
	ep       *tspScriptEndpoint
	pool     *transport.Pool
	streams  []transport.Stream
	sconns   []transport.Conn
	sstreams []transport.Stream
}

const tspServerTimeout = 2 * time.Second

func tspServe(ctx context.Context, ep *transport.MemEndpoint, srv tspPoolServer) {
	for {
		c, err := ep.Accept(ctx)
		if err != nil {
			return
		}
		go func() {
			for {
				s, err := c.AcceptStream(ctx)
				if err != nil {
					return
				}
				go tspHandle(s, srv)
			}
		}()
	}
}

func tspHandle(s transport.Stream, srv tspPoolServer) {
	defer wire.CloseStream(s)
	req, err := wire.ReadMsg(s)
	if err != nil {
		return
	}
	switch srv.Mode {
	case "pong":
		_ = wire.WriteMsg(s, &wire.Msg{Type: wire.TPong, Epoch: req.Epoch + 1})
	case "err":
		m := wire.ErrMsg(srv.Code, srv.Text)
		m.RetryAfter = int64(srv.RetryAfterMS)
		_ = wire.WriteMsg(s, m)
	}
}

func (h *tspHarness) stepCtx(st *tspPoolStep, fallback time.Duration) (context.Context, context.CancelFunc) {
	if st.TimeoutMS > 0 {
		return context.WithTimeout(context.Background(), time.Duration(st.TimeoutMS)*time.Millisecond)
	}
	if fallback > 0 {
		return context.WithTimeout(context.Background(), fallback)
	}
	return context.WithCancel(context.Background())
}

func tspOutcome(st *tspPoolStep, err error) {
	st.OK = err == nil
	st.Error = tspErrText(err)
}

func (h *tspHarness) counts(st *tspPoolStep) {
	a, d := h.ep.counts()
	st.DialAttempts, st.Dials = &a, &d
}

func (h *tspHarness) peer(name string) (view.NodeID, error) {
	id, ok := h.ids[name]
	if !ok {
		return id, fmt.Errorf("unknown node %q", name)
	}
	return id, nil
}

func tspIndex(i *int, n int, what string) (int, error) {
	if i == nil || *i < 0 || *i >= n {
		return 0, fmt.Errorf("bad %s index", what)
	}
	return *i, nil
}

func (h *tspHarness) run(st *tspPoolStep) error {
	switch st.Op {
	case "get", "open", "call", "drop", "path":
		id, err := h.peer(st.Peer)
		if err != nil {
			return err
		}
		switch st.Op {
		case "get":
			ctx, cancel := h.stepCtx(st, 0)
			c, err := h.pool.Get(ctx, id, st.ALPN)
			cancel()
			tspOutcome(st, err)
			if err == nil {
				idx := h.ep.index(c)
				if idx < 0 {
					return errors.New("get returned a connection the endpoint never dialed")
				}
				st.Conn = &idx
			}
			h.counts(st)
		case "open":
			ctx, cancel := h.stepCtx(st, 0)
			s, err := h.pool.Open(ctx, id, st.ALPN)
			cancel()
			tspOutcome(st, err)
			if err == nil {
				idx := len(h.streams)
				h.streams = append(h.streams, s)
				st.Stream = &idx
			}
			h.counts(st)
		case "call":
			if st.Request == nil {
				return errors.New("call without a request")
			}
			ctx, cancel := h.stepCtx(st, 0)
			reply, err := h.pool.Call(ctx, id, st.ALPN, &wire.Msg{Type: st.Request.Type, Epoch: uint64(st.Request.Epoch)})
			cancel()
			tspOutcome(st, err)
			if reply != nil {
				st.Reply = &tspPoolMsg{Type: reply.Type, Epoch: U64(reply.Epoch), Code: reply.Code, Text: reply.Text, RetryAfterMS: I64(reply.RetryAfter)}
			}
			if we, ok := wire.AsError(err); ok {
				st.Remote = &tspPoolRemote{Code: we.Code, Text: we.Text, RetryAfterNS: I64(we.RetryAfter)}
			}
			h.counts(st)
		case "drop":
			h.pool.Drop(id, st.ALPN)
			st.OK = true
		case "path":
			p, ok := h.pool.Path(id, st.ALPN)
			st.OK = ok
			if ok {
				st.Path = &tspPoolPath{Direct: p.Direct, RTTNS: I64(p.RTT)}
			}
		}
	case "close":
		h.pool.Close()
		st.OK = true
	case "set_down":
		id, err := h.peer(st.Peer)
		if err != nil || st.Down == nil {
			return errors.Join(err, errors.New("set_down needs peer and down"))
		}
		h.net.SetDown(id, *st.Down)
		st.OK = true
	case "partition":
		a, errA := h.peer(st.A)
		b, errB := h.peer(st.B)
		if errA != nil || errB != nil || st.Cut == nil {
			return errors.Join(errA, errB, errors.New("partition needs a, b and cut"))
		}
		h.net.Partition(a, b, *st.Cut)
		st.OK = true
	case "fail_next_dial":
		h.ep.mu.Lock()
		h.ep.failNext = st.Text
		h.ep.mu.Unlock()
		st.OK = true
	case "sleep":
		time.Sleep(time.Duration(st.MS) * time.Millisecond)
		st.OK = true
	case "close_endpoint":
		ep, ok := h.eps[st.Peer]
		if !ok {
			return fmt.Errorf("node %q is not bound", st.Peer)
		}
		tspOutcome(st, ep.Close())
	case "stream_write", "stream_read", "stream_close_write", "stream_cancel_read":
		i, err := tspIndex(st.Stream, len(h.streams), "stream")
		if err != nil {
			return err
		}
		return tspStreamOp(st, h.streams[i], strings.TrimPrefix(st.Op, "stream_"))
	case "server_write", "server_read", "server_close_write", "server_cancel_read":
		i, err := tspIndex(st.ServerStream, len(h.sstreams), "server stream")
		if err != nil {
			return err
		}
		return tspStreamOp(st, h.sstreams[i], strings.TrimPrefix(st.Op, "server_"))
	case "server_accept":
		ep, ok := h.eps[st.Peer]
		if !ok {
			return fmt.Errorf("node %q is not bound", st.Peer)
		}
		ctx, cancel := h.stepCtx(st, tspServerTimeout)
		c, err := ep.Accept(ctx)
		cancel()
		tspOutcome(st, err)
		if err == nil {
			idx := len(h.sconns)
			h.sconns = append(h.sconns, c)
			st.ServerConn = &idx
		}
	case "server_accept_stream", "server_open_stream":
		i, err := tspIndex(st.ServerConn, len(h.sconns), "server conn")
		if err != nil {
			return err
		}
		ctx, cancel := h.stepCtx(st, tspServerTimeout)
		var s transport.Stream
		if st.Op == "server_accept_stream" {
			s, err = h.sconns[i].AcceptStream(ctx)
		} else {
			s, err = h.sconns[i].OpenStream(ctx)
		}
		cancel()
		tspOutcome(st, err)
		if err == nil {
			idx := len(h.sstreams)
			h.sstreams = append(h.sstreams, s)
			st.ServerStream = &idx
		}
	case "server_wait_closed":
		i, err := tspIndex(st.ServerConn, len(h.sconns), "server conn")
		if err != nil {
			return err
		}
		select {
		case <-h.sconns[i].Done():
			st.OK = true
		case <-time.After(time.Duration(st.MS) * time.Millisecond):
			st.Error = tspStr("not closed")
		}
	default:
		return fmt.Errorf("unknown op %q", st.Op)
	}
	return nil
}

func tspStreamOp(st *tspPoolStep, s transport.Stream, op string) error {
	switch op {
	case "write":
		n, err := s.Write(st.Data)
		tspOutcome(st, err)
		st.N = &n
	case "read":
		if st.BufLen <= 0 {
			return errors.New("read needs buf_len")
		}
		buf := make([]byte, st.BufLen)
		n, err := s.Read(buf)
		tspOutcome(st, err)
		st.N = &n
		st.Read = append(Hex{}, buf[:n]...)
	case "close_write":
		tspOutcome(st, s.CloseWrite())
	case "cancel_read":
		s.CancelRead(0)
		st.OK = true
	default:
		return fmt.Errorf("unknown stream op %q", op)
	}
	return nil
}

func tspRunScript(sc tspPoolScript) (tspPoolScript, error) {
	h := &tspHarness{net: transport.NewNetwork(), ids: map[string]view.NodeID{}, eps: map[string]*transport.MemEndpoint{}}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	for _, n := range sc.Nodes {
		var id view.NodeID
		copy(id[:], n.ID)
		h.ids[n.Name] = id
		if !n.Bound {
			continue
		}
		ep := h.net.Bind(id, n.ALPNs...)
		h.eps[n.Name] = ep
		if n.Server != nil && n.Server.Mode != "manual" && n.Server.Mode != "none" {
			go tspServe(ctx, ep, *n.Server)
		}
	}
	client, ok := h.eps["client"]
	if !ok {
		return sc, errors.New("script has no bound client")
	}
	h.ep = &tspScriptEndpoint{Endpoint: client}
	h.pool = transport.NewPool(h.ep, func(view.NodeID) []string { return nil }, sc.PerPeer)
	defer h.pool.Close()
	for i := range sc.Steps {
		st := &sc.Steps[i]
		runs := max(st.Repeat, 1)
		var first *tspPoolStep
		for j := 0; j < runs; j++ {
			if err := h.run(st); err != nil {
				return sc, fmt.Errorf("script %s step %d (%s): %w", sc.Name, i, st.Op, err)
			}
			if first == nil {
				c := *st
				first = &c
			} else if first.OK != st.OK || (first.Error == nil) != (st.Error == nil) || (first.Error != nil && *first.Error != *st.Error) {
				return sc, fmt.Errorf("script %s step %d (%s): repeated runs differ", sc.Name, i, st.Op)
			}
		}
	}
	return sc, nil
}

func tspPoolScripts() (any, error) {
	client := tspSeedKey(1)
	a := tspSeedKey(2)
	b := tspSeedKey(3)
	c := tspSeedKey(4)
	cl := wire.ALPNClient
	node := func(name string, id [32]byte, bound bool, srv *tspPoolServer, alpns ...string) tspPoolNode {
		if alpns == nil {
			alpns = []string{}
		}
		return tspPoolNode{Name: name, ID: append(Hex{}, id[:]...), Bound: bound, ALPNs: alpns, Server: srv}
	}
	clientNode := node("client", client, true, nil)
	none := &tspPoolServer{Mode: "none"}
	manual := &tspPoolServer{Mode: "manual"}
	boolp := func(v bool) *bool { return &v }
	intp := func(v int) *int { return &v }
	get := func(peer string) tspPoolStep { return tspPoolStep{Op: "get", Peer: peer, ALPN: cl} }
	getALPN := func(peer, alpn string) tspPoolStep { return tspPoolStep{Op: "get", Peer: peer, ALPN: alpn} }
	path := func(peer string) tspPoolStep { return tspPoolStep{Op: "path", Peer: peer, ALPN: cl} }
	setDown := func(peer string, down bool) tspPoolStep {
		return tspPoolStep{Op: "set_down", Peer: peer, Down: boolp(down)}
	}
	ping := &tspPoolMsg{Type: wire.TPing, Epoch: 7}

	scripts := []tspPoolScript{
		{
			Name:        "grow_to_per_peer_then_round_robin",
			Description: "Get dials a new connection until per_peer are live, then rotates through them by a counter that is never reset.",
			PerPeer:     4,
			Nodes:       []tspPoolNode{clientNode, node("a", a, true, none, cl)},
			Steps:       []tspPoolStep{get("a"), get("a"), get("a"), get("a"), get("a"), get("a"), get("a")},
		},
		{
			Name:        "per_peer_zero_means_one",
			Description: "NewPool turns per_peer <= 0 into 1.",
			PerPeer:     0,
			Nodes:       []tspPoolNode{clientNode, node("a", a, true, none, cl)},
			Steps:       []tspPoolStep{get("a"), get("a"), get("a")},
		},
		{
			Name:        "failed_dial_window",
			Description: "A failed dial with no live connection refuses further dials for 2 s without dialing.",
			PerPeer:     2,
			Nodes:       []tspPoolNode{clientNode, node("a", a, true, none, cl)},
			Steps: []tspPoolStep{
				setDown("a", true), get("a"), get("a"), setDown("a", false), get("a"),
				{Op: "sleep", MS: 2100}, get("a"), get("a"),
			},
		},
		{
			Name:        "failed_dial_ignored_with_live_conn",
			Description: "The failed-dial window applies only while no connection is live.",
			PerPeer:     2,
			Nodes:       []tspPoolNode{clientNode, node("a", a, true, none, cl)},
			Steps: []tspPoolStep{
				get("a"), {Op: "fail_next_dial", Text: "scripted dial failure"}, get("a"), get("a"), get("a"), get("a"), get("a"),
			},
		},
		{
			Name:        "drop_close_and_path",
			Description: "Drop and Close forget connections (a later Get dials again); Path reports the first live connection.",
			PerPeer:     1,
			Nodes:       []tspPoolNode{clientNode, node("a", a, true, none, cl)},
			Steps: []tspPoolStep{
				path("a"), get("a"), get("a"), path("a"), {Op: "drop", Peer: "a", ALPN: cl}, path("a"), get("a"),
				{Op: "close"}, path("a"), get("a"), path("a"),
			},
		},
		{
			Name:        "closed_conns_are_filtered",
			Description: "Get filters closed connections in place; a peer going down closes them without a dial failure.",
			PerPeer:     2,
			Nodes:       []tspPoolNode{clientNode, node("a", a, true, none, cl)},
			Steps: []tspPoolStep{
				get("a"), get("a"), setDown("a", true), path("a"), setDown("a", false), get("a"), get("a"), get("a"), get("a"), get("a"),
			},
		},
		{
			Name:        "mem_dial_errors",
			Description: "In-memory transport dial failures, one pool key per failure so the failed-dial window does not hide them.",
			PerPeer:     1,
			Nodes: []tspPoolNode{
				clientNode, node("a", a, true, none, cl, wire.ALPNCluster), node("b", b, true, none, wire.ALPNCluster), node("c", c, false, nil),
			},
			Steps: []tspPoolStep{
				get("b"), get("c"), {Op: "partition", A: "client", B: "a", Cut: boolp(true)}, get("a"), get("a"),
				{Op: "partition", A: "client", B: "a", Cut: boolp(false)}, getALPN("a", wire.ALPNCluster),
				setDown("client", true), getALPN("a", "amber-dstore-test/1"), setDown("client", false), getALPN("a", "amber-dstore-test/2"),
				{Op: "close_endpoint", Peer: "client"}, getALPN("a", "amber-dstore-test/3"),
			},
		},
		{
			Name:        "call_reply",
			Description: "Call writes the request, finishes the send side and reads one reply frame; the connection stays pooled.",
			PerPeer:     1,
			Nodes:       []tspPoolNode{clientNode, node("a", a, true, &tspPoolServer{Mode: "pong"}, cl)},
			Steps: []tspPoolStep{
				{Op: "call", Peer: "a", ALPN: cl, Request: ping}, {Op: "call", Peer: "a", ALPN: cl, Request: &tspPoolMsg{Type: wire.TPing}}, get("a"),
			},
		},
		{
			Name:        "call_remote_error",
			Description: "A TErr reply is returned both as the reply and as a *wire.Error.",
			PerPeer:     1,
			Nodes: []tspPoolNode{
				clientNode,
				node("a", a, true, &tspPoolServer{Mode: "err", Code: wire.CodeBusy, Text: "slow down", RetryAfterMS: 1500}, cl),
				node("b", b, true, &tspPoolServer{Mode: "err", Code: wire.CodeStaleView}, cl),
			},
			Steps: []tspPoolStep{
				{Op: "call", Peer: "a", ALPN: cl, Request: ping}, {Op: "call", Peer: "b", ALPN: cl, Request: ping}, get("a"), get("b"),
			},
		},
		{
			Name:        "call_ctx_deadline",
			Description: "Call under an expiring ctx cancels the read side (the peer's writes fail) and keeps the connection pooled.",
			PerPeer:     1,
			Nodes:       []tspPoolNode{clientNode, node("a", a, true, manual, cl)},
			Steps: []tspPoolStep{
				{Op: "call", Peer: "a", ALPN: cl, TimeoutMS: 50, Request: ping}, get("a"),
				{Op: "server_accept", Peer: "a"}, {Op: "server_accept_stream", ServerConn: intp(0)},
				{Op: "server_read", ServerStream: intp(0), BufLen: 64}, {Op: "server_read", ServerStream: intp(0), BufLen: 64},
				{Op: "server_write", ServerStream: intp(0), Data: Hex("x")},
			},
		},
		{
			Name:        "call_eof_does_not_drop",
			Description: "A read error in Call is returned without dropping the connection.",
			PerPeer:     1,
			Nodes:       []tspPoolNode{clientNode, node("a", a, true, &tspPoolServer{Mode: "eof"}, cl)},
			Steps:       []tspPoolStep{{Op: "call", Peer: "a", ALPN: cl, Request: ping}, get("a")},
		},
		{
			Name:        "open_stream_failure_drops",
			Description: "The mem peer buffers 256 unaccepted streams; the next OpenStream blocks until ctx ends, and that failure drops the pool's connections.",
			PerPeer:     1,
			Nodes:       []tspPoolNode{clientNode, node("a", a, true, none, cl)},
			Steps: []tspPoolStep{
				{Op: "open", Peer: "a", ALPN: cl, Repeat: 256}, {Op: "open", Peer: "a", ALPN: cl, TimeoutMS: 50}, get("a"),
			},
		},
		{
			Name:        "mem_stream_pipes",
			Description: "In-memory stream semantics: FIN, write after close, cancelled reads and resets, and streams of a closed connection.",
			PerPeer:     1,
			Nodes:       []tspPoolNode{clientNode, node("a", a, true, manual, cl)},
			Steps: []tspPoolStep{
				{Op: "open", Peer: "a", ALPN: cl}, {Op: "server_accept", Peer: "a"}, {Op: "server_accept_stream", ServerConn: intp(0)},
				{Op: "stream_write", Stream: intp(0), Data: Hex("hello")}, {Op: "server_read", ServerStream: intp(0), BufLen: 16},
				{Op: "stream_close_write", Stream: intp(0)}, {Op: "server_read", ServerStream: intp(0), BufLen: 16},
				{Op: "stream_write", Stream: intp(0), Data: Hex("x")},
				{Op: "server_write", ServerStream: intp(0), Data: Hex("abc")}, {Op: "stream_read", Stream: intp(0), BufLen: 2},
				{Op: "stream_read", Stream: intp(0), BufLen: 16}, {Op: "server_close_write", ServerStream: intp(0)},
				{Op: "stream_read", Stream: intp(0), BufLen: 16}, {Op: "server_write", ServerStream: intp(0), Data: Hex("late")},
				{Op: "open", Peer: "a", ALPN: cl}, {Op: "server_accept_stream", ServerConn: intp(0)},
				{Op: "stream_cancel_read", Stream: intp(1)}, {Op: "stream_read", Stream: intp(1), BufLen: 16},
				{Op: "server_write", ServerStream: intp(1), Data: Hex("z")}, {Op: "server_cancel_read", ServerStream: intp(1)},
				{Op: "stream_write", Stream: intp(1), Data: Hex("q")},
				{Op: "drop", Peer: "a", ALPN: cl}, {Op: "server_wait_closed", ServerConn: intp(0), MS: 2000},
				{Op: "server_accept_stream", ServerConn: intp(0)}, {Op: "server_open_stream", ServerConn: intp(0)},
			},
		},
	}
	f := tspPoolFile{}
	for _, sc := range scripts {
		done, err := tspRunScript(sc)
		if err != nil {
			return nil, err
		}
		f.Scripts = append(f.Scripts, done)
	}

	for _, d := range []time.Duration{0, 400 * time.Microsecond, 499999, 500 * time.Microsecond, 1499 * time.Microsecond, 1500 * time.Microsecond,
		12300 * time.Microsecond, 1234500 * time.Microsecond, time.Millisecond, 333 * time.Millisecond, 59999500 * time.Microsecond, 90 * time.Minute} {
		r := d.Round(time.Millisecond)
		f.RTTRound = append(f.RTTRound, tspDuration{NS: I64(d), RoundedNS: I64(r), String: r.String()})
	}

	ipAddr := netaddr.IPAddr{Addr: netip.MustParseAddrPort("127.0.0.1:9")}
	ip6Addr := netaddr.IPAddr{Addr: netip.MustParseAddrPort("[::1]:9")}
	customAddr, err := netaddr.ParseCustomAddr("1_abcd")
	if err != nil {
		return nil, err
	}
	relayURL, err := netaddr.ParseRelayURL("https://use1-1.relay.n0.iroh-canary.iroh.link./")
	if err != nil {
		return nil, err
	}
	id0 := key.NewSecretKey(tspSeed0120()).Public().EndpointID()
	dialIP := fmt.Errorf("dial %s: %w", ipAddr, errors.New("x"))
	for _, et := range []struct {
		name string
		err  error
	}{
		{"err_closed", transport.ErrClosed},
		{"invalid_key", key.ErrInvalidKeyData},
		{"no_address", iroh.ErrNoAddress},
		{"dial_ip", dialIP},
		{"dial_ipv6", fmt.Errorf("dial %s: %w", ip6Addr, errors.New("y"))},
		{"dial_custom", fmt.Errorf("dial %s: %w", customAddr, errors.New("x"))},
		{"dial_relay", fmt.Errorf("dial %s: %w", netaddr.RelayAddr{URL: relayURL}, errors.New("x"))},
		{"joined_dial_discovery", errors.Join(dialIP, fmt.Errorf("discovery: %w", errors.New("y")))},
		{"joined_dial_discovery_no_address", errors.Join(dialIP, fmt.Errorf("discovery: %w", iroh.ErrNoAddress))},
		{"joined_two_dials", errors.Join(dialIP, fmt.Errorf("dial %s: %w", ip6Addr, errors.New("y")))},
		{"joined_nil_skipped", errors.Join(nil, dialIP, nil)},
		{"no_candidates", fmt.Errorf("transport: no candidate addresses for %s", id0.Short())},
		{"bootstrap_wrapper", fmt.Errorf("client: no bootstrap node answered: %w", errors.Join(dialIP, fmt.Errorf("discovery: %w", iroh.ErrNoAddress)))},
	} {
		f.ErrorTexts = append(f.ErrorTexts, tspErrorText{Name: et.name, Text: et.err.Error()})
	}
	return f, nil
}
