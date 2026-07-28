# Changelog

All notable changes to AboveAllGraphs are documented here.

## Unreleased

### Added

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

- The MCP `cypher` tool evaluates queries instead of sniffing their text. The previous surface ignored relationship types and returned every edge in the graph for any query containing `-[`, and reported the first `.name`/`.kind` literal found anywhere in the string as a filter.
- `flow::Call::arguments` groups identifiers per positional argument, so an argument can be matched to the parameter at the same position.
- A sink now takes any tainted name on its line, catching chains like `Command::new(sh).arg(cmd).spawn()` where the value rides the receiver.

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
