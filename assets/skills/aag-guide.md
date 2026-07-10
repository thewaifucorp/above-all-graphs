---
name: aag-guide
description: Reference for aag (AboveAllGraphs) itself — what tools exist, how to query the code knowledge graph, CLI commands, MCP surface, and where the generated site lives. Use when asked "what can aag do", "how do I query the graph", or when unsure which aag tool fits a task.
---

# aag — tool and query reference

`aag` keeps a code knowledge graph of this repo in `.aag/graph.db` (SQLite), refreshed automatically by hooks. Symbols, files, and docs are nodes; imports, calls, and doc-mentions are confidence-tagged edges (`EXTRACTED` / `INFERRED` / `AMBIGUOUS`).

## Pick the right surface

| Question shape | Use |
|---|---|
| "How does X work?", "How does X reach Y?", area survey | MCP tool `explore` (or `aag explore <query>`) |
| "What breaks if I change X?" | `aag impact <symbol>` |
| "Who calls X?" / "What does X call?" | MCP `callers` / `callees` (enable via `AAG_MCP_TOOLS`) or `aag explore X` |
| Rename a symbol everywhere | `aag rename <old> <new> [--write]` |
| Which tests does this diff affect? | `git diff --name-only \| aag affected --stdin` |
| Document something | see the `aag-wiki` skill |

## MCP surface

Default: one tool, `explore` — returns source grouped by file + call paths + blast-radius summary. Granular tools (`node`, `search`, `callers`, `callees`, `impact`, `rename`, `affected`, `cypher`, `detect_changes`, `wiki`, `describe_doc`) are unlisted; enable with env var `AAG_MCP_TOOLS=explore,impact,callers` on the MCP server entry.

## CLI

- `aag bigbang [--force] [--no-viz] [--obsidian]` — (re)build everything: index + site + agent integration.
- `aag sync [--file <path>]` — refresh index + site; instant no-op when `--file` is irrelevant (e.g. `.aag/`, `target/`). Hooks call this — you rarely need to.
- `aag explore <query>` / `aag impact <symbol>` / `aag rename` / `aag affected` — CLI twins of the MCP tools.
- `aag mcp` — the MCP server itself (stdio JSON-RPC).

## Generated site (all offline)

- `.aag/index.html` — landing page with stats and cards.
- `.aag/graph.html` — interactive graph (WebGL). Deep link: `graph.html?focus=<file-or-symbol>`.
- `.aag/wiki/index.html` — per-module wiki. `.aag/GRAPH_REPORT.md` — god nodes, surprising connections, suggested questions.

## Trust the confidence tags

`EXTRACTED` edges are explicit in source. `INFERRED` resolved by unique-name heuristic. `AMBIGUOUS` means several symbols share the name — verify before acting on those.
