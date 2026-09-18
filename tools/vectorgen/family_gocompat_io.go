package main

// Family gocompat_io: Go standard-library behaviour behind dstore-gocompat's json, hex, base32 and path
// modules (go1.26.5 encoding/json v1, encoding/hex, encoding/base32 with NoPadding, path and
// path/filepath). Schemas: tools/vectorgen/docs/gocompat-c.md.

import (
	"encoding/base32"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"path"
	"path/filepath"
	"reflect"
	"strings"

	"github.com/amber-store/dstore/worktree"
)

func init() {
	register("gocompat_io", []string{
		"gocompat/base32.json",
		"gocompat/hex.json",
		"gocompat/json.json",
		"gocompat/path.json",
	}, genGocompatIO)
}

func genGocompatIO(out string) error {
	jv, err := gioJSON()
	if err != nil {
		return err
	}
	files := []struct {
		name string
		v    any
	}{
		{"json.json", jv},
		{"hex.json", gioHex()},
		{"base32.json", gioBase32()},
		{"path.json", gioPath()},
	}
	for _, f := range files {
		if err := writeJSON(filepath.Join(out, "gocompat", f.name), f.v); err != nil {
			return err
		}
	}
	return nil
}

// gioPick maps a splitmix64 output to an index below n.
func gioPick(r uint64, n int) int { return int(r % uint64(n)) }

// gioRandom builds count random strings of 0..maxTokens tokens chosen from alphabet.
func gioRandom(seed uint64, count, maxTokens int, alphabet []string) []string {
	rs := u64s(seed, count*(maxTokens+1))
	out := make([]string, 0, count)
	for i := 0; i < count; i++ {
		row := rs[i*(maxTokens+1):]
		n := gioPick(row[0], maxTokens+1)
		var b strings.Builder
		for j := 1; j <= n; j++ {
			b.WriteString(alphabet[gioPick(row[j], len(alphabet))])
		}
		out = append(out, b.String())
	}
	return out
}

// ---- encoding/json ----------------------------------------------------------------------------------

// stateJSON mirrors the unexported worktree.stateJSON (worktree/tree.go:59-64).
type stateJSON struct {
	Base          string `json:"base"`
	Remote        string `json:"remote"`
	RemoteVersion string `json:"remote_version"`
	SyncedAt      string `json:"synced_at"`
}

type gioFieldSpec struct {
	Name      string `json:"name"`
	Kind      string `json:"kind"`
	OmitEmpty bool   `json:"omitempty"`
}

type gioStructSpec struct {
	Name   string         `json:"name"`
	GoType string         `json:"go_type"`
	Fields []gioFieldSpec `json:"fields"`
}

type gioFieldValue struct {
	Name   string `json:"name"`
	StrHex Hex    `json:"str_hex"`
	Bool   bool   `json:"bool"`
}

type gioMarshalCase struct {
	Name   string          `json:"name"`
	Struct string          `json:"struct"`
	Values []gioFieldValue `json:"values"`
	Out    string          `json:"out"`
}

type gioSlot struct {
	Name   string `json:"name"`
	Set    bool   `json:"set"`
	StrHex Hex    `json:"str_hex"`
	Bool   bool   `json:"bool"`
}

type gioUnmarshalCase struct {
	Name   string    `json:"name"`
	Struct string    `json:"struct"`
	InHex  Hex       `json:"in_hex"`
	Slots  []gioSlot `json:"slots"`
	Error  string    `json:"error"`
}

type gioJSONVectors struct {
	Structs   []gioStructSpec    `json:"structs"`
	Marshal   []gioMarshalCase   `json:"marshal"`
	Unmarshal []gioUnmarshalCase `json:"unmarshal"`
}

type gioStruct struct {
	name  string
	newV  func() any // a pointer to a zero struct
	valid []string   // well-formed inputs, seeds for the mutations
}

