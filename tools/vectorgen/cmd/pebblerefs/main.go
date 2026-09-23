// Command pebblerefs makes DIR a Pebble reference store as Go dstore v0.1.10 and earlier left one in
// <local>/refs: github.com/cockroachdb/pebble/v2 opened as core v0.0.9's refstore.Open opened it, and closed
// again. Since core v0.0.10 the references are in refs.sqlite; Go imports such a directory on its first
// open, and dstore-client-rs refuses it until Go has (PORTING.md §2.3, DD-2). The interop check G1 runs both
// clients over the directory this makes; the clisnap fixture step pebble_refs makes the same one.
//
//	go run ./cmd/pebblerefs DIR
//
// It prints the directory's entries, one per line.
package main

import (
	"fmt"
	"os"
	"sort"

	"github.com/cockroachdb/pebble/v2"
)

// quiet silences Pebble's own logging, as core's refstore did.
type quiet struct{}

func (quiet) Infof(string, ...any)  {}
func (quiet) Errorf(string, ...any) {}
func (quiet) Fatalf(format string, args ...any) {
	panic(fmt.Sprintf("pebble fatal: "+format, args...))
}

func main() {
	if len(os.Args) != 2 {
		fmt.Fprintln(os.Stderr, "usage: pebblerefs DIR")
		os.Exit(2)
	}
	dir := os.Args[1]
	if err := os.MkdirAll(dir, 0o755); err != nil {
		fmt.Fprintln(os.Stderr, "pebblerefs:", err)
		os.Exit(1)
	}
	db, err := pebble.Open(dir, &pebble.Options{Logger: quiet{}})
	if err != nil {
		fmt.Fprintln(os.Stderr, "pebblerefs:", err)
		os.Exit(1)
	}
	if err := db.Close(); err != nil {
		fmt.Fprintln(os.Stderr, "pebblerefs:", err)
		os.Exit(1)
	}
	entries, err := os.ReadDir(dir)
	if err != nil {
		fmt.Fprintln(os.Stderr, "pebblerefs:", err)
		os.Exit(1)
	}
	names := make([]string, 0, len(entries))
	for _, e := range entries {
		names = append(names, e.Name())
	}
	sort.Strings(names)
	for _, n := range names {
		fmt.Println(n)
	}
}
