package main

// Families "wire" (wire/frames.json, wire/decode.json, wire/frame_errors.json)
// and "wire-pack" (wire/pack_frames.json, wire/pack_reader.json): client-ALPN
// frames, fxamacker decoding decisions, frame I/O edge cases and pack framing,
// all computed by dstore v0.1.9 wire/codec, transport-iroh v0.4.0 protocol and
// core v0.0.8 amberpack. Schemas: docs/vectorgen-proto.md.

import (
	"bytes"
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"hash/crc32"
	"io"
	"math"
	"path/filepath"
	"strconv"
	"strings"

	"github.com/amber-store/core/amberpack"
	"github.com/amber-store/core/key"
	"github.com/amber-store/core/reference"
	"github.com/amber-store/dstore/client"
	"github.com/amber-store/dstore/codec"
	"github.com/amber-store/dstore/node"
	"github.com/amber-store/dstore/ticket"
	"github.com/amber-store/dstore/view"
	"github.com/amber-store/dstore/wire"
	"github.com/amber-store/transport-iroh/protocol"
)

func init() {
	register("wire", []string{"wire/frames.json", "wire/decode.json", "wire/frame_errors.json"}, genWire)
	register("wire-pack", []string{"wire/pack_frames.json", "wire/pack_reader.json"}, genWirePack)
}

// genWire writes the wire family.
func genWire(out string) error {
	if err := genWireFrames(out); err != nil {
		return fmt.Errorf("frames: %w", err)
	}
	if err := genWireDecode(out); err != nil {
		return fmt.Errorf("decode: %w", err)
	}
	if err := genWireFrameErrors(out); err != nil {
		return fmt.Errorf("frame_errors: %w", err)
	}
	return nil
}

// genWirePack writes the wire-pack family.
func genWirePack(out string) error {
	if err := genPackFrames(out); err != nil {
		return fmt.Errorf("pack_frames: %w", err)
	}
	if err := genPackReader(out); err != nil {
		return fmt.Errorf("pack_reader: %w", err)
	}
	return nil
}

// ---- JSON value types (Rust field names of PORTING.md §4.3) ----

// wvRefInfo is wire.RefInfo; every field is always present.
type wvRefInfo struct {
	Name      string `json:"name"`
	Key       Hex    `json:"key"`
	Version   Hex    `json:"version"`
	CreatedAt I64    `json:"created_at"`
	User      string `json:"user"`
}

// wvKeyHolders is wire.KeyHolders.
type wvKeyHolders struct {
	Key     Hex   `json:"key"`
	Holders []Hex `json:"holders"`
}

// wvKeyFailure is wire.KeyFailure.
type wvKeyFailure struct {
	Key        Hex    `json:"key"`
	Node       Hex    `json:"node"`
	Reason     string `json:"reason"`
	RetryAfter I64    `json:"retry_after"`
}

// wvKeyReject is wire.KeyReject.
type wvKeyReject struct {
	Key    Hex    `json:"key"`
	Reason string `json:"reason"`
}

// wvScanRow is wire.ScanRow.
type wvScanRow struct {
	Reg      Hex  `json:"reg"`
	Promised Hex  `json:"promised"`
	Accepted Hex  `json:"accepted"`
	Value    Hex  `json:"value"`
	HasValue bool `json:"has_value"`
}

// wvMsg is wire.Msg, sparse: a field is present only when it is non-zero
// (every Msg field but typ is omitempty in Go).
type wvMsg struct {
	Typ             I64            `json:"typ"`
	ClusterID       Hex            `json:"cluster_id,omitempty"`
	Incarnation     U64            `json:"incarnation,omitempty"`
	Epoch           U64            `json:"epoch,omitempty"`
	Keys            []Hex          `json:"keys,omitempty"`
	Pin             bool           `json:"pin,omitempty"`
	Name            string         `json:"name,omitempty"`
	Record          Hex            `json:"record,omitempty"`
	Data            Hex            `json:"data,omitempty"`
	Key             Hex            `json:"key,omitempty"`
	Code            string         `json:"code,omitempty"`
	Text            string         `json:"text,omitempty"`
	View            Hex            `json:"view,omitempty"`
	Version         Hex            `json:"version,omitempty"`
	ExpectedVersion Hex            `json:"expected_version,omitempty"`
	ExpectedOld     Hex            `json:"expected_old,omitempty"`
	Force           bool           `json:"force,omitempty"`
	HasExpected     bool           `json:"has_expected,omitempty"`
	Prefix          Hex            `json:"prefix,omitempty"`
	After           Hex            `json:"after,omitempty"`
	Limit           I64            `json:"limit,omitempty"`
	Refs            []wvRefInfo    `json:"refs,omitempty"`
	Next            Hex            `json:"next,omitempty"`
	Holders         []wvKeyHolders `json:"holders,omitempty"`
	Failed          []wvKeyFailure `json:"failed,omitempty"`
	Rejected        []wvKeyReject  `json:"rejected,omitempty"`
	Short           []wvKeyHolders `json:"short,omitempty"`
	Unreachable     []Hex          `json:"unreachable,omitempty"`
	RetryAfter      I64            `json:"retry_after,omitempty"`
	Current         Hex            `json:"current,omitempty"`
	Shortfall       I64            `json:"shortfall,omitempty"`
	HasCurrent      bool           `json:"has_current,omitempty"`
	Reg             Hex            `json:"reg,omitempty"`
	Ballot          Hex            `json:"ballot,omitempty"`
	Value           Hex            `json:"value,omitempty"`
	HasValue        bool           `json:"has_value,omitempty"`
	Accepted        Hex            `json:"accepted,omitempty"`
	Promised        Hex            `json:"promised,omitempty"`
	NotAfter        I64            `json:"not_after,omitempty"`
	Rows            []wvScanRow    `json:"rows,omitempty"`
	More            bool           `json:"more,omitempty"`
	Since           U64            `json:"since,omitempty"`
	Token           Hex            `json:"token,omitempty"`
	Weight          uint32         `json:"weight,omitempty"`
	Zone            string         `json:"zone,omitempty"`
	Addrs           []string       `json:"addrs,omitempty"`
	NoVote          bool           `json:"no_vote,omitempty"`
	G               U64            `json:"g,omitempty"`
	Nonce           Hex            `json:"nonce,omitempty"`
	Seq             U64            `json:"seq,omitempty"`
	Expand          bool           `json:"expand,omitempty"`
	Params          Hex            `json:"params,omitempty"`
	Sent            U64            `json:"sent,omitempty"`
	Received        U64            `json:"received,omitempty"`
	Idle            bool           `json:"idle,omitempty"`
	Marked          U64            `json:"marked,omitempty"`
	Missing         []Hex          `json:"missing,omitempty"`
	Status          Hex            `json:"status,omitempty"`
	Node            Hex            `json:"node,omitempty"`
	Error           string         `json:"error,omitempty"`
	Pattern         string         `json:"pattern,omitempty"`
	Deleted         []string       `json:"deleted,omitempty"`
}

// wvProtoRefInfo is protocol.RefInfo.
type wvProtoRefInfo struct {
	Name      string `json:"name"`
	Key       Hex    `json:"key"`
	CreatedAt I64    `json:"created_at"`
	User      string `json:"user"`
}

// wvProtoEndpoint is protocol.DataEndpointRec.
type wvProtoEndpoint struct {
	ID    Hex      `json:"id"`
	Addrs []string `json:"addrs"`
}

// wvProtoMsg is protocol.Msg, sparse like wvMsg.
type wvProtoMsg struct {
	Typ           I64               `json:"typ"`
	Name          string            `json:"name,omitempty"`
	Root          Hex               `json:"root,omitempty"`
	CAS           bool              `json:"cas,omitempty"`
	ExpectedOld   Hex               `json:"expected_old,omitempty"`
	Record        Hex               `json:"record,omitempty"`
	Refs          []wvProtoRefInfo  `json:"refs,omitempty"`
	Keys          []Hex             `json:"keys,omitempty"`
	Data          Hex               `json:"data,omitempty"`
	Key           Hex               `json:"key,omitempty"`
	Code          string            `json:"code,omitempty"`
	Text          string            `json:"text,omitempty"`
	Current       Hex               `json:"current,omitempty"`
	Token         Hex               `json:"token,omitempty"`
	DataConns     I64               `json:"data_conns,omitempty"`
	DataPorts     []uint16          `json:"data_ports,omitempty"`
	DataEndpoints []wvProtoEndpoint `json:"data_endpoints,omitempty"`
	Names         []string          `json:"names,omitempty"`
}

// wvHexList converts [][]byte, keeping nil elements as null; the list itself
// is never null.
func wvHexList(raw [][]byte) []Hex {
	out := make([]Hex, len(raw))
	for i, b := range raw {
		out[i] = Hex(b)
	}
	return out
}

// wvHexE converts an omitempty []byte: nil and empty both give "".
func wvHexE(b []byte) Hex {
	if b == nil {
		return Hex{}
	}
	return Hex(b)
}

// wvStrList converts an omitempty []string: never null.
func wvStrList(s []string) []string {
	if s == nil {
		return []string{}
	}
	return s
}

func wvKeyHoldersJSON(in []wire.KeyHolders) []wvKeyHolders {
	out := make([]wvKeyHolders, len(in))
	for i, kh := range in {
		out[i] = wvKeyHolders{Key: Hex(kh.Key), Holders: wvHexList(kh.Holders)}
	}
	return out
}

// wvMsgJSON converts a wire.Msg.
func wvMsgJSON(m *wire.Msg) wvMsg {
	j := wvMsg{
		Typ: I64(m.Type), ClusterID: m.ClusterID, Incarnation: U64(m.Incarnation), Epoch: U64(m.Epoch),
		Pin: m.Pin, Name: m.Name, Record: m.Record, Data: m.Data, Key: m.Key, Code: m.Code, Text: m.Text,
		View: m.View, Version: m.Version, ExpectedVersion: m.ExpectedVersion, ExpectedOld: m.ExpectedOld,
		Force: m.Force, HasExpected: m.HasExpected, Prefix: m.Prefix, After: m.After, Limit: I64(m.Limit),
		Next: m.Next, RetryAfter: I64(m.RetryAfter), Current: m.Current, Shortfall: I64(m.Shortfall),
		HasCurrent: m.HasCurrent, Reg: m.Reg, Ballot: m.Ballot, Value: m.Value, HasValue: m.HasValue,
		Accepted: m.Accepted, Promised: m.Promised, NotAfter: I64(m.NotAfter), More: m.More, Since: U64(m.Since),
		Token: m.Token, Weight: m.Weight, Zone: m.Zone, NoVote: m.NoVote, G: U64(m.G), Nonce: m.Nonce,
		Seq: U64(m.Seq), Expand: m.Expand, Params: m.Params, Sent: U64(m.Sent), Received: U64(m.Received),
		Idle: m.Idle, Marked: U64(m.Marked), Status: m.Status, Node: m.Node, Error: m.Error, Pattern: m.Pattern,
	}
	if len(m.Keys) > 0 {
		j.Keys = wvHexList(m.Keys)
	}
	for _, r := range m.Refs {
		j.Refs = append(j.Refs, wvRefInfo{Name: r.Name, Key: Hex(r.Key), Version: Hex(r.Version), CreatedAt: I64(r.CreatedAt), User: r.User})
	}
	if len(m.Holders) > 0 {
		j.Holders = wvKeyHoldersJSON(m.Holders)
	}
	for _, f := range m.Failed {
		j.Failed = append(j.Failed, wvKeyFailure{Key: Hex(f.Key), Node: Hex(f.Node), Reason: f.Reason, RetryAfter: I64(f.RetryAfter)})
	}
	for _, r := range m.Rejected {
		j.Rejected = append(j.Rejected, wvKeyReject{Key: Hex(r.Key), Reason: r.Reason})
	}
	if len(m.Short) > 0 {
		j.Short = wvKeyHoldersJSON(m.Short)
	}
	if len(m.Unreachable) > 0 {
		j.Unreachable = wvHexList(m.Unreachable)
	}
	for _, r := range m.Rows {
		j.Rows = append(j.Rows, wvScanRow{Reg: Hex(r.Reg), Promised: wvHexE(r.Promised), Accepted: wvHexE(r.Accepted), Value: wvHexE(r.Value), HasValue: r.HasValue})
	}
	if len(m.Addrs) > 0 {
		j.Addrs = m.Addrs
	}
	if len(m.Missing) > 0 {
		j.Missing = wvHexList(m.Missing)
	}
	if len(m.Deleted) > 0 {
		j.Deleted = m.Deleted
	}
	return j
}

// wvProtoMsgJSON converts a protocol.Msg.
func wvProtoMsgJSON(m *protocol.Msg) wvProtoMsg {
	j := wvProtoMsg{
		Typ: I64(m.Type), Name: m.Name, Root: m.Root, CAS: m.CAS, ExpectedOld: m.ExpectedOld, Record: m.Record,
		Data: m.Data, Key: m.Key, Code: m.Code, Text: m.Text, Current: m.Current, Token: m.Token,
		DataConns: I64(m.DataConns),
	}
	for _, r := range m.Refs {
		j.Refs = append(j.Refs, wvProtoRefInfo{Name: r.Name, Key: Hex(r.Key), CreatedAt: I64(r.CreatedAt), User: r.User})
	}
	if len(m.Keys) > 0 {
		j.Keys = wvHexList(m.Keys)
	}
	if len(m.DataPorts) > 0 {
		j.DataPorts = m.DataPorts
	}
	for _, d := range m.DataEndpoints {
		j.DataEndpoints = append(j.DataEndpoints, wvProtoEndpoint{ID: Hex(d.ID), Addrs: wvStrList(d.Addrs)})
	}
	if len(m.Names) > 0 {
		j.Names = m.Names
	}
	return j
}

// wvFloat describes a float64 exactly: Go's shortest 'g' text and the IEEE bits.
func wvFloat(f float64) (text, bits string) {
	return strconv.FormatFloat(f, 'g', -1, 64), fmt.Sprintf("%016x", math.Float64bits(f))
}

// ---- fixtures ----

// wvMust panics on a fixture construction error (a generator bug).
func wvMust[T any](v T, err error) T {
	if err != nil {
		panic("vectorgen: fixture: " + err.Error())
	}
	return v
}

// wvRep returns n copies of b.
func wvRep(b byte, n int) []byte { return bytes.Repeat([]byte{b}, n) }

// wvSeq returns n bytes counting up from start.
func wvSeq(start byte, n int) []byte {
	out := make([]byte, n)
	for i := range out {
		out[i] = start + byte(i)
	}
	return out
}

// wvEdID is id(seed): the ed25519 public key of NewKeyFromSeed(data(seed, 32)).
func wvEdID(seed uint64) []byte {
	pub, ok := ed25519.NewKeyFromSeed(smData(seed, 32)).Public().(ed25519.PublicKey)
	if !ok {
		panic("vectorgen: ed25519 public key type")
	}
	return []byte(pub)
}

// wvIDN is idN of client-transfer §3.1: 32 zero bytes with [0] = [31] = n.
func wvIDN(n byte) []byte {
	b := make([]byte, 32)
	b[0], b[31] = n, n
	return b
}

// wvBlobKey is k(seed, n) = key.New(Blob, n, data(seed, n)).
func wvBlobKey(seed uint64, n int) []byte {
	k := wvMust(key.New(key.Blob, uint64(n), smData(seed, n)))
	return k[:]
}

// wvFrame encodes m with wire.WriteMsg.
func wvFrame(m *wire.Msg) ([]byte, error) {
	var buf bytes.Buffer
	if err := wire.WriteMsg(&buf, m); err != nil {
		return nil, err
	}
	return buf.Bytes(), nil
}

// wvStamp sets the client request stamp (client.stamp): keys 1, 2, 3.
func wvStamp(m *wire.Msg, cid []byte, inc, epoch uint64) *wire.Msg {
	m.ClusterID, m.Incarnation, m.Epoch = cid, inc, epoch
	return m
}

// wvRStamp sets the node reply stamp (stampReply): keys 2, 3.
func wvRStamp(m *wire.Msg, inc, epoch uint64) *wire.Msg {
	m.Incarnation, m.Epoch = inc, epoch
	return m
}