var gioStructs = []gioStruct{
	{"config", func() any { return new(worktree.Config) }, []string{
		`{"ticket":"t","name":"n"}`,
		"{\n  \"ticket\": \"dstore1abc\",\n  \"name\": \"trees/x\",\n  \"relay\": \"https://r\",\n  \"no_relay\": true,\n  \"no_discovery\": false,\n  \"user\": \"Dr \\u003cd@x\\u003e\"\n}\n",
		`{"TICKET":"a","Name":"b","x":[1,2.5e-3,{"y":null}],"no_relay":false}`,
		`{"ticket":"\ud83d\ude00\u00e9","user":null,"relay":"\\\"\/\b\f\n\r\t"}`,
	}},
	{"state", func() any { return new(stateJSON) }, []string{
		"{\n  \"base\": \"20ab\",\n  \"remote\": \"\",\n  \"remote_version\": \"\",\n  \"synced_at\": \"2023-11-14T22:13:20.000000005Z\"\n}\n",
		`{"BASE":"2001bbe6a9f5a014","remote":"20ab","remote_version":"010203","synced_at":"2023-11-14T22:13:20+02:00","base":"20cd"}`,
	}},
}

func gioSpecOf(name string, v any) (gioStructSpec, error) {
	t := reflect.TypeOf(v).Elem()
	s := gioStructSpec{Name: name, GoType: t.String()}
	for i := 0; i < t.NumField(); i++ {
		f := t.Field(i)
		parts := strings.Split(f.Tag.Get("json"), ",")
		k := f.Type.Kind()
		if k != reflect.String && k != reflect.Bool {
			return s, fmt.Errorf("json vectors: %s.%s is %v, not string or bool", t, f.Name, k)
		}
		s.Fields = append(s.Fields, gioFieldSpec{Name: parts[0], Kind: k.String(), OmitEmpty: len(parts) > 1 && parts[1] == "omitempty"})
	}
	return s, nil
}

func gioValues(spec gioStructSpec, v any) []gioFieldValue {
	rv := reflect.ValueOf(v).Elem()
	out := make([]gioFieldValue, len(spec.Fields))
	for i, f := range spec.Fields {
		out[i].Name = f.Name
		out[i].StrHex = Hex{}
		switch fv := rv.Field(i); fv.Kind() {
		case reflect.String:
			out[i].StrHex = Hex(fv.String())
		case reflect.Bool:
			out[i].Bool = fv.Bool()
		}
	}
	return out
}

func gioMarshal(name, structName string, spec gioStructSpec, v any) (gioMarshalCase, error) {
	b, err := json.MarshalIndent(v, "", "  ")
	if err != nil {
		return gioMarshalCase{}, err
	}
	return gioMarshalCase{Name: name, Struct: structName, Values: gioValues(spec, v), Out: string(b)}, nil
}

func gioPrefill(v any, s string, b bool) {
	rv := reflect.ValueOf(v).Elem()
	for i := 0; i < rv.NumField(); i++ {
		switch fv := rv.Field(i); fv.Kind() {
		case reflect.String:
			fv.SetString(s)
		case reflect.Bool:
			fv.SetBool(b)
		}
	}
}

