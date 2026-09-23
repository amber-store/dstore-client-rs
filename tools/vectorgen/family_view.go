package main

// Families "view" (view/view_placement.json) and "placement"
// (placement/all_slots.json): golden vectors of the dstore v0.1.11 view and
// placement packages, worktree.TicketFromView and node.ShortID
// (port-notes/view-placement.md §5 and Appendix A, port-notes/verification.md
// §4.3 item 9). Schemas: docs/vectorgen-view.md.
//
// Placement ids and keys follow view-placement.md §5: splitmix64 data(seed, 32)
// or 32 copies of one byte, not ed25519 keys, since placement never validates
// ids as curve points.

import (
	"bytes"
	"crypto/ed25519"
	"encoding/base32"
	"encoding/binary"
	"encoding/hex"
	"errors"
	"fmt"
	"path/filepath"
	"strings"
	"sync"

	"github.com/amber-store/dstore/node"
	"github.com/amber-store/dstore/placement"
	"github.com/amber-store/dstore/view"
	"github.com/amber-store/dstore/worktree"
	"github.com/zeebo/blake3"
)

func init() {
	register("view", []string{"view/view_placement.json"}, genViewPlacement)
	register("placement", []string{"placement/all_slots.json"}, genPlacementAllSlots)
}

// vplSaltDomain is placement.saltDomain (unexported); every salt vector checks
// it against placement.Salt.
const vplSaltDomain = "amber-dstore/placement/1"

// ---- JSON shapes of view/view_placement.json ----

type vplConstants struct {
	SlotBits         int    `json:"slot_bits"`
	Slots            int    `json:"slots"`
	SaltDomain       string `json:"salt_domain"`
	VoterSyncDone    int    `json:"voter_sync_done"`
	VoterSyncPending int    `json:"voter_sync_pending"`
	Log2FixZeroPanic string `json:"log2fix_zero_panic"`
}

type vplPair struct {
	In  U64 `json:"in"`
	Out U64 `json:"out"`
}

type vplSlot struct {
	Key  Hex    `json:"key"`
	Slot uint32 `json:"slot"`
}

type vplSalt struct {
	ID   Hex `json:"id"`
	Salt U64 `json:"salt"`
}

type vplL struct {
	Slot uint32 `json:"slot"`
	Salt U64    `json:"salt"`
	L    U64    `json:"l"`
}

type vplMember struct {
	ID     Hex    `json:"id"`
	Weight uint32 `json:"weight"`
	Zone   string `json:"zone"`
}

type vplOwners struct {
	R      int   `json:"r"`
	Owners []int `json:"owners"`
}

type vplSetSlot struct {
	Slot   uint32      `json:"slot"`
	L      []*U64      `json:"l"`
	Rank   []int       `json:"rank"`
	Owners []vplOwners `json:"owners"`
}

type vplSet struct {
	Name     string       `json:"name"`
	Members  []vplMember  `json:"members"`
	Replicas []int        `json:"replicas"`
	Slots    []vplSetSlot `json:"slots"`
}

type vplFlags struct {
	ID             Hex  `json:"id"`
	IsOwner        bool `json:"is_owner"`
	IsPendingOwner bool `json:"is_pending_owner"`
	InWriteSet     bool `json:"in_write_set"`
}

type vplKey struct {
	Key           Hex        `json:"key"`
	Slot          uint32     `json:"slot"`
	Owners        []Hex      `json:"owners"`
	PendingOwners []Hex      `json:"pending_owners"`
	WriteSet      []Hex      `json:"write_set"`
	ReadOrder     []Hex      `json:"read_order"`
	Flags         []vplFlags `json:"flags"`
}

type vplView struct {
	Name string   `json:"name"`
	CBOR Hex      `json:"cbor"`
	Keys []vplKey `json:"keys"`
}

type vplCBORCase struct {
	Name string `json:"name"`
	CBOR Hex    `json:"cbor"`
}

type vplDecode struct {
	Name     string  `json:"name"`
	Input    Hex     `json:"input"`
	OK       bool    `json:"ok"`
	Reencode Hex     `json:"reencode"`
	Error    *string `json:"error"`
}

type vplParse struct {
	InputHex Hex     `json:"input_hex"`
	OK       bool    `json:"ok"`
	ID       Hex     `json:"id"`
	Error    *string `json:"error"`
}

type vplShort struct {
	Bytes     Hex     `json:"bytes"`
	NodeShort string  `json:"node_short"`
	ViewShort *string `json:"view_short"`
	IDString  *string `json:"id_string"`
}

type vplTicket struct {
	Name       string `json:"name"`
	View       Hex    `json:"view"`
	Ticket     string `json:"ticket"`
	TicketCBOR Hex    `json:"ticket_cbor"`
}

type vplLookup struct {
	ID                Hex      `json:"id"`
	NodeFound         bool     `json:"node_found"`
	NodeWeight        uint32   `json:"node_weight"`
	NodeAddrs         []string `json:"node_addrs"`
	IsMember          bool     `json:"is_member"`
	DataEndpointOwner Hex      `json:"data_endpoint_owner"`
	IsFormer          bool     `json:"is_former"`
	IsVoter           bool     `json:"is_voter"`
}

type vplHelpers struct {
	Name       string      `json:"name"`
	View       Hex         `json:"view"`
	AllMembers []Hex       `json:"all_members"`
	VoterIDs   []Hex       `json:"voter_ids"`
	Quorum     int         `json:"quorum"`
	Lookups    []vplLookup `json:"lookups"`
}

type vplNodeHelper struct {
	ID       Hex    `json:"id"`
	Zone     string `json:"zone"`
	NID      Hex    `json:"nid"`
	ZoneOrID Hex    `json:"zone_or_id"`
}

type vplIDList struct {
	IDs      []Hex `json:"ids"`
	ID       Hex   `json:"id"`
	Contains bool  `json:"contains"`
	AddID    []Hex `json:"add_id"`
}

type vplIDsOf struct {
	Raw []Hex `json:"raw"`
	IDs []Hex `json:"ids"`
}

type vplCompare struct {
	ViewIncarnation U64 `json:"view_incarnation"`
	ViewEpoch       U64 `json:"view_epoch"`
	Incarnation     U64 `json:"incarnation"`
	Epoch           U64 `json:"epoch"`
	Result          int `json:"result"`
}

type vplMinR struct {
	R           int `json:"r"`
	MinReplicas int `json:"min_replicas"`
}

type vplValidate struct {
	Name     string      `json:"name"`
	Cur      []vplMember `json:"cur"`
	Target   []vplMember `json:"target"`
	Replicas int         `json:"replicas"`
	Force    bool        `json:"force"`
	Error    *string     `json:"error"`
}

type vplSortNodes struct {
	Input  []Hex `json:"input"`
	Output []Hex `json:"output"`
}

// vplStatusCID is one status_cluster_prefix case: cmd/dstore printStatus
// prints v.ClusterID[:4] of the decoded view (cmd/dstore/client.go:136), which
// panics when cap(v.ClusterID) < 4.
type vplStatusCID struct {
	Name      string  `json:"name"`
	View      Hex     `json:"view"`
	ClusterID Hex     `json:"cluster_id"`
	Cap       int     `json:"cap"`
	Prefix    *string `json:"prefix"`
	Panic     *string `json:"panic"`
}

type vplFile struct {
	Constants   vplConstants    `json:"constants"`
	Fmix64      []vplPair       `json:"fmix64"`
	Log2Fix     []vplPair       `json:"log2fix"`
	Slot        []vplSlot       `json:"slot"`
	Salt        []vplSalt       `json:"salt"`
	L           []vplL          `json:"l"`
	Sets        []vplSet        `json:"sets"`
	Views       []vplView       `json:"views"`
	ViewCBOR    []vplCBORCase   `json:"view_cbor"`
	ViewDecode  []vplDecode     `json:"view_decode"`
	ParseNodeID []vplParse      `json:"parse_node_id"`
	ShortID     []vplShort      `json:"short_id"`
	TicketFrom  []vplTicket     `json:"ticket_from_view"`
	Helpers     []vplHelpers    `json:"helpers"`
	NodeHelpers []vplNodeHelper `json:"node_helpers"`
	IDLists     []vplIDList     `json:"id_lists"`
	IDsOf       []vplIDsOf      `json:"ids_of"`
	Compare     []vplCompare    `json:"compare"`
	DefaultMinR []vplMinR       `json:"default_min_replicas"`
	Validate    []vplValidate   `json:"validate_change"`
	SortNodes   vplSortNodes    `json:"sort_nodes"`
	StatusCID   []vplStatusCID  `json:"status_cluster_prefix"`
}

