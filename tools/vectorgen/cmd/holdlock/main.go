// Command holdlock opens the working copy DIR with github.com/amber-store/dstore v0.1.11 worktree.Open,
// which takes its lock (.dstore/lock, an exclusive flock), and keeps it open for SECONDS. The lock-interop
// check (D12) runs a Rust command against a working copy while this Go process holds it;
// examples/holdlock.rs is the Rust twin. Until core v0.0.10 the packstore's single-owner lock kept two
// commands apart, and this helper opened the packstore; a packstore is shared now.
//
//	go run ./cmd/holdlock DIR SECONDS
//
// Once the working copy is open it prints "locked <root>" on stdout.
package main

import (
	"fmt"
	"os"
	"strconv"
	"time"

	"github.com/amber-store/dstore/worktree"
)

func main() {
	if len(os.Args) != 3 {
		fmt.Fprintln(os.Stderr, "usage: holdlock DIR SECONDS")
		os.Exit(2)
	}
	secs, err := strconv.ParseUint(os.Args[2], 10, 32)
	if err != nil {
		fmt.Fprintln(os.Stderr, "holdlock: SECONDS must be a non-negative integer")
		os.Exit(2)
	}
	tr, err := worktree.Open(os.Args[1])
	if err != nil {
		fmt.Fprintln(os.Stderr, "holdlock:", err)
		os.Exit(1)
	}
	fmt.Println("locked", tr.Root)
	time.Sleep(time.Duration(secs) * time.Second)
	if err := tr.Close(); err != nil {
		fmt.Fprintln(os.Stderr, "holdlock:", err)
		os.Exit(1)
	}
}