// wvCond applies a reference condition exactly as client.Cond.apply
// (client/refs.go:58-68).
func wvCond(m *wire.Msg, cond client.Cond) *wire.Msg {
	m.Force = cond.Force
	if cond.Versioned {
		m.HasExpected = true
		m.ExpectedVersion = cond.ExpectedVersion
	}
	if cond.Keyed {
		m.HasExpected = true
		m.ExpectedOld = cond.ExpectedOld
	}
	return m
}

// wvCoreRecord is the reference record of client-core §3.2.
func wvCoreRecord() []byte {
	return wvMust(reference.Reference{Name: "trees/a", Key: wvSeq(1, 32), User: "alice", CreatedAt: 1700000000123456789}.Encode())
}

// wvCoreView is the test view of client-core §3.3.
func wvCoreView() []byte {
	v := &view.View{
		ClusterID: wvSeq(0, 16), Incarnation: 1, Epoch: 7, Version: 9, PlacementEpoch: 7, Replicas: 3, MinReplicas: 2,
		Voters: []view.Voter{{ID: wvRep(0x11, 32), Since: 1}},
		Nodes:  []view.Node{{ID: wvRep(0x11, 32), Weight: 100, Addrs: []string{"ip:192.168.1.10:4433"}, Writable: true}},
	}
	return wvMust(v.Encode())
}

// wvXferView is the minimal view of client-transfer §3.1.
func wvXferView() []byte {
	v := &view.View{
		ClusterID: wvRep(0x11, 16), Incarnation: 1, Epoch: 8, Version: 3, PlacementEpoch: 1, Replicas: 3, MinReplicas: 2,
		Voters: []view.Voter{{ID: wvIDN(1), Since: 1}},
		Nodes:  []view.Node{{ID: wvIDN(1), Weight: 100, Writable: true}},
	}
	return wvMust(v.Encode())
}

// ---- wire/frames.json ----

type wvRemoteError struct {
	Code         string `json:"code"`
	Text         string `json:"text"`
	View         Hex    `json:"view"`
	RetryAfterNs I64    `json:"retry_after_ns"`
	RetryAfter   string `json:"retry_after"`
	Error        string `json:"error"`
}

type wvKeys32 struct {
	OK    bool   `json:"ok"`
	Count int    `json:"count"`
	Error string `json:"error,omitempty"`
}

type wvIncomplete struct {
	Error  string `json:"error"`
	Sample []Hex  `json:"sample"`
}

type wvFrameCase struct {
	Name        string         `json:"name"`
	Aliases     []string       `json:"aliases,omitempty"`
	Source      string         `json:"source"`
	Call        string         `json:"call,omitempty"`
	DecodeOnly  bool           `json:"decode_only"`
	NullElement bool           `json:"null_element,omitempty"`
	Msg         wvMsg          `json:"msg"`
	FrameHex    string         `json:"frame_hex"`
	RemoteError *wvRemoteError `json:"remote_error,omitempty"`
	Keys32      *wvKeys32      `json:"keys32,omitempty"`
	CASMismatch string         `json:"cas_mismatch,omitempty"`
	Incomplete  *wvIncomplete  `json:"incomplete,omitempty"`
	ShortIDs    [][]Hex        `json:"short_ids,omitempty"`
	HoldersIDs  [][]Hex        `json:"holders_ids,omitempty"`
}

type wvBulkCase struct {
	Name        string  `json:"name"`
	Source      string  `json:"source"`
	Call        string  `json:"call"`
	Msg         wvMsg   `json:"msg"`
	Keys        Payload `json:"keys"`
	KeyCount    int     `json:"key_count"`
	FrameLen    int     `json:"frame_len"`
	FrameBlake3 string  `json:"frame_blake3"`
	HeadHex     string  `json:"head_hex"`
}

type wvKeys32Case struct {
	Name  string `json:"name"`
	Keys  []Hex  `json:"keys"`
	OK    bool   `json:"ok"`
	Count int    `json:"count"`
	Error string `json:"error,omitempty"`
}

type wvErrSpec struct {
	Code string `json:"code"`
	Text string `json:"text"`
}

type wvErrorIsCase struct {
	Name    string    `json:"name"`
	Err     wvErrSpec `json:"err"`
	Wrapped bool      `json:"wrapped"`
	Target  wvErrSpec `json:"target"`
	Is      bool      `json:"is"`
}

type wvIsCodeCase struct {
	Name    string `json:"name"`
	Kind    string `json:"kind"`
	Wrapped bool   `json:"wrapped"`
	Code    string `json:"code"`
	Text    string `json:"text"`
	Query   string `json:"query"`
	IsCode  bool   `json:"is_code"`
	Error   string `json:"error"`
}

type wvFrames struct {
	Requests []wvFrameCase   `json:"requests"`
	Replies  []wvFrameCase   `json:"replies"`
	Other    []wvFrameCase   `json:"other"`
	Bulk     []wvBulkCase    `json:"bulk"`
	Keys32   []wvKeys32Case  `json:"keys32"`
	ErrorIs  []wvErrorIsCase `json:"error_is"`
	IsCode   []wvIsCodeCase  `json:"is_code"`

	seen  map[string]wvCaseRef // frame hex → the case that holds it
	added []string             // every name added, in order
	err   error
}

// wvCaseRef locates a case by list and index: a pointer into a list would go
// stale when append moves the list.
type wvCaseRef struct {
	list *[]wvFrameCase
	idx  int
}

// add encodes m with wire.WriteMsg and records the case in dst.
func (f *wvFrames) add(dst *[]wvFrameCase, name, source, call string, m *wire.Msg) {
	if f.err != nil {
		return
	}
	frame, err := wvFrame(m)
	if err != nil {
		f.err = fmt.Errorf("%s: %w", name, err)
		return
	}
	f.addFrame(dst, name, source, call, false, frame, m)
}

// addPayload records a decode-only case: payload framed by hand.
func (f *wvFrames) addPayload(dst *[]wvFrameCase, name, source, call string, payload []byte) {
	frame := binary.BigEndian.AppendUint32(nil, uint32(len(payload)))
	f.addFrame(dst, name, source, call, true, append(frame, payload...), nil)
}

func (f *wvFrames) addFrame(dst *[]wvFrameCase, name, source, call string, decodeOnly bool, frame []byte, in *wire.Msg) {
	if f.err != nil {
		return
	}
	got, err := wire.ReadMsg(bytes.NewReader(frame))
	if err != nil {
		f.err = fmt.Errorf("%s: ReadMsg: %w", name, err)
		return
	}
	if in != nil {
		a, _ := json.Marshal(wvMsgJSON(in))
		b, _ := json.Marshal(wvMsgJSON(got))
		if !bytes.Equal(a, b) {
			f.err = fmt.Errorf("%s: decode differs from input:\n in %s\ngot %s", name, a, b)
			return
		}
	}
	fh := hex.EncodeToString(frame)
	f.added = append(f.added, name)
	if ref, ok := f.seen[fh]; ok && !decodeOnly {
		prev := &(*ref.list)[ref.idx]
		prev.Aliases = append(prev.Aliases, name)
		if !strings.Contains(prev.Source, source) {
			prev.Source += "; " + source
		}
		return
	}
	c := wvFrameCase{Name: name, Source: source, Call: call, DecodeOnly: decodeOnly, NullElement: wvMsgNil(got), Msg: wvMsgJSON(got), FrameHex: fh}
	wvReplyExtras(&c, got)
	*dst = append(*dst, c)
	if !decodeOnly {
		f.seen[fh] = wvCaseRef{list: dst, idx: len(*dst) - 1}
	}
}

// wvCheckNames fails when a case name was lost or used twice: every added
// name must be exactly one case or alias.
func wvCheckNames(f *wvFrames) error {
	count := map[string]int{}
	for _, list := range [][]wvFrameCase{f.Requests, f.Replies, f.Other} {
		for _, c := range list {
			count[c.Name]++
			for _, a := range c.Aliases {
				count[a]++
			}
		}
	}
	for _, n := range f.added {
		if count[n] != 1 {
			return fmt.Errorf("frame case %s appears %d times, want 1", n, count[n])
		}
	}
	if len(count) != len(f.added) {
		return fmt.Errorf("frames.json holds %d names, %d were added", len(count), len(f.added))
	}
	return nil
}

// wvKeys32Of runs wire.Keys32.
func wvKeys32Of(raw [][]byte) *wvKeys32 {
	ks, err := wire.Keys32(raw)
	if err != nil {
		return &wvKeys32{Error: err.Error()}
	}
	return &wvKeys32{OK: true, Count: len(ks)}
}

// wvIDsOf renders view.IDsOf (zero-padded or truncated to 32 bytes).
func wvIDsOf(in []wire.KeyHolders) [][]Hex {
	out := make([][]Hex, len(in))
	for i, kh := range in {
		ids := view.IDsOf(kh.Holders)
		out[i] = make([]Hex, len(ids))
		for j, id := range ids {
			out[i][j] = Hex(append([]byte(nil), id[:]...))
		}
	}
	return out
}

// wvReplyExtras adds what the client derives from a decoded reply.
func wvReplyExtras(c *wvFrameCase, m *wire.Msg) {
	switch m.Type {
	case wire.TErr:
		e := wire.ErrorFromMsg(m)
		c.RemoteError = &wvRemoteError{Code: e.Code, Text: e.Text, View: wvHexE(e.View), RetryAfterNs: I64(e.RetryAfter), RetryAfter: e.RetryAfter.String(), Error: e.Error()}
	case wire.TMissingReply, wire.TAbsent:
		c.Keys32 = wvKeys32Of(m.Keys)
	case wire.TCASMismatch:
		c.CASMismatch = (&client.CASMismatch{Current: m.Current, Record: m.Record, Version: m.Version, HasCurrent: m.HasCurrent}).Error()
	case wire.TIncomplete:
		c.Keys32 = wvKeys32Of(m.Keys)
		sample, _ := wire.Keys32(m.Keys)
		inc := &client.Incomplete{Sample: sample, Shortfall: m.Shortfall}
		c.Incomplete = &wvIncomplete{Error: inc.Error(), Sample: []Hex{}}
		for _, k := range sample {
			c.Incomplete.Sample = append(c.Incomplete.Sample, Hex(append([]byte(nil), k[:]...)))
		}
	}
	if len(m.Short) > 0 {
		c.ShortIDs = wvIDsOf(m.Short)
	}
	if len(m.Holders) > 0 {
		c.HoldersIDs = wvIDsOf(m.Holders)
	}
}

