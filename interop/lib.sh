# shellcheck shell=bash
# Helpers for interop/check.sh: running the Go and the Rust client side by side, comparing what they
# print, bounded waits without timeout(1), and the bookkeeping of processes and check results.
# Portable bash (3.2 and later) on macOS and Linux; no GNU-only flags.
#
# Globals the caller sets: W (work directory), CK (per-check records), GO_CLI and RS_CLI (the two
# clients), CMD_TIMEOUT (seconds one client command may run), SELECTED (the checks named on the command
# line), and check_desc ID (a check's description).

INTEROP_PIDS=""  # background processes the exit trap stops (space separated)
CUR=""           # the check being run
CUR_DESC=""
PASSED=""
FAILED=""
SKIPPED=""
DONE=""          # checks that have run, as selected or as another check's prerequisite
ESC=$(printf '\033')

say() { printf '%s\n' "$*"; }
die() {
	printf 'interop: %s\n' "$*" >&2
	exit 2
}

# ---- processes ----

track_pid() { INTEROP_PIDS="$INTEROP_PIDS $1"; }

untrack_pid() {
	local p rest=""
	for p in $INTEROP_PIDS; do
		[ "$p" = "$1" ] || rest="$rest $p"
	done
	INTEROP_PIDS=$rest
}

# alive PID: whether PID still runs.
alive() { kill -0 "$1" 2>/dev/null; }

# wait_gone PID SECS: polls until PID has exited (0) or SECS pass (1).
wait_gone() {
	local start=$SECONDS
	while alive "$1"; do
		[ $((SECONDS - start)) -ge "$2" ] && return 1
		sleep 0.1
	done
	return 0
}

# reap PID: the exit status of the child PID (128+N when signal N ended it); PID must have been started
# by this shell.
reap() {
	wait "$1" 2>/dev/null
}

# stop_pid PID [SIG]: sends SIG (default TERM), waits up to 10 s, then KILL; reaps it.
stop_pid() {
	local pid=$1 sig=${2:-TERM}
	if alive "$pid"; then
		kill "-$sig" "$pid" 2>/dev/null
		wait_gone "$pid" 10 || kill -KILL "$pid" 2>/dev/null
	fi
	wait "$pid" 2>/dev/null
	untrack_pid "$pid"
}

stop_all() {
	local p
	for p in $INTEROP_PIDS; do
		alive "$p" && kill -TERM "$p" 2>/dev/null
	done
	for p in $INTEROP_PIDS; do
		wait_gone "$p" 15 || kill -KILL "$p" 2>/dev/null
		wait "$p" 2>/dev/null
	done
	INTEROP_PIDS=""
}

# wait_until SECS CMD...: runs CMD every 0.2 s until it succeeds (0) or SECS pass (1).
wait_until() {
	local secs=$1 start=$SECONDS
	shift
	while :; do
		"$@" && return 0
		[ $((SECONDS - start)) -ge "$secs" ] && return 1
		sleep 0.2
	done
}

# has_line FILE REGEX: FILE exists and a line matches the extended REGEX.
has_line() { [ -f "$1" ] && grep -Eq -- "$2" "$1"; }

# run_bounded SECS OUT ERR CMD...: CMD with stdin from /dev/null, stdout to OUT and stderr to ERR; after SECS
# it is killed and 124 is returned. Otherwise CMD's exit status (128+N for signal N).
run_bounded() {
	local secs=$1 out=$2 err=$3 pid start=$SECONDS
	shift 3
	"$@" >"$out" 2>"$err" </dev/null &
	pid=$!
	while alive "$pid"; do
		if [ $((SECONDS - start)) -ge "$secs" ]; then
			kill -KILL "$pid" 2>/dev/null
			wait "$pid" 2>/dev/null
			[ "$err" = /dev/null ] || printf 'interop: killed after %ss\n' "$secs" >>"$err"
			return 124
		fi
		sleep 0.05
	done
	wait "$pid"
}

# ---- one client run ----

# client_bin go|rs
client_bin() {
	case $1 in
	go) printf '%s' "$GO_CLI" ;;
	rs) printf '%s' "$RS_CLI" ;;
	*) die "unknown client $1" ;;
	esac
}

