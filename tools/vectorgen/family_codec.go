package main

// Family codec: codec/encode.json and codec/decode.json.
//
// The values come from dstore v0.1.9 codec.Marshal and codec.Unmarshal, which
// are fxamacker/cbor v2.9.3 CanonicalEncOptions().EncMode() and
// DecOptions{}.DecMode(), over test structs that mirror every field kind and
// struct-tag combination dstore declares. The Rust tests
// (tests/golden_tests/codec.rs) declare the same structs with cbor_struct!.
// Schema: tools/vectorgen/docs/codec.md.

import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"math"
	"path/filepath"
	"reflect"
	"strings"

	"github.com/amber-store/dstore/codec"
	"github.com/fxamacker/cbor/v2"
)

func init() {
	register("codec", []string{"codec/encode.json", "codec/decode.json"}, genCodec)
}

// codecLeaf is a small record with a non-omitempty []byte, like wire.RefInfo.
type codecLeaf struct {
	Name string `cbor:"0,keyasint"`
	Key  []byte `cbor:"1,keyasint"`
	N    int64  `cbor:"2,keyasint,omitempty"`
}

// codecOmit holds every field kind dstore declares with omitempty, plus keys
// whose heads take two, three and five bytes.
type codecOmit struct {
	Int      int         `cbor:"0,keyasint,omitempty"`
	I64      int64       `cbor:"1,keyasint,omitempty"`
	U8       uint8       `cbor:"2,keyasint,omitempty"`
	U16      uint16      `cbor:"3,keyasint,omitempty"`
	U32      uint32      `cbor:"4,keyasint,omitempty"`
	U64      uint64      `cbor:"5,keyasint,omitempty"`
	Bool     bool        `cbor:"6,keyasint,omitempty"`
	F64      float64     `cbor:"7,keyasint,omitempty"`
	Str      string      `cbor:"8,keyasint,omitempty"`
	Bytes    []byte      `cbor:"9,keyasint,omitempty"`
	List     [][]byte    `cbor:"10,keyasint,omitempty"` // Rust Vec<Vec<u8>> (wire)
	ListN    [][]byte    `cbor:"11,keyasint,omitempty"` // Rust Vec<Option<Vec<u8>>> (view)
	Strs     []string    `cbor:"12,keyasint,omitempty"`
	U16s     []uint16    `cbor:"13,keyasint,omitempty"`
	Leaves   []codecLeaf `cbor:"14,keyasint,omitempty"`
	Ptr      *codecLeaf  `cbor:"15,keyasint,omitempty"`
	Far24    uint64      `cbor:"24,keyasint,omitempty"`
	Far256   bool        `cbor:"256,keyasint,omitempty"`
	Far65536 string      `cbor:"65536,keyasint,omitempty"`
}

// codecNoOmit holds every field kind dstore declares without omitempty (and
// float64, uint16 and *T, which dstore only declares with it).
type codecNoOmit struct {
	Int    int         `cbor:"0,keyasint"`
	I64    int64       `cbor:"1,keyasint"`
	U8     uint8       `cbor:"2,keyasint"`
	U16    uint16      `cbor:"3,keyasint"`
	U32    uint32      `cbor:"4,keyasint"`
	U64    uint64      `cbor:"5,keyasint"`
	Bool   bool        `cbor:"6,keyasint"`
	F64    float64     `cbor:"7,keyasint"`
	Str    string      `cbor:"8,keyasint"`
	Bytes  []byte      `cbor:"9,keyasint"`
	Leaves []codecLeaf `cbor:"14,keyasint"`
	Ptr    *codecLeaf  `cbor:"15,keyasint"`
}

// codecMid and codecTop nest struct slices and pointers three levels deep, so
// error texts name the outermost struct field.
type codecMid struct {
	Leaves []codecLeaf `cbor:"0,keyasint,omitempty"`
	Leaf   *codecLeaf  `cbor:"1,keyasint,omitempty"`
}

type codecTop struct {
	Mids []codecMid `cbor:"0,keyasint,omitempty"`
	Mid  *codecMid  `cbor:"1,keyasint,omitempty"`
	Name string     `cbor:"2,keyasint,omitempty"`
}

// codecWide has 25 fields, so its map head takes two bytes when 24 or more
// are present.
type codecWide struct {
	F0  uint8 `cbor:"0,keyasint,omitempty"`
	F1  uint8 `cbor:"1,keyasint,omitempty"`
	F2  uint8 `cbor:"2,keyasint,omitempty"`
	F3  uint8 `cbor:"3,keyasint,omitempty"`
	F4  uint8 `cbor:"4,keyasint,omitempty"`
	F5  uint8 `cbor:"5,keyasint,omitempty"`
	F6  uint8 `cbor:"6,keyasint,omitempty"`
	F7  uint8 `cbor:"7,keyasint,omitempty"`
	F8  uint8 `cbor:"8,keyasint,omitempty"`
	F9  uint8 `cbor:"9,keyasint,omitempty"`
	F10 uint8 `cbor:"10,keyasint,omitempty"`
	F11 uint8 `cbor:"11,keyasint,omitempty"`
	F12 uint8 `cbor:"12,keyasint,omitempty"`
	F13 uint8 `cbor:"13,keyasint,omitempty"`
	F14 uint8 `cbor:"14,keyasint,omitempty"`
	F15 uint8 `cbor:"15,keyasint,omitempty"`
	F16 uint8 `cbor:"16,keyasint,omitempty"`
	F17 uint8 `cbor:"17,keyasint,omitempty"`
	F18 uint8 `cbor:"18,keyasint,omitempty"`
	F19 uint8 `cbor:"19,keyasint,omitempty"`
	F20 uint8 `cbor:"20,keyasint,omitempty"`
	F21 uint8 `cbor:"21,keyasint,omitempty"`
	F22 uint8 `cbor:"22,keyasint,omitempty"`
	F23 uint8 `cbor:"23,keyasint,omitempty"`
	F24 uint8 `cbor:"24,keyasint,omitempty"`
}

// codecNew returns a pointer to a new value of the named test type.
func codecNew(typ string) (any, error) {
	switch typ {
	case "Leaf":
		return new(codecLeaf), nil
	case "Omit":
		return new(codecOmit), nil
	case "NoOmit":
		return new(codecNoOmit), nil
	case "Mid":
		return new(codecMid), nil
	case "Top":
		return new(codecTop), nil
	case "Wide":
		return new(codecWide), nil
	}
	return nil, fmt.Errorf("codec: unknown type %q", typ)
}

