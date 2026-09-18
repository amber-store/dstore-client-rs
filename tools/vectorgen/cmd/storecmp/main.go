// Command storecmp checks that every object reachable from ROOT is present in two packstores with the
// same content. The interop checks use it after one implementation pulled a tree the other pushed.
//
//	go run ./cmd/storecmp [-strict] A B ROOT
//
// A and B are packstore directories (for example <local>/packstore or <wc>/.dstore/packstore); ROOT is a
// 64-hex tree key, and the walk reads A. For each object the records (header and stored payload) are
// compared byte for byte. Records travel verbatim, so a store filled by pulls holds the cluster's bytes;
// but an object the pusher's store already held under its own compression was never uploaded, and the
// cluster (so the puller) has the first uploader's bytes. Such a pair still has to decode to the same
// payload, and is reported as recompressed (PORTING.md DD-1: libzstd and klauspost frames differ). With
// -strict a recompressed object is a failure too.
//
// It prints "N objects: E byte-identical, R recompressed" and exits 0; a missing object, a payload
// difference, or with -strict any byte difference, is printed and the exit status is 1. Opening a store
// takes its flock, so neither may be open elsewhere.
package main

import (
	"bytes"
	"encoding/hex"
	"errors"
	"flag"
	"fmt"
	"os"

	"github.com/amber-store/core/fstree"
	"github.com/amber-store/core/key"
	"github.com/amber-store/core/packstore"
)

func run(dirA, dirB, rootHex string, strict bool) error {
	raw, err := hex.DecodeString(rootHex)
	if err != nil {
		return fmt.Errorf("root: %w", err)
	}
	root, err := key.Parse(raw)
	if err != nil {
		return fmt.Errorf("root: %w", err)
	}
	a, err := packstore.Open(dirA)
	if err != nil {
		return err
	}
	defer a.Close()
	b, err := packstore.Open(dirB)
	if err != nil {
		return err
	}
	defer b.Close()
	keys, err := fstree.ReachableKeys(root, a.Get)
	if err != nil {
		return fmt.Errorf("walking %s in %s: %w", root, dirA, err)
	}
	bad, identical, recompressed := 0, 0, 0
	for _, k := range keys {
		ra, err := a.GetRecord(k)
		if err != nil {
			fmt.Printf("%s: %s: %v\n", k, dirA, err)
			bad++
			continue
		}
		rb, err := b.GetRecord(k)
		switch {
		case errors.Is(err, packstore.ErrNotFound):
			fmt.Printf("%s: missing in %s\n", k, dirB)
			bad++
			continue
		case err != nil:
			fmt.Printf("%s: %s: %v\n", k, dirB, err)
			bad++
			continue
		case bytes.Equal(ra, rb):
			identical++
			continue
		}
		pa, errA := a.Get(k)
		pb, errB := b.Get(k)
		switch {
		case errA != nil || errB != nil:
			fmt.Printf("%s: records differ and a payload does not decode: %v / %v\n", k, errA, errB)
			bad++
		case !bytes.Equal(pa, pb):
			fmt.Printf("%s: payloads differ (%d vs %d bytes)\n", k, len(pa), len(pb))
			bad++
		default:
			recompressed++
			if strict {
				fmt.Printf("%s: same payload, records differ (%d vs %d bytes)\n", k, len(ra), len(rb))
				bad++
			}
		}
	}
	fmt.Printf("%d objects: %d byte-identical, %d recompressed\n", len(keys), identical, recompressed)
	if bad > 0 {
		return fmt.Errorf("%d of %d objects missing or different", bad, len(keys))
	}
	return nil
}

func main() {
	strict := flag.Bool("strict", false, "records must be byte-identical (both stores were filled from the cluster)")
	flag.Usage = func() {
		fmt.Fprintln(flag.CommandLine.Output(), "usage: storecmp [-strict] A B ROOT")
		flag.PrintDefaults()
	}
	flag.Parse()
	if flag.NArg() != 3 {
		flag.Usage()
		os.Exit(2)
	}
	if err := run(flag.Arg(0), flag.Arg(1), flag.Arg(2), *strict); err != nil {
		fmt.Fprintln(os.Stderr, "storecmp:", err)
		os.Exit(1)
	}
}
