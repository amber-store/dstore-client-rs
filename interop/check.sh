#!/usr/bin/env bash
# Live interop suite of dstore-client-rs (port-notes/verification.md §4.5, PORTING.md §7): a 3-node Go
# dstore v0.1.9 cluster on loopback, and the Go and the Rust client run side by side against it.
# interop/README.md describes the inputs, the checks and the comparison modes.
#
#   bash interop/check.sh [--list] [ID | GROUP ...]
#
# Without arguments every check runs. `bash interop/check.sh B3 D1` runs those two, plus the checks they
# build on (printed as well); a letter runs a group (`D`). One PASS, FAIL or SKIP line per check, then a
# summary. Exit status 0 when nothing failed, 1 when a check failed, 2 when the setup failed.
#
# Everything lives in one mktemp directory that the exit trap removes, together with every process the
# suite started: the Go nodes, watchers, lock holders.

REPO=$(cd "$(dirname "$0")/.." && pwd -P)
# shellcheck source=interop/lib.sh
. "$REPO/interop/lib.sh"

CHECKS='
A1|cluster status, go vs rs back to back (normalized)
A2|cluster ticket, cluster ticket --ids; each printed ticket serves the other client
A3|rs token create (64 hex) admits n3; token create --weight output shape
A4|transition status, gc status (held), catalog backups
A5|gc hold, gc release print ok
A6|gc run prints the gc status that follows it (rs run/go status, go run/rs status)
A7|gc why <root of trees/rs>, gc why <64 zeros>
A8|rs catalog backup: backup written, key <64hex>, listed by go catalog backups
A9|node repair <n2>
A10|transition pause, transition resume
A11|node-reported errors: unknown ref, bad glob, bad name, voter add of a voter, replicas 0
A12|heavy: rs node weight <n3> 50, steady, restored by go
A13|heavy: rs cluster replicas 2 --yes, steady, go cluster replicas 3 --yes
B1|rs store push --local: built/pushed lines, root = treekey
B2|go store pull of the Rust push: root, storecmp (Go reads Rust records)
B3|go store push, rs store pull: root, storecmp (Rust reads Go records)
B4|store pull into fresh stores: pulled lines identical
B5|store push CAS: mismatch text, --expected-version, --force
B6|refs, refs trees/ (TZ matrix)
B7|ref get trees/rs (TZ matrix)
B8|ls and cat of trees/rs: entries, contents, 5 MiB file, errors
B9|ref delete (default force), CAS errors of ref delete
B10|packstore cross-write: a push dedups against the other implementation segments
B11|large trees both ways (--jobs 2): roots, storecmp
B12|SIGINT during store push: context canceled, re-run uploads less
B13|id-only ticket (cluster ticket --ids) resolved over mDNS
B14|DSTORE_TICKET=garbage: the flag wins; the env parse error
C1|watch trees/** during the suite, go vs rs (TZ matrix)
C2|watch started after refs exist prints them first
C3|chaos: watch survives a node restart
D1|clone go (wcA) and rs (wcB): lines, roots, trees equal
D2|.dstore/config identical, .dstore/state identical but synced_at
D3|rs status in the Go-created wcA equals go status
D4|status and diff after every kind of change, go vs rs in wcA
D5|rs push in the Go-created wcA; go status clean; nothing to push
D6|go fetch in the Rust-created wcB; status and diffs go vs rs
D7|conflict: push refused, pull conflicts, pull --force; cross-implementation pull and push
D8|init in an existing directory, status, push, fetch up to date
D9|lost state: push recovers (both directions)
D10|missing .dstore/state: incomplete clone
D11|clone into a non-empty dir, onto a file, of an unknown ref; init inside a working copy
D12|lock interop: a Go holder blocks rs, a Rust holder blocks go
D13|stored-ticket refresh: a stale ticket is rewritten identically
E1|SIGINT and SIGTERM end watch with exit 0
E2|DSTORE_LOG_LEVEL=warn hides INFO lines; info shows the same messages
E3|cargo test live_go_node_* against n1 (Rust iroh 1.2.0 to go-iroh)
G1|DD-2: Pebble refs refused by rs; Go on Rust refs; refused after
G2|DD-3: --store ticket derivation and catalog restore texts
H1|cat NAME /: DD-7 panic line and exit 2
H2|cat NAME big.bin piped into head -c1: killed by SIGPIPE
H3|watch trees/** piped into head -1: killed by SIGPIPE
H4|TUI with stderr on /dev/null (character device): store pull, clone
H5|TUI through a pty (script): colour modes
H6|working-copy push checks --user after dialing
H7|catalog restore KEY: fetch, then the --store requirement
H8|cluster status with a cluster id shorter than 4 bytes
'

# The order checks run in (a full run, or the selected ones in this order).
ORDER='A1 A2 A3 A4 A5 A6 A7 A8 A9 A10 A11 H7 H8
B1 B2 B3 B4 B5 B6 B7 B8 H1 H2 B9 B10 B11 B12 B13 B14 H3 H4 H5
C2 E1 E2
D1 D2 D3 D4 D5 D6 D7 D8 D9 D10 D11 D12 D13 H6
C1 G1 G2 A12 A13 C3 E3'

check_desc() { printf '%s\n' "$CHECKS" | awk -F'|' -v id="$1" '$1 == id { print $2 }'; }

usage() {
	sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'
	printf '\nInputs: DSTORE_GO_BIN, DSTORE_GO_REPO, DSTORE_RS_BIN, DSTORE_RS_HOLDLOCK, INTEROP_CARGO_TARGET_DIR,\n'
	printf 'INTEROP_MDNS=auto|require|skip, INTEROP_HEAVY=1, INTEROP_CHAOS=1, INTEROP_BASELINE=1, INTEROP_KEEP=1,\n'
	printf 'INTEROP_FAILFAST=1, INTEROP_LOG_DIR, INTEROP_TZS, INTEROP_CMD_TIMEOUT (see interop/README.md).\n'
}

SELECTED=""
for a in "$@"; do
	case $a in
	-h | --help)
		usage
		exit 0
		;;
	--list)
		printf '%s\n' "$CHECKS" | awk -F'|' 'NF == 2 { printf "%-4s %s\n", $1, $2 }'
		exit 0
		;;
	[A-H]) SELECTED="$SELECTED $a" ;;
	[A-H][0-9]*)
		[ -n "$(check_desc "$a")" ] || die "unknown check $a (see --list)"
		SELECTED="$SELECTED $a"
		;;
	*) die "unknown argument $a (see --help)" ;;
	esac
done

# ---- environment ----

# Nothing from the caller's environment may point a client at another cluster or change its behaviour.
for v in DSTORE_TICKET DSTORE_STORE DSTORE_LOG_LEVEL DSTORE_NO_TUI DSTORE_NO_DISCOVERY DSTORE_PACK_SIZE \
	AMBER_STORE DSTORE_LIVE_STORE NO_COLOR COLORTERM; do
	unset "$v"
done
# QUIC_GO_DISABLE_RECEIVE_BUFFER_WARNING: the Go binary's quic-go fork logs a buffer warning at bind on
# Linux hosts with small UDP buffer limits (PORTING.md DD-16); the Rust CLI prints nothing.
export TZ=UTC LC_ALL=C GOTOOLCHAIN=local CGO_ENABLED=0 QUIC_GO_DISABLE_RECEIVE_BUFFER_WARNING=true

MDNS=${INTEROP_MDNS:-auto}
case $MDNS in auto | require | skip) ;; *) die "INTEROP_MDNS=$MDNS: want auto, require or skip" ;; esac
TZS=${INTEROP_TZS:-UTC Asia/Kolkata America/St_Johns}
CMD_TIMEOUT=${INTEROP_CMD_TIMEOUT:-180}
BASELINE=${INTEROP_BASELINE:-0}
ZERO64=0000000000000000000000000000000000000000000000000000000000000000
EMPTY_TREE=2001bbe6a9f5a0146a1f4d0381e9b0ed1ac2f1a979ce9d5ad84e46ff0b58f36b

W=$(mktemp -d "${TMPDIR:-/tmp}/dstore-interop.XXXXXX") || die "mktemp failed"
W=$(cd "$W" && pwd -P)
CK=$W/checks
BIN=$W/bin
LOGS=$W/logs
WC=$W/wc
mkdir -p "$CK" "$BIN" "$LOGS" "$WC"

cleanup() {
	local rc=$?
	trap - EXIT INT TERM
	stop_all
	if [ -n "$FAILED" ] && [ -n "${INTEROP_LOG_DIR:-}" ]; then
		mkdir -p "$INTEROP_LOG_DIR" && cp -R "$CK" "$LOGS" "$INTEROP_LOG_DIR/" 2>/dev/null &&
			say "logs copied to $INTEROP_LOG_DIR"
	fi
	if [ "${INTEROP_KEEP:-0}" = 1 ]; then
		rm -rf "$BIN" "$W/target" "$W/gobin" "$W/go-src"
		say "kept $W (binaries removed)"
	else
		rm -rf "$W"
	fi
	exit $rc
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# ---- binaries ----

build_go() {
	if [ -n "${DSTORE_GO_BIN:-}" ]; then
		cp "$DSTORE_GO_BIN" "$BIN/dstore-go" || die "cannot copy DSTORE_GO_BIN"
		return
	fi
	local repo=${DSTORE_GO_REPO:-}
	if [ -z "$repo" ] && [ -d "$REPO/../dstore/.git" ]; then
		repo=$(cd "$REPO/../dstore" && pwd -P)
	fi
	if [ -n "$repo" ]; then
		local tag
		tag=$(git -C "$repo" describe --tags --exact-match HEAD 2>/dev/null)
		[ "$tag" = v0.1.9 ] || die "$repo: HEAD is ${tag:-not a tag}, want v0.1.9"
		say "building Go dstore $tag from $repo (git archive HEAD, CGO_ENABLED=0)"
		mkdir -p "$W/go-src" && git -C "$repo" archive HEAD | tar -x -C "$W/go-src" || die "git archive failed"
		(cd "$W/go-src" && go build -trimpath -o "$BIN/dstore-go" ./cmd/dstore) >"$LOGS/go-build.log" 2>&1 ||
			{ cat "$LOGS/go-build.log"; die "go build of dstore failed"; }
		rm -rf "$W/go-src"
	else
		say "installing github.com/amber-store/dstore/cmd/dstore@v0.1.9"
		GOBIN="$W/gobin" go install github.com/amber-store/dstore/cmd/dstore@v0.1.9 >"$LOGS/go-build.log" 2>&1 ||
			{ cat "$LOGS/go-build.log"; die "go install of dstore failed"; }
		mv "$W/gobin/dstore" "$BIN/dstore-go" && rm -rf "$W/gobin"
	fi
}

build_tools() {
	(cd "$REPO/tools/vectorgen" && go build -trimpath -o "$BIN/" ./cmd/mktree ./cmd/treekey ./cmd/storecmp ./cmd/holdlock) \
		>"$LOGS/tools-build.log" 2>&1 || { cat "$LOGS/tools-build.log"; die "go build of the vectorgen helpers failed"; }
	mv "$BIN/holdlock" "$BIN/holdlock-go"
}

CARGO_TD=""
build_rs() {
	if [ -n "${DSTORE_RS_BIN:-}" ]; then
		cp "$DSTORE_RS_BIN" "$BIN/dstore-rs" || die "cannot copy DSTORE_RS_BIN"
		local hl=${DSTORE_RS_HOLDLOCK:-$(dirname "$DSTORE_RS_BIN")/examples/holdlock}
		cp "$hl" "$BIN/holdlock-rs" || die "no holdlock example at $hl (cargo build --release --examples, or DSTORE_RS_HOLDLOCK)"
		CARGO_TD=${INTEROP_CARGO_TARGET_DIR:-${CARGO_TARGET_DIR:-$REPO/target}}
		return
	fi
	CARGO_TD=${INTEROP_CARGO_TARGET_DIR:-$W/target}
	say "building the Rust dstore (cargo build --release --locked --bin dstore --examples)"
	(cd "$REPO" && CARGO_TARGET_DIR=$CARGO_TD cargo build --release --locked --bin dstore --examples) \
		>"$LOGS/cargo-build.log" 2>&1 || { tail -n 30 "$LOGS/cargo-build.log"; die "cargo build failed"; }
	cp "$CARGO_TD/release/dstore" "$BIN/dstore-rs" && cp "$CARGO_TD/release/examples/holdlock" "$BIN/holdlock-rs" ||
		die "cargo build left no binaries in $CARGO_TD/release"
}

build_go
build_tools
if [ "$BASELINE" = 1 ]; then
	say "baseline run: both clients are the Go binary"
	cp "$BIN/dstore-go" "$BIN/dstore-rs" && cp "$BIN/holdlock-go" "$BIN/holdlock-rs"
else
	build_rs
fi
GO=$BIN/dstore-go
GO_CLI=$GO
RS_CLI=$BIN/dstore-rs
MKTREE=$BIN/mktree
TREEKEY=$BIN/treekey
STORECMP=$BIN/storecmp
say "go: $("$GO" --version)   rs: $("$RS_CLI" --version)   work dir: $W"

# ---- the cluster ----

cluster_steady() {
	run_bounded 60 "$LOGS/status.out" "$LOGS/status.err" "$GO" cluster status --no-relay || return 1
	grep -q 'nodes 3 voters 3' "$LOGS/status.out" && ! grep -q '^transition ' "$LOGS/status.out" &&
		! grep -q '^voter change in progress' "$LOGS/status.out"
}

node_id_of_log() { awk '/^node id: / { print $3; exit }' "$1" 2>/dev/null; }

go_token() { run_bounded 60 "$LOGS/token.out" "$LOGS/token.err" "$GO" token create --no-relay; }

nodes_alive() {
	local p
	for p in $NODE_PIDS; do
		alive "$p" || return 1
	done
	return 0
}

wait_steady() {
	local secs=$1 start=$SECONDS
	while :; do
		nodes_alive || { tail -n 20 "$LOGS"/n*.log; die "a node process exited"; }
		cluster_steady && return 0
		[ $((SECONDS - start)) -ge "$secs" ] && return 1
		sleep 2
	done
}

setup_cluster() {
	cd "$W" || die "cd $W"
	export DSTORE_LOG_LEVEL=warn
	"$GO" cluster init --store n1 --replicas 3 --weight 100 --no-relay --loopback >"$LOGS/init.out" 2>"$LOGS/init.err" ||
		{ cat "$LOGS/init.err"; die "cluster init failed"; }
	T=$(awk '/^cluster ticket:/ { print $3 }' "$LOGS/init.out")
	N1_ID=$(node_id_of_log "$LOGS/init.out")
	[ -n "$T" ] && [ -n "$N1_ID" ] || die "cluster init printed no ticket or node id"
	export DSTORE_TICKET=$T
	"$GO" serve --store n1 --no-relay --loopback --gc-interval 1m >"$LOGS/n1.log" 2>&1 &
	N1_PID=$!
	track_pid "$N1_PID"
	NODE_PIDS=$N1_PID
	wait_until 60 go_token || { cat "$LOGS/token.err"; die "n1 does not answer token create"; }
	"$GO" node join --store n2 --seed "$T" --token "$(cat "$LOGS/token.out")" --weight 100 --no-ramp --no-relay \
		--loopback >"$LOGS/n2.log" 2>&1 &
	N2_PID=$!
	track_pid "$N2_PID"
	NODE_PIDS="$NODE_PIDS $N2_PID"
	sleep 2
	# A3: the Rust client creates n3's join token.
	CUR=A3
	run_one rs A3.token -- token create --no-relay
	TOK3=$(out A3.token rs)
	TOK3_BY=rs
	if ! printf '%s\n' "$TOK3" | grep -Eq '^[0-9a-f]{64}$'; then
		say "rs token create failed; n3 joins with a Go token (A3 will fail)"
		go_token || die "go token create failed"
		TOK3=$(cat "$LOGS/token.out")
		TOK3_BY=go
	fi
	CUR=""
	"$GO" node join --store n3 --seed "$T" --token "$TOK3" --weight 100 --no-ramp --no-relay --loopback \
		>"$LOGS/n3.log" 2>&1 &
	N3_PID=$!
	track_pid "$N3_PID"
	NODE_PIDS="$NODE_PIDS $N3_PID"
	local start=$SECONDS
	wait_steady 300 || {
		cat "$LOGS/status.out" "$LOGS/status.err"
		tail -n 20 "$LOGS/n2.log" "$LOGS/n3.log"
		die "the cluster did not reach 3 nodes / 3 voters without a transition"
	}
	wait_until 30 has_line "$LOGS/n3.log" '^node id: ' || die "n3 printed no node id"
	N2_ID=$(node_id_of_log "$LOGS/n2.log")
	N3_ID=$(node_id_of_log "$LOGS/n3.log")
	run_bounded 60 "$LOGS/ids" "$LOGS/ids.err" "$GO" cluster ticket --no-relay --ids || die "cluster ticket --ids failed"
	say "cluster steady after $((SECONDS - start))s: n1 ${N1_ID:0:8} n2 ${N2_ID:0:8} n3 ${N3_ID:0:8}"
	export DSTORE_NO_TUI=true
}

# ---- shared fixtures ----

# ensure_src NAME SEED [SMALL]: a mktree tree at $W/NAME.
ensure_src() {
	[ -d "$W/$1" ] && return 0
	"$MKTREE" -seed "$2" -small "${3:-2000}" "$W/$1" >"$LOGS/mktree-$1.log" 2>&1 && return 0
	note_fail "mktree $1: $(cat "$LOGS/mktree-$1.log")"
	return 1
}

# ensure_small NAME: a directory of two small files (content from NAME).
ensure_small() {
	[ -d "$W/$1" ] && return 0
	mkdir -p "$W/$1/sub" && printf '%s\n' "$1" >"$W/$1/a.txt" && printf 'b of %s\n' "$1" >"$W/$1/sub/b.txt"
}

# tree_key DIR [treekey flags]: the root key core ingest computes for DIR.
tree_key() {
	local d=$1
	shift
	"$TREEKEY" "$@" "$d" 2>>"$LOGS/treekey.err"
}

# random_files DIR N MIB: N files of MIB MiB of random bytes.
random_files() {
	local i=1
	mkdir -p "$1"
	while [ $i -le "$2" ]; do
		head -c $(($3 * 1048576)) /dev/urandom >"$1/rand$i.bin" || return 1
		i=$((i + 1))
	done
}

# storecmp_check STEP [-strict] A B ROOT: storecmp of two packstores; its summary becomes a note. Without
# -strict, objects the pusher's store held under its own compression may differ in bytes (DD-1) but must
# decode to the same payload.
storecmp_check() {
	local step=$1
	shift
	if run_bounded 300 "$CK/$step.out" "$CK/$step.err" "$STORECMP" "$@"; then
		note_info "storecmp $(basename "$(dirname "${*: -3:1}")") $(basename "$(dirname "${*: -2:1}")"): $(tail -n 1 "$CK/$step.out")"
		return 0
	fi
	note_fail "storecmp $*:"
	tail -n 5 "$CK/$step.out" "$CK/$step.err" | sed 's/^/  /' >>"$CK/$CUR.fail"
	return 1
}

# start_watch STEP IMPL [NAME=VALUE...]: a background `watch --no-relay trees/**` at INFO level, its
# output in STEP.IMPL.{out,err}; sets WATCH_PID.
start_watch() {
	local step=$1 impl=$2
	shift 2
	: >"$CK/$step.$impl.err"
	env DSTORE_LOG_LEVEL=info "$@" "$(client_bin "$impl")" watch --no-relay 'trees/**' \
		>"$CK/$step.$impl.out" 2>"$CK/$step.$impl.err" </dev/null &
	WATCH_PID=$!
	track_pid "$WATCH_PID"
	printf 'env DSTORE_LOG_LEVEL=info %s dstore-%s watch --no-relay trees/**\n' "$*" "$impl" >"$CK/$step.$impl.cmd"
}

# wait_synced STEP IMPL: the watcher logged its first `watch: synced` (the initial references are out).
wait_synced() {
	wait_until 30 has_line "$CK/$1.$2.err" 'msg="watch: synced"' && return 0
	note_fail "$1: the $2 watch logged no \"watch: synced\" within 30 s"
	tail -n 3 "$CK/$1.$2.err" | sed 's/^/  /' >>"$CK/$CUR.fail"
	return 1
}

# end_watch STEP IMPL PID SIG: signals the watcher and records its exit status.
end_watch() {
	local rc
	kill "-$4" "$3" 2>/dev/null
	if ! wait_gone "$3" 20; then
		note_fail "$1: the $2 watch did not exit within 20 s of SIG$4"
		kill -KILL "$3" 2>/dev/null
	fi
	reap "$3"
	rc=$?
	untrack_pid "$3"
	printf '%s\n' "$rc" >"$CK/$1.$2.exit"
}

WATCH_LINE=$'^[^\t]+\t(deleted|[0-9a-f]{64}\t[^\t]+\t.*)$'

# ---- A: cluster and admin ----

check_A1() {
	pair_retry 6 A1 normalized -- cluster status --no-relay
	expect_exit A1 rs 0
	has_line "$CK/A1.go.out" '^replicas 3 min_replicas 2 nodes 3 voters 3$' ||
		note_fail "A1: go cluster status lacks 'replicas 3 min_replicas 2 nodes 3 voters 3'"
}

check_A2() {
	pair_retry 3 A2.ticket exact -- cluster ticket --no-relay
	pair_retry 3 A2.ids exact -- cluster ticket --no-relay --ids
	expect_lines "$CK/A2.ticket.rs.out" '^dstore1[a-z2-7]+$'
	expect_lines "$CK/A2.ids.rs.out" '^[0-9a-f]{64},[0-9a-f]{64},[0-9a-f]{64}$'
	# Each implementation's ticket lets the other list the references.
	run_one go A2.cross -- refs --no-relay --ticket "$(out A2.ticket rs)"
	run_one rs A2.cross -- refs --no-relay --ticket "$(out A2.ticket go)"
	compare exact A2.cross
	expect_exit A2.cross go 0
	expect_exit A2.cross rs 0
}

check_A3() {
	[ "$TOK3_BY" = rs ] || note_fail "rs token create printed no token: $(tail -n 1 "$CK/A3.token.rs.err")"
	expect_lines "$CK/A3.token.rs.out" '^[0-9a-f]{64}$'
	case ",$(cat "$LOGS/ids")," in
	*",$N3_ID,"*) ;;
	*) note_fail "n3 ($N3_ID), joined with the Rust token, is not in cluster ticket --ids" ;;
	esac
	pair A3.weight exit -- token create --no-relay --weight 5
	expect_lines "$CK/A3.weight.go.out" '^[0-9a-f]{64}$'
	expect_lines "$CK/A3.weight.rs.out" '^[0-9a-f]{64}$'
}

check_A4() {
	pair_retry 3 A4.transition exact -- transition status --no-relay
	run_one go A4.hold -- gc hold --no-relay
	pair_retry 3 A4.gc exact -- gc status --no-relay
	has_line "$CK/A4.gc.go.out" 'ON HOLD' || note_fail "A4: gc status after gc hold does not say ON HOLD"
	pair_retry 3 A4.backups exact -- catalog backups --no-relay
	run_one go A4.release -- gc release --no-relay
}

check_A5() {
	pair A5.hold exact -- gc hold --no-relay
	expect_lines "$CK/A5.hold.rs.out" '^ok$'
	pair A5.release exact -- gc release --no-relay
	expect_lines "$CK/A5.release.rs.out" '^ok$'
}

# a6_once RUNNER VIEWER: RUNNER's gc run, then VIEWER's gc status; 0 when both succeed and agree.
a6_once() {
	run_one "$1" "A6.$1run" -- gc run --no-relay
	run_one "$2" "A6.$1run" -- gc status --no-relay
	[ "$(code "A6.$1run" "$1")" = 0 ] && [ "$(code "A6.$1run" "$2")" = 0 ] &&
		same "$CK/A6.$1run.$1.out" "$CK/A6.$1run.$2.out" && [ -s "$CK/A6.$1run.$1.out" ]
}

check_A6() {
	local runner viewer i
	for runner in rs go; do
		viewer=go
		[ $runner = go ] && viewer=rs
		i=1
		until a6_once $runner $viewer; do
			if [ $i -ge 3 ]; then
				note_fail "A6: $runner gc run vs the $viewer gc status after it (exit $(code "A6.${runner}run" $runner)/$(code "A6.${runner}run" $viewer))"
				diff_into_fail "$CK/A6.${runner}run.$runner.out" "$CK/A6.${runner}run.$viewer.out" "  gc run vs gc status:"
				tail -n 2 "$CK/A6.${runner}run.$runner.err" | sed 's/^/  /' >>"$CK/$CUR.fail"
				break
			fi
			i=$((i + 1))
			sleep 5
		done
		[ $i -gt 1 ] && [ $i -le 3 ] && note_info "$runner gc run: equal on attempt $i (a scheduled cycle ran in between)"
	done
}

ROOT_RS=""
check_A7() {
	need B1 || return
	pair A7.root exact -- gc why --no-relay "$ROOT_SRC1"
	has_line "$CK/A7.root.go.out" '^trees/rs$' || note_fail "gc why of the trees/rs root does not name trees/rs"
	pair A7.zero exact -- gc why --no-relay "$ZERO64"
	[ -s "$CK/A7.zero.go.out" ] && note_fail "gc why of 64 zeros printed names"
}

BACKUP_KEY=""
check_A8() {
	run_one rs A8.rs -- catalog backup --no-relay
	expect_exit A8.rs rs 0 || return
	expect_lines "$CK/A8.rs.rs.out" '^backup written$' '^key [0-9a-f]{64}$' || return
	BACKUP_KEY=$(awk '/^key / { print $2 }' "$CK/A8.rs.rs.out")
	listed() {
		run_one go A8.list -- catalog backups --no-relay
		grep -qx "$BACKUP_KEY" "$CK/A8.list.go.out"
	}
	wait_until 20 listed || note_fail "go catalog backups does not list $BACKUP_KEY"
	run_one go A8.go -- catalog backup --no-relay
	expect_lines "$CK/A8.go.go.out" '^backup written$' '^key [0-9a-f]{64}$'
}

check_A9() {
	pair A9 exact -- node repair --no-relay "$N2_ID"
	expect_lines "$CK/A9.rs.out" "^repair scheduled: holders will refill ${N2_ID:0:8}\$"
}

check_A10() {
	pair A10.pause exact -- transition pause --no-relay
	expect_lines "$CK/A10.pause.rs.out" '^ok$'
	pair A10.resume exact -- transition resume --no-relay
	expect_lines "$CK/A10.resume.rs.out" '^ok$'
}

check_A11() {
	pair A11.unknown exact -- ref get --no-relay trees/none
	expect_lines "$(errfile A11.unknown go)" '^dstore: client: unknown reference$'
	pair A11.glob exact -- watch --no-relay 'trees/['
	expect_lines "$(errfile A11.glob go)" '^dstore: remote: bad-request: refglob: '
	pair A11.name exact -- ref get --no-relay 'bad@@name'
	expect_lines "$(errfile A11.name go)" '^dstore: remote: bad-request: '
	# A non-member id would make the node add a phantom voter (a majority of the new set answers the ping)
	# and wait a minute for its marker; an existing voter is refused without a state change.
	pair A11.voter exact -- voter add --no-relay "$N2_ID"
	expect_lines "$(errfile A11.voter go)" '^dstore: remote: .*already a voter$'
	pair A11.replicas exact -- cluster replicas --no-relay --yes 0
	expect_exit A11.replicas go 1
	note_info "voter add uses an existing voter's id, not a non-member (verification.md A11): see the comment in check_A11"
}

check_A12() {
	[ "${INTEROP_HEAVY:-0}" = 1 ] || { skip_current "heavy group (set INTEROP_HEAVY=1)"; return; }
	run_one rs A12.weight -- node weight --no-relay "$N3_ID" 50
	expect_exit A12.weight rs 0 && expect_lines "$CK/A12.weight.rs.out" '^transition proposed at epoch [0-9]+$'
	wait_steady 900 || note_fail "the cluster did not settle after the weight change"
	run_one go A12.status -- cluster status --no-relay
	has_line "$CK/A12.status.go.out" "^  $N3_ID weight 50 " || note_fail "go cluster status does not show n3 at weight 50"
	pair_retry 6 A12.status normalized -- cluster status --no-relay
	run_one go A12.restore -- node weight --no-relay "$N3_ID" 100
	expect_exit A12.restore go 0
	wait_steady 900 || note_fail "the cluster did not settle after restoring the weight"
}

check_A13() {
	[ "${INTEROP_HEAVY:-0}" = 1 ] || { skip_current "heavy group (set INTEROP_HEAVY=1)"; return; }
	run_one rs A13.r2 -- cluster replicas --no-relay --yes 2
	expect_exit A13.r2 rs 0 && expect_lines "$CK/A13.r2.rs.out" '^transition proposed$'
	wait_steady 900 || note_fail "the cluster did not settle after replicas 2"
	run_one go A13.status -- cluster status --no-relay
	has_line "$CK/A13.status.go.out" '^replicas 2 ' || note_fail "go cluster status does not show replicas 2"
	run_one go A13.r3 -- cluster replicas --no-relay --yes 3
	expect_exit A13.r3 go 0 && expect_lines "$CK/A13.r3.go.out" '^transition proposed$'
	wait_steady 900 || note_fail "the cluster did not settle after replicas 3"
	# D13 once more: the view changed, so the stored ticket is re-derived by both.
	need D1 && d13_refresh A13.d13
}

# ---- B: standalone local stores ----

ROOT_SRC1=""
check_B1() {
	ensure_src src1 1 || return
	ROOT_SRC1=$(tree_key "$W/src1")
	run_one rs B1 -C "$W" -- store push --no-relay --local rsL --user interop src1 trees/rs
	expect_exit B1 rs 0 || return
	expect_lines "$(errfile B1 rs)" "^built ${ROOT_SRC1:0:16}: [0-9]+ new objects\$"
	expect_lines "$CK/B1.rs.out" '^pushed trees/rs: [0-9]+ objects, [0-9]+ uploaded, version [0-9a-f]+$'
	run_one go B1.ref -- ref get --no-relay trees/rs
	has_line "$CK/B1.ref.go.out" "^key $ROOT_SRC1\$" || note_fail "trees/rs does not point at treekey src1 ($ROOT_SRC1)"
	ROOT_RS=$ROOT_SRC1
}

check_B2() {
	need B1 || return
	run_one go B2 -C "$W" -- store pull --no-relay --local goL trees/rs
	expect_exit B2 go 0 || return
	expect_lines "$CK/B2.go.out" "^pulled trees/rs: root $ROOT_SRC1, [0-9]+ objects fetched \\([0-9]+ bytes\\)\$"
	storecmp_check B2.cmp "$W/rsL/packstore" "$W/goL/packstore" "$ROOT_SRC1"
}

ROOT_SRC2=""
check_B3() {
	ensure_src src2 2 || return
	ROOT_SRC2=$(tree_key "$W/src2")
	run_one go B3.push -C "$W" -- store push --no-relay --local goP --user interop src2 trees/go
	expect_exit B3.push go 0 || return
	run_one rs B3 -C "$W" -- store pull --no-relay --local rsL2 trees/go
	expect_exit B3 rs 0 || return
	expect_lines "$CK/B3.rs.out" "^pulled trees/go: root $ROOT_SRC2, [0-9]+ objects fetched \\([0-9]+ bytes\\)\$"
	storecmp_check B3.cmp "$W/goP/packstore" "$W/rsL2/packstore" "$ROOT_SRC2"
}

check_B4() {
	need B1 || return
	pair B4 stdout+exit -C "$W" -- store pull --no-relay --local b4-@IMPL@ trees/rs
	expect_exit B4 rs 0 && storecmp_check B4.cmp -strict "$W/b4-go/packstore" "$W/b4-rs/packstore" "$ROOT_SRC1"
	need B3 || return
	pair B4.go stdout+exit -C "$W" -- store pull --no-relay --local b4g-@IMPL@ trees/go
	expect_exit B4.go rs 0 && storecmp_check B4.cmp2 -strict "$W/b4g-go/packstore" "$W/b4g-rs/packstore" "$ROOT_SRC2"
}

mask_version() { sed 's/version [0-9a-f]*$/version V/' "$1"; }

check_B5() {
	need B1 || return
	pair B5.cas exact -C "$W" -- store push --no-relay --local b5-@IMPL@ --user interop src1 trees/rs
	expect_lines "$(errfile B5.cas go)" "^built ${ROOT_SRC1:0:16}: [0-9]+ new objects\$" \
		"^dstore: cas mismatch: current key $ROOT_SRC1 \\(pull first, or --force\\)\$"
	expect_exit B5.cas go 1
	local impl v
	for impl in go rs; do
		run_one $impl B5.get -- ref get --no-relay trees/rs
		v=$(awk '/^version / { print $2 }' "$CK/B5.get.$impl.out")
		run_one $impl B5.ev -C "$W" -- store push --no-relay --local b5-@IMPL@ --user interop --expected-version "$v" src1 trees/rs
		expect_exit B5.ev $impl 0
		expect_lines "$CK/B5.ev.$impl.out" '^pushed trees/rs: [0-9]+ objects, [0-9]+ uploaded, version [0-9a-f]+$'
		mask_version "$CK/B5.ev.$impl.out" >"$CK/B5.ev.$impl.masked"
	done
	same "$CK/B5.ev.go.masked" "$CK/B5.ev.rs.masked" ||
		diff_into_fail "$CK/B5.ev.go.masked" "$CK/B5.ev.rs.masked" "B5 --expected-version push, go vs rs (version masked):"
	pair B5.force exit -C "$W" -- store push --no-relay --local b5-@IMPL@ --user interop --force src1 trees/rs
	expect_exit B5.force rs 0
	mask_version "$CK/B5.force.go.out" >"$CK/B5.force.go.masked"
	mask_version "$CK/B5.force.rs.out" >"$CK/B5.force.rs.masked"
	same "$CK/B5.force.go.masked" "$CK/B5.force.rs.masked" ||
		diff_into_fail "$CK/B5.force.go.masked" "$CK/B5.force.rs.masked" "B5 --force push, go vs rs (version masked):"
}

tz_step() { printf '%s' "$1" | tr '/' '-'; }

check_B6() {
	need B1 || return
	local tz t
	for tz in $TZS; do
		t=$(tz_step "$tz")
		pair_retry 3 "B6.$t" exact "TZ=$tz" -- refs --no-relay
		pair_retry 3 "B6.$t.prefix" exact "TZ=$tz" -- refs --no-relay trees/
		expect_exit "B6.$t" rs 0
	done
	for tz in $TZS; do
		t=$(tz_step "$tz")
		case $tz in
		Asia/Kolkata) has_line "$CK/B6.$t.rs.out" '\+05:30	' || note_fail "B6: no +05:30 time under TZ=$tz" ;;
		America/St_Johns) has_line "$CK/B6.$t.rs.out" '-0[23]:30	' || note_fail "B6: no -02:30/-03:30 time under TZ=$tz" ;;
		esac
	done
}

check_B7() {
	need B1 || return
	local tz t
	for tz in $TZS; do
		t=$(tz_step "$tz")
		pair_retry 3 "B7.$t" exact "TZ=$tz" -- ref get --no-relay trees/rs
		expect_lines "$CK/B7.$t.rs.out" '^name trees/rs$' '^key [0-9a-f]{64}$' '^version [0-9a-f]+$' '^user interop$' \
			'^created [0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(Z|[-+][0-9]{2}:[0-9]{2})$'
	done
}

check_B8() {
	need B1 || return
	pair B8.ls exact -- ls --no-relay trees/rs
	expect_exit B8.ls rs 0
	pair B8.ls-sub exact -- ls --no-relay trees/rs deep/er
	pair B8.ls-root exact -- ls --no-relay trees/rs /
	pair B8.ls-slashes exact -- ls --no-relay trees/rs /deep/er/
	pair B8.ls-file exact -- ls --no-relay trees/rs run.sh
	pair B8.ls-nope exact -- ls --no-relay trees/rs nope
	pair B8.cat exact -- cat --no-relay trees/rs run.sh
	pair B8.cat-big exact -- cat --no-relay trees/rs big.bin
	cmp -s "$CK/B8.cat-big.rs.out" "$W/src1/big.bin" || note_fail "rs cat big.bin differs from the pushed file"
	pair B8.cat-unicode exact -- cat --no-relay trees/rs 'é-unicode ☃.txt'
	pair B8.cat-deep exact -- cat --no-relay trees/rs /deep/er/f
	pair B8.cat-empty exact -- cat --no-relay trees/rs empty
	pair B8.cat-dir exact -- cat --no-relay trees/rs deep
	pair B8.cat-link exact -- cat --no-relay trees/rs link
	pair B8.cat-fifo exact -- cat --no-relay trees/rs fifo
	pair B8.cat-nope exact -- cat --no-relay trees/rs nope
	expect_lines "$(errfile B8.cat-link go)" '^dstore: not a regular file with content$'
}

check_B9() {
	need B1 || return
	ensure_small srctmp
	local impl
	for impl in go rs; do
		run_one go B9.mk -C "$W" -- store push --no-relay --local b9 --user interop --force srctmp trees/tmp
		expect_exit B9.mk go 0
		run_one $impl B9.del -- ref delete --no-relay trees/tmp
		run_one $impl B9.refs -- refs --no-relay trees/tmp
		[ -s "$CK/B9.refs.$impl.out" ] && note_fail "refs still lists trees/tmp after $impl ref delete"
	done
	compare exact B9.del
	expect_exit B9.del rs 0
	run_one rs B9.mk2 -C "$W" -- store push --no-relay --local rsL --user interop --force src1 trees/rs2
	expect_exit B9.mk2 rs 0
	pair B9.cas exact -- ref delete --no-relay --expected-version 00 trees/rs2
	expect_lines "$(errfile B9.cas go)" "^dstore: cas mismatch: current key $ROOT_SRC1\$"
	pair B9.absent exact -- ref delete --no-relay --expected-version 00 trees/absent
	pair B9.absent-force exact -- ref delete --no-relay trees/absent
	pair B9.badhex exact -- ref delete --no-relay --expected-version zz trees/rs2
}

check_B10() {
	need B1 || return
	need B2 || return
	rm -rf "$W/b10x" "$W/b10y"
	mkdir -p "$W/b10x" "$W/b10y"
	cp -Rp "$W/rsL/packstore" "$W/b10x/packstore" && cp -Rp "$W/goL/packstore" "$W/b10y/packstore" ||
		{ note_fail "copying the packstores failed"; return; }
	run_one go B10 -C "$W" -- store push --no-relay --local b10x --user interop src1 trees/x
	run_one rs B10 -C "$W" -- store push --no-relay --local b10y --user interop src1 trees/y
	local impl
	for impl in go rs; do
		expect_exit B10 $impl 0
		head -n 1 "$(errfile B10 $impl)" >"$CK/B10.$impl.built"
		expect_lines "$CK/B10.$impl.built" "^built ${ROOT_SRC1:0:16}: 0 new objects\$"
	done
	note_info "go pushed from a copy of the Rust-written rsL/packstore, rs from a copy of the Go-written goL/packstore"
}

ROOT_BIGR=""
check_B11() {
	if [ ! -d "$W/bigR" ]; then
		ensure_src bigR 3 2000 && random_files "$W/bigR/rand" 6 10 || { note_fail "cannot build bigR"; return; }
	fi
	if [ ! -d "$W/bigG" ]; then
		ensure_src bigG 4 2000 && random_files "$W/bigG/rand" 6 10 || { note_fail "cannot build bigG"; return; }
	fi
	ROOT_BIGR=$(tree_key "$W/bigR")
	local rootg
	rootg=$(tree_key "$W/bigG")
	run_one rs B11.push -C "$W" -- store push --no-relay --local b11r --user interop --jobs 2 bigR trees/big-rs
	expect_exit B11.push rs 0 &&
		expect_lines "$CK/B11.push.rs.out" '^pushed trees/big-rs: [0-9]+ objects, [0-9]+ uploaded, version [0-9a-f]+$'
	run_one go B11.pull -C "$W" -- store pull --no-relay --local b11g --jobs 2 trees/big-rs
	expect_exit B11.pull go 0 &&
		expect_lines "$CK/B11.pull.go.out" "^pulled trees/big-rs: root $ROOT_BIGR, [0-9]+ objects fetched \\([0-9]+ bytes\\)\$" &&
		storecmp_check B11.cmp "$W/b11r/packstore" "$W/b11g/packstore" "$ROOT_BIGR"
	run_one go B11.push2 -C "$W" -- store push --no-relay --local b11g2 --user interop --jobs 2 bigG trees/big-go
	expect_exit B11.push2 go 0
	run_one rs B11.pull2 -C "$W" -- store pull --no-relay --local b11r2 --jobs 2 trees/big-go
	expect_exit B11.pull2 rs 0 &&
		expect_lines "$CK/B11.pull2.rs.out" "^pulled trees/big-go: root $rootg, [0-9]+ objects fetched \\([0-9]+ bytes\\)\$" &&
		storecmp_check B11.cmp2 "$W/b11g2/packstore" "$W/b11r2/packstore" "$rootg"
}

# b12_interrupted IMPL: IMPL's store push of b12-IMPL at INFO level, SIGINT once a batch is stored.
b12_interrupted() {
	local impl=$1 pid rc
	local base="$CK/B12.$impl"
	printf 'cd %q && env DSTORE_LOG_LEVEL=info dstore-%s store push --no-relay --local b12-%s --user interop b12src-%s trees/b12-%s; SIGINT after msg=uploaded\n' \
		"$W" "$impl" "$impl" "$impl" "$impl" >"$base.cmd"
	(cd "$W" && exec env DSTORE_LOG_LEVEL=info "$(client_bin "$impl")" store push --no-relay --local "b12-$impl" \
		--user interop "b12src-$impl" "trees/b12-$impl" >"$base.out" 2>"$base.err" </dev/null) &
	pid=$!
	track_pid "$pid"
	if ! wait_until 120 has_line "$base.err" 'msg=uploaded '; then
		note_fail "B12: $impl logged no msg=uploaded within 120 s"
	fi
	kill -INT "$pid" 2>/dev/null
	wait_gone "$pid" 120 || kill -KILL "$pid" 2>/dev/null
	reap "$pid"
	rc=$?
	untrack_pid "$pid"
	printf '%s\n' "$rc" >"$base.exit"
}

check_B12() {
	local impl
	for impl in go rs; do
		[ -d "$W/b12src-$impl" ] || random_files "$W/b12src-$impl" 12 16 || { note_fail "cannot build b12src-$impl"; return; }
	done
	for impl in go rs; do
		b12_interrupted $impl
	done
	# Where the interrupt lands (negotiate, upload, a dial, the reference write) depends on timing, and the
	# wrapped keys differ (other trees; DD-10 picks). The error must come from the cancellation: context
	# canceled; the transport's own text when it cuts a dial (Go: "interrupt signal received"; DD-4); or
	# the pool's memory of such a cancelled dial: Go's Pool.Get records every failed dial, cancelled ones
	# included, and askPrimaries does not re-ask once ctx is done, so the next round's negotiate can fail
	# with "transport: peer recently unreachable" in Go as well. The shapes are noted.
	for impl in go rs; do
		expect_exit B12 $impl 1
		[ -s "$CK/B12.$impl.out" ] && note_fail "B12: the interrupted $impl push printed on stdout: $(cat "$CK/B12.$impl.out")"
		expect_match "B12 $impl error line" "$(last_dstore_line "$CK/B12.$impl.err")" \
			'^dstore: ([^ ].*: )?(context canceled|interrupt signal received|transport: peer recently unreachable)$'
		last_dstore_line "$CK/B12.$impl.err" | sed 's/[0-9a-f]\{16,64\}/KEY/g' >"$CK/B12.$impl.shape"
	done
	note_info "interrupted: go '$(cat "$CK/B12.go.shape")', rs '$(cat "$CK/B12.rs.shape")'"
	# The objects stored before the interrupt are not uploaded again.
	local k u
	for impl in go rs; do
		run_one $impl B12.again -C "$W" -- store push --no-relay --local "b12-$impl" --user interop "b12src-$impl" "trees/b12-$impl"
		expect_exit B12.again $impl 0 || continue
		k=$(sed -n 's/^pushed [^:]*: \([0-9]*\) objects, \([0-9]*\) uploaded, .*/\1/p' "$CK/B12.again.$impl.out")
		u=$(sed -n 's/^pushed [^:]*: \([0-9]*\) objects, \([0-9]*\) uploaded, .*/\2/p' "$CK/B12.again.$impl.out")
		if [ -z "$k" ] || [ -z "$u" ] || [ "$u" -ge "$k" ]; then
			note_fail "B12: $impl re-run uploaded ${u:-?} of ${k:-?} objects, want fewer: $(cat "$CK/B12.again.$impl.out")"
		fi
	done
	note_info "SIGINT is sent after the first msg=uploaded line (a stored batch), so the re-run must upload less"
}