func codecTypeName(v any) (string, error) {
	switch v.(type) {
	case codecLeaf:
		return "Leaf", nil
	case codecOmit:
		return "Omit", nil
	case codecNoOmit:
		return "NoOmit", nil
	case codecMid:
		return "Mid", nil
	case codecTop:
		return "Top", nil
	case codecWide:
		return "Wide", nil
	}
	return "", fmt.Errorf("codec: no vector type for %T", v)
}

func genCodec(out string) error {
	enc, err := codecEncodeVectors()
	if err != nil {
		return err
	}
	if err := writeJSON(filepath.Join(out, "codec", "encode.json"), enc); err != nil {
		return err
	}
	dec, err := codecDecodeVectors()
	if err != nil {
		return err
	}
	return writeJSON(filepath.Join(out, "codec", "decode.json"), dec)
}

// codecField is one field of a rendered struct.
type codecField struct {
	name  string
	value any
}

// codecObject renders a struct as a JSON object with its fields in
// declaration order.
type codecObject []codecField

// MarshalJSON writes the fields in order.
func (o codecObject) MarshalJSON() ([]byte, error) {
	var b bytes.Buffer
	b.WriteByte('{')
	for i, f := range o {
		if i > 0 {
			b.WriteByte(',')
		}
		k, err := json.Marshal(f.name)
		if err != nil {
			return nil, err
		}
		b.Write(k)
		b.WriteByte(':')
		v, err := json.Marshal(f.value)
		if err != nil {
			return nil, err
		}
		b.Write(v)
	}
	b.WriteByte('}')
	return b.Bytes(), nil
}

// codecIsZero is reflect.Value.IsZero, except that a negative zero float is
// not zero (IsZero compares with ==), so the vectors keep -0.0.
func codecIsZero(v reflect.Value) bool {
	if v.Kind() == reflect.Float64 {
		return math.Float64bits(v.Float()) == 0
	}
	return v.IsZero()
}

// codecJSON renders a Go value of the test types: a struct as an object of its
// non-zero fields (codecIsZero, so a nil slice or pointer is left out and an
// empty non-nil slice is kept), a nil slice or pointer as null, []byte
// as hex, int and int64 and uint64 as decimal strings, uint8/16/32 as numbers,
// float64 as the decimal string of its IEEE 754 bits.
func codecJSON(v reflect.Value) (any, error) {
	switch v.Kind() {
	case reflect.Struct:
		obj := codecObject{}
		t := v.Type()
		for i := 0; i < v.NumField(); i++ {
			f := v.Field(i)
			if codecIsZero(f) {
				continue
			}
			j, err := codecJSON(f)
			if err != nil {
				return nil, err
			}
			obj = append(obj, codecField{t.Field(i).Name, j})
		}
		return obj, nil
	case reflect.Pointer:
		if v.IsNil() {
			return nil, nil
		}
		return codecJSON(v.Elem())
	case reflect.Slice:
		if v.IsNil() {
			return nil, nil
		}
		if v.Type().Elem().Kind() == reflect.Uint8 {
			return Hex(v.Bytes()), nil
		}
		out := make([]any, v.Len())
		for i := range out {
			j, err := codecJSON(v.Index(i))
			if err != nil {
				return nil, err
			}
			out[i] = j
		}
		return out, nil
	case reflect.Int, reflect.Int64:
		return I64(v.Int()), nil
	case reflect.Uint64:
		return U64(v.Uint()), nil
	case reflect.Uint8, reflect.Uint16, reflect.Uint32:
		return v.Uint(), nil
	case reflect.Bool:
		return v.Bool(), nil
	case reflect.Float64:
		return U64(math.Float64bits(v.Float())), nil
	case reflect.String:
		return v.String(), nil
	}
	return nil, fmt.Errorf("codec: cannot render kind %s", v.Kind())
}

// codecHex decodes hex parts joined together; spaces are ignored.
func codecHex(parts ...string) []byte {
	s := strings.ReplaceAll(strings.Join(parts, ""), " ", "")
	b, err := hex.DecodeString(s)
	if err != nil {
		panic(fmt.Sprintf("codec: bad hex %q: %v", s, err))
	}
	return b
}

// ---------------------------------------------------------------------------
// codec/encode.json

type codecEncodeFile struct {
	Scalars []codecScalarCase `json:"scalars"`
	Structs []codecStructCase `json:"structs"`
}

// codecScalarCase is codec.Marshal of one scalar. Kind is uint (U64), int
// (I64), float64 (Bits), bool (Bool), null (a nil []byte), bytes or text
// (Data, whose output is OutputHeadHex followed by the data itself); the other
// kinds carry OutputHex.
type codecScalarCase struct {
	Name          string   `json:"name"`
	Kind          string   `json:"kind"`
	U64           *U64     `json:"u64,omitempty"`
	I64           *I64     `json:"i64,omitempty"`
	Bits          *U64     `json:"bits,omitempty"`
	Bool          *bool    `json:"bool,omitempty"`
	Data          *Payload `json:"data,omitempty"`
	OutputHex     string   `json:"output_hex,omitempty"`
	OutputHeadHex *string  `json:"output_head_hex,omitempty"`
}

// codecStructCase is codec.Marshal of a test struct.
type codecStructCase struct {
	Name      string `json:"name"`
	Type      string `json:"type"`
	Value     any    `json:"value"`
	OutputHex string `json:"output_hex"`
}

var codecUints = []uint64{
	0, 1, 23, 24, 255, 256, 65535, 65536, math.MaxUint32, 1 << 32,
	math.MaxInt64, 1 << 63, math.MaxUint64,
}

var codecInts = []int64{
	0, 1, 23, 24, 255, 256, 65535, 65536, math.MaxUint32, 1 << 32, math.MaxInt64,
	-1, -24, -25, -256, -257, -65536, -65537, -(1 << 32), -(1 << 32) - 1, math.MinInt64,
}

// codecHalf is the float64 value of float16 bits b.
func codecHalf(b uint16) float64 {
	sign := 1.0
	if b&0x8000 != 0 {
		sign = -1
	}
	exp := int(b>>10) & 0x1f
	mant := float64(b & 0x3ff)
	switch exp {
	case 0:
		return sign * math.Ldexp(mant, -24)
	case 0x1f:
		if mant == 0 {
			return math.Inf(int(sign))
		}
		return math.NaN()
	}
	return sign * math.Ldexp(1024+mant, exp-25)
}

