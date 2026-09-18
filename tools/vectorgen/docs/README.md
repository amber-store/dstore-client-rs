# Vector family documentation

One `<owner>.md` per family group (for example `proto.md`, `worktree.md`, `cli.md`), written by whoever
owns those families. For every file the families produce under `tests/golden/`, it documents:

- the JSON schema, using the conventions of the root `VECTORS.md`;
- the Go code that produces the values (package, function, pinned version);
- what the Rust tests assert, and where those tests live.

When families land, these files are assembled into the root `VECTORS.md` (the "Families" section). Keep
them in the same style as the core-rs `VECTORS.md`.