MDNS_WORKS=""
check_B13() {
	if [ "$MDNS" = skip ]; then
		MDNS_WORKS=0
		skip_current "INTEROP_MDNS=skip"
		return
	fi
	need B1 || return
	local ids
	ids=$(cat "$LOGS/ids")
	run_one go B13 -- refs --no-relay --ticket "$ids"
	run_one rs B13 -- refs --no-relay --ticket "$ids"
	if [ "$(code B13 go)" != 0 ] && [ "$MDNS" = auto ]; then
		MDNS_WORKS=0
		skip_current "the Go client cannot resolve the node ids over mDNS on this host either (INTEROP_MDNS=auto): $(last_dstore_line "$CK/B13.go.err")"
		return
	fi
	MDNS_WORKS=1
	compare exact B13
	has_line "$CK/B13.rs.out" '^trees/rs	' || note_fail "B13: the id-only rs refs does not list trees/rs"
}

check_B14() {
	pair B14.flag exact DSTORE_TICKET=garbage -- refs --no-relay --ticket "$T"
	expect_exit B14.flag rs 0
	pair B14.env exact DSTORE_TICKET=garbage -- refs --no-relay
	expect_lines "$(errfile B14.env go)" '^dstore: ticket: "garbage" is neither a dstore1 ticket nor a node id: '
}