// codecFloats lists the float64 values the encode vectors pin: the spec's
// list, float16 and float32 boundaries, a sweep around the float16 subnormal
// range (where x448/float16 needs its round-trip test), and random float64,
// float32 and float16 bit patterns. Duplicates by bits are dropped.
func codecFloats() []float64 {
	fs := []float64{
		0, math.Copysign(0, -1), 0.5, 1.5, 0.1, 1, -1, 2, 65504, -65504, 65505, 65519, 65520,
		65536, 100000, 1e-5, 1e-40, 3.4e38, math.MaxFloat32, -math.MaxFloat32,
		math.SmallestNonzeroFloat32, math.MaxFloat64, math.SmallestNonzeroFloat64,
		math.Inf(1), math.Inf(-1), math.NaN(),
		math.Float64frombits(0x7ff0000000000001), math.Float64frombits(0xfff8000000000000),
		math.Float64frombits(0x7ff4000000000000), math.Float64frombits(0x7ff8000020000000),
		1.0 / 3, 2.0 / 3, math.Pi, math.E, 1 << 53, 1<<53 + 1, -(1 << 63), 16777216, 16777217,
		math.Ldexp(1, -24), math.Ldexp(1, -14), math.Ldexp(1, -15), math.Ldexp(1, -25),
		math.Ldexp(1, 15), math.Ldexp(1, 16), math.Ldexp(1, -149), math.Ldexp(1, -150),
	}
	for e := -27; e <= -12; e++ {
		for _, m := range []float64{1, 3, 5, 7, 255, 1023, 1025, 2047, 4097} {
			fs = append(fs, math.Ldexp(m, e), -math.Ldexp(m, e))
		}
	}
	for _, b := range u64s(0xc0dec0de01, 48) {
		fs = append(fs, math.Float64frombits(b))
	}
	for _, b := range u64s(0xc0dec0de02, 48) {
		fs = append(fs, float64(math.Float32frombits(uint32(b))))
	}
	for _, b := range u64s(0xc0dec0de03, 48) {
		fs = append(fs, codecHalf(uint16(b)))
	}
	seen := map[uint64]bool{}
	var out []float64
	for _, f := range fs {
		if seen[math.Float64bits(f)] {
			continue
		}
		seen[math.Float64bits(f)] = true
		out = append(out, f)
	}
	return out
}

// codecText is n bytes of cycled ASCII letters and digits.
func codecText(n int) string {
	const alphabet = "abcdefghijklmnopqrstuvwxyz0123456789"
	var b strings.Builder
	for i := 0; i < n; i++ {
		b.WriteByte(alphabet[i%len(alphabet)])
	}
	return b.String()
}

