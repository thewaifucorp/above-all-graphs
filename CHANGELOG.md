# Changelog

All notable changes to AboveAllGraphs are documented here.

## [0.3.0] - 2026-07-29

### Added

- `aag api spec` emits an `OpenAPI` 3.1 document for the routes the code serves — the inverse of contract ingestion. Paths, methods, the handler with its `file:line` and provenance under `x-aag`; one `default` response stating that shapes are not inferred, since nothing in a handler declares them. Path parameters are read from the path (`:id`, `<int:id>`, `{id}` all become `{id}`). Operations a contract declares but no code serves need `--include-declared`; RPC/MCP tools are reported under `x-aag-tools` rather than dressed up as routes.
- Outcome-backed work memory (`src/memory.rs`): `aag memory save|correct|recall|lessons` and the MCP tools `memory_save`, `memory_recall`, `memory_lessons`. Recall checks every entry against the current graph and marks it `stale` when the symbols it rested on are gone; lessons carry their evidence ids and how many entries the graph still supports. Stored in `.aag/memory.db`, preserved across `aag bigbang --force`.
- Nineteen more languages, each validated by a test that extracts a declaration from it: Vue, Svelte, Astro, Zig, PowerShell, Julia, Groovy, Verilog, SystemVerilog, Fortran, Pascal/Delphi, Apex, Haskell, OCaml, Erlang, Clojure, Nim, Perl, and Solidity — 20 languages to 39. Vue/Svelte/Astro script blocks are handed to the JavaScript frontend with line numbers shifted; Clojure declarations are matched as forms; Groovy on its keyword.
- MCP Streamable HTTP (`src/transport.rs`): `Mcp-Session-Id` sessions with `DELETE` termination and idle expiry, JSON or SSE framing chosen by `Accept`, a keepalive `GET` stream, and `--bind`, `--stateless`, `--max-body`, `--rate-limit` on `aag mcp --transport http`. Binding beyond loopback without `--api-key` refuses to start. Container guidance in `docs/transport.md`.
- Cross-repository protocol links: `aag group links <group>` and the MCP tool `group_links` pair API producer to client, package export to import, event producer to consumer, schema to model, and tool definition to invocation. Each member's graph is read separately and never merged, and every link carries the evidence that produced it. See `docs/federation.md`.
- Events and tool invocations are indexed: a publisher gets a `References` edge to every listener of that event name, and `call_tool('x')` gets a `Calls` edge into the tool `x` was defined as.
- Route, RPC, and tool intelligence (`src/api.rs`): `aag api routes|tools|shapes|impact` and the MCP tools `route_map`, `tool_map`, `shape_check`, `api_impact`. Declared and served endpoints pair by shape, both mismatch states are reported, and declared response shapes are compared with the fields handlers return. See `docs/api.md`.
- RPC/MCP tools are indexed as endpoints whose method is `TOOL`, from a registration call, a `@tool`/`#[tool]` marker, or a `ToolSpec { name: … }` table entry.
- Outbound HTTP calls (`fetch`, `axios.get`, `client.post`, …) become `Calls` edges into the endpoint they request — EXTRACTED for a literal match, INFERRED once path parameters are flattened, AMBIGUOUS when several endpoints share the shape.
- A documented formal subset of Cypher with real pattern evaluation (`src/query.rs`): labels, relationship types and direction, variable-length hops (`*1..3`), `WHERE` with `=`/`<>`/`<`/`<=`/`>`/`>=`/`CONTAINS`/`STARTS WITH`/`ENDS WITH`/`IN`/`IS NULL`/`AND`/`OR`/`NOT`, `count` with grouping, `DISTINCT`, `ORDER BY`, `SKIP`, and `LIMIT`. Anything outside the subset is an error naming what was expected, at the line and column it was found. See `docs/query.md`.
- `aag cypher "<query>"`, printing a table or `--json` rows.
- Taint flows that cross calls: each function is summarized (which parameter positions reach a sink, which reach a `return`, whether it returns an input of its own), and a caller reads that summary instead of re-analyzing the callee. Callees come from the indexed `calls` edges, so an ambiguous call is followed through every candidate and reported as one of them.
- Sanitizer recognition — a short list of escaping, quoting, and narrowing calls, plus any function whose parameter reaches its `return` only through one — with suppressed flows reported rather than silently dropped.
- `aag taint --depth <hops>` and the MCP `taint` tool's `path:hops` form.
- `Graph::find_in_file`, for resolving a symbol name within one file.