// gioUnmarshal decodes in twice, into structs prefilled with different sentinels, so that a field
// the input never set is told apart from one set to any value.
func gioUnmarshal(name string, st gioStruct, spec gioStructSpec, in []byte) (gioUnmarshalCase, error) {
	const sa, sb = "\x00sentinel-a", "\x00sentinel-b"
	a, b := st.newV(), st.newV()
	gioPrefill(a, sa, false)
	gioPrefill(b, sb, true)
	errA, errB := json.Unmarshal(in, a), json.Unmarshal(in, b)
	c := gioUnmarshalCase{Name: name, Struct: st.name, InHex: Hex(append([]byte{}, in...))}
	if (errA == nil) != (errB == nil) || (errA != nil && errA.Error() != errB.Error()) {
		return c, fmt.Errorf("json vectors %s: runs disagree: %v / %v", name, errA, errB)
	}
	if errA != nil {
		c.Error = errA.Error()
	}
	va, vb := reflect.ValueOf(a).Elem(), reflect.ValueOf(b).Elem()
	for i, f := range spec.Fields {
		slot := gioSlot{Name: f.Name, StrHex: Hex{}}
		fa, fb := va.Field(i), vb.Field(i)
		switch fa.Kind() {
		case reflect.String:
			slot.Set = fa.String() != sa || fb.String() != sb
			if slot.Set {
				if fa.String() != fb.String() {
					return c, fmt.Errorf("json vectors %s: field %s differs between runs", name, f.Name)
				}
				slot.StrHex = Hex(fa.String())
			}
		case reflect.Bool:
			slot.Set = fa.Bool() || !fb.Bool()
			if slot.Set {
				if fa.Bool() != fb.Bool() {
					return c, fmt.Errorf("json vectors %s: field %s differs between runs", name, f.Name)
				}
				slot.Bool = fa.Bool()
			}
		}
		c.Slots = append(c.Slots, slot)
	}
	return c, nil
}