// ---- JSON shapes of placement/all_slots.json ----

type vplSpot struct {
	Slot   uint32 `json:"slot"`
	Rank   []int  `json:"rank"`
	Owners []int  `json:"owners"`
}

type vplAllSlotsSet struct {
	Name           string      `json:"name"`
	Members        []vplMember `json:"members"`
	R              int         `json:"r"`
	Spots          []vplSpot   `json:"spots"`
	AllSlotsBlake3 string      `json:"all_slots_blake3"`
}

type vplAllSlotsFile struct {
	Slots int              `json:"slots"`
	Hash  string           `json:"hash"`
	Sets  []vplAllSlotsSet `json:"sets"`
}

// ---- helpers ----

// vplErrs keeps the first error of a generator that builds many values.
type vplErrs struct{ err error }

func (g *vplErrs) set(err error) {
	if g.err == nil && err != nil {
		g.err = err
	}
}

// enc encodes v, recording a failure (and returning nil).
func (g *vplErrs) enc(v *view.View) Hex {
	b, err := v.Encode()
	if err != nil {
		g.set(fmt.Errorf("encode view: %w", err))
		return nil
	}
	return Hex(b)
}

// vplBytes copies b, keeping nil as nil.
func vplBytes(b []byte) Hex {
	if b == nil {
		return nil
	}
	return append(Hex{}, b...)
}

func vplIDFrom(b byte) placement.NodeID {
	var id placement.NodeID
	for i := range id {
		id[i] = b
	}
	return id
}

func vplIDSeed(seed uint64) placement.NodeID {
	var id placement.NodeID
	copy(id[:], smData(seed, 32))
	return id
}

// vplIDEd25519 is id(seed) of port-notes/verification.md §5: the ed25519 public
// key of NewKeyFromSeed(data(seed, 32)).
func vplIDEd25519(seed uint64) placement.NodeID {
	var id placement.NodeID
	priv := ed25519.NewKeyFromSeed(smData(seed, 32))
	copy(id[:], priv[ed25519.SeedSize:])
	return id
}

// vplClusterPrefix runs fmt.Sprintf("%x", id[:4]) as cmd/dstore printStatus
// does, and returns its output or the text of the runtime panic.
func vplClusterPrefix(id []byte) (prefix, panicText *string) {
	defer func() {
		if r := recover(); r != nil {
			t := fmt.Sprint(r)
			panicText = &t
		}
	}()
	p := fmt.Sprintf("%x", id[:4])
	return &p, nil
}

func vplKeySeed(seed uint64) [32]byte {
	var k [32]byte
	copy(k[:], smData(seed, 32))
	return k
}

func vplKeyWithTail(tail uint64) [32]byte {
	var k [32]byte
	binary.BigEndian.PutUint64(k[24:], tail)
	return k
}

func vplModInverse(a uint64) uint64 {
	inv := a
	for i := 0; i < 6; i++ {
		inv *= 2 - a*inv
	}
	return inv
}

// vplInvFmix64 inverts the murmur3 finalizer (x ^= x>>33 is an involution).
func vplInvFmix64(x uint64) uint64 {
	x ^= x >> 33
	x *= vplModInverse(0xc4ceb9fe1a85ec53)
	x ^= x >> 33
	x *= vplModInverse(0xff51afd7ed558ccd)
	x ^= x >> 33
	return x
}

func vplHexIDs(ids []placement.NodeID) []Hex {
	if ids == nil {
		return nil
	}
	out := make([]Hex, len(ids))
	for i := range ids {
		out[i] = append(Hex{}, ids[i][:]...)
	}
	return out
}

func vplMembers(members []placement.Member) []vplMember {
	out := make([]vplMember, 0, len(members))
	for _, m := range members {
		out = append(out, vplMember{ID: append(Hex{}, m.ID[:]...), Weight: m.Weight, Zone: m.Zone})
	}
	return out
}

func vplNodeMembers(nodes []view.Node) []vplMember {
	out := make([]vplMember, 0, len(nodes))
	for _, n := range nodes {
		out = append(out, vplMember{ID: vplBytes(n.ID), Weight: n.Weight, Zone: n.Zone})
	}
	return out
}

// vplLog2FixZeroPanic returns the panic value of placement.Log2Fix(0).
func vplLog2FixZeroPanic() (text string) {
	defer func() {
		if r := recover(); r != nil {
			text = fmt.Sprint(r)
		}
	}()
	placement.Log2Fix(0)
	return ""
}

func vplSetVector(name string, members []placement.Member, rs []int, slots []uint32) vplSet {
	set := placement.NewSet(members)
	sv := vplSet{Name: name, Members: vplMembers(members), Replicas: rs, Slots: []vplSetSlot{}}
	for _, s := range slots {
		ss := vplSetSlot{Slot: s, L: []*U64{}, Rank: set.Rank(s), Owners: []vplOwners{}}
		if ss.Rank == nil {
			ss.Rank = []int{}
		}
		for _, m := range members {
			if m.Weight == 0 {
				ss.L = append(ss.L, nil)
				continue
			}
			l := U64(placement.L(s, placement.Salt(m.ID)))
			ss.L = append(ss.L, &l)
		}
		for _, r := range rs {
			ss.Owners = append(ss.Owners, vplOwners{R: r, Owners: set.Owners(s, r)})
		}
		sv.Slots = append(sv.Slots, ss)
	}
	return sv
}

func vplViewVector(g *vplErrs, name string, v *view.View, keys [][32]byte, extra []placement.NodeID) vplView {
	pl := view.NewPlacement(v)
	vv := vplView{Name: name, CBOR: g.enc(v), Keys: []vplKey{}}
	ids := append(v.AllMembers(), extra...)
	for _, k := range keys {
		kv := vplKey{
			Key:           append(Hex{}, k[:]...),
			Slot:          placement.Slot(k),
			Owners:        vplHexIDs(pl.Owners(k)),
			PendingOwners: vplHexIDs(pl.PendingOwners(k)),
			WriteSet:      vplHexIDs(pl.WriteSet(k)),
			ReadOrder:     vplHexIDs(pl.ReadOrder(k)),
			Flags:         []vplFlags{},
		}
		for _, id := range ids {
			kv.Flags = append(kv.Flags, vplFlags{
				ID:             append(Hex{}, id[:]...),
				IsOwner:        pl.IsOwner(k, id),
				IsPendingOwner: pl.IsPendingOwner(k, id),
				InWriteSet:     pl.InWriteSet(k, id),
			})
		}
		vv.Keys = append(vv.Keys, kv)
	}
	return vv
}

func vplNode(id placement.NodeID, w uint32, zone string, addrs ...string) view.Node {
	return view.Node{ID: append([]byte{}, id[:]...), Weight: w, Addrs: addrs, Zone: zone, Writable: true, Incarnation: 1}
}

// vplCloneNodes deep-copies nodes through the view codec, as Appendix A does.
func vplCloneNodes(g *vplErrs, nodes []view.Node) []view.Node {
	v, err := view.Decode(g.enc(&view.View{Nodes: nodes}))
	if err != nil {
		g.set(fmt.Errorf("clone nodes: %w", err))
		return nil
	}
	return v.Nodes
}

// vplCBOR is a tiny CBOR writer for hand-built decode inputs.
type vplCBOR struct {
	b   []byte
	err error
}

func (c *vplCBOR) head(major byte, n uint64) *vplCBOR {
	m := major << 5
	switch {
	case n < 24:
		c.b = append(c.b, m|byte(n))
	case n <= 0xff:
		c.b = append(c.b, m|24, byte(n))
	case n <= 0xffff:
		c.b = append(c.b, m|25, byte(n>>8), byte(n))
	case n <= 0xffffffff:
		c.b = append(c.b, m|26, byte(n>>24), byte(n>>16), byte(n>>8), byte(n))
	default:
		c.b = append(c.b, m|27)
		c.b = binary.BigEndian.AppendUint64(c.b, n)
	}
	return c
}