func genWireFrames(out string) error {
	f := &wvFrames{seen: map[string]wvCaseRef{}}
	rec := wvCoreRecord()
	coreView := wvCoreView()
	xferView := wvXferView()
	cid := wvSeq(0, 16)
	st := func(m *wire.Msg) *wire.Msg { return wvStamp(m, cid, 1, 7) }
	req := func(name, source, call string, m *wire.Msg) { f.add(&f.Requests, name, source, call, m) }
	gcStatus := codec.MustMarshal(node.AdminRequest{Op: "gc-status"})

	// client-core §3.2: requests the client sends.
	cc := "client-core §3.2"
	req("view-unstamped", cc, "Dial: pool.Call(&Msg{Type: TView}), unstamped", &wire.Msg{Type: wire.TView})
	req("view-stamped", cc, "RefreshView / probeHinted: call(&Msg{Type: TView})", st(&wire.Msg{Type: wire.TView}))
	req("status", cc, "Status(ctx, id)", st(&wire.Msg{Type: wire.TStatus}))
	req("ref-get", cc, `RefGet(ctx, "trees/a")`, st(&wire.Msg{Type: wire.TRefGet, Name: "trees/a"}))
	req("ref-put-versioned-nil", cc, "RefPut(ctx, record, Cond{Versioned: true})", st(wvCond(&wire.Msg{Type: wire.TRefPut, Record: rec}, client.Cond{Versioned: true})))
	req("ref-put-versioned", cc, "RefPut(ctx, record, Cond{Versioned: true, ExpectedVersion: 010203})", st(wvCond(&wire.Msg{Type: wire.TRefPut, Record: rec}, client.Cond{Versioned: true, ExpectedVersion: []byte{1, 2, 3}})))
	req("ref-put-keyed", cc, "RefPut(ctx, record, Cond{Keyed: true, ExpectedOld: 40..5f})", st(wvCond(&wire.Msg{Type: wire.TRefPut, Record: rec}, client.Cond{Keyed: true, ExpectedOld: wvSeq(0x40, 32)})))
	req("ref-put-force", cc, "RefPut(ctx, record, Cond{Force: true})", st(wvCond(&wire.Msg{Type: wire.TRefPut, Record: rec}, client.Cond{Force: true})))
	req("ref-delete-force", cc, `RefDelete(ctx, "trees/a", Cond{Force: true})`, st(wvCond(&wire.Msg{Type: wire.TRefDelete, Name: "trees/a"}, client.Cond{Force: true})))
	req("ref-delete-versioned", cc, `RefDelete(ctx, "trees/a", Cond{Versioned: true, ExpectedVersion: 010203})`, st(wvCond(&wire.Msg{Type: wire.TRefDelete, Name: "trees/a"}, client.Cond{Versioned: true, ExpectedVersion: []byte{1, 2, 3}})))
	req("ref-list-empty-prefix", cc, `RefList(ctx, ""): first page (Prefix []byte("") omitted, After nil)`, st(&wire.Msg{Type: wire.TRefList, Prefix: []byte(""), After: nil}))
	req("ref-list-prefix", cc, `RefList(ctx, "trees/"): first page`, st(&wire.Msg{Type: wire.TRefList, Prefix: []byte("trees/")}))
	req("ref-list-prefix-after", cc, `RefList(ctx, "trees/"): page after Next "trees/b"`, st(&wire.Msg{Type: wire.TRefList, Prefix: []byte("trees/"), After: []byte("trees/b")}))
	req("ref-watch-no-known", cc, `WatchRefs(ctx, "trees/**", nil): known map empty, Refs omitted`, st(&wire.Msg{Type: wire.TRefWatch, Pattern: "trees/**", Refs: []wire.RefInfo{}}))
	req("ref-watch-one-known", cc, `WatchRefs(ctx, "trees/**", {trees/a: aa×32}): RefInfo{Name, Key}, Version nil, CreatedAt 0`, st(&wire.Msg{Type: wire.TRefWatch, Pattern: "trees/**", Refs: []wire.RefInfo{{Name: "trees/a", Key: wvRep(0xaa, 32)}}}))
	req("ref-watch-nil-key", cc, `WatchRefs(ctx, "trees/*", {x: nil})`, st(&wire.Msg{Type: wire.TRefWatch, Pattern: "trees/*", Refs: []wire.RefInfo{{Name: "x"}}}))
	req("ref-watch-two-known", "client-core §5 item 1", `WatchRefs(ctx, "trees/**", {trees/a: aa×32, trees/b: bb×32}): Refs in name order (Go map order is random)`, st(&wire.Msg{Type: wire.TRefWatch, Pattern: "trees/**", Refs: []wire.RefInfo{{Name: "trees/a", Key: wvRep(0xaa, 32)}, {Name: "trees/b", Key: wvRep(0xbb, 32)}}}))
	req("admin-gc-status", cc, `Admin(ctx, NodeID{}, AdminRequest{Op: "gc-status"})`, st(&wire.Msg{Type: wire.TAdmin, Params: gcStatus}))

	// codec-wire-ticket §3.2: k1 = 11×32, k2 = 22×32.
	cw := "codec-wire-ticket §3.2"
	k1, k2 := wvRep(0x11, 32), wvRep(0x22, 32)
	req("missing-pin", cw, "Missing(ctx, [k1], true)", st(&wire.Msg{Type: wire.TMissing, Keys: [][]byte{k1}, Pin: true}))
	req("missing-no-pin", cw, "Missing(ctx, [k1], false)", st(&wire.Msg{Type: wire.TMissing, Keys: [][]byte{k1}}))
	req("get-k1-k2", cw, "getStream(ctx, id, [k1, k2])", st(&wire.Msg{Type: wire.TGet, Keys: [][]byte{k1, k2}}))
	req("put", cw, "putOnce: &Msg{Type: TPut}, stamped", st(&wire.Msg{Type: wire.TPut}))
	req("ref-get-main", cw, `RefGet(ctx, "main")`, st(&wire.Msg{Type: wire.TRefGet, Name: "main"}))
	req("ref-put-versioned-0a0b", cw, "RefPut(ctx, 0203, Cond{Versioned: true, ExpectedVersion: 0a0b})", st(wvCond(&wire.Msg{Type: wire.TRefPut, Record: []byte{2, 3}}, client.Cond{Versioned: true, ExpectedVersion: []byte{0x0a, 0x0b}})))
	req("ref-put-must-not-exist", cw, "RefPut(ctx, 0203, Cond{Versioned: true})", st(wvCond(&wire.Msg{Type: wire.TRefPut, Record: []byte{2, 3}}, client.Cond{Versioned: true})))
	req("ref-put-force-0203", cw, "RefPut(ctx, 0203, Cond{Force: true})", st(wvCond(&wire.Msg{Type: wire.TRefPut, Record: []byte{2, 3}}, client.Cond{Force: true})))
	req("ref-delete-main-force", cw, `RefDelete(ctx, "main", Cond{Force: true})`, st(wvCond(&wire.Msg{Type: wire.TRefDelete, Name: "main"}, client.Cond{Force: true})))
	req("ref-list-a-after-a-b", cw, `RefList(ctx, "a/"): page after "a/b"`, st(&wire.Msg{Type: wire.TRefList, Prefix: []byte("a/"), After: []byte("a/b")}))
	req("ref-watch-known-x", cw, `WatchRefs(ctx, "**", {x: k1})`, st(&wire.Msg{Type: wire.TRefWatch, Pattern: "**", Refs: []wire.RefInfo{{Name: "x", Key: k1}}}))
	req("ref-watch-empty-unstamped", cw, `&Msg{Type: TRefWatch, Pattern: "*"} without a cached view`, &wire.Msg{Type: wire.TRefWatch, Pattern: "*"})
	req("keyed-nil-old", "verification §5", "RefPut(ctx, 0203, Cond{Keyed: true, ExpectedOld: nil}): encodes like must-not-exist", st(wvCond(&wire.Msg{Type: wire.TRefPut, Record: []byte{2, 3}}, client.Cond{Keyed: true})))

	// client-transfer §3.1: cid = 11×16, k1 = k(1, 100), k2 = k(2, 200), idN.
	xt := "client-transfer §3.1"
	xcid := wvRep(0x11, 16)
	xk1, xk2 := wvBlobKey(1, 100), wvBlobKey(2, 200)
	req("xfer-missing-pin", xt, "Missing(ctx, [k1, k2], true)", wvStamp(&wire.Msg{Type: wire.TMissing, Keys: [][]byte{xk1, xk2}, Pin: true}, xcid, 1, 7))
	req("xfer-missing-inc0-epoch300", xt, "Missing(ctx, [k1], false) with a view at incarnation 0, epoch 300", wvStamp(&wire.Msg{Type: wire.TMissing, Keys: [][]byte{xk1}}, xcid, 0, 300))
	req("xfer-get", xt, "getStream(ctx, id, [k1, k2])", wvStamp(&wire.Msg{Type: wire.TGet, Keys: [][]byte{xk1, xk2}}, xcid, 1, 7))
	req("xfer-put", xt, "putOnce: &Msg{Type: TPut}", wvStamp(&wire.Msg{Type: wire.TPut}, xcid, 1, 7))

	// verification §5: cid16, id(s), k(s) = k(s, 64).
	vf := "verification §5"
	vk := func(s uint64) []byte { return wvBlobKey(s, 64) }
	req("ver-missing-one-pin", vf, "Missing(ctx, [k(1)], true)", st(&wire.Msg{Type: wire.TMissing, Keys: [][]byte{vk(1)}, Pin: true}))
	req("ver-get-3", vf, "getStream(ctx, id, [k(1), k(2), k(3)])", st(&wire.Msg{Type: wire.TGet, Keys: [][]byte{vk(1), vk(2), vk(3)}}))
	req("ver-ref-get-demo", vf, `RefGet(ctx, "trees/demo")`, st(&wire.Msg{Type: wire.TRefGet, Name: "trees/demo"}))
	req("ver-ref-put-versioned-8", vf, "RefPut(ctx, record, Cond{Versioned: true, ExpectedVersion: 8 bytes})", st(wvCond(&wire.Msg{Type: wire.TRefPut, Record: rec}, client.Cond{Versioned: true, ExpectedVersion: wvSeq(1, 8)})))
	req("ver-ref-put-keyed-new", vf, "RefPut(ctx, record, Cond{Keyed: true, ExpectedOld: nil})", st(wvCond(&wire.Msg{Type: wire.TRefPut, Record: rec}, client.Cond{Keyed: true})))
	req("ver-ref-put-keyed", vf, "RefPut(ctx, record, Cond{Keyed: true, ExpectedOld: k(4)})", st(wvCond(&wire.Msg{Type: wire.TRefPut, Record: rec}, client.Cond{Keyed: true, ExpectedOld: vk(4)})))
	req("ver-ref-delete-expected-version", vf, `RefDelete(ctx, "trees/demo", Cond{Versioned: true, ExpectedVersion: 8 bytes})`, st(wvCond(&wire.Msg{Type: wire.TRefDelete, Name: "trees/demo"}, client.Cond{Versioned: true, ExpectedVersion: wvSeq(1, 8)})))
	req("ver-ref-watch-unicode", vf, `WatchRefs(ctx, "bäume/**", nil)`, st(&wire.Msg{Type: wire.TRefWatch, Pattern: "bäume/**"}))
	req("ver-ping", vf, "&Msg{Type: TPing}, stamped", st(&wire.Msg{Type: wire.TPing}))

	genWireReplies(f, rec, coreView, xferView)
	genWireOther(f)
	if f.err != nil {
		return f.err
	}
	if err := wvCheckNames(f); err != nil {
		return err
	}
	if err := genWireBulk(f); err != nil {
		return err
	}
	genWireKeys32(f)
	genWireErrorIs(f)
	if err := wvCheckSpecFrames(f, rec, coreView, xferView); err != nil {
		return err
	}
	return writeJSON(filepath.Join(out, "wire", "frames.json"), f)
}

// genWireReplies adds the node replies the client reads.
func genWireReplies(f *wvFrames, rec, coreView, xferView []byte) {
	rs := func(m *wire.Msg) *wire.Msg { return wvRStamp(m, 1, 7) }
	rep := func(name, source string, m *wire.Msg) { f.add(&f.Replies, name, source, "", m) }
	errMsg := func(code, text string) *wire.Msg { return wire.ErrMsg(code, text) }

	// client-core §3.3.
	cc := "client-core §3.3"
	ver := wvSeq(0x30, 16)
	rep("view-reply", cc, rs(&wire.Msg{Type: wire.TViewReply, View: coreView, Unreachable: [][]byte{wvRep(0x22, 32)}}))
	rep("err-stale-view", cc, wvRStamp(&wire.Msg{Type: wire.TErr, Code: wire.CodeStaleView, Text: "request epoch is behind", View: coreView}, 1, 8))
	rep("err-unknown-ref", cc, errMsg(wire.CodeUnknownRef, "no such reference"))
	rep("err-busy-slow-down", cc, &wire.Msg{Type: wire.TErr, Code: wire.CodeBusy, Text: "slow down", RetryAfter: 1500})
	rep("ref", cc, rs(&wire.Msg{Type: wire.TRef, Record: rec, Version: ver}))
	rep("ok-ref-put", cc, rs(&wire.Msg{Type: wire.TOK, Key: wvSeq(1, 32), Version: ver}))
	rep("ok-ref-delete", cc, rs(&wire.Msg{Type: wire.TOK}))
	rep("cas-mismatch-current", cc, rs(&wire.Msg{Type: wire.TCASMismatch, Version: ver, HasCurrent: true, Record: rec, Current: wvSeq(1, 32)}))
	rep("cas-mismatch-absent", cc, rs(&wire.Msg{Type: wire.TCASMismatch}))
	rep("incomplete", cc, rs(&wire.Msg{Type: wire.TIncomplete, Keys: [][]byte{wvSeq(1, 32)}, Shortfall: 5}))
	rep("refs-page", cc, rs(&wire.Msg{Type: wire.TRefs, Refs: []wire.RefInfo{
		{Name: "trees/a", Key: wvSeq(1, 32), Version: wvSeq(0x30, 16), CreatedAt: 1700000000123456789, User: "alice"},
		{Name: "trees/b", Key: wvSeq(2, 32), Version: wvSeq(0x31, 16), CreatedAt: 1700000000123456790},
	}, Next: []byte("trees/b")}))
	rep("ref-changes", cc, rs(&wire.Msg{Type: wire.TRefChanges, Refs: []wire.RefInfo{
		{Name: "trees/a", Key: wvSeq(1, 32), Version: wvSeq(0x30, 16), CreatedAt: 1700000000123456789, User: "alice"},
	}, Deleted: []string{"trees/gone"}}))
	rep("ref-synced", cc, rs(&wire.Msg{Type: wire.TRefSynced}))
	rep("admin-reply-ok", cc, rs(&wire.Msg{Type: wire.TAdminReply, Status: codec.MustMarshal(node.AdminReply{Text: "ok"})}))

	// codec-wire-ticket §3.3: k1 = 11×32, k2 = 22×32.
	cw := "codec-wire-ticket §3.3"
	k1, k2 := wvRep(0x11, 32), wvRep(0x22, 32)
	rep("view-reply-01", cw, rs(&wire.Msg{Type: wire.TViewReply, View: []byte{1}, Unreachable: [][]byte{k2}}))
	rep("missing-reply-short", cw, rs(&wire.Msg{Type: wire.TMissingReply, Short: []wire.KeyHolders{{Key: k1, Holders: [][]byte{k2}}}}))
	rep("absent-none", cw, rs(&wire.Msg{Type: wire.TAbsent}))
	rep("put-result-codec", cw, rs(&wire.Msg{Type: wire.TPutResult,
		Holders:  []wire.KeyHolders{{Key: k2, Holders: [][]byte{k1}}},
		Failed:   []wire.KeyFailure{{Key: k1, Node: k2, Reason: "unreachable", RetryAfter: 1234}},
		Rejected: []wire.KeyReject{{Key: k1, Reason: "not-owner"}},
	}))
	rep("ref-05-06", cw, rs(&wire.Msg{Type: wire.TRef, Record: []byte{5}, Version: []byte{6}}))
	rep("ok-k1-06", cw, rs(&wire.Msg{Type: wire.TOK, Key: k1, Version: []byte{6}}))
	rep("cas-mismatch-05-06", cw, rs(&wire.Msg{Type: wire.TCASMismatch, Record: []byte{5}, Version: []byte{6}, Current: k1, HasCurrent: true}))
	rep("incomplete-k1-3", cw, rs(&wire.Msg{Type: wire.TIncomplete, Keys: [][]byte{k1}, Shortfall: 3}))
	rep("refs-next-a", cw, rs(&wire.Msg{Type: wire.TRefs, Refs: []wire.RefInfo{{Name: "a", Key: k1, Version: []byte{6}, CreatedAt: 1700000000000000000, User: "u"}}, Next: []byte("a")}))
	rep("ref-changes-gone", cw, rs(&wire.Msg{Type: wire.TRefChanges, Refs: []wire.RefInfo{{Name: "a", Key: k1, Version: []byte{6}, CreatedAt: 1}}, Deleted: []string{"gone"}}))
	rep("status-reply-a0", cw, rs(&wire.Msg{Type: wire.TStatusReply, Status: []byte{0xa0}}))
	rep("err-unauthorized", cw, errMsg(wire.CodeUnauthorized, "not on the allowlist"))
	rep("err-stale-view-01", cw, rs(&wire.Msg{Type: wire.TErr, Code: wire.CodeStaleView, Text: "request epoch is behind", View: []byte{1}}))
	rep("pong", cw, rs(&wire.Msg{Type: wire.TPong}))
	rep("data-end", cw, &wire.Msg{Type: wire.TDataEnd})

	// client-transfer §3.1 and §5.2 item 1.
	xt := "client-transfer §3.1"
	xk1, xk2 := wvBlobKey(1, 100), wvBlobKey(2, 200)
	id1, id2, id3 := wvIDN(1), wvIDN(2), wvIDN(3)
	rep("xfer-missing-reply", xt, rs(&wire.Msg{Type: wire.TMissingReply, Keys: [][]byte{xk2}, Short: []wire.KeyHolders{{Key: xk1, Holders: [][]byte{id1, id2}}}}))
	rep("xfer-missing-reply-none", xt, rs(&wire.Msg{Type: wire.TMissingReply}))
	rep("xfer-put-result", xt, rs(&wire.Msg{Type: wire.TPutResult,
		Holders:  []wire.KeyHolders{{Key: xk1, Holders: [][]byte{id1, id2, id3}}, {Key: xk2, Holders: [][]byte{id1}}},
		Failed:   []wire.KeyFailure{{Key: xk2, Node: id2, Reason: "busy", RetryAfter: 1234}, {Key: xk2, Node: id3, Reason: "stale-view"}},
		Rejected: []wire.KeyReject{{Key: xk1, Reason: "not-owner"}},
	}))
	rep("xfer-absent", xt, rs(&wire.Msg{Type: wire.TAbsent, Keys: [][]byte{xk2}}))
	rep("xfer-err-busy", xt, rs(&wire.Msg{Type: wire.TErr, Code: wire.CodeBusy, Text: "too many streams", RetryAfter: 750}))
	rep("xfer-err-stale-view", xt, wvRStamp(&wire.Msg{Type: wire.TErr, Code: wire.CodeStaleView, Text: "request epoch is behind", View: xferView}, 1, 8))
	x5 := "client-transfer §5.2 item 1"
	rep("xfer-missing-reply-short-key-31", x5, rs(&wire.Msg{Type: wire.TMissingReply, Keys: [][]byte{xk1, wvRep(0x31, 31)}}))
	rep("xfer-missing-reply-odd-lengths", x5, rs(&wire.Msg{Type: wire.TMissingReply, Short: []wire.KeyHolders{{Key: wvRep(0x31, 31), Holders: [][]byte{wvRep(0x16, 16), wvSeq(0x40, 40)}}, {Key: xk1, Holders: [][]byte{id1}}}}))
	rep("xfer-put-result-odd-lengths", x5, rs(&wire.Msg{Type: wire.TPutResult, Holders: []wire.KeyHolders{{Key: wvRep(0x33, 33), Holders: [][]byte{wvRep(0x16, 16), wvSeq(0x40, 40)}}, {Key: xk2, Holders: [][]byte{id2}}}}))
	rep("xfer-put-result-holders-only", x5, rs(&wire.Msg{Type: wire.TPutResult, Holders: []wire.KeyHolders{{Key: xk1, Holders: [][]byte{id1}}}}))
	rep("xfer-put-result-failed-no-retry", x5, rs(&wire.Msg{Type: wire.TPutResult, Failed: []wire.KeyFailure{{Key: xk1, Node: id1, Reason: "unreachable"}}}))
	f.addPayload(&f.Replies, "xfer-put-result-unknown-key", x5, "", []byte{0xa2, 0x00, 0x18, 0x33, 0x18, 0x63, 0x01})
	rep("xfer-err-not-owner-view", x5, wvRStamp(&wire.Msg{Type: wire.TErr, Code: wire.CodeNotOwner, Text: "not an owner", View: xferView}, 1, 8))
	rep("xfer-err-busy-no-retry", x5, rs(&wire.Msg{Type: wire.TErr, Code: wire.CodeBusy, Text: "too many streams"}))
	rep("xfer-err-no-space", x5, errMsg(wire.CodeNoSpace, "node below its free-space reserve"))
	rep("xfer-err-batch-over", x5, errMsg(wire.CodeBadRequest, "batch over 64 MiB"))
	rep("xfer-err-too-many-keys", x5, errMsg(wire.CodeBadRequest, "too many keys"))

	// verification §5: id(s), k(s) = k(s, 64).
	vf := "verification §5"
	vk := func(s uint64) []byte { return wvBlobKey(s, 64) }
	vid := wvEdID
	vver := wvSeq(0xa0, 8)
	rep("ver-view-reply", vf, rs(&wire.Msg{Type: wire.TViewReply, View: coreView, Unreachable: [][]byte{vid(1), vid(2)}}))
	rep("ver-missing-reply", vf, rs(&wire.Msg{Type: wire.TMissingReply, Keys: [][]byte{vk(1), vk(2)}, Short: []wire.KeyHolders{{Key: vk(3), Holders: [][]byte{vid(1), vid(2)}}}}))
	rep("ver-missing-reply-short-nil-holders", vf, rs(&wire.Msg{Type: wire.TMissingReply, Short: []wire.KeyHolders{{Key: vk(3)}}}))
	rep("ver-absent-2", vf, rs(&wire.Msg{Type: wire.TAbsent, Keys: [][]byte{vk(1), vk(2)}}))
	rep("ver-data-9", vf, &wire.Msg{Type: wire.TData, Data: smData(9, 9)})
	rep("ver-put-result", vf, rs(&wire.Msg{Type: wire.TPutResult,
		Holders:  []wire.KeyHolders{{Key: vk(1), Holders: [][]byte{vid(1), vid(2), vid(3)}}, {Key: vk(2), Holders: [][]byte{vid(1), vid(2), vid(3)}}},
		Failed:   []wire.KeyFailure{{Key: vk(1), Node: vid(4), Reason: "busy", RetryAfter: 250}, {Key: vk(2), Node: vid(4), Reason: "unreachable"}},
		Rejected: []wire.KeyReject{{Key: vk(5), Reason: "verify"}, {Key: vk(6), Reason: "not-owner"}},
	}))
	rep("ver-ref", vf, rs(&wire.Msg{Type: wire.TRef, Record: rec, Version: vver}))
	rep("ver-ok-ref-put", vf, rs(&wire.Msg{Type: wire.TOK, Key: vk(1), Version: vver}))
	rep("ver-cas-mismatch-current", vf, rs(&wire.Msg{Type: wire.TCASMismatch, Version: vver, HasCurrent: true, Record: rec, Current: vk(4)}))
	rep("ver-cas-mismatch-absent", vf, rs(&wire.Msg{Type: wire.TCASMismatch, Version: vver}))
	keys64 := make([][]byte, 64)
	for i := range keys64 {
		keys64[i] = vk(uint64(100 + i))
	}
	rep("ver-incomplete-64", vf, rs(&wire.Msg{Type: wire.TIncomplete, Keys: keys64, Shortfall: 1000}))
	rep("ver-refs-3-next", vf, rs(&wire.Msg{Type: wire.TRefs, Refs: []wire.RefInfo{
		{Name: "trees/a", Key: vk(1), Version: wvSeq(0x10, 8), CreatedAt: 1700000000123456789, User: "alice"},
		{Name: "trees/b", Key: vk(2), Version: wvSeq(0x20, 8), CreatedAt: -1},
		{Name: "trees/c", Key: vk(3), Version: wvSeq(0x30, 8), CreatedAt: 0, User: "bob@example.com"},
	}, Next: []byte("trees/c")}))
	rep("ver-refs-final", vf, rs(&wire.Msg{Type: wire.TRefs, Refs: []wire.RefInfo{{Name: "trees/d", Key: vk(4), Version: wvSeq(0x40, 8), CreatedAt: 1}}}))
	rep("ver-status-reply", vf, rs(&wire.Msg{Type: wire.TStatusReply, Status: codec.MustMarshal(node.Status{ID: vid(1), Epoch: 7, Incarnation: 1, Writable: true})}))
	rep("ver-admin-reply-names", vf, rs(&wire.Msg{Type: wire.TAdminReply, Status: codec.MustMarshal(node.AdminReply{Names: []string{"trees/a", "trees/b"}})}))
	rep("ver-ref-changes", vf, rs(&wire.Msg{Type: wire.TRefChanges, Refs: []wire.RefInfo{
		{Name: "trees/a", Key: vk(1), Version: wvSeq(0x10, 8), CreatedAt: 1700000000123456789, User: "alice"},
		{Name: "trees/b", Key: vk(2), Version: wvSeq(0x20, 8), CreatedAt: 1700000000123456790},
	}, Deleted: []string{"trees/old"}}))
	codes := []string{wire.CodeStaleView, wire.CodeNotOwner, wire.CodeNoSpace, wire.CodeBusy, wire.CodeBadRequest,
		wire.CodeUnauthorized, wire.CodeUnknownRef, wire.CodeCASMismatch, wire.CodeIncomplete, wire.CodeUnavailable,
		wire.CodeTimeout, wire.CodeInternal, wire.CodeNotMember, wire.CodeNeedView, wire.CodeExpired, wire.CodeAmnesiac,
		wire.CodeMarkFrozen, wire.CodeRetired, wire.CodeTooSoon, wire.CodeConflict, wire.CodeNoMark}
	for _, code := range codes {
		rep("ver-err-"+code, vf, errMsg(code, ""))
		rep("ver-err-"+code+"-text", vf, errMsg(code, "text of "+code))
	}
	rep("ver-err-stale-view-view", vf, rs(&wire.Msg{Type: wire.TErr, Code: wire.CodeStaleView, Text: "request epoch is behind", View: coreView}))
	rep("ver-err-busy-1500", vf, &wire.Msg{Type: wire.TErr, Code: wire.CodeBusy, RetryAfter: 1500})
	rep("ver-err-busy-negative-retry", vf, &wire.Msg{Type: wire.TErr, Code: wire.CodeBusy, RetryAfter: -5})
}

