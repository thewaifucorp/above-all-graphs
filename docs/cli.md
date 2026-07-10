---
wiki: src/cli.rs
---

# cli.rs

## Role

Defines the command-line surface for `aag`. This module owns the `Cli` struct and the `Command` enum, both built with `clap`'s derive API (`Parser`, `Subcommand`). `src/main.rs` parses `Cli` from `std::env::args` and dispatches on `Command` to the rest of the crate. No behavior lives here — this is pure argument shape, defaults, and help text.

## Cli and Command

`Cli` has a single field, `command: Command`, tagged `#[command(subcommand)]`. Every `aag` invocation resolves to exactly one `Command` variant. Most variants take a `path: PathBuf` option (`--path`, default `.`) naming the repository root to operate on, since `aag` is workspace-scoped rather than global.

## Subcommands

- `Bigbang` — one-shot bootstrap: detect the host agent, register hooks, run the first index. Flags: `force` (rebuild from scratch), `no_viz` (skip site artifacts), `obsidian`/`obsidian_dir` (Obsidian export), `no_install` (skip agent integration). This is what `bigbang.rs` implements.
- `Sync` — refresh the index and site in place; this is what the `PostToolUse` hook runs on every edit. Takes an optional `file` so it can short-circuit instantly when the changed path can't affect the index (e.g. `.aag/`, `target/`). `no_viz` skips regenerating site artifacts.
- `Install` — register `aag` with detected agents (MCP config, hooks, skill pack), independent of indexing. `force` rewrites skills/rules even if the user edited them.
- `Workspaces` — lists every workspace this machine has indexed; each repo keeps its own local graph.
- `Uninstall` — removes everything `Install` wrote.
- `Hook` — nests `HookEvent`, the entry point the agent harness calls with a JSON payload on stdin. Never invoked by hand.
- `Explore` — answers a question about the codebase (symbols, call paths, blast radius) given a free-text `query`.
- `Impact` — shows what would break if a given `symbol` changed.
- `Mcp` — runs the MCP server, newline-delimited JSON-RPC 2.0 over stdio.
- `Describe` — records an agent's vision-pass description of a doc or image and links it to any symbol it mentions by name.
- `Rename` — coordinated multi-file rename of `old_name` to `new_name`; previews by default, only writes with `--write`.
- `Affected` — lists test-looking files transitively affected by a set of changed files; reads paths from stdin with `--stdin`, e.g. piped from `git diff --name-only`.

## HookEvent

`HookEvent` mirrors `crate::hook::Event` and nests under `Command::Hook`. Its three variants line up one-to-one with agent harness lifecycle points:

- `PreEdit` — fires on `PreToolUse` for Edit/Write, injects a blast-radius warning.
- `PostEdit` — fires on `PostToolUse` for Write/Edit, kicks off a background `aag sync`.
- `SessionStart` — fires on session start, reconciles the index and injects a graph digest.

## Invariants

- Hook subcommands always exit 0, matching the house rule that hooks must never block or break the host agent — this constraint is enforced in `hook.rs`, not `cli.rs`, but the shape here (no way to signal failure back to the harness) reflects it.
- Nearly every variant defaults `path` to `.`, so `aag` behaves correctly when run from inside the target repository without extra flags.