func (c *vplCBOR) u(n uint64) *vplCBOR   { return c.head(0, n) }
func (c *vplCBOR) neg(n uint64) *vplCBOR { return c.head(1, n) }
func (c *vplCBOR) arr(n uint64) *vplCBOR { return c.head(4, n) }
func (c *vplCBOR) mp(n uint64) *vplCBOR  { return c.head(5, n) }

func (c *vplCBOR) bs(b []byte) *vplCBOR {
	c.head(2, uint64(len(b)))
	c.b = append(c.b, b...)
	return c
}

func (c *vplCBOR) ts(s string) *vplCBOR {
	c.head(3, uint64(len(s)))
	c.b = append(c.b, s...)
	return c
}

func (c *vplCBOR) raw(h string) *vplCBOR {
	b, err := hex.DecodeString(h)
	if err != nil && c.err == nil {
		c.err = err
	}
	c.b = append(c.b, b...)
	return c
}

func (c *vplCBOR) bytes(g *vplErrs) []byte {
	g.set(c.err)
	return append([]byte{}, c.b...)
}

func vplDecodeCase(g *vplErrs, name string, in []byte) vplDecode {
	d := vplDecode{Name: name, Input: append(Hex{}, in...)}
	v, err := view.Decode(in)
	if err != nil {
		s := err.Error()
		d.Error = &s
		return d
	}
	d.OK = true
	d.Reencode = g.enc(v)
	return d
}

func vplHelpersOf(g *vplErrs, name string, v *view.View, ids []placement.NodeID) vplHelpers {
	h := vplHelpers{Name: name, View: g.enc(v), AllMembers: vplHexIDs(v.AllMembers()), VoterIDs: vplHexIDs(v.VoterIDs()), Quorum: v.Quorum(), Lookups: []vplLookup{}}
	if h.AllMembers == nil {
		h.AllMembers = []Hex{}
	}
	for _, id := range ids {
		nd, ok := v.Node(id)
		lv := vplLookup{ID: append(Hex{}, id[:]...), NodeFound: ok, NodeWeight: nd.Weight, NodeAddrs: nd.Addrs, IsMember: v.IsMember(id), IsFormer: v.IsFormer(id), IsVoter: v.IsVoter(id)}
		if p := v.DataEndpointOwner(id); p != nil {
			lv.DataEndpointOwner = append(Hex{}, p[:]...)
		}
		h.Lookups = append(h.Lookups, lv)
	}
	return h
}

// ---- placement sets shared by both families ----

func vplGolden5() []placement.Member {
	return []placement.Member{
		{ID: vplIDFrom(0x01), Weight: 1000}, {ID: vplIDFrom(0x02), Weight: 2000}, {ID: vplIDFrom(0x03), Weight: 500},
		{ID: vplIDFrom(0x04), Weight: 4000}, {ID: vplIDFrom(0x05), Weight: 1000},
	}
}

func vplZones() []placement.Member {
	return []placement.Member{
		{ID: vplIDFrom(1), Weight: 100, Zone: "a"}, {ID: vplIDFrom(2), Weight: 100, Zone: "a"},
		{ID: vplIDFrom(3), Weight: 100, Zone: "b"}, {ID: vplIDFrom(4), Weight: 100, Zone: "b"}, {ID: vplIDFrom(5), Weight: 0, Zone: "c"},
	}
}

func vplDraining() []placement.Member {
	return []placement.Member{
		{ID: vplIDFrom(9), Weight: 0}, {ID: vplIDFrom(8), Weight: 300}, {ID: vplIDFrom(7), Weight: 0},
		{ID: vplIDFrom(6), Weight: 300}, {ID: vplIDFrom(5), Weight: 0}, {ID: vplIDFrom(4), Weight: 300},
	}
}

func vplHeavy() []placement.Member {
	return []placement.Member{
		{ID: vplIDFrom(0x10), Weight: 0xffffffff}, {ID: vplIDFrom(0x11), Weight: 1}, {ID: vplIDFrom(0x12), Weight: 0x80000000}, {ID: vplIDFrom(0x13), Weight: 3},
	}
}

// vplTie returns two members whose weights equal their own L at the returned
// slot, so their 128-bit products tie there and the smaller id ranks first.
func vplTie() ([]placement.Member, uint32, error) {
	s9, s7 := placement.Salt(vplIDFrom(9)), placement.Salt(vplIDFrom(7))
	for slot := uint32(0); slot < placement.Slots; slot++ {
		la, lb := placement.L(slot, s9), placement.L(slot, s7)
		if la > 0 && lb > 0 && la < 1<<32 && lb < 1<<32 && la != lb {
			return []placement.Member{{ID: vplIDFrom(9), Weight: uint32(la)}, {ID: vplIDFrom(7), Weight: uint32(lb)}}, slot, nil
		}
	}
	return nil, 0, errors.New("no tie slot found")
}

func vplRealistic12() []placement.Member {
	weights := []uint32{4096, 4096, 8192, 2048, 512, 16384, 4096, 0, 1024, 4096, 3, 1}
	zones := []string{"h1", "h1", "h2", "h2", "h3", "", "", "h4", "h4", "h5", "", ""}
	var out []placement.Member
	for i := range weights {
		out = append(out, placement.Member{ID: vplIDSeed(uint64(200 + i)), Weight: weights[i], Zone: zones[i]})
	}
	return out
}

// vplSplitmix50 is a 50-member set with splitmix weights, every tenth member
// at weight 0, and a mix of explicit and id-derived zones.
func vplSplitmix50() []placement.Member {
	ws := u64s(4242, 50)
	var out []placement.Member
	for i := 0; i < 50; i++ {
		w := uint32(ws[i] % 65536)
		if i%10 == 7 {
			w = 0
		}
		zone := ""
		if i%4 != 0 {
			zone = fmt.Sprintf("rack%d", i%6)
		}
		out = append(out, placement.Member{ID: vplIDSeed(uint64(400 + i)), Weight: w, Zone: zone})
	}
	return out
}

// vplDuplicates lists weighted members twice: equal products tie on equal ids,
// so the stable sort keeps input order, and the second copy shares the zone
// key (the raw id) of the first.
func vplDuplicates() []placement.Member {
	return []placement.Member{
		{ID: vplIDFrom(1), Weight: 100}, {ID: vplIDFrom(1), Weight: 100}, {ID: vplIDFrom(2), Weight: 300}, {ID: vplIDFrom(3), Weight: 0},
		{ID: vplIDFrom(2), Weight: 300},
	}
}

// ---- view/view_placement.json ----

