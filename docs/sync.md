---
wiki: src/sync.rs
---

## What `aag sync` is

The refresh path hooks call after every edit: one `index_repo` pass plus regenerated site artifacts, reusing the existing `.aag/graph.db` — no directory deletion, unlike `bigbang --force`.

## Why it is a full pass with a short-circuit

After the first full index, parsers persist unresolved imports, calls, doc mentions, and OpenAPI implementation candidates in `raw_references`. `aag sync --file <path>` deletes and reparses only that file; unchanged nodes keep their IDs. It then rebuilds cross-file edges from the persisted references and the SQLite symbol table, without rereading or reparsing the rest of the repository. Deleted and renamed files use the same path.

Older databases without the incremental-ready marker receive one full compatibility pass before file-level updates begin. Irrelevant paths (`.aag/`, `target/`, `node_modules`, agent configuration) still short-circuit without touching the graph.

## Skip list

One definition, `resolve::SKIP_DIRS`, shared by the indexer walk, the watcher, and sync — "what can affect the index" can never drift between the three.