var gioTextTokens = []string{
	"a", "Z", "0", " ", "/", "{", ":", "<", ">", "&", `"`, `\`, "\x00", "\x01", "\b", "\f", "\n", "\r", "\t",
	"\x1f", "\x7f", "é", "\u2028", "\u2029", "\u00a0", "😀", "\xff", "\xe2\x82", "\xed\xa0\x80", "\xc0\xaf",
	"\xf4\x90\x80\x80", "\xef\xbf\xbd",
}

func gioJSON() (gioJSONVectors, error) {
	var v gioJSONVectors
	specs := map[string]gioStructSpec{} // lookups only; never iterated
	for _, st := range gioStructs {
		spec, err := gioSpecOf(st.name, st.newV())
		if err != nil {
			return v, err
		}
		specs[st.name] = spec
		v.Structs = append(v.Structs, spec)
	}

	// Marshal: worktree §5 item 2 and §3.2-3.3, then random field bytes.
	cfgs := []struct {
		name string
		c    worktree.Config
	}{
		{"zero", worktree.Config{}},
		{"ticket_name", worktree.Config{Ticket: "t", Name: "n"}},
		{"verified_sample", worktree.Config{Ticket: "dstore1abc", Name: "trees/<a&b> x\x01\"\\", NoRelay: true, User: "Dr <d@x>"}},
		{"all_fields", worktree.Config{Ticket: "dstore1x", Name: "n", Relay: "https://relay.example/", NoRelay: true, NoDiscovery: true, User: "u"}},
		{"relay_escapes", worktree.Config{Ticket: "t", Name: "n", Relay: "&<>\b\f\n\r\t\x01\x7f\u2028\u2029"}},
		{"invalid_utf8", worktree.Config{Ticket: "\xff", Name: "\xe2\x82", Relay: "\xed\xa0\x80", User: "\xc0\xaf"}},
		{"user_quotes", worktree.Config{Ticket: "t", Name: "n", User: `a"b\c`}},
		{"no_discovery_only", worktree.Config{NoDiscovery: true}},
	}
	for _, c := range cfgs {
		m, err := gioMarshal(c.name, "config", specs["config"], &c.c)
		if err != nil {
			return v, err
		}
		v.Marshal = append(v.Marshal, m)
	}
	var control []byte
	for b := 0; b < 0x80; b++ {
		control = append(control, byte(b))
	}
	m, err := gioMarshal("ascii_bytes", "config", specs["config"], &worktree.Config{Ticket: string(control), Name: string(control[0x60:])})
	if err != nil {
		return v, err
	}
	v.Marshal = append(v.Marshal, m)
	for i, ns := range []int64{0, 5, 120000000, 123456789, 999999999} {
		s := stateJSON{Base: "20ab", SyncedAt: fmt.Sprintf("2023-11-14T22:13:20.%09dZ", ns)}
		if i%2 == 1 {
			s.Remote, s.RemoteVersion = "2001bbe6a9f5a014", "010203"
		}
		m, err := gioMarshal(fmt.Sprintf("state_%d", i), "state", specs["state"], &s)
		if err != nil {
			return v, err
		}
		v.Marshal = append(v.Marshal, m)
	}
	texts := gioRandom(0x6a736f6e, 6*120, 8, gioTextTokens)
	flags := u64s(0x666c6167, 120)
	for i := 0; i < 120; i++ {
		c := worktree.Config{
			Ticket: texts[6*i], Name: texts[6*i+1], Relay: texts[6*i+2], User: texts[6*i+3],
			NoRelay: flags[i]&1 != 0, NoDiscovery: flags[i]&2 != 0,
		}
		m, err := gioMarshal(fmt.Sprintf("random_config_%d", i), "config", specs["config"], &c)
		if err != nil {
			return v, err
		}
		v.Marshal = append(v.Marshal, m)
	}

	// Unmarshal: hand-written cases, then byte mutations of well-formed inputs.
	config := []string{
		``, `   `, `{}`, ` {} `, `null`, `"x"`, `5`, `-1.5e3`, `true`, `false`, `[]`, `[{"ticket":"t"}]`,
		`{"ticket":"t","name":"n"}`,
		`{"ticket":"a","ticket":"b"}`,
		`{"ticket":5,"ticket":"b"}`,
		`{"ticket":"b","ticket":5}`,
		`{"TICKET":"upper","Name":"mixed","USER":"u","No_Relay":true,"NO_DISCOVERY":true,"rElAy":"r"}`,
		`{"ticket":"exact","TICKET":"folded"}`,
		"{\"tic\xe2\x84\xaaet\":\"kelvin\",\"u\xc5\xbfer\":\"long s\"}",
		`{"tic\u212aet":"escaped kelvin","\u0074icket":"escaped t"}`,
		"{\"t\xc4\xb0cket\":\"dotted I\",\"tick\xffet\":\"invalid key\"}",
		`{"ticket":null,"no_relay":null,"user":"u"}`,
		`{"unknown":{"nested":[1,{"a":[true,false,null]}]},"name":"n"}`,
		`{"ticket":5}`, `{"ticket":true}`, `{"ticket":{}}`, `{"ticket":[1]}`,
		`{"no_relay":"true"}`, `{"no_relay":1}`, `{"no_relay":{"a":1}}`, `{"no_relay":[]}`, `{"no_relay":true}`,
		`{"ticket":1,"name":true,"no_relay":"x","user":"kept"}`,
		`{"no_relay":"x","ticket":1}`,
		"{\"ticket\":\"\xff\xe2\x82\xed\xa0\x80\xc0\xaf\xf4\x90\x80\x80\"}",
		`{"ticket":"\ud83d\ude00","name":"\ud800","relay":"\udc00x","user":"\ud800\u0041"}`,
		`{"ticket":"\u0000\u001f\u007f\u00e9\u2028","name":"\"\\\/\b\f\n\r\t"}`,
		`{"ticket":"t"}x`, `{"ticket":"t"} {}`, `{"ticket":01}`, `{"ticket":-}`, `{"ticket":1.}`, `{"ticket":1e}`,
		`tru`, `fals`, `nul`, `123e`, `"hello`, `[1,2,3`, `{"key":1`, `{"key":1,`,
		`{"X": "foo", "Y"}`, `{"X": "foo" "Y": "bar"}`, `{'a':1}`, `{"a":1,}`, `[1,]`, `[1 2]`,
		"{\"ticket\":\"a\nb\"}", `{"ticket":"\x"}`, `{"ticket":"\u12g4"}`, "\xef\xbb\xbf{}", "\x80", "\\", "{\"a\"\x00:1}",
		strings.Repeat("[", 10000) + strings.Repeat("]", 10000),
		strings.Repeat("[", 10001),
		`{"ticket":"t"` + strings.Repeat(" ", 3) + "\t\r\n}",
	}
	state := []string{
		`{"base":"20ab","remote":"","remote_version":"","synced_at":"2023-11-14T22:13:20Z"}`,
		`{"BASE":"20AB","Remote":"20cd","REMOTE_VERSION":"01","Synced_At":"x","extra":1}`,
		`{"base":null,"synced_at":true}`,
		`{"remote_version":{"a":1},"base":"b"}`,
	}
	for _, group := range []struct {
		st     gioStruct
		inputs []string
	}{{gioStructs[0], config}, {gioStructs[1], state}} {
		for i, in := range group.inputs {
			c, err := gioUnmarshal(fmt.Sprintf("%s_%d", group.st.name, i), group.st, specs[group.st.name], []byte(in))
			if err != nil {
				return v, err
			}
			v.Unmarshal = append(v.Unmarshal, c)
		}
	}
	mutBytes := []byte("{}[]\":,\\ \t\n0-1e.E+utfnlasrZ\x00\x1f\x7f\x80\xff")
	for _, st := range gioStructs {
		rs := u64s(0x6d757461+uint64(len(st.name)), 4*300)
		for i := 0; i < 300; i++ {
			r := rs[4*i:]
			in := []byte(st.valid[gioPick(r[0], len(st.valid))])
			for k := 0; k <= gioPick(r[3], 3); k++ {
				pos := gioPick(r[1]>>uint(8*k), len(in)+1)
				c := mutBytes[gioPick(r[2]>>uint(8*k), len(mutBytes))]
				switch gioPick(r[3]>>uint(8*k+2), 3) {
				case 0:
					if pos < len(in) {
						in[pos] = c
					}
				case 1:
					if pos < len(in) {
						in = append(in[:pos], in[pos+1:]...)
					}
				default:
					in = append(in[:pos], append([]byte{c}, in[pos:]...)...)
				}
			}
			c, err := gioUnmarshal(fmt.Sprintf("mutated_%s_%d", st.name, i), st, specs[st.name], in)
			if err != nil {
				return v, err
			}
			v.Unmarshal = append(v.Unmarshal, c)
		}
	}
	return v, nil
}