# run_one IMPL STEP [-C DIR] [-E ERRFILE] [NAME=VALUE | -u NAME]... -- ARGS...
# Runs one client (IMPL go or rs) in DIR, with every @IMPL@ in DIR, the environment items and ARGS replaced
# by IMPL. Records $CK/STEP.IMPL.{cmd,out,err,exit}; with -E, stderr goes to ERRFILE (for example /dev/null)
# and STEP.IMPL.err stays empty. Returns the client's exit status.
run_one() {
	local impl=$1 step=$2 dir="" errto="" bin base rc a
	shift 2
	local envs=() args=()
	while :; do
		case $1 in
		-C)
			dir=${2//@IMPL@/$impl}
			shift 2
			;;
		-E)
			errto=$2
			shift 2
			;;
		*) break ;;
		esac
	done
	while [ $# -gt 0 ] && [ "$1" != -- ]; do
		envs+=("${1//@IMPL@/$impl}")
		shift
	done
	[ $# -gt 0 ] && shift
	for a in "$@"; do
		args+=("${a//@IMPL@/$impl}")
	done
	bin=$(client_bin "$impl")
	base="$CK/$step.$impl"
	: >"$base.err"
	{
		[ -n "$dir" ] && printf 'cd %q && ' "$dir"
		printf '%q ' env ${envs[@]+"${envs[@]}"} "dstore-$impl" ${args[@]+"${args[@]}"}
		[ -n "$errto" ] && printf '2>%q' "$errto"
		printf '\n'
	} >"$base.cmd"
	[ -n "$errto" ] || errto=$base.err
	local attempt=1
	while :; do
		if [ -n "$dir" ]; then
			(cd "$dir" && run_bounded "$CMD_TIMEOUT" "$base.out" "$errto" env ${envs[@]+"${envs[@]}"} "$bin" ${args[@]+"${args[@]}"})
		else
			run_bounded "$CMD_TIMEOUT" "$base.out" "$errto" env ${envs[@]+"${envs[@]}"} "$bin" ${args[@]+"${args[@]}"}
		fi
		rc=$?
		# A node that briefly lost its catalog quorum answers `unavailable: paxos: no quorum`; the Go client
		# hits it as well (baseline runs), so it is the cluster's state, not the client's: run again.
		if [ $rc != 0 ] && [ $attempt -lt 3 ] && last_dstore_line "$base.err" | grep -q '^dstore: remote: unavailable: paxos: no quorum'; then
			[ -n "$CUR" ] && note_info "$step: $impl run $attempt hit the node-side 'paxos: no quorum'; ran again"
			attempt=$((attempt + 1))
			sleep 3
			continue
		fi
		break
	done
	printf '%s\n' "$rc" >"$base.exit"
	return $rc
}

# pair STEP MODE [run_one options] -- ARGS...: the Go client, then the Rust client, with the same arguments;
# then compare MODE STEP.
pair() {
	local step=$1 mode=$2
	shift 2
	run_one go "$step" "$@"
	run_one rs "$step" "$@"
	compare "$mode" "$step"
}

# pair_retry TRIES STEP MODE [run_one options] -- ARGS...: pair until both runs agree, at most TRIES times,
# 3 s apart, for outputs the cluster can change between two runs; the last difference is recorded.
pair_retry() {
	local tries=$1 step=$2 mode=$3 i=1
	shift 3
	while :; do
		run_one go "$step" "$@"
		run_one rs "$step" "$@"
		if compare "$mode" "$step" quiet; then
			[ $i -gt 1 ] && note_info "$step: equal on attempt $i (the cluster moved between two runs)"
			return 0
		fi
		if [ $i -ge "$tries" ]; then
			compare "$mode" "$step"
			return 1
		fi
		i=$((i + 1))
		sleep 3
	done
}

# out STEP IMPL / err STEP IMPL / code STEP IMPL: what a run printed and its exit status.
out() { cat "$CK/$1.$2.out" 2>/dev/null; }
err() { cat "$CK/$1.$2.err" 2>/dev/null; }
code() { cat "$CK/$1.$2.exit" 2>/dev/null; }

# errfile STEP IMPL: the path of the run's stderr without slog lines (it writes STEP.IMPL.noslog).
errfile() {
	grep -v '^time=' "$CK/$1.$2.err" >"$CK/$1.$2.noslog" 2>/dev/null
	printf '%s' "$CK/$1.$2.noslog"
}

# last_dstore_line FILE: the last stderr line starting with "dstore: " (the error), or nothing.
last_dstore_line() { grep '^dstore: ' "$1" 2>/dev/null | tail -n 1; }

# norm_err: stderr with the time= field of slog lines removed (level, message and attributes are kept).
norm_err() { sed 's/^time=[^ ]* //' "$1" 2>/dev/null; }

# ---- results ----

note_fail() { printf '%s\n' "$*" >>"$CK/$CUR.fail"; }
note_info() { printf '%s\n' "$*" >>"$CK/$CUR.info"; }
skip_current() { printf '%s\n' "$*" >"$CK/$CUR.skip"; }

# diff_into_fail A B LABEL: a unified diff of two files into the failure record (at most 40 lines).
diff_into_fail() {
	{
		printf '%s\n' "$3"
		diff -u "$1" "$2" | head -n 40 | sed 's/^/  /'
	} >>"$CK/$CUR.fail"
}

same() { cmp -s "$1" "$2"; }

# compare MODE STEP [quiet]: 0 when the Go and the Rust run of STEP agree under MODE, else 1 and, unless
# quiet, the difference is recorded as a failure of the current check.
#   exact        stdout and exit status identical, and stderr identical up to the slog time= fields
#   stdout+exit  stdout and exit status identical, and the last "dstore: " stderr line
#   normalized   norm_status of stdout identical, the exit status and the last "dstore: " stderr line
#   panic        DD-7: exit 2 for both, stdout identical, the Rust stderr = the first line of Go's
#   exit         the exit status only
compare() {
	local mode=$1 step=$2 quiet=${3:-} ok=0
	local g="$CK/$step.go" r="$CK/$step.rs"
	case $mode in
	exact)
		norm_err "$g.err" >"$g.errnorm"
		norm_err "$r.err" >"$r.errnorm"
		same "$g.out" "$r.out" && same "$g.errnorm" "$r.errnorm" && same "$g.exit" "$r.exit" || ok=1
		;;
	stdout+exit)
		last_dstore_line "$g.err" >"$g.errline"
		last_dstore_line "$r.err" >"$r.errline"
		same "$g.out" "$r.out" && same "$g.exit" "$r.exit" && same "$g.errline" "$r.errline" || ok=1
		;;
	normalized)
		norm_status <"$g.out" >"$g.norm"
		norm_status <"$r.out" >"$r.norm"
		last_dstore_line "$g.err" >"$g.errline"
		last_dstore_line "$r.err" >"$r.errline"
		same "$g.norm" "$r.norm" && same "$g.exit" "$r.exit" && same "$g.errline" "$r.errline" || ok=1
		;;
	panic)
		head -n 1 "$g.err" >"$g.errline"
		if [ "${BASELINE:-0}" = 1 ]; then
			head -n 1 "$r.err" >"$r.errline" # both are Go: the same first line, then the dump
		else
			cp "$r.err" "$r.errline"
		fi
		[ "$(cat "$g.exit")" = 2 ] && [ "$(cat "$r.exit")" = 2 ] && same "$g.out" "$r.out" &&
			same "$g.errline" "$r.errline" || ok=1
		;;
	exit)
		same "$g.exit" "$r.exit" || ok=1
		;;
	*) die "unknown comparison mode $mode" ;;
	esac
	[ $ok = 0 ] && return 0
	[ -n "$quiet" ] && return 1
	note_fail "$step ($mode): go and rs differ (exit go $(cat "$g.exit"), rs $(cat "$r.exit"))"
	note_fail "  $(cat "$r.cmd")"
	case $mode in
	normalized) same "$g.norm" "$r.norm" || diff_into_fail "$g.norm" "$r.norm" "  stdout (normalized), go vs rs:" ;;
	*) same "$g.out" "$r.out" || diff_into_fail "$g.out" "$r.out" "  stdout, go vs rs:" ;;
	esac
	case $mode in
	exact) same "$g.errnorm" "$r.errnorm" || diff_into_fail "$g.errnorm" "$r.errnorm" "  stderr, go vs rs:" ;;
	stdout+exit | normalized | panic) same "$g.errline" "$r.errline" || diff_into_fail "$g.errline" "$r.errline" "  error line, go vs rs:" ;;
	esac
	return 1
}