// genWireOther adds generic Msg encodings: integer heads, negative ints, nil
// elements, cluster-ALPN fields.
func genWireOther(f *wvFrames) {
	cw := "codec-wire-ticket §3.3 other probe payloads"
	oth := func(name, source string, m *wire.Msg) { f.add(&f.Other, name, source, "", m) }
	oth("err-busy-retry-1500", cw, &wire.Msg{Type: wire.TErr, Code: wire.CodeBusy, RetryAfter: 1500})
	oth("ref-list-limit-neg", cw, &wire.Msg{Type: wire.TRefList, Limit: -1, Shortfall: 300})
	oth("join-wide-ints", cw, &wire.Msg{Type: wire.TJoin, Weight: 70000, Since: 1 << 40, G: 1<<32 - 1})
	oth("ref-changes-nil-elements", cw, &wire.Msg{Type: wire.TRefChanges, Deleted: []string{"x", ""}, Unreachable: [][]byte{nil, {}}})
	oth("ok-bare-unstamped", "verification §3.1", &wire.Msg{Type: wire.TOK})
	oth("ref-put-new-a0", "verification §3.1", wvStamp(&wire.Msg{Type: wire.TRefPut, Record: []byte{0xa0}, HasExpected: true}, wvSeq(0, 16), 1, 7))
	g1 := "codec-wire-ticket §5 G1"
	ints := []uint64{0, 23, 24, 255, 256, 65535, 65536, 1<<32 - 1, 1 << 32, math.MaxUint64}
	for _, v := range ints {
		oth(fmt.Sprintf("uint-epoch-%d", v), g1, &wire.Msg{Type: wire.TView, Epoch: v, Incarnation: v, Since: v, G: v, Seq: v, Sent: v, Received: v, Marked: v})
	}
	for _, v := range []uint32{0, 23, 24, 255, 256, 65535, 65536, math.MaxUint32} {
		oth(fmt.Sprintf("uint32-weight-%d", v), g1, &wire.Msg{Type: wire.TJoin, Weight: v})
	}
	for _, v := range []int64{0, 23, 24, 255, 256, 65535, 65536, 1<<32 - 1, 1 << 32, math.MaxInt64, -1, -24, -25, -256, -257, -65537, math.MinInt64} {
		oth(fmt.Sprintf("int-fields-%d", v), g1, &wire.Msg{Type: int(v), Limit: int(v), RetryAfter: v, Shortfall: int(v), NotAfter: v})
	}
	oth("key-holders-nil-key", "codec-wire-ticket §2.2.2", &wire.Msg{Type: wire.TPutResult, Holders: []wire.KeyHolders{{}}, Short: []wire.KeyHolders{{Key: []byte{}}}})
	oth("key-failure-nil-fields", "codec-wire-ticket §2.2.2", &wire.Msg{Type: wire.TPutResult, Failed: []wire.KeyFailure{{Key: []byte{1}}, {Key: []byte{}, Node: []byte{}, Reason: "r", RetryAfter: -1}}})
	oth("key-reject-nil-key", "codec-wire-ticket §2.2.2", &wire.Msg{Type: wire.TPutResult, Rejected: []wire.KeyReject{{}, {Key: []byte{}, Reason: "x"}}})
	oth("ref-info-variants", "codec-wire-ticket §2.2.2", &wire.Msg{Type: wire.TRefs, Refs: []wire.RefInfo{{}, {Name: "n", Key: []byte{}, Version: []byte{9}, CreatedAt: -2, User: "u"}}})
	oth("scan-rows", "codec-wire-ticket §2.2.2", &wire.Msg{Type: wire.TScanReply, Rows: []wire.ScanRow{{}, {Reg: []byte{}, Promised: []byte{1}, Accepted: []byte{2}, Value: []byte{3}, HasValue: true}}, More: true, Next: []byte("n")})
	oth("all-fields", "codec-wire-ticket §5 G1", &wire.Msg{Type: wire.TJoin, ClusterID: []byte{1}, Incarnation: 2, Epoch: 3,
		Keys: [][]byte{{4}}, Pin: true, Name: "n", Record: []byte{7}, Data: []byte{8}, Key: []byte{9}, Code: "c", Text: "t",
		View: []byte{12}, Version: []byte{13}, ExpectedVersion: []byte{14}, ExpectedOld: []byte{15}, Force: true, HasExpected: true,
		Prefix: []byte{18}, After: []byte{19}, Limit: 20, Refs: []wire.RefInfo{{Name: "r", Key: []byte{21}}}, Next: []byte{22},
		Holders: []wire.KeyHolders{{Key: []byte{23}}}, Failed: []wire.KeyFailure{{Key: []byte{24}}}, Rejected: []wire.KeyReject{{Key: []byte{25}}},
		Short: []wire.KeyHolders{{Key: []byte{26}}}, Unreachable: [][]byte{{27}}, RetryAfter: 28, Current: []byte{29}, Shortfall: 30,
		HasCurrent: true, Reg: []byte{32}, Ballot: []byte{33}, Value: []byte{34}, HasValue: true, Accepted: []byte{36}, Promised: []byte{37},
		NotAfter: 38, Rows: []wire.ScanRow{{Reg: []byte{39}}}, More: true, Since: 41, Token: []byte{42}, Weight: 43, Zone: "z",
		Addrs: []string{"ip:127.0.0.1:1"}, NoVote: true, G: 47, Nonce: []byte{48}, Seq: 49, Expand: true, Params: []byte{51}, Sent: 52,
		Received: 53, Idle: true, Marked: 55, Missing: [][]byte{{56}}, Status: []byte{57}, Node: []byte{58}, Error: "e", Pattern: "p",
		Deleted: []string{"d"}})
	oth("unicode-text", "codec-wire-ticket §2.1.1 E9", &wire.Msg{Type: wire.TRefGet, Name: "bäume/€/𝄞", Zone: "\x00\x7f"})
}

// genWireBulk adds frames too large to inline.
func genWireBulk(f *wvFrames) error {
	const n = 8192
	seed := uint64(8192)
	raw := smData(seed, n*32)
	keys := make([][]byte, n)
	for i := range keys {
		keys[i] = raw[i*32 : (i+1)*32]
	}
	m := wvStamp(&wire.Msg{Type: wire.TMissing, Keys: keys}, wvSeq(0, 16), 1, 7)
	frame, err := wvFrame(m)
	if err != nil {
		return err
	}
	got, err := wire.ReadMsg(bytes.NewReader(frame))
	if err != nil || len(got.Keys) != n || !bytes.Equal(got.Keys[n-1], keys[n-1]) {
		return fmt.Errorf("missing-8192: decode: %v", err)
	}
	head := wvMsgJSON(m)
	head.Keys = nil
	f.Bulk = append(f.Bulk, wvBulkCase{
		Name: "missing-8192", Source: "verification §5; client-transfer §5.2 item 1",
		Call: "Missing(ctx, 8192 keys, false): one batch of batchKeys", Msg: head, Keys: SM(seed, n*32), KeyCount: n,
		FrameLen: len(frame), FrameBlake3: blake3Hex(frame), HeadHex: hex.EncodeToString(frame[:64]),
	})
	return nil
}

// genWireKeys32 adds wire.Keys32 cases.
func genWireKeys32(f *wvFrames) {
	cases := []struct {
		name string
		keys [][]byte
	}{
		{"nil", nil},
		{"empty", [][]byte{}},
		{"two", [][]byte{wvRep(1, 32), wvRep(2, 32)}},
		{"second-31", [][]byte{wvRep(1, 32), wvRep(2, 31)}},
		{"first-33", [][]byte{wvRep(1, 33)}},
		{"nil-element", [][]byte{nil}},
		{"first-bad-wins", [][]byte{wvRep(1, 32), {}, wvRep(3, 31)}},
	}
	for _, c := range cases {
		k := wvKeys32Of(c.keys)
		f.Keys32 = append(f.Keys32, wvKeys32Case{Name: c.name, Keys: wvHexList(c.keys), OK: k.OK, Count: k.Count, Error: k.Error})
	}
}

// genWireErrorIs adds (*wire.Error).Is and wire.IsCode cases.
func genWireErrorIs(f *wvFrames) {
	is := []struct {
		name        string
		err, target wvErrSpec
		wrapped     bool
	}{
		{"same-code-no-target-text", wvErrSpec{"busy", "slow"}, wvErrSpec{"busy", ""}, false},
		{"same-code-same-text", wvErrSpec{"busy", "slow"}, wvErrSpec{"busy", "slow"}, false},
		{"same-code-other-text", wvErrSpec{"busy", "slow"}, wvErrSpec{"busy", "fast"}, false},
		{"other-code", wvErrSpec{"busy", ""}, wvErrSpec{"no-space", ""}, false},
		{"target-text-err-empty", wvErrSpec{"busy", ""}, wvErrSpec{"busy", "slow"}, false},
		{"wrapped-same-code", wvErrSpec{"stale-view", "behind"}, wvErrSpec{"stale-view", ""}, true},
	}
	for _, c := range is {
		var err error = &wire.Error{Code: c.err.Code, Text: c.err.Text}
		if c.wrapped {
			err = fmt.Errorf("upload to 01000000: %w", err)
		}
		got := errors.Is(err, &wire.Error{Code: c.target.Code, Text: c.target.Text})
		f.ErrorIs = append(f.ErrorIs, wvErrorIsCase{Name: c.name, Err: c.err, Wrapped: c.wrapped, Target: c.target, Is: got})
	}
	codes := []struct {
		name, kind string
		wrapped    bool
		code, text string
		query      string
	}{
		{"wire-match", "wire", false, "stale-view", "behind", "stale-view"},
		{"wire-other", "wire", false, "busy", "", "stale-view"},
		{"wire-wrapped", "wire", true, "unknown-ref", "no such reference", "unknown-ref"},
		{"protocol-remote-never", "protocol", false, "busy", "", "busy"},
		{"protocol-remote-wrapped", "protocol", true, "internal", "sender died", "internal"},
	}
	for _, c := range codes {
		var err error
		if c.kind == "wire" {
			err = &wire.Error{Code: c.code, Text: c.text}
		} else {
			err = &protocol.RemoteError{Code: c.code, Text: c.text}
		}
		if c.wrapped {
			err = fmt.Errorf("get: %w", err)
		}
		f.IsCode = append(f.IsCode, wvIsCodeCase{Name: c.name, Kind: c.kind, Wrapped: c.wrapped, Code: c.code, Text: c.text, Query: c.query, IsCode: wire.IsCode(err, c.query), Error: err.Error()})
	}
}