// ---- encoding/hex -----------------------------------------------------------------------------------

type gioHexEncode struct {
	InHex Hex    `json:"in_hex"`
	Out   string `json:"out"`
}

type gioHexDecode struct {
	InHex  Hex    `json:"in_hex"`
	OutHex Hex    `json:"out_hex"` // null on error
	Error  string `json:"error"`
}

type gioHexVectors struct {
	Encode []gioHexEncode `json:"encode"`
	Decode []gioHexDecode `json:"decode"`
}

func gioHex() gioHexVectors {
	var v gioHexVectors
	for n := 0; n <= 40; n++ {
		b := smData(0x686578+uint64(n), n)
		v.Encode = append(v.Encode, gioHexEncode{InHex: Hex(b), Out: hex.EncodeToString(b)})
	}
	inputs := []string{
		"", "0", "zd4aa", "d4aaz", "30313", "0g", "00gg", "0\x01", "ffeed", "abc", "a", "zz", "abz", "ÿ", "0G",
		"\x00", "0001020304050607", "F8F9FAFBFCFDFEFF", "e3a1", "2001bbe6a9f5a0146a1f4d0381e9b0ed1ac2f1a979ce9d5ad84e46ff0b58f36b",
	}
	for b := 0; b < 256; b++ {
		inputs = append(inputs, string([]byte{'0', byte(b)}), string([]byte{byte(b)}))
	}
	inputs = append(inputs, gioRandom(0x68657872, 200, 9, []string{"0", "9", "a", "f", "A", "F", "g", "G", "z", " ", "\x00", "\xff", "é"})...)
	for _, in := range inputs {
		out, err := hex.DecodeString(in)
		c := gioHexDecode{InHex: Hex(in)}
		if err != nil {
			c.Error = err.Error()
		} else {
			c.OutHex = Hex(append([]byte{}, out...))
		}
		v.Decode = append(v.Decode, c)
	}
	return v
}