func genViewPlacement(out string) error {
	g := &vplErrs{}
	f := vplFile{Constants: vplConstants{
		SlotBits: placement.SlotBits, Slots: placement.Slots, SaltDomain: vplSaltDomain,
		VoterSyncDone: view.VoterSyncDone, VoterSyncPending: view.VoterSyncPending,
		Log2FixZeroPanic: vplLog2FixZeroPanic(),
	}}
	maxU := ^uint64(0)

	if placement.Fmix64(vplInvFmix64(maxU)) != maxU || placement.Fmix64(vplInvFmix64(0x0123456789abcdef)) != 0x0123456789abcdef {
		return errors.New("vplInvFmix64 does not invert placement.Fmix64")
	}
	for _, x := range append([]uint64{0, 1, 0xdeadbeef, maxU, vplInvFmix64(maxU), vplInvFmix64(0x0123456789abcdef)}, u64s(42, 16)...) {
		f.Fmix64 = append(f.Fmix64, vplPair{In: U64(x), Out: U64(placement.Fmix64(x))})
	}

	l2 := []uint64{1, 2, 3, 5, 7, 255, 256, 12345, 1<<32 - 1, 1 << 32, 1<<32 + 1, 1<<63 - 1, 1 << 63, 1<<63 + 1, maxU - 1, maxU}
	for i := 1; i < 64; i++ {
		l2 = append(l2, uint64(1)<<i, uint64(1)<<i+1, uint64(1)<<i-1)
	}
	for _, x := range append(l2, u64s(7, 1000)...) {
		f.Log2Fix = append(f.Log2Fix, vplPair{In: U64(x), Out: U64(placement.Log2Fix(x))})
	}

	slotKeys := [][32]byte{{}, vplKeyWithTail(maxU), vplKeyWithTail(1 << 44), vplKeyWithTail(1<<44 - 1), vplKeyWithTail(0x8000000000000000), vplKeySeed(1), vplKeySeed(2), vplKeySeed(3)}
	{
		var k [32]byte
		for i := 0; i < 24; i++ {
			k[i] = 0xff
		}
		slotKeys = append(slotKeys, k)
	}
	for i := uint64(0); i < 16; i++ {
		slotKeys = append(slotKeys, vplKeySeed(600+i))
	}
	for _, k := range slotKeys {
		f.Slot = append(f.Slot, vplSlot{Key: append(Hex{}, k[:]...), Slot: placement.Slot(k)})
	}

	saltIDs := []placement.NodeID{vplIDFrom(0), vplIDFrom(1), vplIDFrom(2), vplIDFrom(3), vplIDFrom(4), vplIDFrom(5), vplIDFrom(0xff), vplIDSeed(11), vplIDSeed(12), vplIDSeed(13)}
	for i := uint64(0); i < 12; i++ {
		saltIDs = append(saltIDs, vplIDSeed(200+i))
	}
	for s := uint64(1); s <= 8; s++ {
		saltIDs = append(saltIDs, vplIDEd25519(s))
	}
	for _, id := range saltIDs {
		s := placement.Salt(id)
		sum := blake3.Sum256(append([]byte(vplSaltDomain), id[:]...))
		if binary.BigEndian.Uint64(sum[:8]) != s {
			return fmt.Errorf("salt of %x is not the BLAKE3-256 prefix of the salt domain and id", id)
		}
		f.Salt = append(f.Salt, vplSalt{ID: append(Hex{}, id[:]...), Salt: U64(s)})
	}

	salt1 := placement.Salt(vplIDFrom(1))
	type slotSalt struct {
		slot uint32
		salt uint64
	}
	lCases := []slotSalt{
		{0, 0}, {0, vplInvFmix64(maxU)}, {0, vplInvFmix64(maxU - 1)}, {12345, salt1}, {0, salt1}, {placement.Slots - 1, salt1},
		{524288, placement.Salt(vplIDFrom(4))}, {777777, placement.Salt(vplIDFrom(5))}, {3, placement.Salt(vplIDSeed(11))},
	}
	{
		slots, salts := u64s(31, 32), u64s(32, 32)
		for i := range slots {
			lCases = append(lCases, slotSalt{uint32(slots[i] >> 44), salts[i]})
		}
	}
	for _, c := range lCases {
		f.L = append(f.L, vplL{Slot: c.slot, Salt: U64(c.salt), L: U64(placement.L(c.slot, c.salt))})
	}

	goldenSlots := []uint32{0, 1, 12345, 0xfffff, 524288, 777777}
	for _, x := range u64s(99, 10) {
		goldenSlots = append(goldenSlots, uint32(x>>44))
	}
	f.Sets = append(f.Sets,
		vplSetVector("golden5", vplGolden5(), []int{0, 1, 2, 3, 5, 7}, goldenSlots),
		vplSetVector("zones", vplZones(), []int{1, 2, 3}, goldenSlots[:8]),
		vplSetVector("draining", vplDraining(), []int{2, 3, 4}, goldenSlots[:8]),
		vplSetVector("heavy", vplHeavy(), []int{2, 4}, goldenSlots),
	)
	tie, tieSlot, err := vplTie()
	if err != nil {
		return err
	}
	f.Sets = append(f.Sets,
		vplSetVector("tie", tie, []int{1, 2}, []uint32{tieSlot, 0, 1}),
		vplSetVector("empty", nil, []int{0, 3}, []uint32{0, 5}),
		vplSetVector("realistic12", vplRealistic12(), []int{1, 3, 5, 12}, goldenSlots),
		vplSetVector("duplicates", vplDuplicates(), []int{1, 2, 3}, goldenSlots[:8]),
	)

	ids := make([]placement.NodeID, 12)
	for i := range ids {
		ids[i] = vplIDSeed(uint64(300 + i))
	}
	nodes5 := []view.Node{vplNode(ids[0], 4096, "", "ip:192.168.1.10:4433"), vplNode(ids[1], 4096, ""), vplNode(ids[2], 2048, ""), vplNode(ids[3], 8192, ""), vplNode(ids[4], 1024, "")}
	view.SortNodes(nodes5)
	cid := smData(1000, 16)
	base := func() *view.View {
		return &view.View{ClusterID: cid, Incarnation: 1, Epoch: 9, Version: 20, PlacementEpoch: 8, Replicas: 3, MinReplicas: 2,
			Voters: []view.Voter{{ID: nodes5[0].ID, Since: 1}, {ID: nodes5[1].ID, Since: 3}, {ID: nodes5[2].ID, Since: 5}},
			Nodes:  vplCloneNodes(g, nodes5)}
	}
	vkeys := [][32]byte{{}, vplKeyWithTail(maxU), vplKeyWithTail(1 << 44)}
	for i := uint64(0); i < 24; i++ {
		vkeys = append(vkeys, vplKeySeed(500+i))
	}
	nonMember := []placement.NodeID{vplIDSeed(999)}

	f.Views = append(f.Views, vplViewVector(g, "steady", base(), vkeys, nonMember))
	join := base()
	join.Epoch = 10
	join.Pending = &view.Pending{Nodes: append(vplCloneNodes(g, nodes5), vplNode(ids[5], 512, "")), Replicas: 3, ID: 10, Reason: "join " + view.ShortID(ids[5]), Since: 1757000000000000000}
	view.SortNodes(join.Pending.Nodes)
	f.Views = append(f.Views, vplViewVector(g, "join_pending", join, vkeys, nonMember))
	drain := base()
	drain.Epoch = 10
	drain.Pending = &view.Pending{Nodes: vplCloneNodes(g, nodes5), Replicas: 3, ID: 10, Reason: "node-drain"}
	if len(drain.Pending.Nodes) == 5 {
		drain.Pending.Nodes[2].Weight = 0
	}
	f.Views = append(f.Views, vplViewVector(g, "drain_pending", drain, vkeys, nonMember))
	remove := base()
	remove.Epoch = 10
	remove.Pending = &view.Pending{Nodes: append(vplCloneNodes(g, nodes5[:1]), vplCloneNodes(g, nodes5[2:])...), Replicas: 3, ID: 10, Reason: "node-remove"}
	f.Views = append(f.Views, vplViewVector(g, "remove_pending", remove, vkeys, nonMember))
	repl := base()
	repl.Replicas, repl.Epoch = 2, 10
	repl.Pending = &view.Pending{Nodes: vplCloneNodes(g, nodes5), Replicas: 3, ID: 10, Reason: "replicas"}
	f.Views = append(f.Views, vplViewVector(g, "replicas_pending", repl, vkeys, nonMember))
	zonep := base()
	zonep.Epoch = 10
	zonep.Pending = &view.Pending{Nodes: vplCloneNodes(g, nodes5), Replicas: 3, ID: 10, Reason: "node-zone"}
	if len(zonep.Pending.Nodes) == 5 {
		zonep.Pending.Nodes[0].Zone, zonep.Pending.Nodes[1].Zone, zonep.Pending.Nodes[2].Zone = "rack1", "rack1", "rack2"
	}
	f.Views = append(f.Views, vplViewVector(g, "zone_pending", zonep, vkeys, nonMember))
	drained := base()
	if len(drained.Nodes) == 5 {
		drained.Nodes[2].Weight = 0
	}
	drained.Former = []view.Former{{ID: append([]byte{}, ids[6][:]...), Until: 1760000000000000000}}
	f.Views = append(f.Views, vplViewVector(g, "drained_committed", drained, vkeys, append(append([]placement.NodeID{}, nonMember...), ids[6])))
	unsorted := base()
	for i, j := 0, len(unsorted.Nodes)-1; i < j; i, j = i+1, j-1 {
		unsorted.Nodes[i], unsorted.Nodes[j] = unsorted.Nodes[j], unsorted.Nodes[i]
	}
	f.Views = append(f.Views, vplViewVector(g, "unsorted_nodes", unsorted, vkeys, nonMember))
	zoned := base()
	if len(zoned.Nodes) == 5 {
		zoned.Nodes[0].Zone, zoned.Nodes[1].Zone, zoned.Nodes[3].Zone = "hostA", "hostA", "hostA"
	}
	f.Views = append(f.Views, vplViewVector(g, "zones_committed", zoned, vkeys, nonMember))
	shortid := base()
	if len(shortid.Nodes) == 5 {
		shortid.Nodes[4].ID = shortid.Nodes[4].ID[:31]
	}
	f.Views = append(f.Views, vplViewVector(g, "short_node_id", shortid, vkeys[:6], nonMember))
	dup := base()
	dup.Nodes = append(dup.Nodes, vplCloneNodes(g, nodes5[1:2])...)
	f.Views = append(f.Views, vplViewVector(g, "duplicate_node", dup, vkeys[:6], nonMember))
	nonodes := base()
	nonodes.Nodes = nil
	f.Views = append(f.Views, vplViewVector(g, "no_nodes", nonodes, vkeys[:3], nonMember))
	pendEmpty := base()
	pendEmpty.Pending = &view.Pending{Replicas: 3, ID: 10}
	f.Views = append(f.Views, vplViewVector(g, "pending_no_nodes", pendEmpty, vkeys[:6], nonMember))

	initView := &view.View{ClusterID: cid, Incarnation: 1, Epoch: 1, Version: 1, PlacementEpoch: 1, Replicas: 3, MinReplicas: 2,
		Voters: []view.Voter{{ID: append([]byte{}, ids[0][:]...), Since: 1}},
		Nodes:  []view.Node{{ID: append([]byte{}, ids[0][:]...), Weight: 931, Addrs: []string{"ip:192.168.1.10:51820", "ip:[fe80::1]:51820", "relay:https://euw1-1.relay.n0.iroh-canary.iroh.link./"}, Writable: true, Incarnation: 1}}}
	idb := func(i int) []byte { return append([]byte{}, ids[i][:]...) }
	full := &view.View{
		ClusterID: cid, Incarnation: 2, Epoch: 57, Version: 311, PlacementEpoch: 54, Replicas: 3, MinReplicas: 2,
		Voters:    []view.Voter{{ID: idb(0), Since: 1}, {ID: idb(1), Since: 12}, {ID: idb(2), Since: 30}},
		VoterSync: view.VoterSyncPending, VoterSyncCursor: []byte("join/\x00\x01"),
		Nodes: []view.Node{
			{ID: idb(0), Weight: 4096, Addrs: []string{"ip:10.0.0.1:4433"}, Data: []view.DataEndpoint{{ID: idb(9), Addrs: []string{"ip:10.0.0.1:4434"}}, {ID: idb(10)}}, Token: smData(77, 32), Zone: "rack-1", Incarnation: 3, Writable: true},
			{ID: idb(1), Weight: 2048, Zone: "rack-2", Incarnation: 1, Writable: false},
			{ID: idb(2), Weight: 0, Addrs: []string{"relay:https://relay.example/"}, Writable: true, Incarnation: 1},
		},
		Pending: &view.Pending{
			Nodes: []view.Node{{ID: idb(0), Weight: 4096, Writable: true}, {ID: idb(5), Weight: 512, Writable: true, Incarnation: 1}}, Replicas: 2, ID: 57,
			ParticipantsAck: [][]byte{idb(0), idb(5)}, Participants: [][]byte{idb(0)}, Frozen: true, Round: 2,
			PrimaryDone: [][]byte{idb(0)}, Done: [][]byte{idb(0)}, FrozenAt: 1757999999123456789, Reason: "join " + view.ShortID(ids[5]),
			Ramp: &view.Ramp{Node: idb(5), Target: 4096, Step: 1}, Since: 1757999990000000000,
		},
		Former: []view.Former{{ID: idb(7), Until: 1760000000000000000}}, Fenced: [][]byte{idb(8)},
		RecoveredInc: 1, RecoveredEpoch: 40, RebalancePause: true, RateCap: 104857600,
		VoterSyncTarget: idb(5), VoterSyncAdd: true, DeferredVoters: [][]byte{idb(11)},
		ACL:   &view.ACL{Allowed: [][]byte{idb(9), idb(10)}, Admins: [][]byte{idb(9)}},
		Ramps: []view.Ramp{{Node: idb(5), Target: 4096, Step: 1}}, RemoveVoters: [][]byte{idb(7)},
	}
	for _, c := range []struct {
		name string
		v    *view.View
	}{
		{"zero", &view.View{}},
		{"init", initView},
		{"full", full},
		{"empty_non_nil", &view.View{ClusterID: []byte{}, Voters: []view.Voter{}, Nodes: []view.Node{}, Former: []view.Former{}, Fenced: [][]byte{}, VoterSyncCursor: []byte{}, Ramps: []view.Ramp{}}},
		{"zero_pointers", &view.View{Pending: &view.Pending{}, ACL: &view.ACL{}}},
		{"pending_zero_ramp", &view.View{Pending: &view.Pending{Ramp: &view.Ramp{}, Nodes: []view.Node{}}}},
		{"negatives", &view.View{VoterSync: -1, Former: []view.Former{{ID: idb(0), Until: -1}}, Pending: &view.Pending{FrozenAt: -5, Since: -1 << 63, Ramp: &view.Ramp{Step: -2}}, Ramps: []view.Ramp{{Step: -300}}}},
		{"maxes", &view.View{Incarnation: maxU, Epoch: 1 << 32, Version: 1<<32 - 1, PlacementEpoch: 65536, Replicas: 255, MinReplicas: 24, VoterSync: 1 << 40, RateCap: maxU,
			Nodes: []view.Node{{ID: idb(0), Weight: 0xffffffff, Incarnation: maxU, Writable: true}}, Pending: &view.Pending{Round: 0xffffffff, ID: 23, Replicas: 23}}},
		{"utf8", &view.View{Nodes: []view.Node{{ID: idb(0), Weight: 1, Zone: "zürich-\U0001F3D4"}}, Pending: &view.Pending{Reason: "ramp ✓"}}},
		{"node_nil_and_empty_id", &view.View{Nodes: []view.Node{{ID: nil, Weight: 1}, {ID: []byte{}, Weight: 2}}}},
		{"writable_false_and_data_no_addrs", &view.View{Nodes: []view.Node{{ID: idb(0), Data: []view.DataEndpoint{{ID: idb(1)}, {}}}}}},
		{"long_addrs", &view.View{Nodes: []view.Node{{ID: idb(0), Weight: 1, Addrs: []string{strings.Repeat("a", 23), strings.Repeat("b", 24), strings.Repeat("c", 255), strings.Repeat("d", 256)}}}}},
		{"acl_nil_and_empty_elements", &view.View{ACL: &view.ACL{Allowed: [][]byte{nil, {}}, Admins: [][]byte{idb(3)}}, Fenced: [][]byte{nil}, DeferredVoters: [][]byte{{}}, RemoveVoters: [][]byte{{1, 2}}}},
		{"pending_lists", &view.View{Pending: &view.Pending{Nodes: nil, ParticipantsAck: [][]byte{nil}, Participants: [][]byte{{}}, PrimaryDone: [][]byte{idb(4)}, Done: [][]byte{idb(4), nil}}}},
	} {
		enc := g.enc(c.v)
		dec, err := view.Decode(enc)
		if err != nil {
			return fmt.Errorf("view_cbor %s: %w", c.name, err)
		}
		if !bytes.Equal(g.enc(dec), enc) {
			return fmt.Errorf("view_cbor %s: re-encoding differs", c.name)
		}
		f.ViewCBOR = append(f.ViewCBOR, vplCBORCase{Name: c.name, CBOR: enc})
	}

	helperIDs := append(append([]placement.NodeID{}, ids...), vplIDSeed(999))
	f.Helpers = append(f.Helpers, vplHelpersOf(g, "full", full, helperIDs))
	{
		var padded placement.NodeID
		if len(shortid.Nodes) == 5 {
			copy(padded[:], shortid.Nodes[4].ID)
		}
		f.Helpers = append(f.Helpers, vplHelpersOf(g, "short_node_id", shortid, append(append([]placement.NodeID{}, ids[:5]...), padded, vplIDSeed(999))))
		f.Helpers = append(f.Helpers, vplHelpersOf(g, "no_nodes", nonodes, append(append([]placement.NodeID{}, ids[:5]...), vplIDSeed(999))))
	}

	id32 := idb(0)
	canon := g.enc(initView)
	c := func() *vplCBOR { return &vplCBOR{} }
	nest := func(depth int) []byte {
		w := c().mp(1).u(99)
		for i := 0; i < depth; i++ {
			w.arr(1)
		}
		return w.u(0).bytes(g)
	}
	f.ViewDecode = append(f.ViewDecode,
		vplDecodeCase(g, "empty_input", []byte{}),
		vplDecodeCase(g, "canonical_init", canon),
		vplDecodeCase(g, "trailing_zero_byte", append(append([]byte{}, canon...), 0x00)),
		vplDecodeCase(g, "truncated", canon[:len(canon)-1]),
		vplDecodeCase(g, "top_level_array", c().arr(0).bytes(g)),
		vplDecodeCase(g, "top_level_null", c().raw("f6").bytes(g)),
		vplDecodeCase(g, "empty_map", c().mp(0).bytes(g)),
		vplDecodeCase(g, "reordered_keys", c().mp(3).u(10).arr(0).u(2).u(5).u(0).bs(cid).bytes(g)),
		vplDecodeCase(g, "unknown_int_key", c().mp(2).u(2).u(5).u(99).ts("x").bytes(g)),
		vplDecodeCase(g, "unknown_negative_key", c().mp(2).neg(0).u(1).u(2).u(5).bytes(g)),
		vplDecodeCase(g, "duplicate_key_epoch", c().mp(2).u(2).u(1).u(2).u(7).bytes(g)),
		vplDecodeCase(g, "text_key_2", c().mp(1).ts("2").u(9).bytes(g)),
		vplDecodeCase(g, "text_key_Epoch", c().mp(1).ts("Epoch").u(9).bytes(g)),
		vplDecodeCase(g, "bytes_key", c().mp(2).bs([]byte{2}).u(9).u(1).u(4).bytes(g)),
		vplDecodeCase(g, "null_nodes_and_voters", c().mp(2).u(10).raw("f6").u(7).raw("f6").bytes(g)),
		vplDecodeCase(g, "null_replicas", c().mp(1).u(5).raw("f6").bytes(g)),
		vplDecodeCase(g, "undefined_epoch", c().mp(1).u(2).raw("f7").bytes(g)),
		vplDecodeCase(g, "null_pending", c().mp(1).u(11).raw("f6").bytes(g)),
		vplDecodeCase(g, "empty_pending_map", c().mp(1).u(11).mp(0).bytes(g)),
		vplDecodeCase(g, "replicas_256", c().mp(1).u(5).u(256).bytes(g)),
		vplDecodeCase(g, "weight_2pow32", c().mp(1).u(10).arr(1).mp(2).u(0).bs(id32).u(1).u(1<<32).bytes(g)),
		vplDecodeCase(g, "negative_incarnation", c().mp(1).u(1).neg(0).bytes(g)),
		vplDecodeCase(g, "negative_voter_sync", c().mp(1).u(8).neg(4).bytes(g)),
		vplDecodeCase(g, "float_epoch", c().mp(1).u(2).raw("f93c00").bytes(g)),
		vplDecodeCase(g, "bool_as_int_writable", c().mp(1).u(10).arr(1).mp(2).u(0).bs(id32).u(7).u(1).bytes(g)),
		vplDecodeCase(g, "frozen_true", c().mp(1).u(11).mp(1).u(5).raw("f5").bytes(g)),
		vplDecodeCase(g, "invalid_utf8_zone", c().mp(1).u(10).arr(1).mp(2).u(0).bs(id32).u(5).raw("62c328").bytes(g)),
		vplDecodeCase(g, "bytes_zone", c().mp(1).u(10).arr(1).mp(2).u(0).bs(id32).u(5).bs([]byte("z")).bytes(g)),
		vplDecodeCase(g, "text_cluster_id", c().mp(1).u(0).ts("abcd").bytes(g)),
		vplDecodeCase(g, "node_id_2_bytes", c().mp(1).u(10).arr(1).mp(2).u(0).bs([]byte{1, 2}).u(1).u(5).bytes(g)),
		vplDecodeCase(g, "non_shortest_heads", c().raw("b90001").raw("1802").raw("1a00000005").bytes(g)),
		vplDecodeCase(g, "indefinite_map_and_array", c().raw("bf").u(2).u(5).u(10).raw("9f").mp(2).u(0).bs(id32).u(1).u(3).raw("ff").raw("ff").bytes(g)),
		vplDecodeCase(g, "indefinite_bytes_cluster_id", c().mp(1).u(0).raw("5f").bs([]byte{1, 2}).bs([]byte{3}).raw("ff").bytes(g)),
		vplDecodeCase(g, "self_describe_tag", append([]byte{0xd9, 0xd9, 0xf7}, canon...)),
		vplDecodeCase(g, "tag_on_epoch", c().mp(1).u(2).raw("c1").u(5).bytes(g)),
		vplDecodeCase(g, "type_error_then_overflow", c().mp(2).u(1).ts("x").u(5).u(300).bytes(g)),
		vplDecodeCase(g, "error_then_malformed_later", c().mp(2).u(5).u(300).u(1).ts("x").bytes(g)),
		vplDecodeCase(g, "nodes_not_array", c().mp(1).u(10).mp(0).bytes(g)),
		vplDecodeCase(g, "pending_as_array", c().mp(1).u(11).arr(0).bytes(g)),
		vplDecodeCase(g, "ramp_step_negative", c().mp(1).u(22).arr(1).mp(1).u(2).neg(9).bytes(g)),
		vplDecodeCase(g, "ramp_step_2pow63", c().mp(1).u(22).arr(1).mp(1).u(2).u(1<<63).bytes(g)),
		vplDecodeCase(g, "former_until_uint_2pow63", c().mp(1).u(12).arr(1).mp(1).u(1).u(1<<63).bytes(g)),
		vplDecodeCase(g, "reserved_ai_28", c().raw("bc").bytes(g)),
		vplDecodeCase(g, "simple_value_key", c().mp(1).raw("f4").u(1).bytes(g)),
		vplDecodeCase(g, "nested_33_levels_unknown_key", nest(33)),
		vplDecodeCase(g, "nested_32_levels_unknown_key", nest(32)),
		vplDecodeCase(g, "nested_31_levels_unknown_key", nest(31)),
		vplDecodeCase(g, "pending_ramp_null", c().mp(1).u(11).mp(1).u(11).raw("f6").bytes(g)),
		vplDecodeCase(g, "acl_empty_and_null_element", c().mp(1).u(21).mp(2).u(0).arr(0).u(1).arr(1).raw("f6").bytes(g)),
		vplDecodeCase(g, "fenced_null_and_empty_elements", c().mp(1).u(13).arr(2).raw("f6").bs([]byte{}).bytes(g)),
		vplDecodeCase(g, "voters_null_element", c().mp(1).u(7).arr(1).raw("f6").bytes(g)),
		vplDecodeCase(g, "node_as_array", c().mp(1).u(10).arr(1).arr(0).bytes(g)),
		vplDecodeCase(g, "weight_negative", c().mp(1).u(10).arr(1).mp(1).u(1).neg(0).bytes(g)),
		vplDecodeCase(g, "addr_as_bytes", c().mp(1).u(10).arr(1).mp(1).u(2).arr(1).bs([]byte("a")).bytes(g)),
		vplDecodeCase(g, "bignum_epoch", c().mp(1).u(2).raw("c2").bs([]byte{5}).bytes(g)),
		vplDecodeCase(g, "bignum_epoch_overflow", c().mp(1).u(2).raw("c2").bs([]byte{1, 0, 0, 0, 0, 0, 0, 0, 0}).bytes(g)),
		vplDecodeCase(g, "indefinite_text_zone", c().mp(1).u(10).arr(1).mp(2).u(0).bs(id32).u(5).raw("7f").ts("zü").ts("rich").raw("ff").bytes(g)),
		vplDecodeCase(g, "indefinite_bytes_wrong_chunk", c().mp(1).u(0).raw("5f").ts("a").raw("ff").bytes(g)),
		vplDecodeCase(g, "voter_sync_2pow63", c().mp(1).u(8).u(1<<63).bytes(g)),
		vplDecodeCase(g, "voter_sync_min_int64", c().mp(1).u(8).neg(1<<63-1).bytes(g)),
		vplDecodeCase(g, "voter_sync_below_min_int64", c().mp(1).u(8).neg(1<<63).bytes(g)),
		vplDecodeCase(g, "epoch_max_uint64", c().mp(1).u(2).u(maxU).bytes(g)),
		vplDecodeCase(g, "writable_null", c().mp(1).u(10).arr(1).mp(2).u(0).bs(id32).u(7).raw("f6").bytes(g)),
		vplDecodeCase(g, "invalid_utf8_reason", c().mp(1).u(11).mp(1).u(10).raw("62ff41").bytes(g)),
		vplDecodeCase(g, "top_level_text", c().ts("x").bytes(g)),
		vplDecodeCase(g, "top_level_undefined", c().raw("f7").bytes(g)),
		vplDecodeCase(g, "map_count_131073", c().raw("ba00020001").bytes(g)),
		vplDecodeCase(g, "array_count_131073_under_unknown_key", c().mp(1).u(99).raw("9a00020001").bytes(g)),
	)

	pid := vplIDSeed(7)
	hx := hex.EncodeToString(pid[:])
	b32 := strings.ToLower(base32.StdEncoding.WithPadding(base32.NoPadding).EncodeToString(pid[:]))
	for _, s := range []string{
		hx, strings.ToUpper(hx), hx[:10] + strings.ToUpper(hx[10:40]) + hx[40:], hx[:63], hx + "0", hx + "00", "0x" + hx[:62], " " + hx[1:], hx + "\n",
		b32, "", strings.Repeat("zz", 32), "ab\tcd", "héllo \U0001F600", "\xff\xfe", `quote"back\slash`, "\x00\x7f", "​",
		"0X" + hx[:62], hx[:62] + "é", hx[:63] + "а", " " + hx[1:],
	} {
		p := vplParse{InputHex: append(Hex{}, s...)}
		if id, err := view.ParseNodeID(s); err != nil {
			e := err.Error()
			p.Error = &e
		} else {
			p.OK, p.ID = true, append(Hex{}, id[:]...)
		}
		f.ParseNodeID = append(f.ParseNodeID, p)
	}

	for _, b := range [][]byte{pid[:], pid[:4], {}, nil, append(append([]byte{}, pid[:]...), 0), pid[:31]} {
		sv := vplShort{Bytes: vplBytes(b), NodeShort: node.ShortID(b)}
		if len(b) == 32 {
			vs, is := view.ShortID(view.NodeID(b)), view.IDString(view.NodeID(b))
			sv.ViewShort, sv.IDString = &vs, &is
		}
		f.ShortID = append(f.ShortID, sv)
	}

	five := base()
	five.Nodes = append(five.Nodes, vplNode(ids[11], 9, "", "ip:127.0.0.1:1"))
	for _, tc := range []struct {
		name string
		v    *view.View
	}{
		{"five_nodes_first_four", five},
		{"init_one_node", initView},
		{"no_nodes", &view.View{ClusterID: cid, Incarnation: 4}},
		{"empty_non_nil_nodes", &view.View{ClusterID: cid, Incarnation: 4, Nodes: []view.Node{}}},
		{"nil_cluster_id_node_without_addrs", &view.View{Nodes: []view.Node{{ID: idb(3), Weight: 1}}}},
		{"pending_only_nodes_ignored", pendEmpty},
		{"unsorted_order_kept", unsorted},
		{"node_ids_nil_and_short", &view.View{ClusterID: []byte{}, Incarnation: 1, Nodes: []view.Node{{ID: nil, Weight: 1}, {ID: []byte{1, 2}, Weight: 1, Addrs: []string{"ip:1.2.3.4:5"}}}}},
	} {
		s := worktree.TicketFromView(tc.v).Encode()
		raw, err := base32.StdEncoding.WithPadding(base32.NoPadding).DecodeString(strings.ToUpper(strings.TrimPrefix(s, "dstore1")))
		if err != nil {
			return fmt.Errorf("ticket_from_view %s: %w", tc.name, err)
		}
		f.TicketFrom = append(f.TicketFrom, vplTicket{Name: tc.name, View: g.enc(tc.v), Ticket: s, TicketCBOR: raw})
	}

	for _, n := range []view.Node{
		{ID: nil}, {ID: []byte{}}, {ID: []byte{1, 2}}, {ID: idb(4)[:31]}, {ID: idb(4)}, {ID: append(idb(4), 9)},
		{ID: idb(4), Zone: "rack-1"}, {ID: nil, Zone: "z"}, {ID: []byte{0xff}, Zone: "ü"},
	} {
		nid := n.NID()
		f.NodeHelpers = append(f.NodeHelpers, vplNodeHelper{ID: vplBytes(n.ID), Zone: n.Zone, NID: append(Hex{}, nid[:]...), ZoneOrID: append(Hex{}, n.ZoneOrID()...)})
	}

	{
		x, y := ids[4], ids[5]
		for _, tc := range []struct {
			ids [][]byte
			id  placement.NodeID
		}{
			{nil, x},
			{[][]byte{append([]byte{}, x[:]...)}, x},
			{[][]byte{nil, append([]byte{}, x[:31]...), append(append([]byte{}, x[:]...), 0)}, x},
			{[][]byte{append([]byte{}, y[:]...), append([]byte{}, x[:]...)}, x},
			{[][]byte{{}}, x},
		} {
			in := make([]Hex, 0, len(tc.ids))
			for _, b := range tc.ids {
				in = append(in, vplBytes(b))
			}
			added := view.AddID(tc.ids, tc.id)
			outIDs := make([]Hex, 0, len(added))
			for _, b := range added {
				outIDs = append(outIDs, vplBytes(b))
			}
			f.IDLists = append(f.IDLists, vplIDList{IDs: in, ID: append(Hex{}, tc.id[:]...), Contains: view.Contains(tc.ids, tc.id), AddID: outIDs})
		}
		for _, raw := range [][][]byte{
			{append([]byte{}, x[:]...), append([]byte{}, x[:31]...), append(append([]byte{}, x[:]...), 7), {}},
			{},
		} {
			in := make([]Hex, 0, len(raw))
			for _, b := range raw {
				in = append(in, append(Hex{}, b...))
			}
			f.IDsOf = append(f.IDsOf, vplIDsOf{Raw: in, IDs: vplHexIDs(view.IDsOf(raw))})
		}
	}

	for _, cc := range [][4]uint64{{1, 5, 1, 5}, {1, 5, 1, 6}, {1, 5, 1, 4}, {2, 1, 1, 99}, {1, 99, 2, 1}, {0, 0, 0, 0}, {maxU, 0, maxU, maxU}, {0, 0, 0, 1}, {0, 1, 0, 0}, {maxU, maxU, 0, maxU}} {
		v := &view.View{Incarnation: cc[0], Epoch: cc[1]}
		f.Compare = append(f.Compare, vplCompare{ViewIncarnation: U64(cc[0]), ViewEpoch: U64(cc[1]), Incarnation: U64(cc[2]), Epoch: U64(cc[3]), Result: v.Compare(cc[2], cc[3])})
	}

	for r := 0; r <= 255; r++ {
		f.DefaultMinR = append(f.DefaultMinR, vplMinR{R: r, MinReplicas: int(view.DefaultMinReplicas(uint8(r)))})
	}

	mk := func(ws ...uint32) []view.Node {
		ns := []view.Node{}
		for i, w := range ws {
			ns = append(ns, view.Node{ID: []byte{byte(i + 1)}, Weight: w})
		}
		return ns
	}
	for _, tc := range []struct {
		name        string
		cur, target []view.Node
		r           int
		force       bool
	}{
		{"drop_two_r3", mk(1, 1, 1, 1), mk(1, 1), 3, false},
		{"drop_three_r3", mk(1, 1, 1, 1), mk(1), 3, false},
		{"drop_three_r3_forced", mk(1, 1, 1, 1), mk(1), 3, true},
		{"weight_zero_counts_as_dropped", mk(1, 1, 1), mk(0, 0, 0), 3, false},
		{"current_weight_zero_not_counted", mk(0, 0, 0, 1), mk(), 2, false},
		{"replicas_zero", mk(1, 1), mk(), 0, false},
		{"replicas_negative", mk(1, 1), mk(), -1, false},
		{"drop_one_r1", mk(1, 1), mk(1), 1, false},
		{"new_ids_do_not_count", mk(1, 1, 1), []view.Node{{ID: []byte{4}, Weight: 1}, {ID: []byte{5}, Weight: 1}, {ID: []byte{6}, Weight: 1}}, 3, false},
		{"nil_and_empty_ids_match", []view.Node{{ID: nil, Weight: 1}}, []view.Node{{ID: []byte{}, Weight: 1}}, 1, false},
		{"target_duplicate_zero_then_weighted", mk(1), []view.Node{{ID: []byte{1}, Weight: 0}, {ID: []byte{1}, Weight: 5}}, 1, false},
	} {
		vc := vplValidate{Name: tc.name, Cur: vplNodeMembers(tc.cur), Target: vplNodeMembers(tc.target), Replicas: tc.r, Force: tc.force}
		if err := view.ValidateChange(tc.cur, tc.target, tc.r, tc.force); err != nil {
			e := err.Error()
			vc.Error = &e
		}
		f.Validate = append(f.Validate, vc)
	}

	sn := []view.Node{{ID: []byte{2}}, {ID: []byte{1, 5}}, {ID: []byte{1}}, {ID: nil}, {ID: []byte{0xff}}, {ID: []byte{1, 0}}}
	for _, n := range sn {
		f.SortNodes.Input = append(f.SortNodes.Input, vplBytes(n.ID))
	}
	view.SortNodes(sn)
	for _, n := range sn {
		f.SortNodes.Output = append(f.SortNodes.Output, vplBytes(n.ID))
	}

	// cmd/dstore printStatus (client.go:136) slices v.ClusterID[:4] of the view
	// view.Decode produced. fxamacker copies a definite-length byte string into
	// an exact-capacity slice but keeps the append-grown buffer of an
	// indefinite-length one, so a short cluster id panics only in the first case.
	for _, sc := range []struct {
		name string
		in   []byte
	}{
		{"absent", c().mp(0).bytes(g)},
		{"null", c().mp(1).u(0).raw("f6").bytes(g)},
		{"empty", c().mp(1).u(0).bs([]byte{}).bytes(g)},
		{"one_byte", c().mp(1).u(0).bs([]byte{1}).bytes(g)},
		{"two_bytes", c().mp(1).u(0).bs([]byte{1, 2}).bytes(g)},
		{"three_bytes", c().mp(1).u(0).bs([]byte{1, 2, 3}).bytes(g)},
		{"four_bytes", c().mp(1).u(0).bs([]byte{1, 2, 3, 4}).bytes(g)},
		{"init_16_bytes", canon},
		{"indefinite_empty", c().mp(1).u(0).raw("5fff").bytes(g)},
		{"indefinite_empty_chunk", c().mp(1).u(0).raw("5f").bs([]byte{}).raw("ff").bytes(g)},
		{"indefinite_two_bytes", c().mp(1).u(0).raw("5f").bs([]byte{1, 2}).raw("ff").bytes(g)},
		{"indefinite_three_bytes_two_chunks", c().mp(1).u(0).raw("5f").bs([]byte{1}).bs([]byte{2, 3}).raw("ff").bytes(g)},
		{"indefinite_nine_bytes", c().mp(1).u(0).raw("5f").bs([]byte{1, 2, 3, 4, 5, 6, 7, 8, 9}).raw("ff").bytes(g)},
	} {
		v, err := view.Decode(sc.in)
		if err != nil {
			return fmt.Errorf("status_cluster_prefix %s: %w", sc.name, err)
		}
		s := vplStatusCID{Name: sc.name, View: append(Hex{}, sc.in...), ClusterID: vplBytes(v.ClusterID), Cap: cap(v.ClusterID)}
		s.Prefix, s.Panic = vplClusterPrefix(v.ClusterID)
		f.StatusCID = append(f.StatusCID, s)
	}

	if g.err != nil {
		return g.err
	}
	return writeJSON(filepath.Join(out, "view", "view_placement.json"), f)
}

