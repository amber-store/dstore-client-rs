// Command holdlock opens DIR/.dstore/packstore with github.com/amber-store/core v0.0.9 packstore, which
// takes the directory's flock, and holds it for SECONDS. The lock-interop check (D12) runs a Rust command
// against a working copy while this Go process holds its store; examples/holdlock.rs is the Rust twin.
//
//	go run ./cmd/holdlock DIR SECONDS
//
// Once the lock is held it prints "locked <path>" on stdout.
package main

import (
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"time"

	"github.com/amber-store/core/packstore"
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
	path := filepath.Join(os.Args[1], ".dstore", "packstore")
	st, err := packstore.Open(path)
	if err != nil {
		fmt.Fprintln(os.Stderr, "holdlock:", err)
		os.Exit(1)
	}
	fmt.Println("locked", path)
	time.Sleep(time.Duration(secs) * time.Second)
	if err := st.Close(); err != nil {
		fmt.Fprintln(os.Stderr, "holdlock:", err)
		os.Exit(1)
	}
}