### Changed

- Re-indexing one file no longer re-resolves the repository. The pass collects the names the file declared before and after, finds the other files whose references mention one of them, and resolves only those — a manifest still takes the full pass, because it moves the alias tables themselves.
- The exported graph page carries its payload gzipped and base64'd above 64 KB, inflated in the page with `DecompressionStream`. It stays one self-contained file that opens from `file://`; a 458 MB export for a 98 000-edge repository was most of that size.
- The graph page reads as a graph. Layout is one island per module (two force passes, islands sized by the area their members need) with the repulsion pass back on a quadtree — 5753 nodes went from 52 850 ms to 1 367 ms — running in a worker with a main-thread fallback. Files, types and functions are three visual tiers, a module aggregate is the only thing at full colour, and `structure only` narrows the scene to files and other containers. Path enumerates the k shortest loopless routes (Yen, `routes` 1–12) with a fallback that ignores edge direction when nothing follows the arrows. Labels come from a per-zoom budget rather than a fixed pixel floor, the camera fits the scene it was given, and pins, hover plates and the inspector's relation rows are legible.
- The MCP `cypher` tool evaluates queries instead of sniffing their text. The previous surface ignored relationship types and returned every edge in the graph for any query containing `-[`, and reported the first `.name`/`.kind` literal found anywhere in the string as a filter.
- `flow::Call::arguments` groups identifiers per positional argument, so an argument can be matched to the parameter at the same position.
- A sink now takes any tainted name on its line, catching chains like `Command::new(sh).arg(cmd).spawn()` where the value rides the receiver.

### Fixed

- One path spelling per file. `index_file` computed a file's graph path with `strip_prefix`, which is a literal component match: the hooks pass an absolute path while `--path` defaults to `.`, so the strip failed and the file was indexed a second time under its absolute path. Nothing errored — the repository was simply in the graph twice, with node counts climbing on every edit. Both sides are canonicalized now.
- `install` ignores everything it writes. The fenced `.gitignore` block had fallen behind the agents added since it was introduced, and it was never rewritten once present, so `.vscode/mcp.json`, `.zed/settings.json` and the Roo Code files were left untracked after every `bigbang`. The list is one constant checked by a test against the writers. Markdown context documents stay out on purpose: `install` only appends a fenced section to a file the repository authors.
- The graph page's inspector and Filters tab could not be shown at all: `#tab-filters { display: none }` and a missing `.fpanel.open` rule outranked the classes the toggles set, so clicking a node filled a panel with `display: none` and node/relation/confidence filters were unreachable. The file tree also listed 45 of this repository's 107 indexed files, because it was built by counting symbols per file and a file that produced none never appeared.

## [0.2.0] - 2026-07-20

### Added

- AAG Protocol compiler and validator with declared/observed provenance.
- Structural indexing for 20 languages through one language-neutral graph.
- OpenAPI/Swagger, SQL DDL, foreign-key, and Terraform/HCL ingestion.
- File-level incremental indexing backed by persisted unresolved references.
- Optional local semantic embeddings and hybrid reciprocal-rank fusion.
- MCP request/response HTTP transport with loopback origin checks and optional bearer authentication.
- Named hierarchical repository groups across query, status, contracts, synchronization, CLI, and MCP.
- Communities, execution processes, graph traversal tools, PR impact tools, and Codex skill installation.

### Fixed

- Prevented the filesystem watcher from reacting to its own lock and non-indexable access events.

## [0.1.1] - 2026-07-13

- Batched SQLite indexing transactions and serialized concurrent graph writers.
- Improved query prefix handling and release portability.

## [0.1.0] - 2026-07-10

- Initial public release.

[0.2.0]: https://github.com/thewaifucorp/above-all-graphs/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/thewaifucorp/above-all-graphs/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/thewaifucorp/above-all-graphs/releases/tag/v0.1.0