# expect_exit STEP IMPL CODE: the run exited with CODE.
expect_exit() {
	local got
	got=$(code "$1" "$2")
	[ "$got" = "$3" ] && return 0
	note_fail "$1: $2 exited $got, want $3: $(cat "$CK/$1.$2.cmd")"
	tail -n 5 "$CK/$1.$2.err" | sed 's/^/  stderr: /' >>"$CK/$CUR.fail"
	return 1
}

# expect_match WHAT TEXT REGEX: TEXT matches the extended REGEX.
expect_match() {
	printf '%s\n' "$2" | grep -Eq -- "$3" && return 0
	note_fail "$1: $(printf '%q' "$2") does not match /$3/"
	return 1
}

# expect_eq WHAT GOT WANT
expect_eq() {
	[ "$2" = "$3" ] && return 0
	note_fail "$1: got $(printf '%q' "$2")"
	note_fail "  want $(printf '%q' "$3")"
	return 1
}

# expect_lines FILE REGEX...: FILE has exactly one line per REGEX, each matching its REGEX.
expect_lines() {
	local f=$1 n=0 line re
	shift
	local want=$#
	local res=("$@")
	while IFS= read -r line || [ -n "$line" ]; do
		n=$((n + 1))
		[ $n -gt "$want" ] && continue
		re=${res[$((n - 1))]}
		printf '%s\n' "$line" | grep -Eq -- "$re" && continue
		note_fail "$(basename "$f") line $n: $(printf '%q' "$line") does not match /$re/"
		return 1
	done <"$f"
	[ $n = "$want" ] && return 0
	note_fail "$(basename "$f"): $n lines, want $want"
	sed 's/^/  | /' "$f" | head -n 12 >>"$CK/$CUR.fail"
	return 1
}

