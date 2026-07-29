# Compatibility matrix

What `aag` supports, and at what depth. "Structural" means declarations are
extracted and file-level edges resolve; "language-aware" adds the resolution
ladder — import bindings from that language's own module conventions, receiver
typing, inheritance, and framework routes. The difference is measured in
[capability coverage](capability-coverage.md), not asserted here.

## Languages

| Depth | Languages |
|---|---|
| Language-aware resolution | Rust, JavaScript/JSX, TypeScript/TSX, Python, Java, C#, Go |
| Structural extraction | C, C++, Ruby, PHP, Swift, Kotlin, Scala, Dart, Elixir, Erlang, Clojure, Haskell, OCaml, Julia, Nim, Zig, Fortran, Perl, Pascal, Lua, R, Bash, PowerShell, Objective-C, Groovy/Gradle, Apex, Solidity, Verilog, SystemVerilog |
| Component files | Vue, Svelte, Astro — the file is the component, and template usage becomes an edge |
| Documents | Markdown, text, reStructuredText, AsciiDoc, subtitles; PDF, DOCX, PPTX, ODT/ODP, XLSX/XLS/ODS, CSV, SVG, images (metadata), video via sidecar transcript — see [extract](extract.md) |
| Contracts and infrastructure | OpenAPI/Swagger, SQL DDL, Terraform/HCL, a live PostgreSQL catalog — see [database](database.md) |

A language appears in the first two rows only when a test shows extraction
works on it. The list is a claim about what runs, not about which tree-sitter
grammars exist.

## Coding agents

Fourteen integrations, each written in that agent's own config shape. Detection
is the presence of its config directory in the repository or in your home.

| Agent | MCP registration | Hooks | Guidance |
|---|---|---|---|
| Claude Code | `.mcp.json` | `.claude/settings.json` (3) | `.claude/skills/aag-*` |
| Cursor | `.cursor/mcp.json` | `.cursor/hooks.json` | `.cursor/rules/aag.mdc` |
| Gemini CLI | `.gemini/settings.json` | — | `GEMINI.md` |
| Kiro | `.kiro/settings/mcp.json` | — | `.kiro/steering/aag.md` |
| opencode | `opencode.json` | — | `AGENTS.md` |
| Codex | `~/.codex/config.toml` | — | `.agents/skills/aag-*` + `AGENTS.md` |
| Antigravity | UI-managed | — | `AGENTS.md` |
| VS Code / Copilot | `.vscode/mcp.json` | — | `.github/copilot-instructions.md` |
| Windsurf | `~/.codeium/windsurf/mcp_config.json` | — | `.windsurf/rules/aag.md` |
| Zed | `.zed/settings.json` | — | `AGENTS.md` |
| Roo Code | `.roo/mcp.json` | — | `.roo/rules/aag.md` |
| Cline | UI-managed | — | `.clinerules/aag.md` |
| Crush | `.crush.json` | — | `AGENTS.md` |
| goose | `~/.config/goose/config.yaml` | — | `.goosehints` |

Agents without a hook system still stay fresh: the MCP server reconciles on
connect and runs the native watcher.

## Platforms

| Target | Prebuilt binary | Semantic build |
|---|---|---|
| `x86_64-unknown-linux-gnu` | yes | yes |
| `aarch64-unknown-linux-gnu` | yes | yes |
| `x86_64-apple-darwin` | yes | yes |
| `aarch64-apple-darwin` | yes | yes |
| `x86_64-pc-windows-msvc` | yes | yes |

Anything else builds from source with a stable Rust toolchain (edition 2024).
The npm wrapper downloads a binary and compiles nothing; `AAG_SEMANTIC=1`
selects the build with local embeddings.

## Build features

| Feature | Default | What it adds |
|---|---|---|
| _(none)_ | on | the whole engine: index, resolve, query, export, MCP, hooks |
| `semantic` | off | local embeddings via fastembed, fused with lexical search — see [semantic search](semantic-search.md) |

## External tools

| Tool | Needed for | Without it |
|---|---|---|
| `git` | `aag graph-diff`, `aag pr worktrees`, benchmark profiles | those commands error; indexing is unaffected |
| `gh` | `aag pr *`, `graph_diff pr/<n>` | those commands error with the CLI's own message |
| a PostgreSQL server | `aag db scan` | the DDL half of the graph still works |
| network | first `aag embeddings` (model download), npm install | everything else is local |

## Data compatibility

The index carries a `raw_references` marker in `index_metadata`. A database
written by an older layout reads as not-ready and is rebuilt from scratch on
the next run — see [migration notes](migration.md). Benchmark records carry a
`schema_version` and are refused rather than averaged when it does not match.