// ---- placement/all_slots.json ----

const vplAllSlotsHash = "BLAKE3-256 over slots 0..2^20-1 in order, each contributing u8 len(rank) ‖ rank indexes as u8 ‖ u8 len(owners) ‖ owners as u8, where rank = Set.Rank(slot) and owners = Set.Owners(slot, r)"

func vplAllSlotsVector(name string, members []placement.Member, r int) (vplAllSlotsSet, error) {
	if len(members) > 255 || r > 255 || r < 0 {
		return vplAllSlotsSet{}, fmt.Errorf("set %s: %d members, r %d do not fit the u8 hash layout", name, len(members), r)
	}
	set := placement.NewSet(members)
	out := vplAllSlotsSet{Name: name, Members: vplMembers(members), R: r, Spots: []vplSpot{}}
	for _, s := range []uint32{0, 1, 12345, 0xfffff, 524288, 777777} {
		sp := vplSpot{Slot: s, Rank: set.Rank(s), Owners: set.Owners(s, r)}
		if sp.Rank == nil {
			sp.Rank = []int{}
		}
		out.Spots = append(out.Spots, sp)
	}
	h := blake3.New()
	buf := make([]byte, 0, 2+2*len(members))
	for slot := uint32(0); slot < placement.Slots; slot++ {
		rank := set.Rank(slot)
		owners := set.Owners(slot, r)
		buf = append(buf[:0], byte(len(rank)))
		for _, idx := range rank {
			buf = append(buf, byte(idx))
		}
		buf = append(buf, byte(len(owners)))
		for _, idx := range owners {
			buf = append(buf, byte(idx))
		}
		if _, err := h.Write(buf); err != nil {
			return vplAllSlotsSet{}, err
		}
	}
	out.AllSlotsBlake3 = hex.EncodeToString(h.Sum(nil))
	return out, nil
}