// wvCheckSpecFrames fails when a verified frame of the specs is not reproduced.
func wvCheckSpecFrames(f *wvFrames, rec, coreView, xferView []byte) error {
	have := map[string]bool{}
	for _, list := range [][]wvFrameCase{f.Requests, f.Replies, f.Other} {
		for _, c := range list {
			have[c.FrameHex] = true
		}
	}
	R, V, XV := hex.EncodeToString(rec), hex.EncodeToString(coreView), hex.EncodeToString(xferView)
	if R != "a4006774726565732f610158200102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f200265616c696365031b17979cfe3d85cd15" {
		return fmt.Errorf("client-core record differs: %s", R)
	}
	if V != "aa0050000102030405060708090a0b0c0d0e0f0101020703090407050306020781a20058201111111111111111111111111111111111111111111111111111111111111111010108000a81a4005820111111111111111111111111111111111111111111111111111111111111111101186402817469703a3139322e3136382e312e31303a3434333307f5" {
		return fmt.Errorf("client-core view differs: %s", V)
	}
	if XV != "aa0050111111111111111111111111111111110101020803030401050306020781a20058200100000000000000000000000000000000000000000000000000000000000001010108000a81a3005820010000000000000000000000000000000000000000000000000000000000000101186407f5" {
		return fmt.Errorf("client-transfer view differs: %s", XV)
	}
	x1 := "582000644b75a8842dc16ded120ee1f96d229ac409028f4d5d429499a4ad548aae85"
	x2 := "582000c8069dce890d2e93a2e70679f56a31d5efbdfb4981e1a7c6c80fe602a93f36"
	s01 := "0150000102030405060708090a0b0c0d0e0f02010307"
	want := []string{
		"00000004a1001820",
		"0000001aa4001820" + s01,
		"0000001aa4001828" + s01,
		"00000023a5001824" + s01 + "066774726565732f61",
		"0000005da6001825" + s01 + "07583e" + R + "11f5",
		"00000062a7001825" + s01 + "07583e" + R + "0e4301020311f5",
		"00000080a7001825" + s01 + "07583e" + R + "0f5820404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f11f5",
		"0000005da6001825" + s01 + "07583e" + R + "10f5",
		"00000025a6001826" + s01 + "066774726565732f6110f5",
		"0000002aa7001826" + s01 + "066774726565732f610e4301020311f5",
		"0000001aa4001827" + s01,
		"00000022a5001827" + s01 + "124674726565732f",
		"0000002ba6001827" + s01 + "124674726565732f134774726565732f62",
		"00000025a500182a" + s01 + "183c6874726565732f2a2a",
		"00000058a600182a" + s01 + "1581a4006774726565732f61015820aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa02f60300183c6874726565732f2a2a",
		"00000030a600182a" + s01 + "1581a400617801f602f60300183c6774726565732f2a",
		"00000029a5001829" + s01 + "18334ca1006967632d737461747573",
		"000000bba5001830020103070c588b" + V + "181b8158202222222222222222222222222222222222222222222222222222222222222222",
		"000000baa6000a020103080a6a7374616c652d766965770b77726571756573742065706f636820697320626568696e640c588b" + V,
		"00000023a3000a0a6b756e6b6e6f776e2d7265660b716e6f2073756368207265666572656e6365",
		"00000019a4000a0a64627573790b69736c6f7720646f776e181c1905dc",
		"0000005ba50018340201030707583e" + R + "0d50303132333435363738393a3b3c3d3e3f",
		"0000003da5001835020103070958200102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f200d50303132333435363738393a3b3c3d3e3f",
		"00000008a300183502010307",
		"00000082a70018360201030707583e" + R + "0d50303132333435363738393a3b3c3d3e3f181d58200102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20181ff5",
		"00000008a300183602010307",
		"0000002fa500183702010307048158200102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20181e05",
		"000000aca5001838020103071582a5006774726565732f610158200102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f200250303132333435363738393a3b3c3d3e3f031b17979cfe3d85cd150465616c696365a4006774726565732f6201582002030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202102503132333435363738393a3b3c3d3e3f40031b17979cfe3d85cd16164774726565732f62",
		"00000068a500183b020103071581a5006774726565732f610158200102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f200250303132333435363738393a3b3c3d3e3f031b17979cfe3d85cd150465616c696365183d816a74726565732f676f6e65",
		"00000008a300183c02010307",
		"00000010a400183a02010307183945a100626f6b",
		"00000062a6001821015011111111111111111111111111111111020103070482" + x1 + x2 + "05f5",
		"0000003ea40018210150111111111111111111111111111111110319012c0481" + x1,
		"00000060a5001822015011111111111111111111111111111111020103070482" + x1 + x2,
		"0000001aa400182301501111111111111111111111111111111102010307",
		"00000099a5001831020103070481" + x2 + "181a81a200" + x1 + "01825820010000000000000000000000000000000000000000000000000000000000000158200200000000000000000000000000000000000000000000000000000000000002",
		"00000008a300183102010307",
		"000001b7a6001833020103071782a200" + x1 + "0183582001000000000000000000000000000000000000000000000000000000000000015820020000000000000000000000000000000000000000000000000000000000000258200300000000000000000000000000000000000000000000000000000000000003a200" + x2 + "018158200100000000000000000000000000000000000000000000000000000000000001181882a400" + x2 + "0158200200000000000000000000000000000000000000000000000000000000000002026462757379031904d2a300" + x2 + "0158200300000000000000000000000000000000000000000000000000000000000003026a7374616c652d76696577181981a200" + x1 + "01696e6f742d6f776e6572",
		"0000002ca4001832020103070481" + x2,
		"00000008a300183202010307",
		"00000024a6000a020103070a64627573790b70746f6f206d616e792073747265616d73181c1902ee",
		"000000a3a6000a020103080a6a7374616c652d766965770b77726571756573742065706f636820697320626568696e640c5874" + XV,
		"00000027a3000a0a6c756e617574686f72697a65640b746e6f74206f6e2074686520616c6c6f776c697374",
		"00000003a10008",
		"00000008a300186a02010307",
		"0000000ea3000a0a6462757379181c1905dc",
		"0000000ba30018271420181e19012c",
		"0000001da40018601829" + "1b0000010000000000" + "182b1a00011170" + "182f1affffffff",
		"0000000fa300183b181b82f640183d82617860",
		"00000004a1001835",
		"0000001fa6001825" + s01 + "0741a011f5",
	}
	for _, w := range want {
		if !have[w] {
			return fmt.Errorf("verified spec frame not reproduced: %s", w)
		}
	}
	return nil
}

// ---- wire/decode.json ----

// wvPart is one fragment of a payload: hex repeated n times.
type wvPart struct {
	Hex    string `json:"hex"`
	Repeat int    `json:"repeat"`
}

type wvDecodeCase[T any] struct {
	Name            string   `json:"name"`
	Field           string   `json:"field,omitempty"`
	ShapeHex        string   `json:"shape_hex,omitempty"`
	PayloadHex      *string  `json:"payload_hex,omitempty"`
	PayloadParts    []wvPart `json:"payload_parts,omitempty"`
	OK              bool     `json:"ok"`
	GoError         string   `json:"go_error,omitempty"`
	Value           *T       `json:"value,omitempty"`
	CanonicalHex    string   `json:"canonical_hex,omitempty"`
	CanonicalLen    int      `json:"canonical_len,omitempty"`
	CanonicalBlake3 string   `json:"canonical_blake3,omitempty"`
	NullElement     bool     `json:"null_element,omitempty"`
}

type wvDecodeFile struct {
	WireMsg      []wvDecodeCase[wvMsg]      `json:"wire_msg"`
	ProtocolMsg  []wvDecodeCase[wvProtoMsg] `json:"protocol_msg"`
	Ticket       []wvDecodeCase[tkTicket]   `json:"ticket"`
	AdminRequest []wvDecodeCase[admRequest] `json:"admin_request"`
	AdminReply   []wvDecodeCase[admReply]   `json:"admin_reply"`
	Status       []wvDecodeCase[admStatus]  `json:"status"`
}

// wvParts builds a payload description from alternating hex and repeat pairs.
func wvParts(pairs ...any) []wvPart {
	var out []wvPart
	for i := 0; i+1 < len(pairs); i += 2 {
		h, _ := pairs[i].(string)
		n, _ := pairs[i+1].(int)
		out = append(out, wvPart{Hex: h, Repeat: n})
	}
	return out
}

// wvMaterialize concatenates the parts.
func wvMaterialize(parts []wvPart) ([]byte, error) {
	var buf bytes.Buffer
	for _, p := range parts {
		b, err := hex.DecodeString(p.Hex)
		if err != nil {
			return nil, fmt.Errorf("part %q: %w", p.Hex, err)
		}
		for range p.Repeat {
			buf.Write(b)
		}
	}
	return buf.Bytes(), nil
}

// wvOne is hex once.
func wvOne(h string) []wvPart { return []wvPart{{Hex: h, Repeat: 1}} }

func wvHasNil(lists ...[][]byte) bool {
	for _, l := range lists {
		for _, b := range l {
			if b == nil {
				return true
			}
		}
	}
	return false
}

// wvDecodeRun decodes the payload into a fresh G with decode and records the result.
func wvDecodeRun[G any, J any](name, field, shape string, parts []wvPart, decode func([]byte) (*G, error), conv func(*G) J, nullElem func(*G) bool) (wvDecodeCase[J], error) {
	c := wvDecodeCase[J]{Name: name, Field: field, ShapeHex: shape}
	payload, err := wvMaterialize(parts)
	if err != nil {
		return c, fmt.Errorf("%s: %w", name, err)
	}
	if len(payload) <= 4096 {
		h := hex.EncodeToString(payload)
		c.PayloadHex = &h
	} else {
		c.PayloadParts = parts
	}
	v, err := decode(payload)
	if err != nil {
		c.GoError = err.Error()
		return c, nil
	}
	c.OK = true
	j := conv(v)
	c.Value = &j
	enc := codec.MustMarshal(v)
	if len(enc) <= 4096 {
		c.CanonicalHex = hex.EncodeToString(enc)
	} else {
		c.CanonicalLen, c.CanonicalBlake3 = len(enc), blake3Hex(enc)
	}
	c.NullElement = nullElem(v)
	return c, nil
}

// wvUintHead is the shortest CBOR head of major 0 for n.
func wvUintHead(n uint64) string {
	switch {
	case n < 24:
		return fmt.Sprintf("%02x", n)
	case n <= 0xff:
		return fmt.Sprintf("18%02x", n)
	default:
		return fmt.Sprintf("19%04x", n)
	}
}

// wvShapes are the G4 value shapes.
var wvShapes = []string{
	"00", "1bffffffffffffffff", "20", "3bffffffffffffffff", "40", "4161", "60", "6161", "61ff", "80", "8101",
	"81f6", "814161", "a0", "f6", "f7", "f4", "f5", "f93e00", "fb3fb999999999999a", "f0", "f8ff", "c24105",
	"c249010000000000000000", "c34101", "c06161", "c001", "c16161", "d82a4161", "d9d9f74161", "5f41614162ff",
	"7f6161ff", "9f01ff", "bfff",
}

// wvMatrixField is one struct field of the G4 matrix: payload = prefix ‖ key head ‖ shape.
type wvMatrixField struct {
	label  string
	prefix string
	keys   int
}

// wvDecoders are the decode entry points: codec.Unmarshal as ReadMsg, ticket.Parse,
// the CLI's admin, node.DecodeStatus and protocol.ReadMsg use it.
func wvDecMsg(b []byte) (*wire.Msg, error) {
	var m wire.Msg
	if err := codec.Unmarshal(b, &m); err != nil {
		return nil, err
	}
	return &m, nil
}

func wvDecProto(b []byte) (*protocol.Msg, error) {
	var m protocol.Msg
	if err := codec.Unmarshal(b, &m); err != nil {
		return nil, err
	}
	return &m, nil
}

func wvDecTicket(b []byte) (*ticket.Ticket, error) {
	var t ticket.Ticket
	if err := codec.Unmarshal(b, &t); err != nil {
		return nil, err
	}
	return &t, nil
}

func wvDecAdminRequest(b []byte) (*node.AdminRequest, error) {
	var r node.AdminRequest
	if err := codec.Unmarshal(b, &r); err != nil {
		return nil, err
	}
	return &r, nil
}

func wvDecAdminReply(b []byte) (*node.AdminReply, error) {
	var r node.AdminReply
	if err := codec.Unmarshal(b, &r); err != nil {
		return nil, err
	}
	return &r, nil
}

func wvDecStatus(b []byte) (*node.Status, error) {
	st, err := node.DecodeStatus(b)
	if err != nil {
		return nil, err
	}
	return &st, nil
}

func wvMsgNil(m *wire.Msg) bool {
	lists := [][][]byte{m.Keys, m.Unreachable, m.Missing}
	for _, kh := range m.Holders {
		lists = append(lists, kh.Holders)
	}
	for _, kh := range m.Short {
		lists = append(lists, kh.Holders)
	}
	return wvHasNil(lists...)
}

func wvProtoNil(m *protocol.Msg) bool        { return wvHasNil(m.Keys) }
func wvNoNil[G any](*G) bool                 { return false }
func wvStatusNil(s *node.Status) bool        { return wvHasNil(s.Unreachable) }
func wvProtoConv(m *protocol.Msg) wvProtoMsg { return wvProtoMsgJSON(m) }
func wvTicketConv(t *ticket.Ticket) tkTicket { return tkTicketJSON(*t) }
func wvReqConv(r *node.AdminRequest) admRequest {
	return admRequestJSON(*r)
}
func wvReplyConv(r *node.AdminReply) admReply { return admReplyJSON(*r) }
func wvStatusConv(s *node.Status) admStatus   { return admStatusJSON(*s) }

type wvDecoder struct {
	f   *wvDecodeFile
	err error
}

func (d *wvDecoder) msg(name, field, shape string, parts []wvPart) {
	if d.err != nil {
		return
	}
	c, err := wvDecodeRun(name, field, shape, parts, wvDecMsg, wvMsgJSON, wvMsgNil)
	if err == nil {
		err = wvCrossReadMsg(parts, c.GoError)
	}
	d.err = err
	d.f.WireMsg = append(d.f.WireMsg, c)
}

func (d *wvDecoder) proto(name, field, shape string, parts []wvPart) {
	if d.err != nil {
		return
	}
	c, err := wvDecodeRun(name, field, shape, parts, wvDecProto, wvProtoConv, wvProtoNil)
	if err == nil {
		err = wvCrossProtoReadMsg(parts, c.GoError)
	}
	d.err = err
	d.f.ProtocolMsg = append(d.f.ProtocolMsg, c)
}

func (d *wvDecoder) ticket(name, field, shape string, parts []wvPart) {
	if d.err != nil {
		return
	}
	c, err := wvDecodeRun(name, field, shape, parts, wvDecTicket, wvTicketConv, wvNoNil[ticket.Ticket])
	d.err = err
	d.f.Ticket = append(d.f.Ticket, c)
}

func (d *wvDecoder) adminRequest(name, field, shape string, parts []wvPart) {
	if d.err != nil {
		return
	}
	c, err := wvDecodeRun(name, field, shape, parts, wvDecAdminRequest, wvReqConv, wvNoNil[node.AdminRequest])
	d.err = err
	d.f.AdminRequest = append(d.f.AdminRequest, c)
}

func (d *wvDecoder) adminReply(name, field, shape string, parts []wvPart) {
	if d.err != nil {
		return
	}
	c, err := wvDecodeRun(name, field, shape, parts, wvDecAdminReply, wvReplyConv, wvNoNil[node.AdminReply])
	d.err = err
	d.f.AdminReply = append(d.f.AdminReply, c)
}

func (d *wvDecoder) status(name, field, shape string, parts []wvPart) {
	if d.err != nil {
		return
	}
	c, err := wvDecodeRun(name, field, shape, parts, wvDecStatus, wvStatusConv, wvStatusNil)
	d.err = err
	d.f.Status = append(d.f.Status, c)
}

// wvCrossReadMsg checks that wire.ReadMsg over the framed payload reports the
// same decoding decision with its "wire: decode frame: " prefix.
func wvCrossReadMsg(parts []wvPart, goErr string) error {
	payload, err := wvMaterialize(parts)
	if err != nil {
		return err
	}
	frame := append(binary.BigEndian.AppendUint32(nil, uint32(len(payload))), payload...)
	_, rerr := wire.ReadMsg(bytes.NewReader(frame))
	switch {
	case goErr == "" && rerr != nil:
		return fmt.Errorf("ReadMsg rejects %x: %v", payload, rerr)
	case goErr != "" && (rerr == nil || rerr.Error() != "wire: decode frame: "+goErr):
		return fmt.Errorf("ReadMsg of %x: %v, want wire: decode frame: %s", payload, rerr, goErr)
	}
	return nil
}

