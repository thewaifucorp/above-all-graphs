# Architecture

`aag` is one Rust binary. It walks a repository, extracts structure with
tree-sitter, resolves cross-file references, stores the result in SQLite, and
serves it to whatever coding agent is on the machine. Everything else in this
document is detail.

```
repository ──▶ resolve ──▶ storage (SQLite + FTS5) ──┬──▶ explore / impact / query   (answers)
                 ▲                                    ├──▶ export                     (offline site)
                 │                                    ├──▶ mcp / api                  (agents)
              parse                                   └──▶ areas / pr / refs          (derived views)
            (tree-sitter)
```

## The pipeline

**`resolve.rs`** owns the walk. It decides what is indexable, skips what is not
(`SKIP_DIRS`, shared with the watcher and the sync path), and drives everything
below. It runs in one SQLite transaction: clear, insert, resolve, commit.

**`parse.rs`** turns source into declarations and *unresolved references*. It
never guesses across files. An import comes out as an `ImportRef` (module
source, name, alias, glob), a call as a `CallRef` (with the receiver kept), a
type declaration as an `InheritRef`. Keeping the receiver is what lets
resolution do anything better than name matching.

**`bindings.rs`** maps an import's module source onto files in the repository
using each language's own convention — relative paths with inferred extensions,
`index`/`mod`/`__init__` directory modules, Go package directories, Java and C#
namespaces, Rust module paths. **`toolchain.rs`** reads the repository's own
manifests for the aliases they declare (`tsconfig` `paths`, `package.json`
workspace names and subpath imports, the `go.mod` module line). No toolchain is
executed and nothing is downloaded.

**Resolution is a ladder**, narrowest rung first: receiver type, import
binding, `self`/`this`, module qualifier, wildcard import, enclosing file, then
an AMBIGUOUS fan-out. Every edge carries the confidence it was resolved with —
`EXTRACTED`, `INFERRED`, or `AMBIGUOUS` — and nothing downstream is allowed to
forget which it is.

**`storage.rs`** is SQLite: nodes, edges, FTS5 over names and descriptions, and
provenance on both (`Declared` versus `Observed`, plus the evidence kind).
Node ids are per-database, which is why anything comparing two databases keys
on `kind name` instead.

## What hangs off the graph

| Module | What it produces |
|---|---|
| `explore.rs` / `impact.rs` | the two answers agents ask for: how something works, what breaks if it changes |
| `query.rs` | a real Cypher subset — lexer, parser, evaluator |
| `analysis.rs` | communities, entrypoints, processes |
| `areas.rs` | one generated skill per detected area |
| `export.rs` | the offline site (graph, wiki, report) plus GraphML, Cypher, Obsidian |
| `api.rs` / `protocol.rs` | the declared/observed manifest surface |
| `pr.rs` | pull requests ranked by what the graph says they reach |
| `refs.rs` | per-ref snapshots and graph-state diffs |
| `database.rs` | a live PostgreSQL catalog, as `Observed` nodes |
| `extract.rs` | text out of PDFs, office documents, spreadsheets, subtitles |
| `semantic.rs` | optional local embeddings, fused with lexical results |
| `bench.rs` | the Track E engine benchmark |

## How it stays fresh

`bigbang.rs` is the one-shot bootstrap: index, export, and install into every
detected agent. `install.rs` writes each agent's own config shape — fourteen of
them — additively, idempotently, and reversibly. `hook.rs` answers the three
Claude Code hooks (`pre-edit`, `post-edit`, `session-start`) and always exits 0.
`sync.rs` is the refresh those hooks call, with a per-file relevance
short-circuit so an edit to `.aag/` output costs nothing. `watch.rs` is the
native watcher for agents with no hook system, and `lock.rs` keeps the watcher
and a rebuild from racing each other.

## Invariants

- **Nothing blocks the host agent.** Hooks and install swallow errors, warn,
  and exit 0. A broken integration must never break the editor.
- **Config edits are additive and reversible.** Unparseable user files are
  skipped, never clobbered; `aag uninstall` removes exactly what was written.
- **The site is offline.** Every asset is vendored via `include_str!`; there is
  no CDN reference anywhere in the output.
- **Confidence survives to the surface.** An `AMBIGUOUS` edge is labelled as
  such in the CLI, the MCP response, and the site.
- **Declared and observed never merge.** DDL and an OpenAPI file are what
  someone wrote down; a live catalog and a registered route are what is there.
  `drift` is the report of the difference.
- **Tests are hermetic.** They never touch the real home or a user's agent
  config.

## Where the numbers are

Extraction and resolution behavior is measured in
[capability coverage](capability-coverage.md); scale and operations in
[benchmarks](benchmarks.md). Both distinguish what is measured from what is
merely believed.
