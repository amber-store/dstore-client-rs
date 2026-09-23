// Command treekey prints the root key github.com/amber-store/core v0.0.9 ingest computes for a directory
// (or a single regular file) without storing anything, as 64 lowercase hex digits. The interop checks
// compare it with the root a push reports.
//
//	go run ./cmd/treekey [-exclude NAME]... [-jobs N] [-no-ignore] PATH
//
// -exclude names an entry directly under the root that is not ingested (repeatable; a working copy
// needs -exclude .dstore, as worktree.Push does).
package main

import (
	"flag"
	"fmt"
	"os"
	"strings"

	"github.com/amber-store/core/ingest"
)

// names collects the repeated -exclude flag.
type names []string

func (n *names) String() string { return strings.Join(*n, ",") }

func (n *names) Set(s string) error {
	*n = append(*n, s)
	return nil
}

func main() {
	var exclude names
	flag.Var(&exclude, "exclude", "a name directly under the root that is not ingested (repeatable)")
	jobs := flag.Int("jobs", 0, "build workers (0 = GOMAXPROCS)")
	noIgnore := flag.Bool("no-ignore", false, "do not apply .amberignore files")
	flag.Usage = func() {
		fmt.Fprintln(flag.CommandLine.Output(), "usage: treekey [-exclude NAME]... [-jobs N] [-no-ignore] PATH")
		flag.PrintDefaults()
	}
	flag.Parse()
	if flag.NArg() != 1 {
		flag.Usage()
		os.Exit(2)
	}
	seq, root, err := ingest.Objects(flag.Arg(0), ingest.Opts{Jobs: *jobs, NoIgnore: *noIgnore, Exclude: exclude})
	if err != nil {
		fmt.Fprintln(os.Stderr, "treekey:", err)
		os.Exit(1)
	}
	for _, err := range seq {
		if err != nil {
			fmt.Fprintln(os.Stderr, "treekey:", err)
			os.Exit(1)
		}
	}
	fmt.Println(root.String())
}
