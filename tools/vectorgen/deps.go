package main

// Blank imports of every package the generators and helper programs will use,
// so that go mod tidy keeps their modules (and go.sum) before the families that
// import them land. Remove an import here only when no family needs it.
import (
	_ "charm.land/bubbles/v2/progress"
	_ "charm.land/bubbletea/v2"
	_ "charm.land/lipgloss/v2"
	_ "github.com/amber-store/core/amberpack"
	_ "github.com/amber-store/core/fstree"
	_ "github.com/amber-store/core/ingest"
	_ "github.com/amber-store/core/key"
	_ "github.com/amber-store/core/packstore"
	_ "github.com/amber-store/core/reference"
	_ "github.com/amber-store/core/refstore"
	_ "github.com/amber-store/dstore/client"
	_ "github.com/amber-store/dstore/codec"
	_ "github.com/amber-store/dstore/node"
	_ "github.com/amber-store/dstore/placement"
	_ "github.com/amber-store/dstore/refglob"
	_ "github.com/amber-store/dstore/ticket"
	_ "github.com/amber-store/dstore/transport"
	_ "github.com/amber-store/dstore/view"
	_ "github.com/amber-store/dstore/wire"
	_ "github.com/amber-store/dstore/worktree"
	_ "github.com/amber-store/transport-iroh/protocol"
	_ "github.com/aymanbagabas/go-udiff"
	_ "github.com/aymanbagabas/go-udiff/lcs"
	_ "github.com/fxamacker/cbor/v2"
	_ "github.com/tmc/go-iroh/iroh/mdns"
	_ "github.com/tmc/go-iroh/key"
	_ "github.com/tmc/go-iroh/netaddr"
	_ "github.com/tmc/go-iroh/relay"
	_ "github.com/urfave/cli/v2"
	_ "golang.org/x/sys/unix"
)