// wvCrossProtoReadMsg is wvCrossReadMsg for protocol.ReadMsg.
func wvCrossProtoReadMsg(parts []wvPart, goErr string) error {
	payload, err := wvMaterialize(parts)
	if err != nil {
		return err
	}
	frame := append(binary.BigEndian.AppendUint32(nil, uint32(len(payload))), payload...)
	_, rerr := protocol.ReadMsg(bytes.NewReader(frame))
	switch {
	case goErr == "" && rerr != nil:
		return fmt.Errorf("protocol.ReadMsg rejects %x: %v", payload, rerr)
	case goErr != "" && (rerr == nil || rerr.Error() != "protocol: decode frame: "+goErr):
		return fmt.Errorf("protocol.ReadMsg of %x: %v, want protocol: decode frame: %s", payload, rerr, goErr)
	}
	return nil
}

func genWireDecode(out string) error {
	d := &wvDecoder{f: &wvDecodeFile{}}
	wvDecodeMatrix(d)
	wvDecodeStructure(d)
	if d.err != nil {
		return d.err
	}
	return writeJSON(filepath.Join(out, "wire", "decode.json"), d.f)
}

// wvDecodeMatrix emits G4: every field of every decoded struct × every shape.
func wvDecodeMatrix(d *wvDecoder) {
	run := func(fields []wvMatrixField, emit func(name, field, shape string, parts []wvPart)) {
		for _, fd := range fields {
			for k := 0; k < fd.keys; k++ {
				for _, shape := range wvShapes {
					field := fmt.Sprintf("%s.%d", fd.label, k)
					emit("matrix "+field+" "+shape, field, shape, wvOne(fd.prefix+wvUintHead(uint64(k))+shape))
				}
			}
		}
	}
	run([]wvMatrixField{
		{"wire.Msg", "a1", 62},
		{"wire.Msg.21[]", "a11581a1", 5},
		{"wire.Msg.23[]", "a11781a1", 2},
		{"wire.Msg.24[]", "a1181881a1", 4},
		{"wire.Msg.25[]", "a1181981a1", 2},
		{"wire.Msg.26[]", "a1181a81a1", 2},
		{"wire.Msg.39[]", "a1182781a1", 5},
	}, d.msg)
	run([]wvMatrixField{
		{"protocol.Msg", "a1", 18},
		{"protocol.Msg.6[]", "a10681a1", 4},
		{"protocol.Msg.16[]", "a11081a1", 2},
	}, d.proto)
	run([]wvMatrixField{
		{"ticket.Ticket", "a1", 3},
		{"ticket.Ticket.2[]", "a10281a1", 2},
	}, d.ticket)
	run([]wvMatrixField{{"node.AdminRequest", "a1", 15}}, d.adminRequest)
	run([]wvMatrixField{{"node.AdminReply", "a1", 7}}, d.adminReply)
	run([]wvMatrixField{
		{"node.Status", "a1", 29},
		{"node.Status.12[]", "a10c81a1", 4},
	}, d.status)
}

// wvDecodeStructure emits the hand-made structure and laxness cases.
func wvDecodeStructure(d *wvDecoder) {
	m := func(name, h string) { d.msg(name, "", "", wvOne(h)) }
	k32 := hex.EncodeToString(wvRep(0x11, 32))

	// verification §2.1 probes.
	m("probe keys-out-of-order", "a20307001835")
	m("probe non-shortest-int", "a100190035")
	m("probe indefinite-map", "bf001835ff")
	m("probe unknown-key-99", "a2001835186301")
	m("probe duplicate-key", "a2001835001820")
	m("probe text-into-bytes-element", "a20018210481636162")
	m("probe bytes-into-string", "a2001824064178")
	m("probe trailing-byte", "a100183500")
	m("probe negint-into-uint64", "a20018350320")
	m("probe tag0-around-text", "a20018350bc06174")
	m("probe float-into-int", "a100f93800")
	m("probe int-into-bool", "a20018350501")
	m("probe empty-payload", "")
	m("probe array-top-level", "8100")
	// verification §5 extras.
	m("null-into-bytes", "a2001834"+"07f6")
	m("null-into-string", "a2001834"+"06f6")
	m("nested-indefinite-array", "a20018210"+"49f5820"+k32+"ff")
	m("big-uint-epoch", "a2001830031bffffffffffffffff")
	m("epoch-overflow-int", "a1001b0000000100000000")
	m("unknown-nested-key-in-refinfo", "a200183815"+"81a2006161186301")
	m("text-key", "a26161010018"+"35")
	m("map-key-uint-nonshortest", "a118001835")
	// D1 null and undefined at top level.
	m("top-null", "f6")
	m("top-undefined", "f7")
	m("top-tag-55799-null", "d9d9f7f6")
	// D2 tag preamble.
	m("tag-55799-stripped", "d9d9f7a1001820")
	m("tag-55799-twice", "d9d9f7d9d9f7a1001820")
	m("tag0-on-uint", "a100c001")
	m("tag1-on-text", "a10bc16161")
	m("tag1-on-float", "a100c1f93800")
	m("tag2-on-text", "a107c26161")
	m("tag3-on-uint", "a107c301")
	m("tag0-chain-inner-bad", "a10bd82ac001")
	// D3 bignums and other tags.
	m("bignum-overflow-uint64", "a102c249010000000000000000")
	m("bignum-fits-uint64", "a102c248ffffffffffffffff")
	m("bignum-into-int-overflow", "a100c2488000000000000000")
	m("bignum-into-int-fits", "a100c2487fffffffffffffff")
	m("negbignum-into-int", "a100c34101")
	m("negbignum-into-uint", "a102c34101")
	m("bignum-into-string", "a106c24101")
	m("bignum-into-bool", "a105c24101")
	m("bignum-into-bytes", "a107c2420102")
	m("negbignum-into-bytes", "a107c3420102")
	m("other-tag-into-int", "a100d82a1820")
	m("other-tag-into-keys-element", "a10481d82a4101")
	m("tag-on-map-key", "a1c1001820")
	// D4 positive integers.
	m("uint32-overflow", "a1182b1b0000000100000000")
	m("uint32-max", "a1182b1affffffff")
	m("int-overflow", "a1001b8000000000000000")
	m("int-max", "a1001b7fffffffffffffff")
	m("int64-overflow-retry-after", "a1181c1b8000000000000000")
	m("uint-into-string", "a10601")
	m("uint-into-keys", "a10401")
	// D5 negative integers.
	m("negint-overflow", "a1003bffffffffffffffff")
	m("negint-min-int64", "a1003b7fffffffffffffff")
	m("negint-into-uint64", "a10220")
	m("negint-into-uint32", "a1182b20")
	m("negint-into-bytes-element", "a1078120")
	// D6/D7 strings.
	m("indefinite-bytes-chunks", "a1075f41614162ff")
	m("indefinite-bytes-empty", "a1075fff")
	m("indefinite-text-chunks", "a1067f61616162ff")
	m("indefinite-text-invalid-chunk", "a1067f616161ffff")
	m("text-invalid-utf8", "a10661ff")
	m("text-into-bytes", "a1076161")
	m("text-surrogate", "a10663eda080")
	// D8 primitives.
	m("simple-16-into-int", "a100f0")
	m("simple-32-into-int", "a100f820")
	m("simple-255-into-int", "a100f8ff")
	m("simple-into-bool", "a105f0")
	m("simple-into-string", "a106f0")
	m("false-into-int", "a100f4")
	m("true-into-bool", "a105f5")
	m("float-into-bool", "a105f93800")
	m("float32-into-uint", "a102fa3f800000")
	// D9 arrays.
	m("array-into-bytes", "a10783010203")
	m("array-null-element-into-bytes", "a10781f6")
	m("array-element-overflow-uint8", "a1078119"+"0100")
	m("array-empty-into-keys", "a10480")
	m("array-into-string", "a10680")
	m("array-null-element-into-keys", "a10482f640")
	m("array-null-element-into-refs", "a11581f6")
	m("array-null-element-into-deleted", "a1183d82f66161")
	m("indefinite-array-into-keys", "a1049f4101ff")
	// D10 map keys.
	m("key-bstr", "a1410001")
	m("key-float", "a1f93e0001")
	m("key-array", "a18001")
	m("key-map", "a1a001")
	m("key-tag-uint", "a1c10001")
	m("key-negint", "a12001")
	m("key-text", "a1616101")
	m("key-text-invalid-utf8", "a161ff00")
	m("key-uint-overflow", "a11bffffffffffffffff01")
	m("key-negint-overflow", "a13bffffffffffffffff01")
	m("key-true", "a1f501")
	m("key-null", "a1f601")
	m("key-bstr-then-valid", "a241000100"+"1835")
	m("dup-matched-same-type", "a200182000"+"1821")
	m("dup-matched-other-type", "a20018200061"+"61")
	m("dup-unmatched", "a300182018630118"+"636161")
	m("unmatched-invalid-utf8-skipped", "a1186361ff")
	m("map-into-string", "a106a0")
	m("map-into-keys-element", "a10481a0")
	// D12 top level.
	m("top-uint", "00")
	m("top-bstr", "40")
	m("top-text", "6161")
	m("top-true", "f5")
	m("top-float", "f93800")
	m("top-tag1-map", "c1a0")
	m("top-other-tag-map", "d82aa1001820")
	m("top-map-indefinite-empty", "bfff")
	// W2 truncation.
	for _, h := range []string{"a1", "a100", "a10018", "a1066261", "a1048240", "c1", "a100d82a", "bf00", "5f41", "a1071a00"} {
		m("truncated "+h, h)
	}
	// W3/W4 invalid heads, as the whole payload and as a map value.
	majors := []string{"positive-integer", "negative-integer", "byte-string", "text-string", "array", "map", "tag", "primitives"}
	for mj, label := range majors {
		for _, ai := range []int{28, 29, 30} {
			m(fmt.Sprintf("invalid-ai-%d %s", ai, label), fmt.Sprintf("%02x", mj<<5|ai))
		}
		m(fmt.Sprintf("invalid-ai-28 %s as value", label), fmt.Sprintf("a100%02x", mj<<5|28))
	}
	m("ai31 positive-integer", "1f")
	m("ai31 negative-integer", "3f")
	m("ai31 tag", "df")
	m("ai31 value", "a1003f")
	m("stray-break", "ff")
	m("stray-break-as-value", "a100ff")
	for v := 0; v < 32; v++ {
		m(fmt.Sprintf("simple-f8-%02x", v), fmt.Sprintf("f8%02x", v))
	}
	m("simple-f8-1f-as-value", "a100f81f")
	// W5 lengths not fitting int.
	m("bstr-length-overflow", "a1075b8000000000000000")
	m("text-length-overflow", "a1067b8000000000000000")
	m("bstr-length-past-input", "a1075b7fffffffffffffff")
	m("array-length-overflow", "a1049b8000000000000000")
	m("map-length-overflow", "bb8000000000000000")
	// W6 nesting.
	for n := 29; n <= 33; n++ {
		m(fmt.Sprintf("nested-arrays-%d under refs", n), "a115"+strings.Repeat("81", n)+"00")
		m(fmt.Sprintf("nested-arrays-%d under unknown key", n), "a11863"+strings.Repeat("81", n)+"00")
		m(fmt.Sprintf("nested-tags-%d", n), "a100"+strings.Repeat("d82a", n)+"00")
		m(fmt.Sprintf("nested-maps-%d under unknown key", n), "a11863"+strings.Repeat("a100", n)+"00")
	}
	// W6/W8 element caps.
	d.msg("array-131072 definite under unknown key", "", "", wvParts("a2000018639a00020000", 1, "00", 131072))
	m("array-131073 definite head only", "a2000018639a00020001")
	d.msg("array-131073 definite with elements", "", "", wvParts("a2000018639a00020001", 1, "00", 131073))
	d.msg("array-131072 indefinite", "", "", wvParts("a2000018639f", 1, "00", 131072, "ff", 1))
	d.msg("array-131073 indefinite", "", "", wvParts("a2000018639f", 1, "00", 131073, "ff", 1))
	d.msg("map-131072 definite under unknown key", "", "", wvParts("a200001863ba00020000", 1, "0000", 131072))
	m("map-131073 definite head only", "a200001863ba00020001")
	m("map-131073 definite top level head only", "ba00020001")
	d.msg("map-131072 indefinite", "", "", wvParts("a200001863bf", 1, "0000", 131072, "ff", 1))
	d.msg("map-131073 indefinite", "", "", wvParts("a200001863bf", 1, "0000", 131073, "ff", 1))
	d.msg("keys-131072 into keys", "", "", wvParts("a100049a00020000", 1, "40", 131072))
	m("indefinite-map-odd-items", "bf00ff")
	m("indefinite-map-odd-items as value", "a11863bf00ff")
	// W7 chunks.
	m("bstr-chunk-wrong-type", "a1075f6161ff")
	m("bstr-chunk-indefinite", "a1075f5fffff")
	m("text-chunk-wrong-type", "a1067f4161ff")
	m("text-chunk-indefinite", "a1067f7fffff")
	// W9 tags without content, W11 extraneous data.
	m("tag-without-content", "a100c1")
	m("extraneous-2", "a10018350000")
	m("extraneous-after-null", "f600")
	// Pass 1 precedes pass 2: a type error before a malformed item.
	m("type-error-then-malformed", "a20661011c")

	// protocol.Msg.
	p := func(name, h string) { d.proto(name, "", "", wvOne(h)) }
	p("tdata", "a2000708450102030405")
	p("tdata-empty", "a200070840")
	p("tdataend", "a10008")
	p("dstore-terr-busy-view-retry", "a5000a0a64627573790b61740c420102181c07")
	p("dstore-tabsent-keys", "a200183204814101")
	p("dstore-trefs", "a2001838158"+"1a1006161")
	p("tdata-with-epoch", "a300070307084100")
	p("data-ports", "a2000c0f82191068191069")
	p("data-port-overflow", "a2000c0f811a00010000")
	p("data-endpoints", "a2000c1081a20041010181"+"6161")
	p("names", "a2000d1181626162")
	p("empty", "")

	// ticket.Ticket.
	t := func(name, h string) { d.ticket(name, "", "", wvOne(h)) }
	t("empty-cluster-id-member-with-id", "a3004001000281a1004140")
	t("nil-everything", "a300f601000281a100f6")
	t("null-members", "a300f6010002f6")
	t("empty-members", "a3004001000280")
	t("unknown-key-3", "a40040010002800301")
	t("null-member-element", "a300f601000281f6")
	t("member-null-addr-element", "a3004001000281a2004101018"+"1f6")
	t("incarnation-max", "a30040011bffffffffffffffff0280")
	t("incarnation-negative", "a300400120"+"0280")
	t("members-not-array", "a3004001000201")
	t("top-level-array", "83400080")

	// node.AdminRequest.
	ar := func(name, h string) { d.adminRequest(name, "", "", wvOne(h)) }
	ar("gc-run-half", "a2006667632d72756e09f93800")
	ar("gc-run-nan", "a2006667632d72756e09f97e00")
	ar("gc-run-float32", "a2006667632d72756e09fa47c35000")
	ar("gc-run-float64", "a2006667632d72756e09fb3fb999999999999a")
	ar("gc-run-uint", "a2006667632d72756e0901")
	ar("replicas-overflow", "a200687265706c6963617304190100")
	ar("weight-overflow", "a2006b6e6f64652d776569676874021b0000000100000000")
	ar("names-null-element", "a200646b656570"+"0e82f66161")

	// node.AdminReply.
	rp := func(name, h string) { d.adminReply(name, "", "", wvOne(h)) }
	rp("text-ok", "a100626f6b")
	rp("unknown-key-99", "a200626f6b186301")
	rp("empty", "a0")
	rp("null", "f6")
	rp("break", "ff")
	rp("text-as-bytes", "a1004161")
	rp("names-and-key", "a3006174038161780443010203")

	// node.Status.
	s := func(name, h string) { d.status(name, "", "", wvOne(h)) }
	s("zero-status", "b000f601000200030004000500060008000df40e000f0010001100120013001400")
	s("break", "ff")
	s("empty-map", "a0")
	s("unknown-key", "a20105186301")
	s("voter-null", "a10c81f6")
	s("free-bytes-negative", "a10e3903ff")
	s("packs-overflow", "a1031b8000000000000000")
}

