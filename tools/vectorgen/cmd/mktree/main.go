// Command mktree writes a deterministic source tree for the interop checks (port-notes/verification.md
// §4.2): splitmix64 files of 0 B, 1 B and 5 MiB (several chunks), 300 KiB of a constant byte
// (compressible), unicode names, directories nested four deep, 2000 small files, a symlink, a fifo, a
// 0o755 script, an .amberignore with one ignored file, and a best-effort user.* xattr. Every entry gets a
// fixed mode and mtime; directories are dated after their contents.
//
//	go run ./cmd/mktree [-seed S] [-small N] DIR
//
// DIR must not exist or must be empty. Contents depend only on the seed; ownership is the caller's, so
// the tree key is the same for two tools on one machine, not across machines.
package main

import (
	"bytes"
	"encoding/binary"
	"errors"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"time"

	"golang.org/x/sys/unix"
)

// next advances splitmix64 exactly as VECTORS.md defines it.
func next(state *uint64) uint64 {
	*state += 0x9E3779B97F4A7C15
	z := *state
	z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9
	z = (z ^ (z >> 27)) * 0x94D049BB133111EB
	return z ^ (z >> 31)
}

// data is data(seed, n) of VECTORS.md: splitmix64 outputs as 8 little-endian bytes, truncated to n.
func data(seed uint64, n int) []byte {
	out := make([]byte, 0, n+8)
	state := seed
	for len(out) < n {
		out = binary.LittleEndian.AppendUint64(out, next(&state))
	}
	return out[:n]
}

// baseMtime is the first mtime handed out (2020-09-13T12:26:40Z); each entry gets the next second.
const baseMtime = 1_600_000_000

type builder struct {
	root  string
	seed  uint64
	clock int64
	dirs  []string // in creation order; dated last, deepest first
}

// stamp gives path the next fixed mtime (atime too), without following a symlink.
func (b *builder) stamp(path string) error {
	b.clock++
	ts := unix.NsecToTimespec(time.Unix(baseMtime+b.clock, b.clock%1_000_000_000).UnixNano())
	return unix.UtimesNanoAt(unix.AT_FDCWD, path, []unix.Timespec{ts, ts}, unix.AT_SYMLINK_NOFOLLOW)
}

func (b *builder) dir(rel string) error {
	p := filepath.Join(b.root, filepath.FromSlash(rel))
	if err := os.Mkdir(p, 0o700); err != nil {
		return err
	}
	if err := unix.Chmod(p, 0o755); err != nil {
		return err
	}
	b.dirs = append(b.dirs, p)
	return nil
}

func (b *builder) file(rel string, content []byte, perm uint32) error {
	p := filepath.Join(b.root, filepath.FromSlash(rel))
	if err := os.WriteFile(p, content, 0o600); err != nil {
		return err
	}
	if err := unix.Chmod(p, perm); err != nil {
		return err
	}
	return b.stamp(p)
}

func run(root string, seed uint64, small int) error {
	switch ents, err := os.ReadDir(root); {
	case errors.Is(err, os.ErrNotExist):
		if err := os.MkdirAll(root, 0o755); err != nil {
			return err
		}
	case err != nil:
		return err
	case len(ents) > 0:
		return fmt.Errorf("%s is not empty", root)
	}
	b := &builder{root: root, seed: seed}
	s := func(i uint64) uint64 { return seed*1_000_003 + i }
	steps := []func() error{
		func() error { return b.file("empty", nil, 0o644) },
		func() error { return b.file("one-byte", data(s(1), 1), 0o644) },
		func() error { return b.file("big.bin", data(s(2), 5<<20), 0o644) },
		func() error { return b.file("constant.dat", bytes.Repeat([]byte{0xaa}, 300<<10), 0o644) },
		func() error { return b.file("run.sh", []byte("#!/bin/sh\necho hello\n"), 0o755) },
		func() error { return b.file("é-unicode ☃.txt", data(s(3), 100), 0o644) },
		func() error { return b.file("A-upper", data(s(4), 10), 0o600) },
		func() error { return b.file(".amberignore", []byte("*.log\n"), 0o644) },
		func() error { return b.file("ignored.log", []byte("not ingested\n"), 0o644) },
		func() error {
			p := filepath.Join(b.root, "link")
			if err := os.Symlink("run.sh", p); err != nil {
				return err
			}
			return b.stamp(p)
		},
		func() error {
			p := filepath.Join(b.root, "fifo")
			if err := unix.Mkfifo(p, 0o600); err != nil {
				return err
			}
			if err := unix.Chmod(p, 0o644); err != nil {
				return err
			}
			return b.stamp(p)
		},
		func() error {
			p := filepath.Join(b.root, "xattr.txt")
			if err := b.file("xattr.txt", data(s(5), 64), 0o644); err != nil {
				return err
			}
			// Best effort: filesystems without user xattrs (or policies refusing them) are fine.
			_ = unix.Setxattr(p, "user.mktree", []byte("1"), 0)
			return b.stamp(p)
		},
		func() error {
			rel := ""
			for i, name := range []string{"deep", "er", "and", "deeper"} {
				if rel == "" {
					rel = name
				} else {
					rel += "/" + name
				}
				if err := b.dir(rel); err != nil {
					return err
				}
				if err := b.file(rel+"/f", data(s(uint64(10+i)), 20+i), 0o644); err != nil {
					return err
				}
			}
			return nil
		},
		func() error {
			if err := b.dir("small"); err != nil {
				return err
			}
			for i := 0; i < small; i++ {
				if err := b.file(fmt.Sprintf("small/s%05d", i), data(s(uint64(1000+i)), i%97), 0o644); err != nil {
					return err
				}
			}
			return nil
		},
	}
	for _, step := range steps {
		if err := step(); err != nil {
			return err
		}
	}
	for i := len(b.dirs) - 1; i >= 0; i-- {
		if err := b.stamp(b.dirs[i]); err != nil {
			return err
		}
	}
	return nil
}

func main() {
	seed := flag.Uint64("seed", 1, "splitmix64 seed of every file's content")
	small := flag.Int("small", 2000, "number of small files under small/")
	flag.Usage = func() {
		fmt.Fprintln(flag.CommandLine.Output(), "usage: mktree [-seed S] [-small N] DIR")
		flag.PrintDefaults()
	}
	flag.Parse()
	if flag.NArg() != 1 || *small < 0 {
		flag.Usage()
		os.Exit(2)
	}
	if err := run(flag.Arg(0), *seed, *small); err != nil {
		fmt.Fprintln(os.Stderr, "mktree:", err)
		os.Exit(1)
	}
}
