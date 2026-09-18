package main

import (
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strconv"

	"github.com/zeebo/blake3"
)

// splitmixNext advances the splitmix64 generator exactly as VECTORS.md defines.
func splitmixNext(state *uint64) uint64 {
	*state += 0x9E3779B97F4A7C15
	z := *state
	z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9
	z = (z ^ (z >> 27)) * 0x94D049BB133111EB
	return z ^ (z >> 31)
}

// smData is data(seed, n): splitmix64 outputs appended as 8 little-endian
// bytes, truncated to n bytes.
func smData(seed uint64, n int) []byte {
	out := make([]byte, 0, n+8)
	state := seed
	for len(out) < n {
		out = binary.LittleEndian.AppendUint64(out, splitmixNext(&state))
	}
	return out[:n]
}

// u64s is u64s(seed, n): the first n splitmix64 outputs.
func u64s(seed uint64, n int) []uint64 {
	out := make([]uint64, n)
	state := seed
	for i := range out {
		out[i] = splitmixNext(&state)
	}
	return out
}

// U64 marshals a uint64 as a decimal JSON string: 64-bit values exceed JSON's
// 2^53 exact range.
type U64 uint64

// MarshalJSON writes the decimal string.
func (v U64) MarshalJSON() ([]byte, error) {
	return json.Marshal(strconv.FormatUint(uint64(v), 10))
}

// I64 marshals an int64 as a decimal JSON string.
type I64 int64

// MarshalJSON writes the decimal string.
func (v I64) MarshalJSON() ([]byte, error) {
	return json.Marshal(strconv.FormatInt(int64(v), 10))
}

// Hex marshals bytes as a lowercase hex string. A nil slice marshals as null
// and an empty one as "", so vectors keep Go's nil-versus-empty distinction.
type Hex []byte

// MarshalJSON writes the hex string, or null for nil.
func (b Hex) MarshalJSON() ([]byte, error) {
	if b == nil {
		return []byte("null"), nil
	}
	return json.Marshal(hex.EncodeToString(b))
}

// Payload describes a deterministic byte payload in a vector file:
// {"hex": "..."} for inline bytes, or {"seed": "S", "len": N} for data(S, N).
// Rust: dstore_testkit::golden::Payload.
type Payload struct {
	Hex  *string `json:"hex,omitempty"`
	Seed *U64    `json:"seed,omitempty"`
	Len  *int    `json:"len,omitempty"`
}

// SM describes data(seed, n).
func SM(seed uint64, n int) Payload {
	s := U64(seed)
	return Payload{Seed: &s, Len: &n}
}

// Inline describes b verbatim.
func Inline(b []byte) Payload {
	h := hex.EncodeToString(b)
	return Payload{Hex: &h}
}

// Materialize produces the payload's bytes.
func (p Payload) Materialize() []byte {
	switch {
	case p.Hex != nil:
		b, err := hex.DecodeString(*p.Hex)
		if err != nil {
			panic(fmt.Sprintf("payload: bad hex %q: %v", *p.Hex, err))
		}
		return b
	case p.Seed != nil && p.Len != nil:
		return smData(uint64(*p.Seed), *p.Len)
	default:
		panic("payload: neither hex nor seed and len")
	}
}

// writeJSON writes v with two-space indentation plus a trailing newline to
// path, creating parent directories. Determinism: v must be built from
// structs and ordered slices only, never bare maps, so field order is fixed by
// declaration order.
func writeJSON(path string, v any) error {
	b, err := json.MarshalIndent(v, "", "  ")
	if err != nil {
		return err
	}
	return writeFile(path, append(b, '\n'))
}

// writeFile writes b to path (0644), creating parent directories.
func writeFile(path string, b []byte) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	return os.WriteFile(path, b, 0o644)
}

// blake3Hex returns the lowercase hex BLAKE3-256 digest of b.
func blake3Hex(b []byte) string {
	sum := blake3.Sum256(b)
	return hex.EncodeToString(sum[:])
}