// ---- encoding/base32 --------------------------------------------------------------------------------

type gioB32Encode struct {
	Alphabet string `json:"alphabet"` // "std" or "zbase32"
	InHex    Hex    `json:"in_hex"`
	Out      string `json:"out"`
}

type gioB32Decode struct {
	Alphabet string `json:"alphabet"`
	InHex    Hex    `json:"in_hex"`
	OutHex   Hex    `json:"out_hex"` // null on error
	Error    string `json:"error"`
}

type gioB32Vectors struct {
	Encode []gioB32Encode `json:"encode"`
	Decode []gioB32Decode `json:"decode"`
}

func gioBase32() gioB32Vectors {
	var v gioB32Vectors
	encs := []struct {
		name string
		enc  *base32.Encoding
		syms string
	}{
		{"std", base32.StdEncoding.WithPadding(base32.NoPadding), "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"},
		{"zbase32", base32.NewEncoding("ybndrfg8ejkmcpqxot1uwisza345h769").WithPadding(base32.NoPadding), "ybndrfg8ejkmcpqxot1uwisza345h769"},
	}
	for ei, e := range encs {
		for n := 0; n <= 40; n++ {
			b := smData(0x623332+uint64(100*ei+n), n)
			v.Encode = append(v.Encode, gioB32Encode{Alphabet: e.name, InHex: Hex(b), Out: e.enc.EncodeToString(b)})
		}
	}
	fixed := []string{
		"", "MY", "MZXQ", "MZXW6", "MZXW6YQ", "MZXW6YTB", "MZXW6YTBOI", "ON2XEZJO", "ON2XEZI", "NRSWC43VOJSS4",
		"MY======", "!!!!", "x===", "AA=A====", "AAA=AAAA", "MMMMMMMMM", "MMMMMM", "A=", "AA=", "AA==", "AAAA=",
		"A", "AA", "AAA", "AAAA", "AAAAA", "AAAAAA", "AAAAAAA", "AAAAAAAA", "AAAAAAAAA", "ME", "MF", "0A", "a",
		"A\nA", "ON2XEZ\r\nI", "\n\r", "\nA=", "ON2X\rEZ\nI", "MZXW6YTBOI\r\n",
		"AA\xff\xff\xff\xff\xff\xff", "AA\xff\xff\xff\xff\xff\xffMY", "AA\xff", "AA\xff\xffA\xff\xff\xff",
		"AAA\xff\xff\xff\xff\xff", "A\xff", "AAAA\xff\xff\xff\xff", "AAAAA\xff\xff\xff", "AAAAAAA\xff",
		"AAAAAAAA\xff", "MZXW6YTB\xff\xff\xff\xff\xff\xff\xff", "\xffAAAAAAA", "AA\xff\n\xff\xff\xff\xff\xff",
		"ybndrfg8", "YBNDRFG8", "ON2XEZJO\xc4\xb1",
	}
	// codec-wire-ticket §5 G8: "A"×n and "a"×n for n = 0..17 (and the z-base-32 symbol in both cases),
	// "=" at every position of 8-symbol inputs, and mixed newlines. Every alphabet decodes every input.
	seen := map[string]bool{} // lookups only; never iterated
	for _, in := range fixed {
		seen[in] = true
	}
	addFixed := func(in string) {
		if !seen[in] {
			seen[in] = true
			fixed = append(fixed, in)
		}
	}
	for n := 0; n <= 17; n++ {
		for _, sym := range []string{"A", "a", "y", "Y"} {
			addFixed(strings.Repeat(sym, n))
		}
	}
	for i := 0; i < 8; i++ {
		for _, quantum := range []string{"AAAAAAAA", "MZXW6YTB", "yyyyyyyy", "ybndrfg8"} {
			addFixed(quantum[:i] + "=" + quantum[i+1:])
		}
	}
	for _, in := range []string{
		"\nMZXW6YTB", "MZXW\r\n6YTB\n", "MZ\rXW\n6Y\r\nTBOI\r", "\r\r\n\nMY", "\n\n\n", "A\r\n=\n",
		"ybnd\r\nrfg8\ny\r", "\ryb\nnd\r\nrf\n", "AAAA\nAAAA\rAAAA\r\nAAAA",
	} {
		addFixed(in)
	}
	tokens := func(syms string) []string {
		out := []string{"=", "a", "!", "\n", "\r", "\xff", "0", "1", "8", "9"}
		for _, s := range syms {
			out = append(out, string(s), string(s), string(s))
		}
		return out
	}
	for ei, e := range encs {
		inputs := append(append([]string{}, fixed...), gioRandom(0x62333272+uint64(ei), 300, 20, tokens(e.syms))...)
		for _, in := range inputs {
			out, err := e.enc.DecodeString(in)
			c := gioB32Decode{Alphabet: e.name, InHex: Hex(in)}
			if err != nil {
				c.Error = err.Error()
			} else {
				c.OutHex = Hex(append([]byte{}, out...))
			}
			v.Decode = append(v.Decode, c)
		}
	}
	return v
}

