---
wiki: src/docs.rs
---

# describe

`src/docs.rs` implements `aag describe`, the CLI surface for attaching a human- or agent-written description to a `Doc` node already sitting in the graph, and for auto-linking that description to any symbols it mentions by name.

## Why this module exists

`aag` has no vision model of its own and, per `SPEC.md` section 5, doesn't default to calling out to one. Binary or otherwise non-parseable docs (screenshots, diagrams) get indexed as unprocessed `Doc` nodes with no description. The trick this module implements is "custo zero": whichever coding agent is already looking at the file — because it just read or viewed it as part of its own task — writes back what it saw through `describe`, and that pass runs on the host agent's model, not on a budget `aag` itself pays for.

## Entry points

- `run` — the function `src/main.rs` wires to `Command::Describe { doc, description, path }`. Opens the index under `root`, calls `format`, and prints the result.
- `format` — does the real work and returns the message as a `String` instead of printing it, so `crate::mcp`'s `describe_doc` tool can return the identical text over MCP without drifting from the CLI wording.

## What `format` does

1. Opens the existing graph with `Graph::open_existing` — errors with `Error::IndexMissing` if `root` has never been indexed.
2. Looks up the `Doc` node by `doc_path` via `find_by_name` (doc node names are their file path, relative to `root`, matching how `crate::resolve` names them). Missing doc raises `Error::SymbolNotFound`; a node found but not of `NodeKind::Doc` raises `Error::NotADoc`.
3. Calls `graph.set_description(doc_id, description)` to persist the text.
4. Builds a `name_index` — every node name in the graph mapped to the `Vec<i64>` of node ids sharing that name — then runs `crate::resolve::mentioned_names` against the description text to find which known symbol names actually appear in it.
5. For each mentioned name, inserts an `Edge` of kind `EdgeKind::Explains` from the doc to every matching node (skipping self-links back to the doc itself). Confidence is `Confidence::Inferred` when the name resolves to exactly one node, `Confidence::Ambiguous` when multiple nodes share that name.
6. Returns a summary string: `described '<doc_path>' — linked to <n> mentioned symbol(s)`.

## Notable behavior

- Text-based docs (like README files parseable at index time) already get their content scanned and `Explains` edges inserted automatically during `bigbang::run` — `describe` is specifically the path for docs `aag` cannot read on its own, closing that gap using the calling agent's eyes instead.
- The `node.id.expect(...)` call on nodes read back from storage is documented as a non-panicking invariant: anything loaded via `Graph` methods always carries a populated id.
- `name_index` is a private helper, rebuilt fresh on every `format` call; it has no persistence of its own.
