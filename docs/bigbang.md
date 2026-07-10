---
wiki: src/bigbang.rs
---

# bigbang

`bigbang` is the bootstrap entry point for `aag`: it creates the `.aag/` index directory and runs the first index of a repo from scratch. Later phases — the watcher, the MCP server — hang off the same `.aag/` root that `bigbang` establishes here. This module owns that directory's lifecycle: create, rebuild, or skip.

## Options

The `Options` struct controls one `aag bigbang` invocation:

- `force` — discard any existing index and rebuild from scratch.
- `no_viz` — skip writing `graph.json`, `graph.html`, `report.html`, wiki, and other export artifacts.
- `obsidian_dir` — also write an Obsidian-vault-compatible export under `<dir>/aag/`. The export always nests under an `aag/` subdirectory of the given path, never the vault root — see `export::write_obsidian`.
- `no_install` — skip agent integration (MCP config, hooks, skill pack).

## `run`

`run` is the main entry point, taking a repo `root` and an `Options` reference. Its behavior:

- Agent integration via `install::run` fires on every `bigbang` call, even a skipped reindex, so that a newly installed coding agent gets wired up regardless of whether the index itself changes. This call is wrapped so a failure only logs a `tracing::warn!` and never aborts indexing — install-and-forget, not install-or-crash.
- If `.aag/` does not exist yet, it's created and a fresh index runs.
- If `.aag/` exists and `force` is `false`, `run` is a no-op — a repeat `bigbang` on an already-indexed repo is not an error, it just skips.
- If `.aag/` exists and `force` is `true`, the existing directory is removed via `fs::remove_dir_all` before recreating and re-indexing.

Errors surface as `Error::RemoveDir` when a forced rebuild can't delete the existing `.aag/` directory, `Error::CreateDir` when the directory can't be (re)created, or the underlying storage/parse/IO error if indexing or exporting fails.

After the index directory is ready, `run` calls the private `index` helper to perform the actual indexing, then — unless `no_viz` is set — calls `export::write_default` to produce the default artifact set (`index.html`, `graph.html`, `report.html`, wiki, `graph.json`, `graph.graphml`, `cypher.txt`). If `obsidian_dir` is set, `export::write_obsidian` runs afterward for that additional export target.

## `index`

The private `index` function opens a `Graph` at `<aag_dir>/graph.db` via `Graph::open`, then delegates the actual walk-and-parse work to `resolve::index_repo`, which returns an `IndexSummary` (files, nodes, docs, edges counts) that gets logged. Outside of `#[cfg(test)]` builds, it also calls `workspaces::register` to add the repo to the user's global workspace registry — this is compiled out under `cfg(test)` so unit tests indexing throwaway temp repos never pollute the real workspace registry on the developer's machine.

## Test invariants

The test module enforces the module's key contracts: creating the index when missing, indexing source files on first run, writing the full default export set, `no_viz` skipping those artifacts, skipping an existing index without `force`, and discarding/rebuilding it when `force` is set. All tests use `no_install: true` because agent detection in `install::run` reads the real home directory, and tests must never touch the machine's actual agent configs.
