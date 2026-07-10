# aag — AboveAllGraphs

Rust CLI + MCP server: code knowledge graph that installs itself into every coding agent. `SPEC.md` is the design contract — read it before structural changes; update it when behavior changes.

## Commands

```bash
cargo test                      # all tests must pass
cargo clippy --all-targets      # pedantic is on; zero warnings is the bar
cargo fmt                       # rustfmt.toml is authoritative
cargo build --release           # binary at target/release/aag
```

CI (`.github/workflows/ci.yml`) enforces fmt + clippy `-D warnings` + tests. Releases: tag `v*` triggers `release.yml` (prebuilt binaries) which `npm/` wraps.

## Architecture (one line each)

- `bigbang.rs` — bootstrap: index + exports + agent integration, one shot
- `resolve.rs` — walk + parse + cross-file resolution; owns `SKIP_DIRS` (shared by watcher and sync)
- `parse.rs` / `storage.rs` — tree-sitter extraction / SQLite graph (FTS5)
- `install.rs` — agent detection + MCP/hooks/skills/rules registration, idempotent + reversible
- `hook.rs` — `aag hook pre-edit|post-edit|session-start`: stdin JSON, always exit 0
- `sync.rs` — hook-driven refresh: full pass + per-file relevance short-circuit
- `workspaces.rs` — global registry + hub.html (multi-repo = selection, not unification)
- `export.rs` — the offline site (graph.html/wiki/report) + GraphML/Cypher/Obsidian
- `mcp.rs` — stdio JSON-RPC; `explore` listed by default, rest via `AAG_MCP_TOOLS`
- Skill templates live in `assets/skills/`, embedded via `include_str!`

## House rules

- Tests are hermetic: never touch the real home or user agent configs — use `run_with_home`/`uninstall_with_home` with a scratch home, and `no_install: true` when calling `bigbang` in tests.
- Hooks and install must never block or break the host agent: swallow errors, exit 0, warn-and-continue.
- Config merges are additive and idempotent; unparseable user files are skipped, never clobbered.
- The site is 100% offline: vendor assets via `include_str!`, never CDN.
- This repo dogfoods itself: `aag` hooks are active here. Query the graph (`aag explore`, `aag impact`, MCP `explore`) before manual grepping.