func codecEncodeVectors() (*codecEncodeFile, error) {
	f := &codecEncodeFile{}
	names := map[string]bool{}
	var firstErr error
	fail := func(err error) {
		if firstErr == nil {
			firstErr = err
		}
	}
	unique := func(name string) string {
		if names[name] {
			fail(fmt.Errorf("codec: duplicate encode case %q", name))
		}
		names[name] = true
		return name
	}
	marshal := func(v any) []byte {
		b, err := codec.Marshal(v)
		if err != nil {
			fail(err)
		}
		return b
	}

	for _, u := range codecUints {
		x := U64(u)
		f.Scalars = append(f.Scalars, codecScalarCase{
			Name: unique(fmt.Sprintf("uint %d", u)), Kind: "uint", U64: &x,
			OutputHex: hex.EncodeToString(marshal(u)),
		})
	}
	for _, i := range codecInts {
		x := I64(i)
		f.Scalars = append(f.Scalars, codecScalarCase{
			Name: unique(fmt.Sprintf("int %d", i)), Kind: "int", I64: &x,
			OutputHex: hex.EncodeToString(marshal(i)),
		})
	}
	for _, fl := range codecFloats() {
		bits := U64(math.Float64bits(fl))
		f.Scalars = append(f.Scalars, codecScalarCase{
			Name: unique(fmt.Sprintf("float64 %016x", uint64(bits))), Kind: "float64", Bits: &bits,
			OutputHex: hex.EncodeToString(marshal(fl)),
		})
	}
	for _, b := range []bool{false, true} {
		f.Scalars = append(f.Scalars, codecScalarCase{
			Name: unique(fmt.Sprintf("bool %v", b)), Kind: "bool", Bool: &b,
			OutputHex: hex.EncodeToString(marshal(b)),
		})
	}
	f.Scalars = append(f.Scalars, codecScalarCase{
		Name: unique("nil []byte"), Kind: "null", OutputHex: hex.EncodeToString(marshal([]byte(nil))),
	})
	addData := func(kind string, data []byte, p Payload, out []byte) {
		if !bytes.HasSuffix(out, data) {
			fail(fmt.Errorf("codec: %s of %d bytes does not end in its data", kind, len(data)))
			return
		}
		head := hex.EncodeToString(out[:len(out)-len(data)])
		f.Scalars = append(f.Scalars, codecScalarCase{
			Name: unique(fmt.Sprintf("%s %d", kind, len(data))), Kind: kind, Data: &p, OutputHeadHex: &head,
		})
	}
	for _, n := range []int{0, 1, 23, 24, 255, 256, 65535, 65536} {
		data := smData(uint64(n), n)
		p := Inline(data)
		if n > 256 {
			p = SM(uint64(n), n)
		}
		addData("bytes", data, p, marshal(data))
	}
	for _, n := range []int{0, 1, 23, 24, 255, 256} {
		s := codecText(n)
		addData("text", []byte(s), Inline([]byte(s)), marshal(s))
	}
	for _, s := range []string{"héllo ✓ 😀", "\x00\u2028\uFEFF", "\U0010FFFF"} {
		b := []byte(s)
		out := marshal(s)
		if !bytes.HasSuffix(out, b) {
			fail(fmt.Errorf("codec: text %q does not end in its data", s))
			continue
		}
		head := hex.EncodeToString(out[:len(out)-len(b)])
		p := Inline(b)
		f.Scalars = append(f.Scalars, codecScalarCase{
			Name: unique(fmt.Sprintf("text %x", b)), Kind: "text", Data: &p, OutputHeadHex: &head,
		})
	}

	addStruct := func(name string, v any) {
		typ, err := codecTypeName(v)
		if err != nil {
			fail(err)
			return
		}
		j, err := codecJSON(reflect.ValueOf(v))
		if err != nil {
			fail(err)
			return
		}
		f.Structs = append(f.Structs, codecStructCase{
			Name: unique(name), Type: typ, Value: j, OutputHex: hex.EncodeToString(marshal(v)),
		})
	}

	// Omit: the zero value, each field alone, the far keys, and everything at once.
	addStruct("Omit zero", codecOmit{})
	for _, i := range codecInts {
		addStruct(fmt.Sprintf("Omit.Int %d", i), codecOmit{Int: int(i)})
		addStruct(fmt.Sprintf("Omit.I64 %d", i), codecOmit{I64: i})
	}
	for _, u := range codecUints {
		if u <= math.MaxUint8 {
			addStruct(fmt.Sprintf("Omit.U8 %d", u), codecOmit{U8: uint8(u)})
		}
		if u <= math.MaxUint16 {
			addStruct(fmt.Sprintf("Omit.U16 %d", u), codecOmit{U16: uint16(u)})
		}
		if u <= math.MaxUint32 {
			addStruct(fmt.Sprintf("Omit.U32 %d", u), codecOmit{U32: uint32(u)})
		}
		addStruct(fmt.Sprintf("Omit.U64 %d", u), codecOmit{U64: u})
	}
	addStruct("Omit.Bool true", codecOmit{Bool: true})
	for _, fl := range []float64{
		math.Copysign(0, -1), 0.5, 1.5, 0.1, 65504, 65520, 1e-40, 3.4e38, math.Inf(1), math.Inf(-1),
		math.NaN(), math.MaxFloat64, math.SmallestNonzeroFloat64, math.Ldexp(1, -24), math.Ldexp(3, -25),
	} {
		addStruct(fmt.Sprintf("Omit.F64 %016x", math.Float64bits(fl)), codecOmit{F64: fl})
	}
	for _, n := range []int{1, 23, 24, 255, 256} {
		addStruct(fmt.Sprintf("Omit.Str %d bytes", n), codecOmit{Str: codecText(n)})
	}
	addStruct("Omit.Str unicode", codecOmit{Str: "héllo ✓ 😀"})
	addStruct("Omit.Str NUL", codecOmit{Str: "\x00"})
	addStruct("Omit.Bytes empty", codecOmit{Bytes: []byte{}})
	for _, n := range []int{1, 23, 24, 255, 256} {
		addStruct(fmt.Sprintf("Omit.Bytes %d bytes", n), codecOmit{Bytes: smData(uint64(1000+n), n)})
	}
	addStruct("Omit.List empty", codecOmit{List: [][]byte{}})
	addStruct("Omit.List one empty", codecOmit{List: [][]byte{{}}})
	addStruct("Omit.List two", codecOmit{List: [][]byte{{0x01}, {}}})
	list24 := make([][]byte, 24)
	strs24 := make([]string, 24)
	u16s24 := make([]uint16, 24)
	for i := range list24 {
		list24[i] = smData(uint64(i), i)
		strs24[i] = codecText(i)
		u16s24[i] = uint16(i * 2851)
	}
	addStruct("Omit.List 24", codecOmit{List: list24})
	addStruct("Omit.ListN nil element", codecOmit{ListN: [][]byte{nil}})
	addStruct("Omit.ListN nil and empty", codecOmit{ListN: [][]byte{nil, {}}})
	addStruct("Omit.ListN bytes and nil", codecOmit{ListN: [][]byte{{0x01, 0x02}, nil}})
	addStruct("Omit.Strs empty element", codecOmit{Strs: []string{""}})
	addStruct("Omit.Strs two", codecOmit{Strs: []string{"a", ""}})
	addStruct("Omit.Strs 24", codecOmit{Strs: strs24})
	addStruct("Omit.U16s zero", codecOmit{U16s: []uint16{0}})
	addStruct("Omit.U16s boundaries", codecOmit{U16s: []uint16{23, 24, 255, 256, 65535}})
	addStruct("Omit.U16s 24", codecOmit{U16s: u16s24})
	addStruct("Omit.Leaves zero leaf", codecOmit{Leaves: []codecLeaf{{}}})
	addStruct("Omit.Leaves empty key", codecOmit{Leaves: []codecLeaf{{Name: "n", Key: []byte{}}}})
	addStruct("Omit.Leaves two", codecOmit{Leaves: []codecLeaf{
		{N: -1}, {Name: "x", Key: []byte{1, 2}, N: math.MaxInt64},
	}})
	addStruct("Omit.Ptr zero leaf", codecOmit{Ptr: &codecLeaf{}})
	addStruct("Omit.Ptr leaf", codecOmit{Ptr: &codecLeaf{Name: "p", Key: []byte{}, N: 7}})
	addStruct("Omit.Far24", codecOmit{Far24: 1})
	addStruct("Omit.Far256", codecOmit{Far256: true})
	addStruct("Omit.Far65536", codecOmit{Far65536: "far"})
	full := codecFullOmit()
	addStruct("Omit full", full)

	// NoOmit: nil versus empty.
	addStruct("NoOmit zero", codecNoOmit{})
	addStruct("NoOmit.Bytes empty", codecNoOmit{Bytes: []byte{}})
	addStruct("NoOmit.Bytes one", codecNoOmit{Bytes: []byte{0xff}})
	addStruct("NoOmit.Leaves empty", codecNoOmit{Leaves: []codecLeaf{}})
	addStruct("NoOmit.Leaves zero leaf", codecNoOmit{Leaves: []codecLeaf{{}}})
	addStruct("NoOmit.Ptr zero leaf", codecNoOmit{Ptr: &codecLeaf{}})
	addStruct("NoOmit.F64 -0", codecNoOmit{F64: math.Copysign(0, -1)})
	addStruct("NoOmit.F64 NaN", codecNoOmit{F64: math.NaN()})
	addStruct("NoOmit full", codecFullNoOmit())

	addStruct("Leaf zero", codecLeaf{})
	addStruct("Leaf empty key", codecLeaf{Key: []byte{}})
	addStruct("Leaf full", codecLeaf{Name: "n", Key: []byte{1}, N: math.MinInt64})
	addStruct("Mid zero", codecMid{})
	addStruct("Mid leaf", codecMid{Leaf: &codecLeaf{Name: "m"}})
	addStruct("Top zero", codecTop{})
	addStruct("Top zero mid", codecTop{Mids: []codecMid{{}}})
	addStruct("Top zero ptr", codecTop{Mid: &codecMid{}})
	addStruct("Top full", codecFullTop())

	addStruct("Wide zero", codecWide{})
	for _, n := range []int{1, 23, 24, 25} {
		addStruct(fmt.Sprintf("Wide %d fields", n), codecWideN(n))
	}
	return f, firstErr
}

func codecFullOmit() codecOmit {
	return codecOmit{
		Int: -1000, I64: math.MinInt64, U8: 200, U16: 40000, U32: 3000000000, U64: math.MaxUint64,
		Bool: true, F64: 1.5, Str: "full", Bytes: []byte{0xde, 0xad},
		List: [][]byte{{1}, {}}, ListN: [][]byte{nil, {2}}, Strs: []string{"a", "bc"},
		U16s: []uint16{1, 65535}, Leaves: []codecLeaf{{Name: "l", Key: []byte{3}}},
		Ptr: &codecLeaf{Key: []byte{}}, Far24: 24, Far256: true, Far65536: "x",
	}
}