// ---- path, path/filepath ----------------------------------------------------------------------------

type gioPathCase struct {
	InHex  Hex `json:"in_hex"`
	OutHex Hex `json:"out_hex"`
}

type gioJoinCase struct {
	ElemsHex []Hex `json:"elems_hex"`
	OutHex   Hex   `json:"out_hex"`
}

type gioRelCase struct {
	BaseHex   Hex `json:"base_hex"`
	TargetHex Hex `json:"target_hex"`
	OutHex    Hex `json:"out_hex"` // null on error
	// ErrorHex is Go's error text, "" when none. It echoes both paths, so it can be invalid UTF-8.
	ErrorHex Hex `json:"error_hex"`
}

type gioPathVectors struct {
	Clean []gioPathCase `json:"clean"` // filepath.Clean
	Join  []gioJoinCase `json:"join"`  // filepath.Join
	Dir   []gioPathCase `json:"dir"`   // filepath.Dir
	Base  []gioPathCase `json:"base"`  // path.Base (filepath.Base agrees, checked here)
	Rel   []gioRelCase  `json:"rel"`   // filepath.Rel
	Abs   []gioPathCase `json:"abs"`   // filepath.Abs of absolute paths
}

func gioPath() gioPathVectors {
	var v gioPathVectors
	inputs := []string{
		"abc", "abc/def", "a/b/c", ".", "..", "../..", "../../abc", "/abc", "/", "", "abc/", "abc/def/", "a/b/c/",
		"./", "../", "../../", "/abc/", "abc//def//ghi", "abc//", "abc/./def", "/./abc/def", "abc/.",
		"abc/def/ghi/../jkl", "abc/def/../ghi/../jkl", "abc/def/..", "abc/def/../..", "/abc/def/../..",
		"abc/def/../../..", "/abc/def/../../..", "abc/def/../../../ghi/jkl/../../../mno", "/../abc", "a/../b:/../../c",
		"abc/./../def", "abc//./../def", "abc/../../././../def", "//abc", "///abc", "//abc//", "////", "/.", "x/",
		"a/b/.x", "a/b/c.", "a/b/c.x", "/foo", "...", ".../..", "a/.../b", "\xff/../\xff", "é/./é/",
	}
	tokens := []string{"a", "b", ".", ".", "..", "/", "/", "/", "\xff", "é"}
	inputs = append(inputs, gioRandom(0x70617468, 600, 10, tokens)...)
	for _, in := range inputs {
		v.Clean = append(v.Clean, gioPathCase{Hex(in), Hex(filepath.Clean(in))})
		v.Dir = append(v.Dir, gioPathCase{Hex(in), Hex(filepath.Dir(in))})
		if path.Base(in) != filepath.Base(in) {
			panic(fmt.Sprintf("path.Base and filepath.Base disagree on %q", in))
		}
		v.Base = append(v.Base, gioPathCase{Hex(in), Hex(path.Base(in))})
		if strings.HasPrefix(in, "/") {
			abs, err := filepath.Abs(in)
			if err != nil {
				panic(err)
			}
			v.Abs = append(v.Abs, gioPathCase{Hex(in), Hex(abs)})
		}
	}
	joins := [][]string{
		{}, {""}, {"/"}, {"a"}, {"a", "b"}, {"a", ""}, {"", "b"}, {"/", "a"}, {"/", "a/b"}, {"/", ""}, {"/a", "b"},
		{"a", "/b"}, {"/a", "/b"}, {"a/", "b"}, {"a/", ""}, {"", ""}, {"/", "a", "b"}, {"//", "a"}, {"", "", "a/../.."},
		{"/wd/x", "../../../y"}, {"a", "..", "..", "b"},
	}
	rj := gioRandom(0x6a6f696e, 3*200, 6, tokens)
	counts := u64s(0x6a6e, 200)
	for i := 0; i < 200; i++ {
		joins = append(joins, rj[3*i:3*i+gioPick(counts[i], 4)])
	}
	for _, elems := range joins {
		c := gioJoinCase{ElemsHex: []Hex{}, OutHex: Hex(filepath.Join(elems...))}
		for _, e := range elems {
			c.ElemsHex = append(c.ElemsHex, Hex(e))
		}
		v.Join = append(v.Join, c)
	}
	rels := [][2]string{
		{"a/b", "a/b"}, {"a/b/.", "a/b"}, {"a/b", "a/b/."}, {"./a/b", "a/b"}, {"a/b", "./a/b"}, {"ab/cd", "ab/cde"},
		{"ab/cd", "ab/c"}, {"a/b", "a/b/c/d"}, {"a/b", "a/b/../c"}, {"a/b/../c", "a/b"}, {"a/b/c", "a/c/d"},
		{"a/b", "c/d"}, {"a/b/c/d", "a/b"}, {"a/b/c/d", "a/b/"}, {"a/b/c/d/", "a/b"}, {"a/b/c/d/", "a/b/"},
		{"../../a/b", "../../a/b/c/d"}, {"/a/b", "/a/b"}, {"/a/b/.", "/a/b"}, {"/a/b", "/a/b/."}, {"/ab/cd", "/ab/cde"},
		{"/ab/cd", "/ab/c"}, {"/a/b", "/a/b/c/d"}, {"/a/b", "/a/b/../c"}, {"/a/b/../c", "/a/b"}, {"/a/b/c", "/a/c/d"},
		{"/a/b", "/c/d"}, {"/a/b/c/d", "/a/b"}, {"/a/b/c/d", "/a/b/"}, {"/a/b/c/d/", "/a/b"}, {"/a/b/c/d/", "/a/b/"},
		{"/../../a/b", "/../../a/b/c/d"}, {".", "a/b"}, {".", ".."}, {"", "../../."}, {"..", "."}, {"..", "a"},
		{"../..", ".."}, {"a", "/a"}, {"/a", "a"}, {"/wc", "/wc/sub/x"}, {"/wc", "/other"}, {"/", "/x"}, {"/x", "/"},
		{"a/..", "b"}, {"../a", "../b"}, {"..", "../a"},
	}
	rr := gioRandom(0x72656c, 2*400, 8, tokens)
	for i := 0; i < 400; i++ {
		rels = append(rels, [2]string{rr[2*i], rr[2*i+1]})
	}
	for _, p := range rels {
		out, err := filepath.Rel(p[0], p[1])
		c := gioRelCase{BaseHex: Hex(p[0]), TargetHex: Hex(p[1]), ErrorHex: Hex{}}
		if err != nil {
			c.ErrorHex = Hex(err.Error())
		} else {
			c.OutHex = Hex(out)
		}
		v.Rel = append(v.Rel, c)
	}
	return v
}