func genPlacementAllSlots(out string) error {
	tie, _, err := vplTie()
	if err != nil {
		return err
	}
	defs := []struct {
		name    string
		members []placement.Member
		r       int
	}{
		{"golden5_r3", vplGolden5(), 3},
		{"golden5_r5", vplGolden5(), 5},
		{"golden5_r0", vplGolden5(), 0},
		{"zones_r3", vplZones(), 3},
		{"draining_r4", vplDraining(), 4},
		{"heavy_r2", vplHeavy(), 2},
		{"tie_r1", tie, 1},
		{"realistic12_r3", vplRealistic12(), 3},
		{"splitmix50_r3", vplSplitmix50(), 3},
		{"duplicates_r2", vplDuplicates(), 2},
		{"empty_r3", nil, 3},
	}
	sets := make([]vplAllSlotsSet, len(defs))
	errs := make([]error, len(defs))
	var wg sync.WaitGroup
	for i, d := range defs {
		wg.Add(1)
		go func() {
			defer wg.Done()
			sets[i], errs[i] = vplAllSlotsVector(d.name, d.members, d.r)
		}()
	}
	wg.Wait()
	if err := errors.Join(errs...); err != nil {
		return err
	}
	return writeJSON(filepath.Join(out, "placement", "all_slots.json"), vplAllSlotsFile{Slots: placement.Slots, Hash: vplAllSlotsHash, Sets: sets})
}