# ---- normalisation ----

# norm_status: `cluster status` with the counters of the "      epoch " and "      voter " lines masked
# (digit runs become N) and each run of consecutive "      voter " lines sorted (the node iterates a map).
norm_status() {
	awk '
	function flush(   i, j, v) {
		for (i = 2; i <= n; i++) {
			v = buf[i]
			for (j = i - 1; j >= 1 && buf[j] > v; j--) buf[j + 1] = buf[j]
			buf[j + 1] = v
		}
		for (i = 1; i <= n; i++) print buf[i]
		n = 0
	}
	/^      voter / { line = $0; gsub(/[0-9]+/, "N", line); buf[++n] = line; next }
	{ flush() }
	/^      epoch / { line = $0; gsub(/[0-9]+/, "N", line); print line; next }
	{ print }
	END { flush() }
	'
}

# strip_tty FILE: a pty transcript without escape sequences, carriage returns and other control bytes.
# A CSI sequence is ESC [, parameter bytes 0x30-0x3f, intermediate bytes 0x20-0x2f and one final byte
# 0x40-0x7e (ECMA-48 5.4). The intermediate class must stop at '/': a class up to '\' would also eat the
# first letter after a final byte such as the J of ESC [ J.
strip_tty() {
	LC_ALL=C sed -e "s#${ESC}\\[[0-?]*[ -/]*[@-~]##g" -e "s#${ESC}[()][A-Z0-9]##g" -e "s#${ESC}[=>78]##g" "$1" 2>/dev/null |
		LC_ALL=C tr -d '\000-\010\013-\037'
}

# count_seq FILE TEXT: how many times the fixed TEXT occurs in FILE.
count_seq() { LC_ALL=C grep -oF -- "$2" "$1" 2>/dev/null | wc -l | tr -d ' '; }

# ---- check registry and selection ----

# want ID: whether ID was selected on the command line (every check when nothing was); a letter selects
# its group.
want() {
	[ -z "$SELECTED" ] && return 0
	case " $SELECTED " in
	*" $1 "* | *" ${1%%[0-9]*} "*) return 0 ;;
	esac
	return 1
}

ran() {
	case " $DONE " in *" $1 "*) return 0 ;; esac
	return 1
}

passed() {
	case " $PASSED " in *" $1 "*) return 0 ;; esac
	return 1
}

# run_check ID: runs check_ID as check ID and prints its PASS, FAIL or SKIP line.
run_check() {
	local id=$1 saved_cur=$CUR saved_desc=$CUR_DESC start took desc
	ran "$id" && return 0
	DONE="$DONE $id"
	desc=$(check_desc "$id")
	CUR=$id
	CUR_DESC=$desc
	rm -f "$CK/$id.fail" "$CK/$id.skip" "$CK/$id.info"
	start=$SECONDS
	"check_$id"
	took=$((SECONDS - start))
	if [ -s "$CK/$id.skip" ]; then
		printf 'SKIP %-4s %s: %s\n' "$id" "$desc" "$(head -n 1 "$CK/$id.skip")"
		SKIPPED="$SKIPPED $id"
	elif [ -s "$CK/$id.fail" ]; then
		printf 'FAIL %-4s %s (%ss)\n' "$id" "$desc" "$took"
		sed 's/^/       /' "$CK/$id.fail" | head -n 100
		FAILED="$FAILED $id"
	else
		printf 'PASS %-4s %s (%ss)\n' "$id" "$desc" "$took"
		PASSED="$PASSED $id"
	fi
	[ -s "$CK/$id.info" ] && sed 's/^/       note: /' "$CK/$id.info"
	CUR=$saved_cur
	CUR_DESC=$saved_desc
	if [ -s "$CK/$id.fail" ] && [ ! -s "$CK/$id.skip" ] && [ "${INTEROP_FAILFAST:-0}" = 1 ]; then
		exit 1
	fi
	return 0
}

# need ID: runs check ID first (printing its line) when it has not run yet; returns 1, and records that
# on the current check, when ID did not pass.
need() {
	ran "$1" || run_check "$1"
	passed "$1" && return 0
	note_fail "prerequisite $1 did not pass"
	return 1
}