// ---- wire/frame_errors.json ----

type wvReadResult struct {
	Result  string `json:"result"`
	Msg     *wvMsg `json:"msg,omitempty"`
	GoError string `json:"go_error,omitempty"`
}

type wvProtoReadResult struct {
	Result  string      `json:"result"`
	Msg     *wvProtoMsg `json:"msg,omitempty"`
	GoError string      `json:"go_error,omitempty"`
}

type wvStreamCase struct {
	Name          string              `json:"name"`
	Source        string              `json:"source"`
	StreamHex     string              `json:"stream_hex"`
	Reads         []wvReadResult      `json:"reads"`
	ProtocolReads []wvProtoReadResult `json:"protocol_reads"`
}

type wvWriteCase struct {
	Name         string  `json:"name"`
	Msg          wvMsg   `json:"msg"`
	Data         Payload `json:"data"`
	OK           bool    `json:"ok"`
	GoError      string  `json:"go_error,omitempty"`
	BytesWritten int     `json:"bytes_written"`
	FrameBlake3  string  `json:"frame_blake3,omitempty"`
	HeadHex      string  `json:"head_hex,omitempty"`
}

type wvExpectCase struct {
	Name      string         `json:"name"`
	StreamHex string         `json:"stream_hex"`
	Want      I64            `json:"want"`
	OK        bool           `json:"ok"`
	Msg       *wvMsg         `json:"msg,omitempty"`
	Kind      string         `json:"kind,omitempty"`
	GoError   string         `json:"go_error,omitempty"`
	Remote    *wvRemoteError `json:"remote,omitempty"`
}

type wvFrameErrorsFile struct {
	Reads  []wvStreamCase `json:"reads"`
	Writes []wvWriteCase  `json:"writes"`
	Expect []wvExpectCase `json:"expect"`
}

// wvReadKind classifies a ReadMsg error of wire ("wire: ") or protocol ("protocol: ").
func wvReadKind(err error, prefix string) string {
	switch {
	case err == nil:
		return "ok"
	case err == io.EOF:
		return "eof"
	case err == io.ErrUnexpectedEOF:
		return "unexpected_eof"
	case strings.HasPrefix(err.Error(), prefix+"frame of "):
		return "too_large"
	case strings.HasPrefix(err.Error(), prefix+"short frame: "):
		return "short"
	case strings.HasPrefix(err.Error(), prefix+"decode frame: "):
		return "decode"
	}
	return "io"
}

func genWireFrameErrors(out string) error {
	f := &wvFrameErrorsFile{}
	okFrame := "00000004a1001835"
	refFrame := "00000008a300183502010307"
	streams := []struct{ name, source, h string }{
		{"empty-stream", "codec-wire-ticket §5 G3", ""},
		{"header-1-byte", "codec-wire-ticket §5 G3", "00"},
		{"header-2-bytes", "codec-wire-ticket §5 G3", "0000"},
		{"header-3-bytes", "verification §5", "000000"},
		{"too-large-16777217", "verification §5", "01000001"},
		{"too-large-max-u32", "codec-wire-ticket §2.2.4", "ffffffff"},
		{"max-frame-no-payload", "codec-wire-ticket §2.2.4", "01000000"},
		{"len-5-with-1-byte", "codec-wire-ticket §5 G3", "00000005a1"},
		{"len-5-with-0-bytes", "codec-wire-ticket §5 G3", "00000005"},
		{"len-4-with-2-bytes", "verification §5", "00000004a100"},
		{"len-0", "verification §5", "00000000"},
		{"decode-array", "codec-wire-ticket §2.2.4", "000000028100"},
		{"decode-extraneous", "codec-wire-ticket §5 G3", "00000005a100183500"},
		{"decode-invalid-utf8", "codec-wire-ticket §5 G3", "00000004a10661ff"},
		{"top-null", "codec-wire-ticket §2.1.2 D1", "00000001f6"},
		{"two-frames", "verification §5", okFrame + refFrame},
		{"frame-then-partial-header", "codec-wire-ticket §5 G3", okFrame + "0000"},
		{"frame-then-partial-payload", "codec-wire-ticket §5 G3", okFrame + "00000004a1"},
		{"frame-then-bad-frame", "codec-wire-ticket §5 G3", okFrame + "00000001ff"},
		{"dstore-tdata-epoch", "codec-wire-ticket §2.3.5", "0000000aa4000702010307084100"},
	}
	for _, s := range streams {
		raw, err := hex.DecodeString(s.h)
		if err != nil {
			return fmt.Errorf("%s: %w", s.name, err)
		}
		c := wvStreamCase{Name: s.name, Source: s.source, StreamHex: s.h, Reads: []wvReadResult{}, ProtocolReads: []wvProtoReadResult{}}
		r := bytes.NewReader(raw)
		for {
			m, err := wire.ReadMsg(r)
			res := wvReadResult{Result: wvReadKind(err, "wire: ")}
			if err != nil {
				if err != io.EOF {
					res.GoError = err.Error()
				}
				c.Reads = append(c.Reads, res)
				break
			}
			j := wvMsgJSON(m)
			res.Msg = &j
			c.Reads = append(c.Reads, res)
		}
		pr := bytes.NewReader(raw)
		for {
			m, err := protocol.ReadMsg(pr)
			res := wvProtoReadResult{Result: wvReadKind(err, "protocol: ")}
			if err != nil {
				if err != io.EOF {
					res.GoError = err.Error()
				}
				c.ProtocolReads = append(c.ProtocolReads, res)
				break
			}
			j := wvProtoMsgJSON(&m)
			res.Msg = &j
			c.ProtocolReads = append(c.ProtocolReads, res)
		}
		f.Reads = append(f.Reads, c)
	}

	// G2: WriteMsg refuses a payload over MaxFrame and writes nothing.
	for _, w := range []struct {
		name string
		n    int
	}{{"tdata-payload-exactly-max", wire.MaxFrame - 9}, {"tdata-payload-max-plus-1", wire.MaxFrame - 8}, {"tdata-16mib-data", wire.MaxFrame}} {
		seed := uint64(0x16)
		m := &wire.Msg{Type: wire.TData, Data: smData(seed, w.n)}
		var buf bytes.Buffer
		err := wire.WriteMsg(&buf, m)
		c := wvWriteCase{Name: w.name, Msg: wvMsg{Typ: I64(wire.TData)}, Data: SM(seed, w.n), OK: err == nil, BytesWritten: buf.Len()}
		if err != nil {
			c.GoError = err.Error()
		} else {
			c.FrameBlake3 = blake3Hex(buf.Bytes())
			c.HeadHex = hex.EncodeToString(buf.Bytes()[:13])
		}
		f.Writes = append(f.Writes, c)
	}

	// Expect.
	stale, err := wvFrame(&wire.Msg{Type: wire.TErr, Code: wire.CodeStaleView, Text: "behind", View: []byte{1, 2}, RetryAfter: 1500})
	if err != nil {
		return err
	}
	expects := []struct {
		name string
		h    string
		want int
	}{
		{"terr-want-ok", hex.EncodeToString(stale), wire.TOK},
		{"terr-want-any", hex.EncodeToString(stale), 0},
		{"ok-want-ref", okFrame, wire.TRef},
		{"ok-want-ok", okFrame, wire.TOK},
		{"ok-want-any", okFrame, 0},
		{"empty-want-ref", "", wire.TRef},
		{"decode-error-want-ok", "000000028100", wire.TOK},
		{"short-want-ok", "00000004a100", wire.TOK},
	}
	for _, e := range expects {
		raw, err := hex.DecodeString(e.h)
		if err != nil {
			return err
		}
		m, err := wire.Expect(bytes.NewReader(raw), e.want)
		c := wvExpectCase{Name: e.name, StreamHex: e.h, Want: I64(e.want), OK: err == nil}
		if m != nil {
			j := wvMsgJSON(m)
			c.Msg = &j
		}
		if err != nil {
			c.GoError = err.Error()
			var we *wire.Error
			switch {
			case errors.As(err, &we):
				c.Kind = "remote"
				c.Remote = &wvRemoteError{Code: we.Code, Text: we.Text, View: wvHexE(we.View), RetryAfterNs: I64(we.RetryAfter), RetryAfter: we.RetryAfter.String(), Error: we.Error()}
			case errors.Is(err, wire.ErrProtocol):
				c.Kind = "protocol"
			default:
				c.Kind = wvReadKind(err, "wire: ")
			}
		}
		f.Expect = append(f.Expect, c)
	}
	return writeJSON(filepath.Join(out, "wire", "frame_errors.json"), f)
}

// ---- wire/pack_frames.json ----

type wvPackRecord struct {
	Key       Hex     `json:"key"`
	Flags     uint8   `json:"flags"`
	Ulen      uint32  `json:"ulen"`
	Slen      uint32  `json:"slen"`
	HeaderHex string  `json:"header_hex"`
	Payload   Payload `json:"payload"`
	RecordHex string  `json:"record_hex,omitempty"`
}

type wvRecordSeq struct {
	Count     int `json:"count"`
	FirstSeed U64 `json:"first_seed"`
	Len       int `json:"len"`
}

type wvPackFrame struct {
	Type       int    `json:"type"`
	FrameLen   int    `json:"frame_len"`
	HeadHex    string `json:"head_hex"`
	DataLen    int    `json:"data_len"`
	DataBlake3 string `json:"data_blake3"`
	DataHex    string `json:"data_hex,omitempty"`
}

type wvReadBack struct {
	Records int    `json:"records"`
	Error   string `json:"error,omitempty"`
	Next    string `json:"next"`
}

type wvPackCase struct {
	Name             string         `json:"name"`
	Source           string         `json:"source"`
	Records          []wvPackRecord `json:"records"`
	RecordSeq        *wvRecordSeq   `json:"record_seq,omitempty"`
	RecordsBlake3    string         `json:"records_blake3"`
	SourceErrorAfter *int           `json:"source_error_after,omitempty"`
	GoError          string         `json:"go_error,omitempty"`
	Frames           []wvPackFrame  `json:"frames"`
	StreamLen        int            `json:"stream_len"`
	StreamBlake3     string         `json:"stream_blake3"`
	StreamHex        string         `json:"stream_hex,omitempty"`
	ReadBack         wvReadBack     `json:"read_back"`
}

// wvRec is a record's bytes and its description.
type wvRec struct {
	bytes []byte
	desc  wvPackRecord
}

// wvRawRecord is amberpack.EncodeRecord of Blob data(seed, n); it must stay raw.
func wvRawRecord(seed uint64, n int) (wvRec, error) {
	data := smData(seed, n)
	k, err := key.New(key.Blob, uint64(n), data)
	if err != nil {
		return wvRec{}, err
	}
	rec, err := amberpack.EncodeRecord(k, data)
	if err != nil {
		return wvRec{}, err
	}
	if rec[33] != 0 {
		return wvRec{}, fmt.Errorf("record data(%d, %d) was compressed", seed, n)
	}
	d := wvPackRecord{Key: Hex(k[:]), Ulen: uint32(n), Slen: uint32(n), HeaderHex: hex.EncodeToString(rec[:amberpack.RecHeaderSize]), Payload: SM(seed, n)}
	if len(rec) <= 4096 {
		d.RecordHex = hex.EncodeToString(rec)
	}
	return wvRec{bytes: rec, desc: d}, nil
}

// wvZstdRecord is the Go zstd record of client-transfer §3.3 (4096 × 'a').
func wvZstdRecord() (wvRec, error) {
	data := bytes.Repeat([]byte("a"), 4096)
	k, err := key.New(key.Blob, uint64(len(data)), data)
	if err != nil {
		return wvRec{}, err
	}
	rec, err := amberpack.EncodeRecord(k, data)
	if err != nil {
		return wvRec{}, err
	}
	if rec[33] != 1 {
		return wvRec{}, errors.New("zstd record was not compressed")
	}
	d := wvPackRecord{Key: Hex(k[:]), Flags: 1, Ulen: binary.BigEndian.Uint32(rec[34:38]), Slen: binary.BigEndian.Uint32(rec[38:42]),
		HeaderHex: hex.EncodeToString(rec[:amberpack.RecHeaderSize]), Payload: Inline(rec[amberpack.RecHeaderSize:]), RecordHex: hex.EncodeToString(rec)}
	return wvRec{bytes: rec, desc: d}, nil
}

// wvSendPack runs wire.SendPackRecords; failAfter >= 0 yields an error in place of record failAfter.
func wvSendPack(recs [][]byte, failAfter int) ([]byte, error) {
	var buf bytes.Buffer
	err := wire.SendPackRecords(&buf, func(yield func([]byte, error) bool) {
		for i, r := range recs {
			if i == failAfter {
				yield(nil, errors.New("boom"))
				return
			}
			if !yield(r, nil) {
				return
			}
		}
		if failAfter == len(recs) {
			yield(nil, errors.New("boom"))
		}
	})
	return buf.Bytes(), err
}

// wvPackFramesOf splits a TData…TDataEnd stream into frames.
func wvPackFramesOf(stream []byte) ([]wvPackFrame, error) {
	out := []wvPackFrame{}
	r := bytes.NewReader(stream)
	off := 0
	for r.Len() > 0 {
		n := int(binary.BigEndian.Uint32(stream[off:]))
		m, err := protocol.ReadMsg(r)
		if err != nil {
			return nil, err
		}
		frame := stream[off : off+4+n]
		pf := wvPackFrame{Type: m.Type, FrameLen: n, HeadHex: hex.EncodeToString(frame[:len(frame)-len(m.Data)]), DataLen: len(m.Data), DataBlake3: blake3Hex(m.Data)}
		if len(m.Data) <= 256 {
			pf.DataHex = hex.EncodeToString(m.Data)
		}
		out = append(out, pf)
		off += 4 + n
	}
	return out, nil
}

// wvReadBackOf reads a stream back through wire.NewPackReader and amberpack Records.
func wvReadBackOf(stream []byte) wvReadBack {
	r := bytes.NewReader(stream)
	pr := wire.NewPackReader(r)
	rb := wvReadBack{}
	for _, err := range amberpack.NewReader(pr).Records() {
		if err != nil {
			rb.Error = err.Error()
			break
		}
		rb.Records++
	}
	if rb.Error == "" {
		if _, err := io.Copy(io.Discard, pr); err != nil {
			rb.Error = "drain: " + err.Error()
		}
	}
	_, err := wire.ReadMsg(r)
	rb.Next = wvReadKind(err, "wire: ")
	return rb
}