func codecFullNoOmit() codecNoOmit {
	return codecNoOmit{
		Int: 1, I64: -2, U8: 3, U16: 4, U32: 5, U64: 6, Bool: true, F64: 0.1, Str: "s",
		Bytes: []byte{7}, Leaves: []codecLeaf{{Name: "n", Key: []byte{8}, N: 9}}, Ptr: &codecLeaf{},
	}
}

func codecFullTop() codecTop {
	return codecTop{
		Mids: []codecMid{
			{Leaves: []codecLeaf{{Name: "a"}, {Key: []byte{1}}}, Leaf: &codecLeaf{Key: []byte{}}},
			{},
		},
		Mid:  &codecMid{Leaf: &codecLeaf{Name: "b", N: 1}},
		Name: "t",
	}
}

// codecWideN sets the first n fields of a codecWide to 1.
func codecWideN(n int) codecWide {
	var w codecWide
	v := reflect.ValueOf(&w).Elem()
	for i := 0; i < n; i++ {
		v.Field(i).SetUint(1)
	}
	return w
}

// ---------------------------------------------------------------------------
// codec/decode.json

type codecDecodeFile struct {
	Cases []codecDecodeCase `json:"cases"`
}

// codecDecodeCase is codec.Unmarshal of the input into a new value of Type.
// The input is InputHex, then RepeatHex RepeatCount times, then SuffixHex.
// WellformedError is DecOptions{}.DecMode().Wellformed (pass 1 alone). With
// GoError null, Value is the decoded value and ValueCBORHex its codec.Marshal
// re-encoding; otherwise both are null.
type codecDecodeCase struct {
	Name            string  `json:"name"`
	Type            string  `json:"type"`
	InputHex        string  `json:"input_hex"`
	RepeatHex       string  `json:"repeat_hex,omitempty"`
	RepeatCount     int     `json:"repeat_count,omitempty"`
	SuffixHex       string  `json:"suffix_hex,omitempty"`
	WellformedError *string `json:"wellformed_error"`
	GoError         *string `json:"go_error"`
	Value           any     `json:"value"`
	ValueCBORHex    *string `json:"value_cbor_hex"`
}

type codecDecodeGen struct {
	dm       cbor.DecMode
	cases    []codecDecodeCase
	names    map[string]bool
	firstErr error
}

func (g *codecDecodeGen) fail(err error) {
	if g.firstErr == nil {
		g.firstErr = err
	}
}

func (g *codecDecodeGen) add(name, typ string, input []byte) {
	g.addRepeat(name, typ, input, nil, 0, nil)
}

func (g *codecDecodeGen) addRepeat(name, typ string, prefix, repeat []byte, count int, suffix []byte) {
	if g.names[name] {
		g.fail(fmt.Errorf("codec: duplicate decode case %q", name))
		return
	}
	g.names[name] = true
	in := append([]byte(nil), prefix...)
	in = append(in, bytes.Repeat(repeat, count)...)
	in = append(in, suffix...)
	c := codecDecodeCase{
		Name: name, Type: typ, InputHex: hex.EncodeToString(prefix),
		RepeatHex: hex.EncodeToString(repeat), RepeatCount: count, SuffixHex: hex.EncodeToString(suffix),
	}
	if err := g.dm.Wellformed(in); err != nil {
		s := err.Error()
		c.WellformedError = &s
	}
	p, err := codecNew(typ)
	if err != nil {
		g.fail(err)
		return
	}
	if err := codec.Unmarshal(in, p); err != nil {
		s := err.Error()
		c.GoError = &s
	} else {
		v := reflect.ValueOf(p).Elem()
		j, err := codecJSON(v)
		if err != nil {
			g.fail(err)
			return
		}
		c.Value = j
		out, err := codec.Marshal(v.Interface())
		if err != nil {
			g.fail(err)
			return
		}
		h := hex.EncodeToString(out)
		c.ValueCBORHex = &h
	}
	g.cases = append(g.cases, c)
}

// codecShapes are single CBOR items crossed with every field kind: canonical
// and non-canonical heads, integer width boundaries, definite and indefinite
// strings, arrays and maps, simple values, floats, null and undefined, and
// tags (self-described, unknown, bignums, and built-in tags with bad content).
var codecShapes = []string{
	"00", "01", "17", "1818", "18ff", "190100", "19ffff", "1a00010000", "1affffffff",
	"1b0000000100000000", "1b7fffffffffffffff", "1b8000000000000000", "1bffffffffffffffff",
	"1800", "190017", "1b000000000000ffff",
	"20", "37", "3818", "38ff", "390100", "3a7fffffff", "3a80000000",
	"3b7fffffffffffffff", "3b8000000000000000", "3bffffffffffffffff", "3800",
	"40", "4161", "420102", "5800", "5f41614162ff", "5fff", "5f40ff",
	"60", "6161", "61ff", "62c3a9", "63eda080", "7800", "7f6161ff", "7fff", "7f61c361a9ff",
	"80", "8100", "8101", "81f6", "81f7", "8118ff", "81190100", "8120", "814161", "816161",
	"8180", "81a0", "81f5", "81f93e00", "81c24105", "81c24201ff", "81c34100", "81d82a01",
	"8240f6", "9f01ff", "9fff", "9f4161ff",
	"a0", "a10001", "a1006161", "bfff", "bf0001ff",
	"f4", "f5", "f6", "f7", "e0", "f0", "f3", "f8ff", "f820",
	"f90000", "f98000", "f93e00", "f97e00", "f97c00", "f9fc00", "fa3fc00000",
	"fb3fb999999999999a", "fb7ff8000000000001", "fbc010000000000000",
	"c24105", "c240", "c2420001", "c249010000000000000000", "c2480000000000000100",
	"c2488000000000000000", "c248ffffffffffffffff", "c25f4101ff",
	"c34101", "c340", "c3487fffffffffffffff", "c3488000000000000000", "c349010000000000000000",
	"c06161", "c001", "c16161", "c101", "c1f93e00", "c120", "c201", "c36161", "c2c24101",
	"d82a4161", "d82a00", "d82a80", "d82aa0", "d82af6", "d82ac24105",
	"d9d9f74161", "d9d9f700", "d9d9f7f6", "d9d9f7a0", "c6d9d9f700", "c6c601", "d9d9f7c24105",
}

// codecOmitKeys are the Omit fields crossed with every shape.
var codecOmitKeys = []struct {
	field string
	key   string
}{
	{"Int", "00"}, {"I64", "01"}, {"U8", "02"}, {"U16", "03"}, {"U32", "04"}, {"U64", "05"},
	{"Bool", "06"}, {"F64", "07"}, {"Str", "08"}, {"Bytes", "09"}, {"List", "0a"}, {"ListN", "0b"},
	{"Strs", "0c"}, {"U16s", "0d"}, {"Leaves", "0e"}, {"Ptr", "0f"},
}

