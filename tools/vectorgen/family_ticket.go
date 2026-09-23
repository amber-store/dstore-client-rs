package main

// Family "ticket" (ticket/encode.json, ticket/parse.json, ticket/curve.json):
// dstore v0.1.10 ticket.Ticket Encode/IDs/Parse and go-iroh v0.2.0 endpoint-id
// parsing and curve-point acceptance. Schemas: docs/vectorgen-proto.md.

import (
	"encoding/base32"
	"encoding/hex"
	"fmt"
	"path/filepath"
	"strings"
	"unicode"
	"unicode/utf8"

	"github.com/amber-store/dstore/codec"
	"github.com/amber-store/dstore/ticket"
	irohkey "github.com/tmc/go-iroh/key"
)

func init() {
	register("ticket", []string{"ticket/encode.json", "ticket/parse.json", "ticket/curve.json"}, genTicket)
}

// tkMember is ticket.Member: id null for nil; addrs null for nil, [] for empty.
type tkMember struct {
	ID    Hex      `json:"id"`
	Addrs []string `json:"addrs"`
}

// tkTicket is ticket.Ticket: members null for nil.
type tkTicket struct {
	ClusterID   Hex        `json:"cluster_id"`
	Incarnation U64        `json:"incarnation"`
	Members     []tkMember `json:"members"`
}

func tkTicketJSON(t ticket.Ticket) tkTicket {
	j := tkTicket{ClusterID: Hex(t.ClusterID), Incarnation: U64(t.Incarnation)}
	if t.Members != nil {
		j.Members = make([]tkMember, len(t.Members))
		for i, m := range t.Members {
			j.Members[i] = tkMember{ID: Hex(m.ID), Addrs: m.Addrs}
		}
	}
	return j
}

type tkParseResult struct {
	OK      bool      `json:"ok"`
	Error   string    `json:"error,omitempty"`
	Ticket  *tkTicket `json:"ticket,omitempty"`
	Encoded string    `json:"encoded,omitempty"`
	IDs     string    `json:"ids"`
}

type tkEncodeCase struct {
	Name    string        `json:"name"`
	Source  string        `json:"source"`
	Ticket  tkTicket      `json:"ticket"`
	CBORHex string        `json:"cbor_hex"`
	Encoded string        `json:"encoded"`
	IDs     string        `json:"ids"`
	Parse   tkParseResult `json:"parse"`
}

type tkParseCase struct {
	Name     string  `json:"name"`
	Source   string  `json:"source"`
	Input    *string `json:"input,omitempty"`
	InputHex string  `json:"input_hex"`
	tkParseResult
}

type tkCurveCase struct {
	Name             string `json:"name"`
	Bytes            Hex    `json:"bytes"`
	Valid            bool   `json:"valid"`
	HexParseError    string `json:"hex_parse_error,omitempty"`
	Base32ParseError string `json:"base32_parse_error,omitempty"`
}

func tkParse(s string) tkParseResult {
	t, err := ticket.Parse(s)
	if err != nil {
		return tkParseResult{Error: err.Error()}
	}
	j := tkTicketJSON(t)
	return tkParseResult{OK: true, Ticket: &j, Encoded: t.Encode(), IDs: t.IDs()}
}

// tkB32 is iroh's base32 form: RFC 4648, no padding, lower case.
func tkB32(b []byte) string {
	return strings.ToLower(base32.StdEncoding.WithPadding(base32.NoPadding).EncodeToString(b))
}

// tkBody encodes CBOR bytes as a dstore1 ticket.
func tkBody(cbor []byte) string { return ticket.Prefix + tkB32(cbor) }

// tkProbe is the TestRoundTrip ticket.
func tkProbe() ticket.Ticket {
	id := make([]byte, 32)
	id[0] = 7
	return ticket.Ticket{ClusterID: []byte("0123456789abcdef"), Incarnation: 3, Members: []ticket.Member{{ID: id, Addrs: []string{"ip:127.0.0.1:4433", "relay:https://relay.example/"}}}}
}

