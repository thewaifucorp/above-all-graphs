---
wiki: src/storage.rs
---

# storage.rs

_The `SQLite`-backed graph store: nodes and edges, with FTS5 full-text search bolted on._

`storage.rs` is the single source of truth that the parser/resolver (writer side) and the CLI/MCP surface (reader side) both sit on top of. Everything else in the codebase — `resolve.rs` building the graph, `export.rs` reading it out for `graph.html`/`GRAPH_REPORT.md`, the MCP `explore` tool — goes through the `Graph` handle defined here rather than touching the database directly.

## Schema shape

`Graph::migrate` creates two tables plus one virtual table:

- `nodes`: one row per symbol or file. Columns are `id`, `kind`, `name`, `file_path`, `start_line`, `end_line`, `description`.
- `edges`: one row per relation, with a composite primary key of `(src, dst, kind)` so re-inserting the same relation is a no-op rather than a duplicate.
- `nodes_fts`: an FTS5 virtual table shadowing `name` and `description` from `nodes` (`content='nodes'`, `content_rowid='id'`), kept in sync by three triggers (`nodes_ai`, `nodes_ad`, `nodes_au`) fired on insert/delete/update.

Two plain indexes (`idx_nodes_name`, `idx_edges_dst`) back the non-FTS lookup paths — `find_by_name` and `callers`.

## Core types

- `Node` — a symbol or file. `kind` is a `NodeKind` (`File`, `Function`, `Struct`, `Method`, `Interface`, `Doc`). `description` is `None` for code nodes; for `NodeKind::Doc` it holds either the full text (text docs, indexed immediately by `resolve.rs`) or the host agent's vision-pass description (binary docs, filled in later by `crate::docs` via `set_description`).
- `Edge` — a directed relation between two node ids, tagged with an `EdgeKind` (`Imports`, `Calls`, `Inherits`, `Implements`, `Explains`) and a `Confidence`.
- `Confidence` — `Extracted` (explicit in source), `Inferred` (resolved by heuristic), or `Ambiguous` (not resolved with certainty). This tri-state is called out in the module doc comment as matching `SPEC.md` section 3, and every edge carries one.

All three enums store their `SQLite` representation as a plain string via `as_str`/`from_str` rather than relying on integer discriminants, so the on-disk values stay human-readable when inspecting `.aag/graph.db` directly.

## Opening the database

`Graph::open` opens (or creates) a database at a given path and runs `migrate`. `Graph::open_existing` is the normal CLI entry point: it looks for `.aag/graph.db` under a repo root and returns `Error::IndexMissing` if it isn't there yet, telling the caller to run `aag bigbang` first. `Graph::open_in_memory` is for tests and any throwaway, process-lifetime-only graph.

## Writing

`insert_node` inserts a row and returns the assigned rowid (`last_insert_rowid`). `insert_edge` uses `INSERT OR REPLACE`, so re-running resolution over the same edge just refreshes its confidence rather than erroring on the primary key. `set_description` updates a node's `description` in place — the path `crate::docs` uses once a host agent finishes describing a binary doc — and relies on the `nodes_au` trigger to keep `nodes_fts` current so the new text is searchable right away. `clear` deletes all edges then all nodes, used by `resolve::index_repo` before a full reindex so re-running it (e.g. from the watcher) never accumulates stale nodes from a previous pass.

## Reading

- `search` is the FTS5 entry point: it joins `nodes_fts` back to `nodes` and orders by FTS5's built-in `rank`, so callers can pass raw FTS query syntax (e.g. a prefix query like `"bigbang*"`) and get relevance-ranked `Node`s.
- `callers` and `callees` walk edges in each direction from a given node id — `callers` matches `edges.dst`, `callees` matches `edges.src` — returning tuples of `(Node, EdgeKind, Confidence)` so the caller can see both what's connected and how sure the graph is about it.
- `all_nodes` and `all_edges` return the entire graph in one pass; `export.rs` uses these to build `graph.json`, `GRAPH_REPORT.md`, `graph.html`, and the other offline site outputs without issuing many targeted queries.
- `find_by_name` is an exact-match lookup (not FTS), returning `Option<Node>` — `Ok(None)` on no match rather than an error, distinguishing "not found" from a real storage failure.

## Notable invariants

- Every fallible operation returns `Result` wrapping `Error::Storage` with a short `context` string describing what failed, so errors surfacing at the CLI/MCP layer are traceable to a specific query without a stack trace.
- Row decoding is centralized in `row_to_node` (shared by `search`, `callers`, `callees`, `all_nodes`, `find_by_name`) and `collect_nodes` (shared by the multi-row query paths), so the seven-column node layout only needs to be kept in sync with the schema in one place.
- The FTS5 shadow table is a derived index, not independent storage — it's rebuilt from `nodes` via triggers, so nothing outside `storage.rs` ever writes to `nodes_fts` directly.