func codecDecodeVectors() (*codecDecodeFile, error) {
	dm, err := cbor.DecOptions{}.DecMode()
	if err != nil {
		return nil, err
	}
	g := &codecDecodeGen{dm: dm, names: map[string]bool{}}
	h := codecHex

	// The field matrix.
	for _, k := range codecOmitKeys {
		for _, s := range codecShapes {
			g.add("Omit."+k.field+" = "+s, "Omit", h("a1", k.key, s))
		}
	}
	for _, k := range []struct{ field, key string }{{"Bytes", "09"}, {"Leaves", "0e"}, {"Ptr", "0f"}} {
		for _, s := range codecShapes {
			g.add("NoOmit."+k.field+" = "+s, "NoOmit", h("a1", k.key, s))
		}
	}
	// Slice elements.
	for _, k := range []struct{ field, key string }{
		{"Bytes", "09"}, {"List", "0a"}, {"ListN", "0b"}, {"Strs", "0c"}, {"U16s", "0d"}, {"Leaves", "0e"},
	} {
		for _, s := range codecShapes {
			g.add("Omit."+k.field+"[0] = "+s, "Omit", h("a1", k.key, "81", s))
		}
	}
	// Fields of a struct inside a slice of an outer struct.
	for _, k := range []struct{ field, key string }{{"Name", "00"}, {"Key", "01"}, {"N", "02"}} {
		for _, s := range codecShapes {
			g.add("Omit.Leaves[0]."+k.field+" = "+s, "Omit", h("a1 0e 81 a1", k.key, s))
		}
	}
	// Top-level items.
	for _, s := range codecShapes {
		g.add("top-level "+s, "Omit", h(s))
	}
	for _, s := range []string{"f6", "f7", "a0", "00", "80", "c24105", "d9d9f7a0", "c6a0"} {
		g.add("NoOmit top-level "+s, "NoOmit", h(s))
		g.add("Leaf top-level "+s, "Leaf", h(s))
		g.add("Top top-level "+s, "Top", h(s))
	}

	g.structureCases()
	return &codecDecodeFile{Cases: g.cases}, g.firstErr
}