func genTicket(out string) error {
	if err := genTicketEncode(out); err != nil {
		return fmt.Errorf("encode: %w", err)
	}
	if err := genTicketParse(out); err != nil {
		return fmt.Errorf("parse: %w", err)
	}
	return genTicketCurve(out)
}

func genTicketEncode(out string) error {
	id1, id2, id3 := wvEdID(1), wvEdID(2), wvEdID(3)
	cases := []struct {
		name, source string
		t            ticket.Ticket
	}{
		{"probe", "codec-wire-ticket §2.4.2; ticket_test TestRoundTrip", tkProbe()},
		{"cli-four-members", "cli §5.4", ticket.Ticket{ClusterID: wvRep(0xaa, 16), Incarnation: 1, Members: []ticket.Member{
			{ID: wvRep(0x01, 32), Addrs: []string{"192.168.1.10:4433"}}, {ID: wvRep(0xab, 32)}, {ID: wvRep(0x01, 32)}, {ID: []byte{1, 2}}}}},
		{"zero", "codec-wire-ticket §2.4.1", ticket.Ticket{}},
		{"nil-id-empty-addrs", "codec-wire-ticket §2.4.1", ticket.Ticket{Members: []ticket.Member{{ID: nil, Addrs: []string{}}}}},
		{"empty-cluster-id-empty-members", "codec-wire-ticket §2.4.1; verification §5", ticket.Ticket{ClusterID: []byte{}, Members: []ticket.Member{}}},
		{"no-addrs", "verification §5", ticket.Ticket{ClusterID: wvSeq(0, 16), Incarnation: 1, Members: []ticket.Member{{ID: id1}}}},
		{"relay-addr", "verification §5", ticket.Ticket{ClusterID: wvSeq(0, 16), Incarnation: 1, Members: []ticket.Member{{ID: id1, Addrs: []string{"relay:https://relay.example/"}}}}},
		{"four-members", "verification §5 (TicketFromView cap)", ticket.Ticket{ClusterID: wvSeq(0, 16), Incarnation: 2, Members: []ticket.Member{
			{ID: id1, Addrs: []string{"ip:192.168.1.1:4433"}}, {ID: id2, Addrs: []string{"ip:192.168.1.2:4433", "relay:https://use1-1.relay.n0.iroh-canary.iroh.link./"}},
			{ID: id3, Addrs: []string{"ip:[fe80::1]:4433"}}, {ID: wvEdID(4)}}}},
		{"incarnation-0", "verification §5", ticket.Ticket{ClusterID: wvSeq(0, 16), Members: []ticket.Member{{ID: id1}}}},
		{"incarnation-max", "verification §5", ticket.Ticket{ClusterID: wvSeq(0, 16), Incarnation: 18446744073709551615, Members: []ticket.Member{{ID: id1}}}},
		{"nil-members", "verification §5", ticket.Ticket{ClusterID: wvSeq(0, 16), Incarnation: 1}},
		{"short-id", "verification §5", ticket.Ticket{ClusterID: wvSeq(0, 16), Incarnation: 1, Members: []ticket.Member{{ID: wvRep(9, 31)}, {ID: id2}}}},
		{"only-short-ids", "codec-wire-ticket §2.4.3", ticket.Ticket{Members: []ticket.Member{{ID: wvRep(9, 31)}, {ID: wvRep(9, 33)}}}},
		{"duplicate-ids", "verification §5", ticket.Ticket{ClusterID: wvSeq(0, 16), Incarnation: 1, Members: []ticket.Member{{ID: id1}, {ID: id2, Addrs: []string{"ip:10.0.0.2:1"}}, {ID: id1, Addrs: []string{"ip:10.0.0.1:1"}}}}},
		{"unicode-addr", "verification §5", ticket.Ticket{ClusterID: wvSeq(0, 16), Incarnation: 1, Members: []ticket.Member{{ID: id1, Addrs: []string{"relay:https://rélay.example/", "ip:[fe80::1%eth0]:4433"}}}}},
	}
	var res []tkEncodeCase
	for _, c := range cases {
		cbor := codec.MustMarshal(c.t)
		res = append(res, tkEncodeCase{Name: c.name, Source: c.source, Ticket: tkTicketJSON(c.t), CBORHex: hex.EncodeToString(cbor), Encoded: c.t.Encode(), IDs: c.t.IDs(), Parse: tkParse(c.t.Encode())})
	}
	// Fixture self-checks against codec-wire-ticket §2.4.2 and cli §5.4.
	x := func(s string) string { return hex.EncodeToString([]byte(s)) }
	probeCBOR := "a30050" + x("0123456789abcdef") + "0103" + "0281a2005820" + "07" + strings.Repeat("00", 31) +
		"018271" + x("ip:127.0.0.1:4433") + "781c" + x("relay:https://relay.example/")
	if res[0].CBORHex != probeCBOR || len(res[0].Encoded) != 182 || !strings.HasPrefix(res[0].Encoded, "dstore1umafambrgiztinjwg44dsylcmnsgkzqbambidiqalaqaoaaaa") ||
		!strings.HasSuffix(res[0].Encoded, "aabqjyws4b2gezdolrqfyyc4mj2gq2dgm3ydrzgk3dbpe5gq5duobztulzpojswyylzfzsxqylnobwgkly") {
		return fmt.Errorf("probe ticket differs: %s %s", res[0].CBORHex, res[0].Encoded)
	}
	cliCBOR := "a30050" + strings.Repeat("aa", 16) + "0101" + "0284" + "a2005820" + strings.Repeat("01", 32) + "018171" + x("192.168.1.10:4433") +
		"a1005820" + strings.Repeat("ab", 32) + "a1005820" + strings.Repeat("01", 32) + "a100420102"
	if res[1].CBORHex != cliCBOR || res[1].IDs != strings.Repeat("01", 32)+","+strings.Repeat("ab", 32) ||
		!strings.HasPrefix(res[1].Encoded, "dstore1umafbkvkvkvkvkvkvkvkvkvkvkvkvkqbaebijiqalaqacaibaeaqc") {
		return fmt.Errorf("cli ticket differs: %s %s %s", res[1].CBORHex, res[1].IDs, res[1].Encoded)
	}
	for _, c := range res[2:5] {
		want := map[string]string{"zero": "a300f6010002f6", "nil-id-empty-addrs": "a300f601000281a100f6", "empty-cluster-id-empty-members": "a3004001000280"}[c.Name]
		if c.CBORHex != want {
			return fmt.Errorf("%s: cbor %s, want %s", c.Name, c.CBORHex, want)
		}
	}
	return writeJSON(filepath.Join(out, "ticket", "encode.json"), struct {
		Cases []tkEncodeCase `json:"cases"`
	}{res})
}