# ---- C: watch ----

C1_PIDS=""
C1_STARTED=0
start_c1() {
	local tz t impl
	for tz in $TZS; do
		t=$(tz_step "$tz")
		for impl in go rs; do
			start_watch "C1.$t" $impl "TZ=$tz"
			C1_PIDS="$C1_PIDS $t:$impl:$WATCH_PID"
		done
	done
	C1_STARTED=1
	for tz in $TZS; do
		t=$(tz_step "$tz")
		for impl in go rs; do
			wait_until 30 has_line "$CK/C1.$t.$impl.err" 'msg="watch: synced"' ||
				say "warning: the C1 $impl watch (TZ=$tz) logged no \"watch: synced\" within 30 s"
		done
	done
}

# watch_lines_equal STEP: the go and rs watch outputs hold the same lines, in the same order per name, and
# every line has the watch format.
watch_lines_equal() {
	local s=$1 g="$CK/$1.go.out" r="$CK/$1.rs.out" names name
	sort "$g" >"$g.sorted"
	sort "$r" >"$r.sorted"
	same "$g.sorted" "$r.sorted" || diff_into_fail "$g.sorted" "$r.sorted" "$s: the watch lines differ (sorted, go vs rs):"
	names=$(cut -f1 "$g" "$r" | sort -u)
	for name in $names; do
		awk -F'\t' -v n="$name" '$1 == n' "$g" >"$g.one"
		awk -F'\t' -v n="$name" '$1 == n' "$r" >"$r.one"
		same "$g.one" "$r.one" || diff_into_fail "$g.one" "$r.one" "$s: the events of $name differ in order:"
	done
	for f in "$g" "$r"; do
		if grep -Evq "$WATCH_LINE" "$f"; then
			note_fail "$s: $(basename "$f") has lines that are not watch lines:"
			grep -Ev "$WATCH_LINE" "$f" | head -n 3 | sed 's/^/  /' >>"$CK/$CUR.fail"
		fi
	done
}