func (g *codecDecodeGen) structureCases() {
	h := codecHex
	rep := func(s string, n int) string { return strings.Repeat(s, n) }

	// Empty input, trailing data, stray breaks.
	for _, c := range []struct{ name, in string }{
		{"empty input", ""},
		{"trailing byte after map", "a000"},
		{"trailing map after map", "a0a0"},
		{"trailing byte after null", "f601"},
		{"trailing byte after wrong type", "0000"},
		{"trailing byte after field", "a1000501"},
		{"trailing break after map", "a0ff"},
		{"trailing two bytes", "a1000100ff"},
		{"trailing after invalid utf8 field", "a10861ff00"},
		{"break at top level", "ff"},
		{"break in definite array", "a10a81ff"},
		{"break as map value", "a100ff"},
		{"break as map key", "a1ff00"},
		{"break after tag", "c6ff"},
	} {
		g.add(c.name, "Omit", h(c.in))
	}

	// Additional information 28-30 on every major, 31 on majors 0, 1 and 6.
	for major := 0; major < 8; major++ {
		for _, ai := range []int{28, 29, 30} {
			b := fmt.Sprintf("%02x", major<<5|ai)
			g.add("top-level ai "+b, "Omit", h(b))
			g.add("Omit.Int = ai "+b, "Omit", h("a100", b))
		}
	}
	for _, b := range []string{"1f", "3f", "df"} {
		g.add("top-level ai 31 "+b, "Omit", h(b))
		g.add("Omit.Int = ai 31 "+b, "Omit", h("a100", b))
	}
	// Two-byte simple values.
	for v := 0; v <= 33; v++ {
		b := fmt.Sprintf("f8%02x", v)
		g.add("top-level simple "+b, "Omit", h(b))
		g.add("Omit.U8 = simple "+b, "Omit", h("a102", b))
	}

	// Indefinite-length strings, arrays and maps.
	for _, c := range []struct{ name, in string }{
		{"indefinite bstr chunks", "a1095f4101410241ff"},
		{"indefinite bstr empty chunks", "a1095f4040ff"},
		{"indefinite bstr text chunk", "a1095f6161ff"},
		{"indefinite bstr nested indefinite", "a1095f5fffff"},
		{"indefinite bstr integer chunk", "a1095f00ff"},
		{"indefinite bstr chunk ai 28", "a1095f5cff"},
		{"indefinite bstr truncated chunk", "a1095f41"},
		{"indefinite bstr unterminated", "a1095f4161"},
		{"indefinite tstr chunks", "a1087f61616262ff"},
		{"indefinite tstr bytes chunk", "a1087f4161ff"},
		{"indefinite tstr nested indefinite", "a1087f7fffff"},
		{"indefinite tstr split rune", "a1087f61c361a9ff"},
		{"indefinite tstr second chunk invalid", "a1087f616161ffff"},
		{"indefinite bstr under unknown key text chunk", "a118635f6161ff"},
		{"indefinite map", "bf0001ff"},
		{"indefinite map empty", "bfff"},
		{"indefinite map odd items", "bf00ff"},
		{"indefinite map odd items nested", "a11863bf00ff"},
		{"indefinite map unterminated", "bf0001"},
		{"indefinite array unterminated", "a10d9f01"},
		{"indefinite array of leaves", "a10e9fa100616ea0ff"},
		{"indefinite leaf map", "a10e81bf00616eff"},
		{"indefinite nested arrays into bytes", "a10b9f9f01ffff"},
		{"indefinite map with indefinite values", "bf095f41ff0c9f6161ff0e9fbfffffff"},
	} {
		g.add(c.name, "Omit", h(c.in))
	}

	// Map key types.
	for _, c := range []struct{ name, typ, in string }{
		{"key bstr", "Omit", "a1410001"},
		{"key empty bstr", "Omit", "a14001"},
		{"key float16", "Omit", "a1f93e0001"},
		{"key float64", "Omit", "a1fb3ff000000000000001"},
		{"key array", "Omit", "a18001"},
		{"key map", "Omit", "a1a001"},
		{"key tag", "Omit", "a1c60001"},
		{"key self-described tag", "Omit", "a1d9d9f70001"},
		{"key false", "Omit", "a1f401"},
		{"key null", "Omit", "a1f601"},
		{"key simple", "Omit", "a1f001"},
		{"key negint", "Omit", "a12001"},
		{"key negint max int64", "Omit", "a13b7fffffffffffffff01"},
		{"key negint overflow", "Omit", "a13b800000000000000001"},
		{"key negint max overflow", "Omit", "a13bffffffffffffffff01"},
		{"key uint max int64", "Omit", "a11b7fffffffffffffff01"},
		{"key uint overflow", "Omit", "a11b800000000000000001"},
		{"key uint max", "Omit", "a11bffffffffffffffff01"},
		{"key text digit", "Omit", "a1613001"},
		{"key text field name", "Omit", "a163496e7401"},
		{"key text lowercase name", "Omit", "a163696e7401"},
		{"key text invalid utf8", "Omit", "a161ff01"},
		{"key indefinite text", "Omit", "a17f6130ff01"},
		{"key indefinite text invalid", "Omit", "a17f61ffff01"},
		{"key non-canonical 0", "Omit", "a1180001"},
		{"key non-canonical 8", "Omit", "a11b00000000000000086161"},
		{"key bstr in leaf", "Omit", "a10e81a1410001"},
		{"key uint overflow in leaf", "Omit", "a10e81a11bffffffffffffffff01"},
		{"key negint overflow in ptr", "Omit", "a10fa13b800000000000000001"},
		{"key text invalid in leaf", "Omit", "a10e81a161ff01"},
		{"key float deep", "Top", "a10081a10081a1f93e0001"},
		{"key array deep ptr", "Top", "a101a101a18001"},
	} {
		g.add(c.name, c.typ, h(c.in))
	}

	// Duplicate keys.
	for _, c := range []struct{ name, in string }{
		{"duplicate same type", "a200010002"},
		{"duplicate different type skipped unchecked", "a2086161080a"},
		{"duplicate first wrong type", "a20800086161"},
		{"duplicate after null", "a208f6086161"},
		{"duplicate unknown keys", "a2186300186361ff"},
		{"duplicate three", "a3000118630000 02"},
		{"duplicate indefinite map", "bf00010002ff"},
		{"duplicate in leaf", "a10e81a2006161000a"},
		{"duplicate ptr null then map", "a20ff60fa100616e"},
		{"duplicate container skipped unexamined", "a20e800e8100"},
		{"duplicate bad tag skipped unexamined", "a2080f08c000"},
		{"keys out of order", "a2010500 04"},
	} {
		g.add(c.name, "Omit", h(c.in))
	}

	// The first error in document order wins; pass 1 precedes pass 2.
	for _, c := range []struct{ name, typ, in string }{
		{"order two type errors", "Omit", "a208000060"},
		{"order utf8 then type", "Omit", "a20861ff0060"},
		{"order type then utf8", "Omit", "a200600861ff"},
		{"order pass 1 precedes pass 2", "Omit", "a2080018635c"},
		{"order type then key overflow", "Omit", "a200601b800000000000000000"},
		{"order key overflow then type", "Omit", codecJoin("a2", "1b8000000000000000", "00", "00", "60")},
		{"order map key then type", "Omit", "a24100000060"},
		{"order element errors", "Omit", "a10d8261616262"},
		{"order nested then outer", "Omit", "a20e81a10000 0800"},
		{"order bad tag then type", "Omit", "a208c0000060"},
		{"order top-level extraneous precedes type", "Omit", "0505"},
	} {
		g.add(c.name, c.typ, h(c.in))
	}

	// Skipped values are never examined.
	for _, c := range []struct{ name, in string }{
		{"unknown key invalid utf8 value", "a1186361ff"},
		{"unknown key bad tag content", "a11863c001"},
		{"unknown key bignum without bytes", "a11863c201"},
		{"unknown key nested indefinite", "a118639f5f4100ffff"},
		{"unknown key simple value", "a11863f8ff"},
		{"unknown negint key bad value", "a12061ff"},
		{"unknown text key bad value", "a1617861ff"},
	} {
		g.add(c.name, "Omit", h(c.in))
	}

	// nil versus empty, and pointers.
	for _, c := range []struct{ name, typ, in string }{
		{"Omit.Ptr tagged null", "Omit", "a10fc6f6"},
		{"Omit.Ptr self-described null", "Omit", "a10fd9d9f7f6"},
		{"Omit.Ptr indefinite empty map", "Omit", "a10fbfff"},
		{"Omit.Ptr tagged map", "Omit", "a10fc6a100616e"},
		{"NoOmit.Ptr tagged null", "NoOmit", "a10fc6f6"},
		{"NoOmit.Bytes tagged null", "NoOmit", "a109c6f6"},
		{"NoOmit.Leaves tagged null", "NoOmit", "a10ec6f6"},
		{"NoOmit.Leaves null element", "NoOmit", "a10e81f6"},
		{"Leaf.Key null", "Omit", "a10e81a101f6"},
		{"Leaf.Key empty", "Omit", "a10e81a10140"},
		{"Leaf.Key tagged null", "Omit", "a10e81a101c6f6"},
		{"Leaf.Key empty array", "Omit", "a10e81a10180"},
		{"Omit.List null and empty elements", "Omit", "a10a82f640"},
		{"Omit.ListN null and empty elements", "Omit", "a10b82f640"},
		{"Omit.ListN tagged null element", "Omit", "a10b81c6f6"},
		{"Omit.Bytes null element", "Omit", "a10982f601"},
		{"Omit.Leaves null and empty elements", "Omit", "a10e82f6a0"},
		{"NoOmit all null", "NoOmit", "ac00f601f602f603f604f605f606f607f608f609f60ef60ff6"},
		{"NoOmit all empty", "NoOmit", "a30940 0e80 0fa0"},
	} {
		g.add(c.name, c.typ, h(c.in))
	}

	// Errors name the outermost struct field.
	for _, c := range []struct{ name, in string }{
		{"deep leaf name", "a10081a10081a10000"},
		{"deep ptr leaf name", "a101a101a10000"},
		{"deep leaf key", "a10081a101a10100"},
		{"deep leaf key element", "a10081a10081a101816161"},
		{"deep array into ptr", "a101a10180"},
		{"deep integer into mid", "a1008100"},
		{"deep integer overflow", "a10081a10081a1021bffffffffffffffff"},
		{"deep bignum overflow", "a101a101a102c249010000000000000000"},
		{"deep utf8 not rewritten", "a10081a10081a10061ff"},
		{"deep bad tag not rewritten", "a10081a10081a100c000"},
		{"deep second mid", "a10082a0a10100"},
	} {
		g.add("Top "+c.name, "Top", h(c.in))
	}

	// Nesting limits.
	for _, n := range []int{30, 31, 32} {
		g.add(fmt.Sprintf("nested arrays under List %d", n), "Omit", h("a10a", rep("81", n), "40"))
		g.add(fmt.Sprintf("nested arrays under unknown key %d", n), "Omit", h("a11863", rep("81", n), "00"))
		g.add(fmt.Sprintf("nested indefinite arrays under unknown key %d", n), "Omit", h("a11863", rep("9f", n), rep("ff", n)))
		g.add(fmt.Sprintf("nested maps under unknown key %d", n), "Omit", h("a11863", rep("a100", n), "00"))
		g.add(fmt.Sprintf("nested tag and array under unknown key %d", n), "Omit", h("a11863", rep("c681", n), "00"))
	}
	for _, n := range []int{31, 32, 33} {
		g.add(fmt.Sprintf("nested tags under Str %d", n), "Omit", h("a108", rep("c6", n), "6161"))
		g.add(fmt.Sprintf("nested self-described tags under Str %d", n), "Omit", h("a108", rep("d9d9f7", n), "6161"))
	}
	for _, n := range []int{32, 33, 34} {
		g.add(fmt.Sprintf("nested tags at top level %d", n), "Omit", h(rep("c6", n), "a0"))
	}
	for _, n := range []int{32, 33} {
		g.add(fmt.Sprintf("nested arrays at top level %d", n), "Omit", h(rep("81", n), "00"))
	}
	g.add("nested arrays 1000", "Omit", h("a11863", rep("81", 1000), "00"))
	g.addRepeat("nested tags 100000", "Omit", h("a108"), h("c6"), 100000, h("6161"))

	// Counts and lengths.
	for _, c := range []struct{ name, in string }{
		{"array claims 131072 elements", "a118639a00020000"},
		{"array claims 131073 elements", "a118639a00020001"},
		{"map claims 131072 pairs", "a11863ba00020000"},
		{"map claims 131073 pairs", "a11863ba00020001"},
		{"top-level map claims 131073 pairs", "ba00020001"},
		{"array length beyond int64", "a118639b8000000000000000"},
		{"map length beyond int64", "a11863bb8000000000000000"},
		{"array length max int64", "a118639b7fffffffffffffff"},
		{"bstr length beyond int64", "a1095b8000000000000000"},
		{"tstr length beyond int64", "a1087bffffffffffffffff"},
		{"bstr length max int64", "a1095b7fffffffffffffff"},
		{"bstr claims more than input", "a1095820"},
	} {
		g.add(c.name, "Omit", h(c.in))
	}
	g.addRepeat("array of 131072 under unknown key", "Omit", h("a118639a00020000"), h("00"), 131072, nil)
	g.addRepeat("indefinite array of 131072 under unknown key", "Omit", h("a118639f"), h("00"), 131072, h("ff"))
	g.addRepeat("indefinite array of 131073 under unknown key", "Omit", h("a118639f"), h("00"), 131073, h("ff"))
	g.addRepeat("map of 131072 pairs under unknown key", "Omit", h("a11863ba00020000"), h("0000"), 131072, nil)
	g.addRepeat("indefinite map of 131073 pairs under unknown key", "Omit", h("a11863bf"), h("0000"), 131073, h("ff"))
	g.addRepeat("indefinite map of 131072 pairs and a key", "Omit", h("a11863bf"), h("0000"), 131072, h("00ff"))
	g.addRepeat("List of 131073 elements", "Omit", h("a10a9a00020001"), h("40"), 131073, nil)
	g.addRepeat("top-level indefinite map of 131073 pairs", "Omit", h("bf"), h("186300"), 131073, h("ff"))

	// Large bignums into integer fields: the overflow detail is the big.Int
	// decimal text of the whole content. Rust converts numbers above 64 limbs
	// (256 bytes) recursively with Karatsuba multiplication.
	pattern := make([]byte, 256)
	for i := range pattern {
		pattern[i] = byte(i*151 + 7)
	}
	g.addRepeat("bignum of 256 bytes into U64", "Omit", h("a105c2590100"), h("ab"), 256, nil)
	g.addRepeat("bignum of 257 bytes into U64", "Omit", h("a105c2590101"), h("ab"), 257, nil)
	g.addRepeat("negative bignum of 260 bytes ff into I64", "Omit", h("a101c3590104"), h("ff"), 260, nil)
	g.addRepeat("bignum of 4096 bytes ff into U64", "Omit", h("a105c2591000"), h("ff"), 4096, nil)
	g.addRepeat("negative bignum of 4096 bytes into Int", "Omit", h("a100c3591000"), pattern, 16, nil)
	g.addRepeat("bignum of 16384 bytes deep in Top", "Top", h("a10081a10081a102c2594000"), pattern, 64, nil)

	// Far keys and the wide struct.
	for _, c := range []struct{ name, typ, in string }{
		{"Omit.Far24", "Omit", "a1181801"},
		{"Omit.Far256", "Omit", "a1190100f5"},
		{"Omit.Far65536", "Omit", "a11a000100006161"},
		{"Omit.Far24 non-canonical key", "Omit", "a11b000000000000001801"},
		{"Omit.Far256 wrong type", "Omit", "a119010001"},
		{"Omit.Far65536 wrong type", "Omit", "a11a0001000000"},
		{"Omit key 23 unknown", "Omit", "a11701"},
		{"Wide two-byte head", "Wide", codecJoin("b819", codecWideBody(25))},
		{"Wide indefinite", "Wide", codecJoin("bf", codecWideBody(25), "ff")},
		{"Wide wrong type in last field", "Wide", codecJoin("b819", codecWideBody(24), "181840")},
	} {
		g.add(c.name, c.typ, h(c.in))
	}

	// Round trips of canonical encodings, and truncations of them.
	fullOmit, err := codec.Marshal(codecFullOmit())
	if err != nil {
		g.fail(err)
		return
	}
	fullTop, err := codec.Marshal(codecFullTop())
	if err != nil {
		g.fail(err)
		return
	}
	fullNoOmit, err := codec.Marshal(codecFullNoOmit())
	if err != nil {
		g.fail(err)
		return
	}
	g.add("Omit full", "Omit", fullOmit)
	g.add("NoOmit full", "NoOmit", fullNoOmit)
	g.add("Top full", "Top", fullTop)
	for i := 1; i < len(fullOmit); i++ {
		g.add(fmt.Sprintf("Omit full truncated to %d bytes", i), "Omit", fullOmit[:i])
	}
	for i := 1; i < len(fullTop); i++ {
		g.add(fmt.Sprintf("Top full truncated to %d bytes", i), "Top", fullTop[:i])
	}
}

// codecJoin joins hex parts.
func codecJoin(parts ...string) string {
	return strings.Join(parts, "")
}

// codecWideBody encodes keys 0..n-1 each with the value 1.
func codecWideBody(n int) string {
	var b strings.Builder
	for i := 0; i < n; i++ {
		if i < 24 {
			fmt.Fprintf(&b, "%02x01", i)
		} else {
			fmt.Fprintf(&b, "18%02x01", i)
		}
	}
	return b.String()
}