func genTicketParse(out string) error {
	var cases []tkParseCase
	add := func(name, source, s string) {
		c := tkParseCase{Name: name, Source: source, InputHex: hex.EncodeToString([]byte(s)), tkParseResult: tkParse(s)}
		if utf8.ValidString(s) {
			in := s
			c.Input = &in
		}
		cases = append(cases, c)
	}
	probe := tkProbe()
	s := probe.Encode()
	body := s[len(ticket.Prefix):]
	id1, id2 := wvEdID(1), wvEdID(2)
	hex1, hex2 := hex.EncodeToString(id1), hex.EncodeToString(id2)
	b32id1, b32id2 := tkB32(id1), tkB32(id2)
	cw := "codec-wire-ticket §2.4.9"
	g7 := "codec-wire-ticket §5 G7"
	vf := "verification §5"

	add("empty", cw, "")
	add("spaces", cw, "  ")
	add("tab-newline", g7, "\t\n")
	add("prefix-only", cw, "dstore1")
	add("nope", cw, "nope")
	add("separators-only", cw, " , ,")
	add("hex-63", cw, hex1[:63])
	add("ab-x32", cw, strings.Repeat("ab", 32))
	add("hex-07-x32", vf, strings.Repeat("07", 32))
	add("hex-comma-zz", cw, hex1+",zz")
	add("quote-escapes", cw, "zz\x01\"é")
	add("z32-id1", cw, wvMust(irohkey.EndpointIDFromSlice(id1)).Z32())
	add("hex-nbsp-hex", cw, hex1+"\u00a0"+hex2)
	add("hex-vt-hex", cw, hex1+"\v"+hex2)
	add("hex-ff-hex", g7, hex1+"\f"+hex2)
	add("hex-semicolon-hex", vf, hex1+";"+hex2)
	add("hex-ideographic-space-hex", g7, hex1+"\u3000"+hex2)
	add("hex-all-separators-hex", g7, hex1+",\t\r\n "+hex2)
	for _, extra := range []string{"a", "aaa", "aaaaaa", "aaaaaaaa", "aa"} {
		add("probe-plus-"+extra, cw, s+extra)
	}
	add("bang-at-3", cw, ticket.Prefix+body[:3]+"!"+body[4:])
	add("eight-at-8", cw, ticket.Prefix+body[:8]+"8"+body[9:])
	add("equals-at-0", g7, ticket.Prefix+"="+body[1:])
	add("dstore1-bang-bang-bang", "cli §5.2", "dstore1!!!")
	add("dstore1aaaa", "cli §5.2", "dstore1aaaa")
	add("bogus", "cli §5.2", "bogus")
	add("zz-yy", "cli §5.2", "zz,yy")
	add("probe", vf, s)
	add("upper-case", cw, strings.ToUpper(s))
	var mixed strings.Builder
	for i, r := range body {
		if i%2 == 0 {
			mixed.WriteRune(unicode.ToUpper(r))
		} else {
			mixed.WriteRune(r)
		}
	}
	add("mixed-case-prefix-and-body", cw, "DsToRe1"+mixed.String())
	add("prefix-dStOrE1", g7, "dStOrE1"+body)
	add("newline-cr-inside-body", cw, ticket.Prefix+body[:10]+"\n\r"+body[10:])
	var crlf strings.Builder
	for i := 0; i < len(body); i += 8 {
		crlf.WriteString(body[i:min(i+8, len(body))])
		crlf.WriteString("\r\n")
	}
	add("crlf-every-8-symbols", g7, ticket.Prefix+crlf.String())
	add("nbsp-and-em-space-around", cw, "\u00a0"+s+"\u2003")
	iAt := strings.IndexByte(body, 'i')
	sAt := strings.IndexByte(body, 's')
	kAt := strings.IndexByte(body, 'k')
	add("body-dotless-i", cw, ticket.Prefix+body[:iAt]+"\u0131"+body[iAt+1:])
	add("body-dotted-capital-i", g7, ticket.Prefix+body[:iAt]+"\u0130"+body[iAt+1:])
	add("body-long-s", g7, ticket.Prefix+body[:sAt]+"\u017f"+body[sAt+1:])
	add("body-kelvin-k", g7, ticket.Prefix+body[:kAt]+"\u212a"+body[kAt+1:])
	const alphabet = "abcdefghijklmnopqrstuvwxyz234567"
	for _, r := range alphabet {
		add("body-last-symbol-"+string(r), cw, s[:len(s)-1]+string(r))
	}
	add("hex1", vf, hex1)
	add("hex1-comma-hex2", vf, hex1+","+hex2)
	add("hex1-space-hex2", vf, hex1+" "+hex2)
	add("hex1-newline-b32id2", vf, " "+hex1+",\n"+b32id2+" ")
	add("upper-hex1", vf, strings.ToUpper(hex1))
	add("hex1-hex2-hex1", vf, hex1+","+hex2+","+hex1)
	add("b32id1-upper", cw, strings.ToUpper(b32id1))
	for n := 48; n <= 60; n++ {
		in := b32id1
		if n <= len(in) {
			in = in[:n]
		} else {
			in += strings.Repeat("a", n-len(in))
		}
		add(fmt.Sprintf("b32id1-length-%d", n), g7, in)
	}
	for _, r := range alphabet {
		add("b32id1-last-symbol-"+string(r), g7, b32id1[:len(b32id1)-1]+string(r))
	}
	// Lookalike runes in a base32 id.
	var look string
	for seed := uint64(1); seed < 1000; seed++ {
		b := tkB32(wvEdID(seed))
		if strings.ContainsRune(b, 'i') && strings.ContainsRune(b, 's') && strings.ContainsRune(b, 'k') {
			look = b
			break
		}
	}
	if look == "" {
		return fmt.Errorf("no base32 id with i, s and k")
	}
	repl := func(in string, old byte, new string) string {
		at := strings.IndexByte(in, old)
		return in[:at] + new + in[at+1:]
	}
	add("b32id-dotted-capital-i", cw, repl(look, 'i', "\u0130"))
	add("b32id-dotless-i", cw, repl(look, 'i', "\u0131"))
	add("b32id-long-s", cw, repl(look, 's', "\u017f"))
	add("b32id-kelvin-k", cw, repl(look, 'k', "\u212a"))
	add("b32id-all-lookalikes", g7, repl(repl(repl(look, 'i', "\u0130"), 's', "\u017f"), 'k', "\u212a"))
	// z-base-32 ids that avoid 8, 1, 9 decode under RFC 4648 too.
	for seed := uint64(1); seed < 4000; seed++ {
		z := wvMust(irohkey.EndpointIDFromSlice(wvEdID(seed))).Z32()
		if !strings.ContainsAny(z, "819") {
			add(fmt.Sprintf("z32-rfc-decodable-id%d", seed), g7, z)
			break
		}
	}
	// Unicode White_Space around ids and tickets; runes that are not trimmed.
	for r := rune(0); r <= unicode.MaxRune; r++ {
		if unicode.IsSpace(r) {
			add(fmt.Sprintf("space-U+%04X-around-hex", r), g7, string(r)+hex1+string(r))
			add(fmt.Sprintf("space-U+%04X-around-ticket", r), g7, string(r)+s+string(r))
		}
	}
	for _, r := range []rune{0x180e, 0x200b, 0xfeff} {
		add(fmt.Sprintf("not-space-U+%04X-before-hex", r), g7, string(r)+hex1)
	}
	// A body whose length is a multiple of 8, then 1/2/3/6 extra symbols.
	var quantum ticket.Ticket
	for n := 16; n < 32; n++ {
		t := tkProbe()
		t.ClusterID = wvSeq('a', n)
		if len(codec.MustMarshal(t))%5 == 0 {
			quantum = t
			break
		}
	}
	qs := quantum.Encode()
	add("full-quantum-body", g7, qs)
	for _, extra := range []string{"a", "aa", "aaa", "aaaaaa", "aaaaaaaa"} {
		add("full-quantum-plus-"+extra, g7, qs+extra)
	}
	// 64-byte fields with non-ASCII runes (hex routing on the lowered length).
	add("dotted-i-then-62-hex", g7, "\u0130"+hex1[:62])
	add("61-hex-then-kelvin", g7, hex1[:61]+"\u212a")
	add("62-hex-then-dotted-i", g7, hex1[:62]+"\u0130")
	add("63-hex-then-long-s", g7, hex1[:63]+"\u017f")
	// Crafted CBOR bodies.
	pc := codec.MustMarshal(probe)
	add("cbor-unknown-key-3", vf, tkBody(append(append([]byte{0xa4}, pc[1:]...), 0x03, 0x01)))
	add("cbor-null-members", g7, tkBody([]byte{0xa3, 0x00, 0x40, 0x01, 0x00, 0x02, 0xf6}))
	add("cbor-empty-members", g7, tkBody([]byte{0xa3, 0x00, 0x40, 0x01, 0x00, 0x02, 0x80}))
	add("cbor-null-member-element", g7, tkBody([]byte{0xa3, 0x00, 0x40, 0x01, 0x00, 0x02, 0x81, 0xf6}))
	add("cbor-member-31-byte-id", g7, ticket.Ticket{Members: []ticket.Member{{ID: wvRep(9, 31)}}}.Encode())
	add("cbor-incarnation-text", g7, tkBody([]byte{0xa3, 0x00, 0x40, 0x01, 0x61, 0x61, 0x02, 0x80}))
	add("cbor-trailing-byte", g7, tkBody(append(append([]byte(nil), pc...), 0x00)))
	add("cbor-top-null", g7, tkBody([]byte{0xf6}))
	add("cbor-tag-55799", g7, tkBody(append([]byte{0xd9, 0xd9, 0xf7}, pc...)))
	add("cbor-top-array", g7, tkBody([]byte{0x83, 0x40, 0x00, 0x80}))
	// Invalid UTF-8 input.
	add("invalid-utf8-ff", g7, "\xff")
	add("invalid-utf8-in-body", g7, ticket.Prefix+body[:5]+"\xc3"+body[6:])
	add("invalid-utf8-after-prefix", g7, "dstore1\xff")
	add("hex-comma-invalid-utf8", g7, hex1+",\xff")
	return writeJSON(filepath.Join(out, "ticket", "parse.json"), struct {
		Cases []tkParseCase `json:"cases"`
	}{cases})
}