check_C1() {
	[ $C1_STARTED = 1 ] || { note_fail "the C1 watchers were not started"; return; }
	# Events of its own, so that C1 has something to compare when it runs alone.
	ensure_small srcc1
	run_one go C1.mk -C "$W" -- store push --no-relay --local c1 --user interop srcc1 trees/c1
	run_one rs C1.mk2 -C "$W" -- store push --no-relay --local c1r --user interop --force srcc1 trees/c1
	run_one go C1.del -- ref delete --no-relay trees/c1
	sleep 3
	local e t impl pid
	for e in $C1_PIDS; do
		t=${e%%:*}
		impl=${e#*:}
		impl=${impl%%:*}
		pid=${e##*:}
		end_watch "C1.$t" "$impl" "$pid" INT
	done
	local tz
	for tz in $TZS; do
		t=$(tz_step "$tz")
		expect_exit "C1.$t" go 0
		expect_exit "C1.$t" rs 0
		watch_lines_equal "C1.$t"
		has_line "$CK/C1.$t.rs.out" $'^trees/c1\tdeleted$' || note_fail "C1: the rs watch (TZ=$tz) missed the trees/c1 deletion"
	done
	note_info "$(wc -l <"$CK/C1.UTC.go.out" | tr -d ' ') events per watcher (UTC)"
}

check_C2() {
	need B1 || return
	local impl pids=""
	for impl in go rs; do
		start_watch C2 $impl
		pids="$pids $WATCH_PID"
	done
	for impl in go rs; do
		wait_synced C2 $impl
	done
	set -- $pids
	end_watch C2 go "$1" INT
	end_watch C2 rs "$2" INT
	expect_exit C2 go 0
	expect_exit C2 rs 0
	watch_lines_equal C2
	has_line "$CK/C2.rs.out" '^trees/rs	[0-9a-f]{64}	' || note_fail "C2: the rs watch did not print trees/rs first"
}

check_C3() {
	[ "${INTEROP_CHAOS:-0}" = 1 ] || { skip_current "chaos group (set INTEROP_CHAOS=1)"; return; }
	need B1 || return
	start_watch C3 rs
	local wpid=$WATCH_PID
	wait_synced C3 rs || return
	stop_pid "$N2_PID" TERM
	NODE_PIDS="$N1_PID $N3_PID"
	ensure_small srcc3
	run_one go C3.push -C "$W" -- store push --no-relay --local c3 --user interop srcc3 trees/c3
	expect_exit C3.push go 0
	wait_until 90 has_line "$CK/C3.rs.out" '^trees/c3	[0-9a-f]{64}	' ||
		note_fail "C3: the rs watch did not report trees/c3, pushed while n2 was down"
	(cd "$W" && exec "$GO" serve --store n2 --no-relay --loopback >>"$LOGS/n2.log" 2>&1) &
	N2_PID=$!
	track_pid "$N2_PID"
	NODE_PIDS="$N1_PID $N2_PID $N3_PID"
	wait_steady 600 || note_fail "C3: the cluster did not settle after n2 restarted"
	run_one go C3.push2 -C "$W" -- store push --no-relay --local c3 --user interop --force srcc3 trees/c3b
	wait_until 90 has_line "$CK/C3.rs.out" '^trees/c3b	[0-9a-f]{64}	' ||
		note_fail "C3: the rs watch did not report trees/c3b, pushed after n2 restarted"
	end_watch C3 rs "$wpid" INT
	expect_exit C3 rs 0
}

# ---- D: working copies across implementations ----

check_D1() {
	need B1 || return
	rm -rf "$WC/wcA" "$WC/wcB"
	run_one go D1 -C "$WC" -- clone --no-relay trees/rs wcA
	run_one rs D1 -C "$WC" -- clone --no-relay trees/rs wcB
	expect_exit D1 go 0 && expect_exit D1 rs 0 || return
	sed 's/ into wcA: / into DIR: /' "$CK/D1.go.out" >"$CK/D1.go.sub"
	sed 's/ into wcB: / into DIR: /' "$CK/D1.rs.out" >"$CK/D1.rs.sub"
	same "$CK/D1.go.sub" "$CK/D1.rs.sub" || diff_into_fail "$CK/D1.go.sub" "$CK/D1.rs.sub" "D1 clone lines (dir replaced), go vs rs:"
	expect_lines "$CK/D1.go.out" "^cloned trees/rs into wcA: root ${ROOT_RS:0:16}, [0-9]+ objects fetched \\([0-9]+ bytes\\)\$"
	local ka kb
	ka=$(tree_key "$WC/wcA" -exclude .dstore)
	kb=$(tree_key "$WC/wcB" -exclude .dstore)
	expect_eq "treekey wcA (go clone)" "$ka" "$ROOT_RS"
	expect_eq "treekey wcB (rs clone)" "$kb" "$ROOT_RS"
	diff -r -x .dstore -x fifo "$WC/wcA" "$WC/wcB" >"$CK/D1.diffr" 2>&1 ||
		{ note_fail "diff -r wcA wcB:"; head -n 10 "$CK/D1.diffr" | sed 's/^/  /' >>"$CK/$CUR.fail"; }
	[ -p "$WC/wcA/fifo" ] && [ -p "$WC/wcB/fifo" ] || note_fail "a clone did not restore the fifo"
}

check_D2() {
	need D1 || return
	same "$WC/wcA/.dstore/config" "$WC/wcB/.dstore/config" ||
		diff_into_fail "$WC/wcA/.dstore/config" "$WC/wcB/.dstore/config" "D2 .dstore/config, go (wcA) vs rs (wcB):"
	grep -v '"synced_at"' "$WC/wcA/.dstore/state" >"$CK/D2.go.state"
	grep -v '"synced_at"' "$WC/wcB/.dstore/state" >"$CK/D2.rs.state"
	same "$CK/D2.go.state" "$CK/D2.rs.state" ||
		diff_into_fail "$CK/D2.go.state" "$CK/D2.rs.state" "D2 .dstore/state without synced_at, go vs rs:"
	has_line "$WC/wcB/.dstore/state" '^  "synced_at": "[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9:]{8}(\.[0-9]*[1-9])?Z"$' ||
		note_fail "D2: the rs synced_at is not RFC3339Nano UTC: $(grep synced_at "$WC/wcB/.dstore/state")"
	has_line "$WC/wcB/.dstore/config" '^  "no_relay": true,?$' || note_fail "D2: the rs clone did not store no_relay"
}

check_D3() {
	need D1 || return
	pair D3 exact -C "$WC/wcA" -- status
	expect_lines "$CK/D3.rs.out" "^reference trees/rs, synced to ${ROOT_RS:0:16}\$" '^remote: up to date$' '^nothing to push$'
	pair D3.sub exact -C "$WC/wcA/deep/er" -- status
}

set_xattr() {
	if command -v xattr >/dev/null 2>&1; then
		xattr -w "$2" "$3" "$1" 2>/dev/null && return 0
	fi
	if command -v setfattr >/dev/null 2>&1; then
		setfattr -n "$2" -v "$3" "$1" 2>/dev/null && return 0
	fi
	return 1
}

check_D4() {
	need D3 || return
	local a=$WC/wcA
	printf 'changed\n' >>"$a/run.sh" &&
		chmod +x "$a/A-upper" &&
		rm "$a/one-byte" &&
		printf 'new\n' >"$a/added.txt" &&
		mkdir "$a/newdir" && printf 'inside\n' >"$a/newdir/in.txt" &&
		rm "$a/link" && ln -s A-upper "$a/link" &&
		rm "$a/empty" && mkdir "$a/empty" && printf 'x\n' >"$a/empty/y" &&
		rm -rf "$a/deep/er/and" && printf 'nowfile\n' >"$a/deep/er/and" &&
		touch -t 202201010000 "$a/constant.dat" &&
		printf '*.log\nsmall/s00001\n' >"$a/.amberignore" || { note_fail "D4: changing wcA failed"; return; }
	set_xattr "$a/xattr.txt" user.wc 1 || note_info "no user xattr on this filesystem: the xattr-only change is not covered"
	pair D4.status exact -C "$a" -- status
	expect_exit D4.status rs 0
	has_line "$CK/D4.status.go.out" '^  type      deep/er/and \(directory → file\)$' || note_fail "D4: go status lacks the dir→file change"
	pair D4.status-sub exact -C "$a/newdir" -- status
	pair D4.diff exact -C "$a" -- diff
	pair D4.stat exact -C "$a" -- diff --stat
	pair D4.path exact -C "$a" -- diff deep
	pair D4.paths exact -C "$a/deep" -- diff er ../run.sh
	pair D4.outside exact -C "$a" -- diff ../outside
	expect_lines "$(errfile D4.outside go)" '^dstore: \.\./outside is outside the working copy$'
	pair D4.remote exact -C "$a" -- diff --remote --stat
	pair D4.incoming exact -C "$a" -- diff --incoming
	pair D4.both exact -C "$a" -- diff --remote --incoming
}

ROOT_D5=""
check_D5() {
	need D4 || return
	local a=$WC/wcA
	run_one rs D5 -C "$a" -- push --user interop
	expect_exit D5 rs 0 || return
	ROOT_D5=$(tree_key "$a" -exclude .dstore)
	expect_lines "$CK/D5.rs.out" "^pushed trees/rs: root ${ROOT_D5:0:16}, [0-9]+ objects, [0-9]+ uploaded, version [0-9a-f]+\$"
	ROOT_RS=$ROOT_D5
	run_one go D5.status -C "$a" -- status
	expect_lines "$CK/D5.status.go.out" "^reference trees/rs, synced to ${ROOT_D5:0:16}\$" '^remote: up to date$' '^nothing to push$'
	pair D5.st exact -C "$a" -- status
	pair D5.again exact -C "$a" -- push --user interop
	expect_lines "$CK/D5.again.rs.out" '^nothing to push$'
}

check_D6() {
	need D5 || return
	rm -rf "$WC/d6-rs"
	cp -Rp "$WC/wcB" "$WC/d6-rs"
	run_one go D6.fetch -C "$WC/wcB" -- fetch
	run_one rs D6.fetch -C "$WC/d6-rs" -- fetch
	compare exact D6.fetch
	expect_lines "$CK/D6.fetch.go.out" "^fetched trees/rs: root ${ROOT_D5:0:16}, [0-9]+ objects fetched \\([0-9]+ bytes\\)\$"
	grep -v '"synced_at"' "$WC/wcB/.dstore/state" >"$CK/D6.go.state"
	grep -v '"synced_at"' "$WC/d6-rs/.dstore/state" >"$CK/D6.rs.state"
	same "$CK/D6.go.state" "$CK/D6.rs.state" || diff_into_fail "$CK/D6.go.state" "$CK/D6.rs.state" "D6 state after fetch, go vs rs:"
	same "$WC/wcB/.dstore/config" "$WC/d6-rs/.dstore/config" || note_fail "D6: the configs differ after fetch"
	pair D6.status exact -C "$WC/wcB" -- status
	has_line "$CK/D6.status.rs.out" '^remote: moved since your last fetch \(\+[0-9]+ ~[0-9]+ -[0-9]+; run pull\)$' ||
		note_fail "D6: rs status does not report the moved remote"
	pair D6.incoming exact -C "$WC/wcB" -- diff --incoming
	pair D6.incoming-stat exact -C "$WC/wcB" -- diff --incoming --stat
	pair D6.remote exact -C "$WC/wcB" -- diff --remote
	pair D6.again exact -C "$WC/d6-rs" -- fetch
	expect_lines "$CK/D6.again.rs.out" "^trees/rs: up to date \\(${ROOT_D5:0:16}\\)\$"
}

check_D7() {
	need D6 || return
	local a=$WC/wcA b=$WC/wcB
	# A Go-created copy that is behind, for the cross-implementation pulls below.
	rm -rf "$WC/wcC"
	run_one go D7.cloneC -C "$WC" -- clone --no-relay trees/rs wcC
	expect_exit D7.cloneC go 0
	# The cluster moves (a Go push from wcA) while wcB edits the same file.
	printf 'go side\n' >>"$a/run.sh"
	run_one go D7.gopush -C "$a" -- push --user interop
	expect_exit D7.gopush go 0 || return
	printf 'rust side\n' >>"$b/run.sh"
	rm -rf "$WC/d7-go" "$WC/d7-rs"
	cp -Rp "$b" "$WC/d7-go" && cp -Rp "$b" "$WC/d7-rs"
	# wcB fetched in D6 (remote != base): the push is refused before any upload (worktree ErrRemoteMoved).
	pair D7.push exact -C "$WC/d7-@IMPL@" -- push --user interop
	expect_lines "$(errfile D7.push go)" "^dstore: the cluster's tree moved since your last sync: pull first, or --force\$"
	# wcC never fetched (remote = base) while the reference moved: the reference write fails its CAS
	# (worktree ErrRefChanged).
	rm -rf "$WC/d7p-go" "$WC/d7p-rs"
	cp -Rp "$WC/wcC" "$WC/d7p-go" && cp -Rp "$WC/wcC" "$WC/d7p-rs"
	printf 'cas side\n' >"$WC/d7p-go/cas.txt" && printf 'cas side\n' >"$WC/d7p-rs/cas.txt"
	pair D7.pushcas exact -C "$WC/d7p-@IMPL@" -- push --user interop
	expect_lines "$(errfile D7.pushcas go)" '^dstore: reference changed on the cluster since your last fetch: pull first, or --force \(cas mismatch: current key [0-9a-f]{64}\)$'
	rm -rf "$WC/d7p-go" "$WC/d7p-rs"
	pair D7.status exact -C "$WC/d7-@IMPL@" -- status
	pair D7.pull exact -C "$WC/d7-@IMPL@" -- pull
	expect_exit D7.pull rs 1
	expect_lines "$(errfile D7.pull go)" '^conflicts:$' '^  run\.sh \(local: modified, cluster: modified\)$' \
		"^dstore: conflicting changes: resolve them, or --force to take the cluster's side\$"
	pair D7.force exact -C "$WC/d7-@IMPL@" -- pull --force
	expect_lines "$CK/D7.force.rs.out" '^pulled: [0-9]+ paths updated, 1 conflicts taken from the cluster$'
	pair D7.after exact -C "$WC/d7-@IMPL@" -- status
	pair D7.uptodate exact -C "$WC/d7-@IMPL@" -- pull
	expect_lines "$CK/D7.uptodate.rs.out" '^already up to date$'
	# rs pull in a Go-created copy, go pull in its twin.
	rm -rf "$WC/d7c-go" "$WC/d7c-rs"
	cp -Rp "$WC/wcC" "$WC/d7c-go" && cp -Rp "$WC/wcC" "$WC/d7c-rs"
	pair D7.crosspull exact -C "$WC/d7c-@IMPL@" -- pull
	expect_lines "$CK/D7.crosspull.rs.out" '^pulled: [0-9]+ paths updated$'
	pair D7.crossstatus exact -C "$WC/d7c-@IMPL@" -- status
	# go push in the Rust-created copy.
	printf 'pushed by go from a Rust-created copy\n' >"$WC/d7-go/from-go.txt"
	run_one go D7.gopush2 -C "$WC/d7-go" -- push --user interop
	local k
	k=$(tree_key "$WC/d7-go" -exclude .dstore)
	expect_exit D7.gopush2 go 0 &&
		expect_lines "$CK/D7.gopush2.go.out" "^pushed trees/rs: root ${k:0:16}, [0-9]+ objects, [0-9]+ uploaded, version [0-9a-f]+\$"
	ROOT_RS=$k
	pair D7.crossfetch exact -C "$WC/d7c-@IMPL@" -- fetch
}

check_D8() {
	local impl
	rm -rf "$WC/d8-go" "$WC/d8-rs"
	for impl in go rs; do
		mkdir -p "$WC/d8-$impl/sub" && printf 'a\n' >"$WC/d8-$impl/a.txt" && printf 'b\n' >"$WC/d8-$impl/sub/b.txt"
	done
	pair D8.init exact -C "$WC/d8-@IMPL@" -- init --no-relay trees/new
	expect_lines "$CK/D8.init.rs.out" '^initialised working copy of trees/new; the reference does not exist yet: push creates it$'
	same "$WC/d8-go/.dstore/config" "$WC/d8-rs/.dstore/config" ||
		diff_into_fail "$WC/d8-go/.dstore/config" "$WC/d8-rs/.dstore/config" "D8 .dstore/config after init, go vs rs:"
	pair D8.status exact -C "$WC/d8-rs" -- status
	expect_lines "$CK/D8.status.rs.out" "^reference trees/new, synced to ${EMPTY_TREE:0:16}\$" \
		'^remote: the reference does not exist on the cluster$' '^changes:$' '^  new       a\.txt$' '^  new       sub/$' '^  new       sub/b\.txt$'
	run_one go D8.push -C "$WC/d8-rs" -- push --user interop
	local k
	k=$(tree_key "$WC/d8-rs" -exclude .dstore)
	expect_exit D8.push go 0 &&
		expect_lines "$CK/D8.push.go.out" "^pushed trees/new: root ${k:0:16}, [0-9]+ objects, [0-9]+ uploaded, version [0-9a-f]+\$"
	pair D8.fetch exact -C "$WC/d8-rs" -- fetch
	expect_lines "$CK/D8.fetch.rs.out" "^trees/new: up to date \\(${k:0:16}\\)\$"
	# init of an existing reference
	rm -rf "$WC/d8e-go" "$WC/d8e-rs"
	mkdir -p "$WC/d8e-go" "$WC/d8e-rs"
	pair D8.existing exact -C "$WC/d8e-@IMPL@" -- init --no-relay trees/new
	expect_lines "$CK/D8.existing.rs.out" "^initialised working copy of trees/new; the reference exists \\(root ${k:0:16}\\): status shows everything as new, pull merges\$"
	pair D8.existing-pull exact -C "$WC/d8e-@IMPL@" -- pull
	rm -rf "$WC/d8-go" "$WC/d8e-go" "$WC/d8e-rs"
}

# d9_recover PUSHER STEP: in d8-rs, PUSHER pushes a change and loses the new state; then both clients,
# in twin copies, report the earlier push as completed.
d9_recover() {
	local pusher=$1 step=$2 d=$WC/d8-rs k
	cp "$d/.dstore/state" "$CK/$step.saved-state"
	printf '%s\n' "$step" >>"$d/a.txt"
	run_one "$pusher" "$step.push" -C "$d" -- push --user interop
	expect_exit "$step.push" "$pusher" 0 || return
	k=$(tree_key "$d" -exclude .dstore)
	cp "$CK/$step.saved-state" "$d/.dstore/state"
	rm -rf "$WC/$step-go" "$WC/$step-rs"
	cp -Rp "$d" "$WC/$step-go" && cp -Rp "$d" "$WC/$step-rs"
	pair "$step" exact -C "$WC/$step-@IMPL@" -- push --user interop
	expect_lines "$CK/$step.rs.out" "^trees/new already holds ${k:0:16} \\(an earlier push completed\\); state updated\$"
	pair "$step.status" exact -C "$WC/$step-@IMPL@" -- status
	# d8-rs continues from the recovered state.
	rm -rf "$d" && mv "$WC/$step-rs" "$d" && rm -rf "$WC/$step-go"
}

check_D9() {
	need D8 || return
	d9_recover rs D9.a
	d9_recover go D9.b
}

check_D10() {
	need D1 || return
	rm -rf "$WC/d10"
	cp -Rp "$WC/wcA" "$WC/d10" && rm "$WC/d10/.dstore/state"
	pair D10 exact -C "$WC/d10" -- status
	expect_lines "$(errfile D10 go)" '^dstore: incomplete clone: delete the directory and clone again$'
	pair D10.diff exact -C "$WC/d10" -- diff
	rm -rf "$WC/d10"
}

check_D11() {
	need D1 || return
	local d=$WC/d11 impl
	rm -rf "$d"
	mkdir -p "$d/full" && : >"$d/full/x" && : >"$d/afile"
	pair D11.full exact -C "$d" -- clone --no-relay trees/rs full
	expect_lines "$(errfile D11.full go)" '^dstore: full is not empty$'
	pair D11.file exact -C "$d" -- clone --no-relay trees/rs afile
	expect_lines "$(errfile D11.file go)" '^dstore: .*afile.* not a directory$'
	for impl in go rs; do
		run_one $impl D11.none -C "$d" -- clone --no-relay trees/none gone
		[ -e "$d/gone" ] && note_fail "the failed $impl clone of trees/none left $d/gone behind"
		rm -rf "$d/gone"
	done
	compare exact D11.none
	expect_lines "$(errfile D11.none go)" '^dstore: client: unknown reference: trees/none$'
	pair D11.inside exact -C "$WC/wcA/deep" -- init --no-relay trees/x
	expect_lines "$(errfile D11.inside go)" "^dstore: .* is inside the working copy at $WC/wcA\$"
	pair D11.badname exact -C "$d" -- clone --no-relay 'bad@name'
}

# d12_held HOLDER: HOLDER's holdlock on wcA, then status from both clients.
d12_held() {
	local holder=$1 pid
	"$BIN/holdlock-$holder" "$WC/wcA" 30 >"$CK/D12.$holder.hold" 2>&1 &
	pid=$!
	track_pid "$pid"
	if ! wait_until 15 has_line "$CK/D12.$holder.hold" '^locked '; then
		note_fail "D12: the $holder holdlock did not take the lock: $(cat "$CK/D12.$holder.hold")"
		stop_pid "$pid"
		return
	fi
	pair "D12.$holder-held" exact -C "$WC/wcA" -- status
	expect_exit "D12.$holder-held" go 1
	expect_exit "D12.$holder-held" rs 1
	expect_lines "$(errfile "D12.$holder-held" go)" "^dstore: packstore: .*/\\.dstore/packstore is already open: resource temporarily unavailable\$"
	stop_pid "$pid"
}

check_D12() {
	need D1 || return
	d12_held go
	d12_held rs
	pair D12.free exact -C "$WC/wcA" -- status
	expect_exit D12.free rs 0
}

# d13_refresh STEP: twin copies of wcA with the stale init ticket (n1 only) in .dstore/config; one fetch
# each rewrites the stored ticket from the view, identically.
d13_refresh() {
	local step=$1 impl
	for impl in go rs; do
		rm -rf "$WC/$step-$impl"
		cp -Rp "$WC/wcA" "$WC/$step-$impl"
		sed "s|^  \"ticket\": \"[^\"]*\"|  \"ticket\": \"$T\"|" "$WC/wcA/.dstore/config" >"$WC/$step-$impl/.dstore/config"
	done
	grep -q "\"ticket\": \"$T\"" "$WC/$step-go/.dstore/config" || { note_fail "$step: could not plant the stale ticket"; return; }
	pair "$step" exact -C "$WC/$step-@IMPL@" -- fetch
	expect_exit "$step" rs 0
	same "$WC/$step-go/.dstore/config" "$WC/$step-rs/.dstore/config" ||
		diff_into_fail "$WC/$step-go/.dstore/config" "$WC/$step-rs/.dstore/config" "$step .dstore/config after fetch, go vs rs:"
	grep -q "\"ticket\": \"$T\"" "$WC/$step-rs/.dstore/config" && note_fail "$step: rs fetch kept the stale ticket"
	rm -rf "$WC/$step-go" "$WC/$step-rs"
}

check_D13() {
	need D1 || return
	d13_refresh D13
}

# ---- E: CLI behaviour ----

check_E1() {
	need B1 || return
	local sig impl pids
	for sig in INT TERM; do
		pids=""
		for impl in go rs; do
			start_watch "E1.$sig" $impl
			pids="$pids $WATCH_PID"
		done
		for impl in go rs; do
			wait_synced "E1.$sig" $impl
		done
		set -- $pids
		end_watch "E1.$sig" go "$1" "$sig"
		end_watch "E1.$sig" rs "$2" "$sig"
		expect_exit "E1.$sig" go 0
		expect_exit "E1.$sig" rs 0
		sort "$CK/E1.$sig.go.out" >"$CK/E1.$sig.go.sorted"
		sort "$CK/E1.$sig.rs.out" >"$CK/E1.$sig.rs.sorted"
		same "$CK/E1.$sig.go.sorted" "$CK/E1.$sig.rs.sorted" ||
			diff_into_fail "$CK/E1.$sig.go.sorted" "$CK/E1.$sig.rs.sorted" "E1 SIG$sig watch output, go vs rs:"
	done
}

check_E2() {
	ensure_small srce2
	run_one go E2.warn -C "$W" DSTORE_LOG_LEVEL=warn -- store push --no-relay --local e2w-@IMPL@ --user interop srce2 trees/e2w-@IMPL@
	run_one rs E2.warn -C "$W" DSTORE_LOG_LEVEL=warn -- store push --no-relay --local e2w-@IMPL@ --user interop srce2 trees/e2w-@IMPL@
	local impl
	for impl in go rs; do
		expect_exit E2.warn $impl 0
		has_line "$CK/E2.warn.$impl.err" 'level=INFO' && note_fail "E2: $impl printed INFO lines with DSTORE_LOG_LEVEL=warn"
	done
	run_one go E2.info -C "$W" DSTORE_LOG_LEVEL=info -- store push --no-relay --local e2i-@IMPL@ --user interop srce2 trees/e2i-@IMPL@
	run_one rs E2.info -C "$W" DSTORE_LOG_LEVEL=info -- store push --no-relay --local e2i-@IMPL@ --user interop srce2 trees/e2i-@IMPL@
	for impl in go rs; do
		expect_exit E2.info $impl 0
		sed -n 's/^time=[^ ]* level=\([A-Z]*\) msg=\("[^"]*"\|[^ ]*\).*/\1 \2/p' "$CK/E2.info.$impl.err" | sort -u >"$CK/E2.info.$impl.msgs"
	done
	[ -s "$CK/E2.info.go.msgs" ] || note_fail "E2: go printed no INFO lines at DSTORE_LOG_LEVEL=info"
	same "$CK/E2.info.go.msgs" "$CK/E2.info.rs.msgs" ||
		diff_into_fail "$CK/E2.info.go.msgs" "$CK/E2.info.rs.msgs" "E2 the set of level/msg pairs of a push at info, go vs rs:"
	# the first INFO line of each: connected, with the same attribute keys
	for impl in go rs; do
		grep -m 1 'msg=connected' "$CK/E2.info.$impl.err" | sed 's/^time=[^ ]* //; s/=[^ ]*/=/g' >"$CK/E2.info.$impl.connected"
	done
	same "$CK/E2.info.go.connected" "$CK/E2.info.rs.connected" ||
		diff_into_fail "$CK/E2.info.go.connected" "$CK/E2.info.rs.connected" "E2 the attribute keys of msg=connected, go vs rs:"
}

check_E3() {
	[ "$BASELINE" = 1 ] && { skip_current "baseline run (Rust test binary)"; return; }
	command -v cargo >/dev/null 2>&1 || { skip_current "no cargo on PATH"; return; }
	[ -n "$MDNS_WORKS" ] || { [ "$MDNS" = skip ] && MDNS_WORKS=0; }
	# Every live_go_node_* test, at once (cargo's default test threads): the stamped ping, the view call, and
	# the slow-dialled-path cases of D9, which must stay on the dialled path with the vendored iroh patch
	# (third_party/README.md): live_go_node_stays_direct_behind_a_slow_path from the CLI's every-interface
	# endpoint, live_go_node_pool_stays_direct_behind_a_slow_path from a loopback-bound one
	# (interop-fix-3 in port-notes/impl-interop-fixes.md).
	local skip=""
	if [ "$MDNS_WORKS" = 0 ]; then
		skip="--skip live_go_node_view_call"
		note_info "mDNS is unavailable here (B13), so live_go_node_view_call (it dials by id) does not run"
	fi
	printf 'cd %q && DSTORE_LIVE_STORE=%q CARGO_TARGET_DIR=%q cargo test --release --locked --test iroh_loopback -- --ignored live_go_node %s --nocapture\n' \
		"$REPO" "$W/n1" "$CARGO_TD" "$skip" >"$CK/E3.cmd"
	# $skip is empty or two words.
	# shellcheck disable=SC2086
	(cd "$REPO" && DSTORE_LIVE_STORE=$W/n1 CARGO_TARGET_DIR=$CARGO_TD run_bounded 1500 "$CK/E3.out" "$CK/E3.err" \
		cargo test --release --locked --test iroh_loopback -- --ignored live_go_node $skip --nocapture)
	local rc=$?
	if [ $rc != 0 ]; then
		note_fail "E3: cargo test exited $rc: $(cat "$CK/E3.cmd")"
		# Rust prints a panic's location first and its message on the lines after it.
		{
			grep -h -A3 'panicked at' "$CK/E3.out" "$CK/E3.err" | grep -v '^--$'
			grep -h -E '^test .* FAILED$|^error' "$CK/E3.out" "$CK/E3.err"
		} | head -n 30 | sed 's/^/  /' >>"$CK/$CUR.fail"
		return
	fi
	grep -h 'test result:' "$CK/E3.out" | head -n 1 | sed 's/^/cargo: /' >>"$CK/$CUR.info"
	grep -hE '^(view reply|pong stamped|dial by id took|nat traversal)' "$CK/E3.out" "$CK/E3.err" | head -n 5 >>"$CK/$CUR.info"
}

# ---- G: documented exceptions ----

PEBBLE_REFUSAL='holds a Pebble database written by Go dstore; dstore-client-rs keeps local references in redb and cannot open it (use another --local directory)'
STORE_TICKET_TEXT="deriving a ticket from --store needs the node's Pebble meta store, which dstore-client-rs does not implement; pass --ticket or \$DSTORE_TICKET, or use the Go dstore binary"
RESTORE_TEXT="catalog restore writes through the node's paxos acceptor, which dstore-client-rs does not implement; use the Go dstore binary"

check_G1() {
	[ "$BASELINE" = 1 ] && { skip_current "baseline run (Rust-only texts)"; return; }
	need B2 || return
	run_one rs G1.pull -C "$W" -- store pull --no-relay --local goL trees/rs
	expect_exit G1.pull rs 1
	expect_eq "G1 rs store pull on Go refs" "$(last_dstore_line "$CK/G1.pull.rs.err")" "dstore: refstore: goL/refs $PEBBLE_REFUSAL"
	run_one rs G1.push -C "$W" -- store push --no-relay --local goL --user interop src1 trees/g1
	expect_exit G1.push rs 1
	expect_eq "G1 rs store push on Go refs" "$(last_dstore_line "$CK/G1.push.rs.err")" "dstore: refstore: goL/refs $PEBBLE_REFUSAL"
	# Go on a Rust-written directory opens a second (Pebble) database next to refs.redb (PORTING.md §2.3),
	# after which the Rust client refuses the directory.
	rm -rf "$W/g1r"
	cp -Rp "$W/rsL" "$W/g1r"
	[ -f "$W/g1r/refs/refs.redb" ] || note_fail "G1: rsL/refs has no refs.redb"
	run_one go G1.go -C "$W" -- store pull --no-relay --local g1r trees/rs
	expect_exit G1.go go 0
	ls "$W/g1r/refs" | grep -Eq '^(CURRENT|MANIFEST-.*|OPTIONS-.*)$' ||
		note_fail "G1: go store pull on Rust refs left no Pebble files in refs/: $(ls "$W/g1r/refs" | tr '\n' ' ')"
	run_one rs G1.after -C "$W" -- store pull --no-relay --local g1r trees/rs
	expect_exit G1.after rs 1
	expect_eq "G1 rs store pull after go" "$(last_dstore_line "$CK/G1.after.rs.err")" "dstore: refstore: g1r/refs $PEBBLE_REFUSAL"
	note_info "go store pull on a Rust-written --local directory succeeds (it adds Pebble files next to refs.redb): PORTING.md §2.3"
}

check_G2() {
	[ "$BASELINE" = 1 ] && { skip_current "baseline run (Rust-only texts)"; return; }
	run_one rs G2.ticket -C "$W" -u DSTORE_TICKET -- cluster ticket --no-relay --store n1
	expect_exit G2.ticket rs 1
	expect_eq "G2 rs cluster ticket --store n1" "$(last_dstore_line "$CK/G2.ticket.rs.err")" "dstore: $STORE_TICKET_TEXT"
	run_one rs G2.status -C "$W" -u DSTORE_TICKET -- cluster status --no-relay --store n1
	expect_exit G2.status rs 1
	expect_eq "G2 rs cluster status --store n1" "$(last_dstore_line "$CK/G2.status.rs.err")" "dstore: $STORE_TICKET_TEXT"
	printf 'not a catalog backup\n' >"$W/g2-backup.bin"
	run_one rs G2.restore -C "$W" -u DSTORE_TICKET -- catalog restore --no-relay --store n1 g2-backup.bin
	expect_exit G2.restore rs 1
	expect_eq "G2 rs catalog restore --store n1 FILE" "$(last_dstore_line "$CK/G2.restore.rs.err")" "dstore: $RESTORE_TEXT"
	run_one rs G2.restore-t -C "$W" -- catalog restore --no-relay --store n1 g2-backup.bin
	expect_eq "G2 rs catalog restore --store n1 FILE with a ticket" "$(last_dstore_line "$CK/G2.restore-t.rs.err")" "dstore: $RESTORE_TEXT"
	# A store without an identity fails the same way in Go: Go's text, compared.
	pair G2.noid exact -C "$W" -u DSTORE_TICKET -- cluster status --no-relay --store g2-nostore
	expect_lines "$(errfile G2.noid go)" '^dstore: node: no identity in g2-nostore: open g2-nostore/identity: no such file or directory$'
	[ -e "$W/g2-nostore" ] && note_fail "G2: a client created g2-nostore"
}

# ---- H: live cases handed over by the CLI snapshot gate and the L5 reviews ----

check_H1() {
	need B1 || return
	pair H1 panic -- cat --no-relay trees/rs /
	# DD-7: the Rust client writes only the panic's first line; a baseline run's "rs" is Go, with the dump.
	if [ "$BASELINE" = 1 ]; then
		head -n 1 "$CK/H1.rs.err" >"$CK/H1.rs.first"
	else
		cp "$CK/H1.rs.err" "$CK/H1.rs.first"
	fi
	expect_lines "$CK/H1.rs.first" '^panic: runtime error: invalid memory address or nil pointer dereference$'
	has_line "$CK/H1.go.err" '^goroutine 1 \[running\]:$' || note_fail "H1: the Go run printed no goroutine dump"
}

# sigpipe_cat IMPL: `cat NAME big.bin | head -c1`, with the exit status of cat.
sigpipe_cat() {
	local impl=$1 fifo=$CK/H2.$1.fifo cpid hpid rc
	rm -f "$fifo"
	mkfifo "$fifo" || { note_fail "mkfifo failed"; return; }
	printf 'dstore-%s cat --no-relay trees/rs big.bin | head -c1\n' "$impl" >"$CK/H2.$impl.cmd"
	"$(client_bin "$impl")" cat --no-relay trees/rs big.bin >"$fifo" 2>"$CK/H2.$impl.err" </dev/null &
	cpid=$!
	track_pid "$cpid"
	head -c 1 <"$fifo" >"$CK/H2.$impl.out" &
	hpid=$!
	track_pid "$hpid"
	wait_gone "$hpid" 60 || note_fail "H2: head -c1 did not finish"
	stop_pid "$hpid"
	wait_gone "$cpid" 60 || note_fail "H2: $impl cat still runs after its reader went away"
	[ -n "$(alive "$cpid" && echo y)" ] && kill -KILL "$cpid"
	reap "$cpid"
	rc=$?
	untrack_pid "$cpid"
	printf '%s\n' "$rc" >"$CK/H2.$impl.exit"
	rm -f "$fifo"
}

check_H2() {
	need B1 || return
	sigpipe_cat go
	sigpipe_cat rs
	compare exact H2
	expect_exit H2 go 141
	expect_exit H2 rs 141
	[ "$(wc -c <"$CK/H2.rs.out" | tr -d ' ')" = 1 ] || note_fail "H2: head read $(wc -c <"$CK/H2.rs.out") bytes"
}

# sigpipe_watch IMPL: `watch trees/** | head -1`; once head is gone a deletion makes watch write again.
sigpipe_watch() {
	local impl=$1 fifo=$CK/H3.$1.fifo wpid hpid rc
	ensure_small srch3
	run_one go "H3.mk-$impl" -C "$W" -- store push --no-relay --local h3 --user interop --force srch3 "trees/h3-$impl"
	rm -f "$fifo"
	mkfifo "$fifo" || { note_fail "mkfifo failed"; return; }
	printf 'dstore-%s watch --no-relay trees/** | head -1\n' "$impl" >"$CK/H3.$impl.cmd"
	"$(client_bin "$impl")" watch --no-relay 'trees/**' >"$fifo" 2>"$CK/H3.$impl.err" </dev/null &
	wpid=$!
	track_pid "$wpid"
	head -n 1 <"$fifo" >"$CK/H3.$impl.out" &
	hpid=$!
	track_pid "$hpid"
	wait_gone "$hpid" 60 || note_fail "H3: head -1 did not finish"
	stop_pid "$hpid"
	run_one go "H3.del-$impl" -- ref delete --no-relay "trees/h3-$impl"
	if ! wait_gone "$wpid" 60; then
		note_fail "H3: $impl watch still runs 60 s after its reader went away and a reference changed"
		kill -KILL "$wpid" 2>/dev/null
	fi
	reap "$wpid"
	rc=$?
	untrack_pid "$wpid"
	printf '%s\n' "$rc" >"$CK/H3.$impl.exit"
	rm -f "$fifo"
}

check_H3() {
	need B1 || return
	sigpipe_watch go
	sigpipe_watch rs
	compare exit H3
	expect_exit H3 go 141
	expect_exit H3 rs 141
	expect_lines "$CK/H3.rs.out" "$WATCH_LINE"
}

check_H4() {
	need B1 || return
	pair H4.pull stdout+exit -C "$W" -E /dev/null -u DSTORE_NO_TUI -- store pull --no-relay --local h4-@IMPL@ trees/rs
	# Go is the reference. Its TUI hands Bubble Tea stdin, which run_bounded makes /dev/null. On Linux,
	# Bubble Tea's epoll input reader refuses /dev/null, and both clients exit 1 before transferring
	# anything (port-notes/impl-interop-fixes.md). On macOS the TUI runs and the transfer succeeds.
	local go_pull go_clone
	go_pull=$(code H4.pull go)
	if [ "$go_pull" = 0 ]; then
		expect_lines "$CK/H4.pull.rs.out" "^pulled trees/rs: root [0-9a-f]{64}, [0-9]+ objects fetched \\([0-9]+ bytes\\)\$"
	else
		note_info "go exited $go_pull (Bubble Tea cannot read a non-pollable stdin here); rs must match"
	fi
	rm -rf "$WC/h4-go" "$WC/h4-rs"
	run_one go H4.clone -C "$WC" -E /dev/null -u DSTORE_NO_TUI -- clone --no-relay trees/rs h4-go
	run_one rs H4.clone -C "$WC" -E /dev/null -u DSTORE_NO_TUI -- clone --no-relay trees/rs h4-rs
	go_clone=$(code H4.clone go)
	expect_exit H4.clone rs "$go_clone"
	sed 's/ into h4-go: / into DIR: /' "$CK/H4.clone.go.out" >"$CK/H4.clone.go.sub"
	sed 's/ into h4-rs: / into DIR: /' "$CK/H4.clone.rs.out" >"$CK/H4.clone.rs.sub"
	same "$CK/H4.clone.go.sub" "$CK/H4.clone.rs.sub" ||
		diff_into_fail "$CK/H4.clone.go.sub" "$CK/H4.clone.rs.sub" "H4 clone line (dir replaced), go vs rs:"
	rm -rf "$WC/h4-go" "$WC/h4-rs"
	note_info "stderr is /dev/null, a character device: both clients take the TUI path (isTerminal)"
}

# h5_script TRANSCRIPT CMD...: runs CMD under script(1) (a pty), stdin /dev/null; returns its status. The
# pty gets a 100x40 window first: script copies the size of its own stdin, and a 0x0 window makes Bubble
# Tea render nothing.
h5_script() {
	local ts=$1
	shift
	set -- sh -c 'stty cols 100 rows 40 2>/dev/null; exec "$@"' sh "$@"
	if [ "$SCRIPT_KIND" = util-linux ]; then
		run_bounded 300 /dev/null /dev/null script -q -e -c "$(printf '%q ' "$@")" "$ts"
	else
		run_bounded 300 /dev/null /dev/null script -q "$ts" "$@"
	fi
}

# h5_classes TRANSCRIPT: the kinds of styling a TUI transcript uses, as four 0/1 flags: 24-bit colours
# (SGR 38;2 or 48;2), 256 colours (38;5 or 48;5), the green "done" line (SGR 32, a basic colour) and the
# bold title (SGR 1).
h5_classes() {
	local c24=0 c256=0 green=0 bold=0
	LC_ALL=C grep -qE "${ESC}\\[([0-9]*;)*[34]8;2;" "$1" && c24=1
	LC_ALL=C grep -qE "${ESC}\\[([0-9]*;)*[34]8;5;" "$1" && c256=1
	LC_ALL=C grep -qF "${ESC}[32mdone" "$1" && green=1
	LC_ALL=C grep -qF "${ESC}[1mpull " "$1" && bold=1
	printf '%s%s%s%s\n' "$c24" "$c256" "$green" "$bold"
}

check_H5() {
	command -v script >/dev/null 2>&1 || { skip_current "no script(1) for a pty"; return; }
	SCRIPT_KIND=bsd
	script --version 2>&1 | grep -q util-linux && SCRIPT_KIND=util-linux
	local name=trees/big-rs
	if ! ran B11 || ! passed B11; then
		need B1 || return
		name=trees/rs
	fi
	local mode impl ts rc envs go_cls rs_cls
	for mode in truecolor 256 nocolor dumb; do
		case $mode in
		truecolor) envs="COLORTERM=truecolor TERM=xterm-256color" ;;
		256) envs="TERM=xterm-256color" ;;
		nocolor) envs="NO_COLOR=1 TERM=xterm-256color" ;;
		dumb) envs="TERM=dumb" ;;
		esac
		for impl in go rs; do
			ts=$CK/H5.$mode.$impl.typescript
			rm -rf "$W/h5-$mode-$impl"
			printf 'cd %q && env -u DSTORE_NO_TUI %s script dstore-%s store pull --no-relay --local h5-%s-%s %s\n' \
				"$W" "$envs" "$impl" "$mode" "$impl" "$name" >"$CK/H5.$mode.$impl.cmd"
			# shellcheck disable=SC2086
			(cd "$W" && h5_script "$ts" env -u DSTORE_NO_TUI $envs "$(client_bin "$impl")" store pull --no-relay \
				--local "h5-$mode-$impl" "$name")
			rc=$?
			printf '%s\n' "$rc" >"$CK/H5.$mode.$impl.exit"
			[ "$rc" = 0 ] || note_fail "H5 $mode: $impl under script exited $rc"
			# The line can follow the renderer's last escape sequences on the same terminal line.
			strip_tty "$ts" | sed -n 's/.*\(pulled trees\/[^ ]*: root [0-9a-f]*, .*\)$/\1/p' >"$CK/H5.$mode.$impl.pulled"
		done
		same "$CK/H5.$mode.go.pulled" "$CK/H5.$mode.rs.pulled" && [ -s "$CK/H5.$mode.rs.pulled" ] ||
			diff_into_fail "$CK/H5.$mode.go.pulled" "$CK/H5.$mode.rs.pulled" "H5 $mode: the pulled line through the pty, go vs rs:"
		# Every mode draws TUI frames; under TERM=dumb they are plain text.
		for impl in go rs; do
			strip_tty "$CK/H5.$mode.$impl.typescript" | grep -q "pull $name" ||
				note_fail "H5 $mode: the $impl transcript has no TUI frame (title \"pull $name\"): the TUI path did not run"
		done
		# Bubble Tea downsamples to the colorprofile it detects. The bytes differ (DD-6), so compare which
		# kinds of styling each transcript uses; they must be the same.
		go_cls=$(h5_classes "$CK/H5.$mode.go.typescript")
		rs_cls=$(h5_classes "$CK/H5.$mode.rs.typescript")
		[ "$go_cls" = "$rs_cls" ] ||
			note_fail "H5 $mode: the styling differs from Go's (24-bit colour, 256 colours, green done line, bold title): go $go_cls, rs $rs_cls"
		note_info "$mode: 24-bit SGR go $(count_seq "$CK/H5.$mode.go.typescript" "${ESC}[38;2;") rs $(count_seq "$CK/H5.$mode.rs.typescript" "${ESC}[38;2;"), 256-colour SGR go $(count_seq "$CK/H5.$mode.go.typescript" "${ESC}[38;5;") rs $(count_seq "$CK/H5.$mode.rs.typescript" "${ESC}[38;5;"), frames go $(strip_tty "$CK/H5.$mode.go.typescript" | grep -c "pull $name") rs $(strip_tty "$CK/H5.$mode.rs.typescript" | grep -c "pull $name")"
		rm -rf "$W/h5-$mode-go" "$W/h5-$mode-rs"
	done
	note_info "terminal bytes are DD-6 (only UiModel::view() is byte-identical); the styling flags (24-bit, 256, green done, bold) must match Go's"
}

check_H6() {
	need D1 || return
	local impl
	for impl in go rs; do
		rm -rf "$WC/h6-$impl"
		cp -Rp "$WC/wcA" "$WC/h6-$impl" && printf 'h6\n' >"$WC/h6-$impl/h6.txt"
	done
	local bad
	bad=$(printf 'bad\001user')
	pair H6.user exact -C "$WC/h6-@IMPL@" -- push --user "$bad"
	expect_lines "$(errfile H6.user go)" '^dstore: user: user must not contain control characters$'
	# A cluster that cannot be reached fails the dial first, so the user is never checked.
	pair H6.nodial stdout+exit -C "$WC/h6-@IMPL@" -- push --no-discovery --ticket "$N1_ID" --user "$bad"
	expect_exit H6.nodial go 1
	case $(last_dstore_line "$CK/H6.nodial.go.err") in
	*user:*) note_fail "H6: go checked the user before dialing?" ;;
	esac
	case $(last_dstore_line "$CK/H6.nodial.rs.err") in
	*user:*) note_fail "H6: rs checked the user before dialing: $(last_dstore_line "$CK/H6.nodial.rs.err")" ;;
	esac
	rm -rf "$WC/h6-go" "$WC/h6-rs"
}

check_H7() {
	[ -n "$BACKUP_KEY" ] || need A8 || return
	pair H7.key exact -- catalog restore --no-relay "$BACKUP_KEY"
	expect_lines "$(errfile H7.key go)" '^dstore: restore runs on a voter: give --store$'
	pair H7.absent exact -- catalog restore --no-relay "$ZERO64"
	expect_lines "$(errfile H7.absent go)" '^dstore: backup object not found in the cluster$'
	pair H7.arg exact -C "$W" -- catalog restore --no-relay no-such-file
	expect_lines "$(errfile H7.arg go)" '^dstore: restore KEY\|FILE$'
}

check_H8() {
	skip_current "real nodes encode views canonically with a 16-byte cluster id, so a shorter id needs a hand-crafted view reply; DD-7/DD-15 are covered by the unit tests"
}

# ---- main ----

setup_cluster
CUR=""
want C1 && start_c1
for id in $ORDER; do
	want "$id" && run_check "$id"
done

say "----"
n() { set -- $1; printf '%s' $#; }
say "interop: $(n "$PASSED") passed, $(n "$FAILED") failed, $(n "$SKIPPED") skipped"
[ -n "$SKIPPED" ] && say "skipped:$SKIPPED"
if [ -n "$FAILED" ]; then
	say "failed:$FAILED"
	exit 1
fi
exit 0
