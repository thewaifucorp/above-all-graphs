---
wiki: src/sync.rs
---

## What `aag sync` is

The refresh path hooks call after every edit: one `index_repo` pass plus regenerated site artifacts, reusing the existing `.aag/graph.db` — no directory deletion, unlike `bigbang --force`.

## Why it is a full pass with a short-circuit

Cross-file resolution recomputes from the whole repo's symbol table (`resolve::index_repo` clears and rebuilds), so patching a single file's nodes would still need the whole-repo pass to stay correct. The per-file win is the short-circuit instead: when `--file` points at a path the index does not care about (`.aag/`, `target/`, `node_modules`, `.claude/`, `.cursor/`...), sync returns instantly without opening the graph. Since agent hooks fire on every single Write/Edit — including ones to generated artifacts — the short-circuit is what keeps the hook free in practice.

## Skip list

One definition, `resolve::SKIP_DIRS`, shared by the indexer walk, the watcher, and sync — "what can affect the index" can never drift between the three.