func genTicketCurve(out string) error {
	var cases []tkCurveCase
	add := func(name string, b [32]byte) {
		c := tkCurveCase{Name: name, Bytes: Hex(append([]byte(nil), b[:]...))}
		_, err := irohkey.NewPublicKey(b)
		c.Valid = err == nil
		if _, err := irohkey.ParseEndpointID(hex.EncodeToString(b[:])); err != nil {
			c.HexParseError = err.Error()
		}
		if _, err := irohkey.ParseEndpointID(tkB32(b[:])); err != nil {
			c.Base32ParseError = err.Error()
		}
		cases = append(cases, c)
	}
	// y encodings are little-endian; the sign of x is bit 255.
	y := func(low byte, sign bool) [32]byte {
		var b [32]byte
		b[0] = low
		if sign {
			b[31] = 0x80
		}
		return b
	}
	modP := func(add byte, sign bool) [32]byte {
		var b [32]byte
		for i := range b {
			b[i] = 0xff
		}
		b[0] = 0xed + add
		b[31] = 0x7f
		if sign {
			b[31] = 0xff
		}
		return b
	}
	var all0, allff [32]byte
	for i := range allff {
		allff[i] = 0xff
	}
	add("all-zero", all0)
	add("all-0xff", allff)
	add("y-1-sign-0", y(1, false))
	add("y-1-sign-1", y(1, true))
	add("y-0-sign-1", y(0, true))
	for i := byte(2); i <= 20; i++ {
		add(fmt.Sprintf("y-%d-sign-0", i), y(i, false))
	}
	pm1 := modP(0, false)
	pm1[0] = 0xec
	add("y-p-minus-1-sign-0", pm1)
	pm1[31] = 0xff
	add("y-p-minus-1-sign-1", pm1)
	for i := byte(0); i <= 18; i++ {
		add(fmt.Sprintf("y-p-plus-%d-sign-0", i), modP(i, false))
		add(fmt.Sprintf("y-p-plus-%d-sign-1", i), modP(i, true))
	}
	add("07-x32", [32]byte(wvRep(0x07, 32)))
	add("ab-x32", [32]byte(wvRep(0xab, 32)))
	for s := uint64(1); s <= 8; s++ {
		id := [32]byte(wvEdID(s))
		add(fmt.Sprintf("id%d", s), id)
		id[31] ^= 0x80
		add(fmt.Sprintf("id%d-sign-flipped", s), id)
	}
	for i := uint64(0); i < 64; i++ {
		add(fmt.Sprintf("splitmix-%d", 200+i), [32]byte(smData(200+i, 32)))
	}
	return writeJSON(filepath.Join(out, "ticket", "curve.json"), struct {
		Cases []tkCurveCase `json:"cases"`
	}{cases})
}