func genPackFrames(out string) error {
	var cases []wvPackCase
	add := func(name, source string, recs []wvRec, seq *wvRecordSeq, failAfter int) error {
		raw := make([][]byte, len(recs))
		var all bytes.Buffer
		c := wvPackCase{Name: name, Source: source, Records: []wvPackRecord{}, RecordSeq: seq}
		for i, r := range recs {
			raw[i] = r.bytes
			all.Write(r.bytes)
			if seq == nil {
				c.Records = append(c.Records, r.desc)
			}
		}
		c.RecordsBlake3 = blake3Hex(all.Bytes())
		stream, err := wvSendPack(raw, failAfter)
		if failAfter >= 0 {
			fa := failAfter
			c.SourceErrorAfter = &fa
			if err == nil {
				return fmt.Errorf("%s: source error not returned", name)
			}
			c.GoError = err.Error()
		} else if err != nil {
			return fmt.Errorf("%s: %w", name, err)
		}
		if c.Frames, err = wvPackFramesOf(stream); err != nil {
			return fmt.Errorf("%s: frames: %w", name, err)
		}
		c.StreamLen, c.StreamBlake3 = len(stream), blake3Hex(stream)
		if len(stream) <= 4096 {
			c.StreamHex = hex.EncodeToString(stream)
		}
		c.ReadBack = wvReadBackOf(stream)
		if failAfter < 0 && (c.ReadBack.Records != len(recs) || c.ReadBack.Error != "" || c.ReadBack.Next != "eof") {
			return fmt.Errorf("%s: read back %+v", name, c.ReadBack)
		}
		cases = append(cases, c)
		return nil
	}
	raws := func(specs ...[2]int) ([]wvRec, error) {
		var out []wvRec
		for _, s := range specs {
			r, err := wvRawRecord(uint64(s[0]), s[1])
			if err != nil {
				return nil, err
			}
			out = append(out, r)
		}
		return out, nil
	}
	type spec struct {
		name, source string
		recs         [][2]int
		failAfter    int
	}
	specs := []spec{
		{"empty-pack", "codec-wire-ticket §2.3.4; verification §5", nil, -1},
		{"one-small", "verification §5", [][2]int{{1, 100}}, -1},
		{"xfer-k1-k2", "client-transfer §3.2", [][2]int{{1, 100}, {2, 200}}, -1},
		{"exactly-chunk", "client-transfer §3.2; codec-wire-ticket §2.3.4", [][2]int{{3, 1048521}}, -1},
		{"chunk-plus-one", "client-transfer §3.2; codec-wire-ticket §2.3.4", [][2]int{{3, 1048522}}, -1},
		{"two-records-2621541", "codec-wire-ticket §2.3.4", [][2]int{{4, 1310720}, {5, 1310720}}, -1},
		{"exactly-2mib", "client-transfer §5.2 item 2", [][2]int{{6, 2097097}}, -1},
		{"record-larger-than-chunk", "verification §5", [][2]int{{7, 2621440}}, -1},
		{"source-error-first", "codec-wire-ticket §5 G6", [][2]int{{1, 100}}, 0},
		{"source-error-after-2-small", "client-transfer §5.2 item 2", [][2]int{{1, 100}, {2, 200}}, 2},
		{"source-error-after-2-large", "codec-wire-ticket §5 G6", [][2]int{{8, 734000}, {9, 734000}, {10, 100}}, 2},
	}
	for _, s := range specs {
		recs, err := raws(s.recs...)
		if err != nil {
			return err
		}
		if err := add(s.name, s.source, recs, nil, s.failAfter); err != nil {
			return err
		}
	}
	var thirty [][2]int
	for i := range 30 {
		thirty = append(thirty, [2]int{100 + i, 102400})
	}
	recs, err := raws(thirty...)
	if err != nil {
		return err
	}
	if err := add("3-mib-30-records", "verification §5", recs, nil, -1); err != nil {
		return err
	}
	var many [][2]int
	for i := range 10000 {
		many = append(many, [2]int{10000 + i, 100})
	}
	if recs, err = raws(many...); err != nil {
		return err
	}
	if err := add("10000-records-of-100", "client-transfer §5.2 item 2", recs, &wvRecordSeq{Count: 10000, FirstSeed: 10000, Len: 100}, -1); err != nil {
		return err
	}
	z, err := wvZstdRecord()
	if err != nil {
		return err
	}
	if recs, err = raws([2]int{1, 100}); err != nil {
		return err
	}
	if err := add("zstd-and-raw", "client-transfer §3.3", append([]wvRec{z}, recs...), nil, -1); err != nil {
		return err
	}
	// Spec self-checks (client-transfer §3.2, codec-wire-ticket §2.3.4).
	want := map[string]string{
		"xfer-k1-k2":     "9031de2f144e916cbe8fe6f664970e60f398c5c98a525d90a808c40883e9fec2",
		"exactly-chunk":  "35dff4ed54e5f98f57aff54cc8631682d85a6450f01782f6f937c6ba36652554",
		"chunk-plus-one": "576a30d47faa6e9d7c8e1243c68c1fca3542a21604cd80ad0e7b9716ea1cc39d",
	}
	for i := range cases {
		c := &cases[i]
		if c.Name == "empty-pack" && c.StreamHex != "0000000ea200070849414d424552504b030000000003a10008" {
			return fmt.Errorf("empty-pack stream differs: %s", c.StreamHex)
		}
		if w, ok := want[c.Name]; ok {
			stream, err := wvSendPack(wvRecBytes(c), -1)
			if err != nil {
				return err
			}
			if got := fmt.Sprintf("%x", sha256.Sum256(stream)); got != w {
				return fmt.Errorf("%s: stream sha256 %s, want %s", c.Name, got, w)
			}
		}
	}
	if z.desc.RecordHex != "01011000cf657d3fd42311a258afdf3b5261c256983e3deb2bb38980cb0e754db901000010000000000fca6d649528b52ffd64000f0380006103162c89" {
		return fmt.Errorf("zstd record differs: %s", z.desc.RecordHex)
	}
	return writeJSON(filepath.Join(out, "wire", "pack_frames.json"), struct {
		Cases []wvPackCase `json:"cases"`
	}{cases})
}

// wvRecBytes rebuilds the record bytes of an explicitly listed case.
func wvRecBytes(c *wvPackCase) [][]byte {
	var out [][]byte
	for _, r := range c.Records {
		h, _ := hex.DecodeString(r.HeaderHex)
		out = append(out, append(h, r.Payload.Materialize()...))
	}
	return out
}

// ---- wire/pack_reader.json ----

type wvProtoRemote struct {
	Code    string `json:"code"`
	Text    string `json:"text"`
	Current Hex    `json:"current"`
}

type wvPackEnd struct {
	Kind           string         `json:"kind"`
	FrameKind      string         `json:"frame_kind,omitempty"`
	Error          string         `json:"error,omitempty"`
	Remote         *wvProtoRemote `json:"remote,omitempty"`
	UnexpectedType *I64           `json:"unexpected_type,omitempty"`
}

type wvPackRead struct {
	ReadSize int       `json:"read_size"`
	DataHex  string    `json:"data_hex"`
	End      wvPackEnd `json:"end"`
	Again    wvPackEnd `json:"again"`
}

type wvReadRecord struct {
	Key      Hex    `json:"key"`
	Flags    uint8  `json:"flags"`
	Ulen     uint32 `json:"ulen"`
	Slen     uint32 `json:"slen"`
	BytesHex string `json:"bytes_hex"`
}

type wvRecordsRead struct {
	Records    []wvReadRecord `json:"records"`
	Error      string         `json:"error,omitempty"`
	DrainError string         `json:"drain_error,omitempty"`
	Next       wvReadResult   `json:"next"`
}

type wvPackReaderCase struct {
	Name      string        `json:"name"`
	Source    string        `json:"source"`
	StreamHex string        `json:"stream_hex"`
	Read      wvPackRead    `json:"read"`
	Records   wvRecordsRead `json:"records"`
}

// wvPackEndOf classifies a packReader error.
func wvPackEndOf(err error) wvPackEnd {
	var re *protocol.RemoteError
	switch {
	case err == io.EOF:
		return wvPackEnd{Kind: "eof"}
	case errors.As(err, &re):
		return wvPackEnd{Kind: "remote", Error: err.Error(), Remote: &wvProtoRemote{Code: re.Code, Text: re.Text, Current: wvHexE(re.Current)}}
	case errors.Is(err, protocol.ErrProtocol):
		var t int
		_, _ = fmt.Sscanf(err.Error(), "protocol: unexpected frame: type %d during pack transfer", &t)
		ut := I64(t)
		return wvPackEnd{Kind: "unexpected", Error: err.Error(), UnexpectedType: &ut}
	}
	return wvPackEnd{Kind: "frame", FrameKind: wvReadKind(err, "protocol: "), Error: err.Error()}
}

func genPackReader(out string) error {
	fr := func(m *wire.Msg) []byte { return wvMust(wvFrame(m)) }
	pf := func(m protocol.Msg) []byte {
		var buf bytes.Buffer
		if err := protocol.WriteMsg(&buf, m); err != nil {
			panic("vectorgen: " + err.Error())
		}
		return buf.Bytes()
	}
	data := func(b []byte) []byte { return pf(protocol.Msg{Type: protocol.TData, Data: b}) }
	end := pf(protocol.Msg{Type: protocol.TDataEnd})
	cat := func(parts ...[]byte) []byte { return bytes.Join(parts, nil) }
	magic := []byte("AMBERPK\x03")
	rec1r, err := wvRawRecord(1, 100)
	if err != nil {
		return err
	}
	rec1 := rec1r.bytes
	z, err := wvZstdRecord()
	if err != nil {
		return err
	}
	recrc := func(rec []byte) []byte {
		out := append([]byte(nil), rec...)
		binary.BigEndian.PutUint32(out[42:46], 0)
		binary.BigEndian.PutUint32(out[42:46], crc32.Checksum(out, crc32.MakeTable(crc32.Castagnoli)))
		return out
	}
	flipped := append([]byte(nil), rec1...)
	flipped[amberpack.RecHeaderSize+99] ^= 1
	badUlen := append([]byte(nil), rec1...)
	binary.BigEndian.PutUint32(badUlen[34:38], 101)
	badUlen = recrc(badUlen)
	reservedKey := append([]byte(nil), rec1...)
	reservedKey[1] |= 0x80
	reservedKey = recrc(reservedKey)
	overLimit := append([]byte(nil), rec1[:amberpack.RecHeaderSize]...)
	binary.BigEndian.PutUint32(overLimit[38:42], amberpack.MaxPayload+1)
	okMsg := fr(&wire.Msg{Type: wire.TOK})
	sent, err := wvSendPack([][]byte{rec1}, -1)
	if err != nil {
		return err
	}
	streams := []struct {
		name, source string
		b            []byte
	}{
		{"tdata-tdataend-then-ok", "codec-wire-ticket §5 G6", cat(data([]byte("hello")), end, okMsg)},
		{"tdata-without-data", "codec-wire-ticket §5 G6", cat(pf(protocol.Msg{Type: protocol.TData}), data([]byte("ab")), end)},
		{"tdataend-only", "codec-wire-ticket §5 G6", end},
		{"empty-stream", "codec-wire-ticket §2.3.5", nil},
		{"eof-at-frame-boundary", "codec-wire-ticket §5 G6", data([]byte("abc"))},
		{"eof-mid-frame", "codec-wire-ticket §5 G6", data([]byte("abcdef"))[:12]},
		{"oversize-frame", "transport-iroh protocol_test", []byte{0x01, 0x00, 0x00, 0x01}},
		{"terr-internal-sender-died", "transport-iroh pack_test; client-transfer §5.2 item 2", cat(data([]byte("junk")), fr(wire.ErrMsg(wire.CodeInternal, "sender died")))},
		{"terr-busy-no-text", "verification §5", cat(data(magic), fr(wire.ErrMsg(wire.CodeBusy, "")))},
		{"dstore-terr-view-retry-after", "codec-wire-ticket §2.3.5", fr(&wire.Msg{Type: wire.TErr, Code: wire.CodeBusy, View: []byte{1, 2}, RetryAfter: 7})},
		{"dstore-terr-stamped", "codec-wire-ticket §2.3.5 (key 2 is protocol.Msg.Root)", fr(wvRStamp(&wire.Msg{Type: wire.TErr, Code: wire.CodeBusy, View: []byte{1, 2}, RetryAfter: 7}, 1, 7))},
		{"dstore-tabsent", "codec-wire-ticket §2.3.5", fr(&wire.Msg{Type: wire.TAbsent, Keys: [][]byte{{1}}})},
		{"dstore-trefs", "codec-wire-ticket §2.3.5", fr(&wire.Msg{Type: wire.TRefs, Refs: []wire.RefInfo{{Name: "a"}}})},
		{"tdata-with-epoch", "codec-wire-ticket §2.3.5", fr(&wire.Msg{Type: wire.TData, Epoch: 7, Data: []byte{0}})},
		{"type-51-mid-pack", "client-transfer §5.2 item 2", cat(data(magic), fr(&wire.Msg{Type: wire.TPutResult}))},
		{"type-51-stamped-mid-pack", "client-transfer §5.2 item 2 (stamped: key 2 is protocol.Msg.Root)", cat(data(magic), fr(wvRStamp(&wire.Msg{Type: wire.TPutResult}, 1, 7)))},
		{"twants", "transport-iroh pack_test", pf(protocol.Msg{Type: protocol.TWants})},
		{"pack-one-record-then-ok", "wire_test TestPackFramesInterop", cat(sent, okMsg)},
		{"bad-magic", "client-transfer §5.2 item 2", cat(data([]byte("AMBERPK\x02\x00")), end)},
		{"short-magic", "core amberpack", cat(data([]byte("AMBER")), end)},
		{"record-payload-over-limit", "client-transfer §5.2 item 2", cat(data(cat(magic, overLimit)), end)},
		{"truncated-record-header", "core amberpack", cat(data(cat(magic, rec1[:10])), end)},
		{"truncated-record-payload", "core amberpack", cat(data(cat(magic, rec1[:60])), end)},
		{"bad-record-tag", "core amberpack", cat(data(cat(magic, []byte{2})), end)},
		{"record-crc-mismatch", "core amberpack", cat(data(cat(magic, flipped, []byte{0})), end)},
		{"record-raw-ulen-differs", "core amberpack", cat(data(cat(magic, badUlen, []byte{0})), end)},
		{"record-reserved-key-bit", "core amberpack", cat(data(cat(magic, reservedKey, []byte{0})), end)},
		{"no-end-marker", "client-transfer §5.2 item 2", cat(data(cat(magic, rec1)), end)},
		{"end-marker-own-frame", "codec-wire-ticket §2.3.5", cat(data(magic), data(rec1), data([]byte{0}), end, okMsg)},
		{"data-after-end-marker", "codec-wire-ticket §2.3.5", cat(data(cat(magic, []byte{0, 0xff, 0xff})), end, okMsg)},
		{"terr-after-end-marker", "codec-wire-ticket §2.3.5", cat(data(cat(magic, []byte{0})), fr(wire.ErrMsg(wire.CodeBusy, "late")))},
		{"terr-mid-record", "codec-wire-ticket §2.3.5", cat(data(cat(magic, rec1[:50])), fr(wire.ErrMsg(wire.CodeInternal, "sender died")))},
		{"eof-mid-record", "codec-wire-ticket §2.3.5", data(cat(magic, rec1[:50]))},
		{"zstd-record", "client-transfer §3.3", cat(data(cat(magic, z.bytes, []byte{0})), end)},
	}
	var cases []wvPackReaderCase
	for _, s := range streams {
		c := wvPackReaderCase{Name: s.name, Source: s.source, StreamHex: hex.EncodeToString(s.b)}
		pr := wire.NewPackReader(bytes.NewReader(s.b))
		buf := make([]byte, 5)
		var got []byte
		var endErr error
		for {
			n, err := pr.Read(buf)
			got = append(got, buf[:n]...)
			if err != nil {
				endErr = err
				break
			}
		}
		_, again := pr.Read(buf)
		c.Read = wvPackRead{ReadSize: len(buf), DataHex: hex.EncodeToString(got), End: wvPackEndOf(endErr), Again: wvPackEndOf(again)}

		r := bytes.NewReader(s.b)
		pr2 := wire.NewPackReader(r)
		rr := wvRecordsRead{Records: []wvReadRecord{}}
		for raw, err := range amberpack.NewReader(pr2).Records() {
			if err != nil {
				rr.Error = err.Error()
				break
			}
			rr.Records = append(rr.Records, wvReadRecord{Key: Hex(append([]byte(nil), raw.Key[:]...)), Flags: raw.Flags, Ulen: raw.Ulen, Slen: raw.Slen, BytesHex: hex.EncodeToString(raw.Bytes)})
		}
		if _, err := io.Copy(io.Discard, pr2); err != nil {
			rr.DrainError = err.Error()
		}
		m, err := wire.ReadMsg(r)
		rr.Next = wvReadResult{Result: wvReadKind(err, "wire: ")}
		if err != nil && err != io.EOF {
			rr.Next.GoError = err.Error()
		}
		if m != nil {
			j := wvMsgJSON(m)
			rr.Next.Msg = &j
		}
		c.Records = rr
		cases = append(cases, c)
	}
	return writeJSON(filepath.Join(out, "wire", "pack_reader.json"), struct {
		Cases []wvPackReaderCase `json:"cases"`
	}{cases})
}
