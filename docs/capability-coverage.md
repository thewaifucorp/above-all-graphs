---
title: Capability coverage and priorities
---

# Capability coverage and priorities

This page is the public regression contract for the AAG surface that overlaps
GitNexus and Graphify. A capability counts only when it is callable, tested,
and described at the depth actually delivered. Tool count and language count
alone do not establish parity.

## Implemented foundation

- One static Rust binary distributed for Linux, macOS, and Windows on x64 and
  arm64, with no compile step during npm installation.
- Thirty-nine structural language frontends sharing one graph model. Rust and
  JavaScript have dedicated extraction paths; Vue, Svelte, and Astro hand their
  script block to the JavaScript one; the rest use the tree-sitter language pack
  plus AAG declaration and call extraction. Every listed language is covered by a
  test that extracts a declaration from it.
- Confidence-tagged calls, imports, inheritance, implementation,
  documentation, contract, schema, and infrastructure relations. Imports and
  declared type relations are resolved through language-aware module
  resolution; calls are resolved through import bindings, receivers, module
  qualifiers, and enclosing scope, and are tagged AMBIGUOUS when more than
  one candidate survives.
- Import aliases read from the repository's own manifests (`tsconfig`/
  `jsconfig` `baseUrl` and `paths`, `package.json` workspace names and subpath
  imports, the `go.mod` module prefix), with no toolchain invoked.
- HTTP routes registered in code — Express family, Flask/FastAPI decorators,
  axum, actix/rocket, Spring, ASP.NET — as observed endpoints linked to their
  handlers, alongside the endpoints a contract file declares. RPC/MCP tools are
  endpoints too, and outbound HTTP calls are edges into the endpoint they
  request, so a route map, a tool map, a response-shape check, and API impact all
  read from one graph.
- Graph-aware search, node context, neighbors, impact, affected tests,
  shortest path, god nodes, communities, detected entrypoints, and execution
  processes rooted at them.
- Statement-level flow for seven languages: basic blocks, a control-flow graph,
  reaching definitions, def-use chains, control dependence, the program
  dependence graph, and taint findings that cross calls by joining per-function
  summaries to the resolved call graph, with sanitizer recognition and reported
  suppression.
- Coordinated whole-word rename, a read-only query surface over a documented
  formal subset of Cypher — labels, relationship types and direction,
  variable-length hops, `WHERE`, `count`, `DISTINCT`, `ORDER BY`, paging — with
  real pattern evaluation and an error for anything outside the subset. Diff
  change detection, wiki, report, GraphML, JSON, Cypher export, Obsidian export,
  and an offline WebGL graph.
- OpenAPI and Swagger operations, parameters, bodies, responses, security,
  schemas, references, implementation matching, SQL DDL and foreign keys, and
  Terraform/HCL resources.
- Basic PR listing, filtering, and graph impact through the read-only GitHub
  CLI.
- Multi-workspace query and named slash-hierarchical repository groups while
  keeping each repository graph independent, plus cross-repository protocol links
  (API, package, event, schema, tool) computed by reading each graph separately.
- AAG Protocol compilation, structural and semantic validation, provenance,
  declared/observed separation, uncertainty preservation, and automatic
  SQLite migration.
- Optional local embeddings through fastembed/ONNX with lexical, semantic,
  and structural reciprocal-rank fusion. The standard prebuilt npm binary is
  intentionally lightweight and does not include this feature yet.
- True file-level incremental parsing with persisted unresolved references,
  global edge re-resolution, watcher reconciliation, and agent hooks.
- MCP over stdio, and Streamable HTTP with sessions, SSE or JSON framing,
  stateless mode, a configurable bind address, bearer authentication, and
  size/rate limits.
- Integrations for Claude Code, Cursor, Codex, Gemini CLI, Kiro, OpenCode, and
  Antigravity.
- Outcome-backed work memory: recorded questions, answers, outcomes, and
  corrections, recalled with a staleness check against the current graph.

## Priority gates

The number is the delivery order. P0 gates determine whether AAG can credibly
claim best-in-class code intelligence. P1 gates close major product workflows.
P2 gates expand reach after the core is measurable and dependable.

### P0 — correctness and product credibility

- 1. Deliver the empirical evaluation contract specified below. It must
   distinguish protocol conformance, engine extraction quality, agent utility,
   end-to-end economics, and scale. Protocol-only, LLM-only, simulated, and
   dogfood results cannot substantiate AboveAllGraphs Engine claims.
- 2. **Closed.** Language-aware resolution for TypeScript/JavaScript, Python,
   Java, C#, Go, and Rust: aliases, named imports, re-exports, receiver types,
   constructor inference, self/this resolution, inheritance, toolchain config,
   framework patterns, and entrypoints. What that means concretely:
   - Import sources map onto repository files by each language's own module
     conventions — relative and package-relative paths, directory modules
     (`index`/`mod`/`__init__`), Go package directories, Java and C#
     namespaces, Rust module paths — producing a per-file table of local name
     to target that resolves named imports, aliases, namespace imports,
     wildcards, and re-exports.
   - The repository's own build manifests are read for the aliases they
     declare: `tsconfig`/`jsconfig` `baseUrl` and `paths`, workspace package
     names and Node subpath imports from `package.json`, and the `go.mod`
     module prefix. No toolchain is invoked and nothing is fetched.
   - Calls resolve through a narrowing ladder: receiver type, import binding,
     `self`/`this`, module qualifier, wildcard import, enclosing file, then an
     AMBIGUOUS fan-out. A receiver is typed when the syntax states the type —
     construction (`new Store()`, `Store::open()`, `Store{}`), an explicit
     annotation, or the enclosing type of a method — and a call then resolves
     only against members that type declares.
   - Declared `extends`/`implements`/`impl … for …` relations become
     `Inherits`/`Implements` edges.
   - HTTP routes registered in code become observed `Endpoint` nodes linked to
     their handler: the Express family, Python decorators (Flask/FastAPI),
     axum's `.route`, actix/rocket attributes, Spring annotations, and ASP.NET
     attributes.
   - Entrypoints are detected (`main`, and any handler wired to an endpoint)
     and used as the roots of process tracing, instead of "callable that
     nothing calls".

   Not claimed: this is syntactic typing, not a type checker. A receiver whose
   type comes from a function's return value, a field, a generic parameter, or
   a longer expression chain stays untyped and falls through to the weaker
   rungs. An import specifier no manifest and no convention explains is
   treated as external.
- 3. **Closed.** The graph experience redesign specified below: five task modes
   (Overview, Explore, Path, Impact, Contracts) on one scene engine, shareable
   URL state with working history, a unified inspector, community aggregates
   with expand-in-place, a command palette, semantic zoom, deterministic layout
   caching, and keyboard/reduced-motion coverage. Measured against the
   pre-redesign baseline on an external 5971-node repository: document 12.97 MB
   to 5.17 MB, first paint 1373 ms to 400 ms, peak heap 271.8 MB to 125 MB,
   initial scene 5971 nodes / 87 928 edges to 146 / 151. Payload and page-size
   budgets are enforced by `cargo test`. Still open, and recorded as such in
   [graph experience](graph-experience.md): visual regression baselines and
   browser-side budget enforcement, and browser-side budget
   enforcement.
   Original scope, for the record:
   Complete the graph experience redesign specified below. Preserve the
   fast offline renderer, but replace the single hairball-oriented interaction
   model with task-specific overview, explore, path, impact, and contract
   views. The current graph is functional but not yet a competitive visual
   product.
- 4. **Closed.** The Cypher-shaped shim is replaced by a documented formal
   subset with real pattern evaluation: a lexer, a parser, and an evaluator that
   honors labels, relationship types, direction, variable-length hops, `WHERE`
   with the usual comparisons plus `CONTAINS`/`STARTS WITH`/`ENDS WITH`/`IN`/`IS
   NULL`, `OPTIONAL MATCH`, `UNION`/`UNION ALL`, the aggregates `count`,
   `collect`, `min`, `max`, `sum`, and `avg` with grouping, `DISTINCT`,
   `ORDER BY`, `SKIP`, and `LIMIT`.
   The grammar is written down in [query](query.md), `aag cypher` exposes it on
   the CLI, and the MCP `cypher` tool returns the same answers as JSON. What the
   shim did instead is on the record there: it ignored relationship types and
   returned every edge in the graph for any query mentioning `-[`, which is a
   wrong answer that reads like a right one.
   Bounds are stated: `LIMIT` clamped to 1000, hops to 5, 20 000 intermediate
   rows, no repeated edge within a path, and a paged answer says it was paged.
   `OPTIONAL MATCH` keeps the row it could not extend, with that pattern's
   variables null, so "functions that call nothing" is answerable rather than
   invisible; `UNION` follows Cypher's rule that the arms return the same
   columns; and every aggregate ignores nulls, so `count(x)` counts values while
   `count(*)` counts rows and `sum` over non-numbers is null rather than a
   guess.
   Not claimed: this is not full Cypher and not a query planner. There is no
   `WITH`, `UNWIND`, subquery, arithmetic, `CASE`, or scalar function — a
   pipeline needs an intermediate row shape that is not a graph match, and this
   evaluator has one stage on purpose. Each is rejected by name rather than
   reinterpreted, and an unknown function is named against the six that exist.
- 5. **Closed.** Statement-level control-flow and data-flow foundations: basic
   blocks, CFG, def-use, control/data dependence, PDG queries, and
   provenance-aware source-to-sink taint findings, starting with TypeScript and
   JavaScript.
   Landed: basic blocks, a control-flow graph with typed edges (sequential,
   true, false, back), definitions and uses, reaching definitions, def-use
   chains, and control dependence from post-dominance — for Rust, JavaScript,
   TypeScript, Python, Java, C#, and Go, callable as `aag flow <file>` and
   covered by tests. See [flow](flow.md).
   Also landed: the program dependence graph (`aag pdg`, MCP `pdg_query`,
   including the transitive backward slice of one line) and source-to-sink
   taint findings (`aag taint`, MCP `taint`) carrying the assignments that
   carried the value and whether a branch decides the sink runs.
   Also landed: interprocedural flow and sanitizer recognition. Each function
   gets a summary — which parameter positions reach a sink, which reach a
   `return`, whether it returns an input of its own, whether it neutralizes what
   it is given — and a flow crosses a call by reading the callee's summary rather
   than re-analyzing it. Callees come from the indexed `calls` edges, so
   resolution is the language-aware ladder of priority 2 and not a second
   implementation of it; an ambiguous call is followed through every candidate
   and the finding says which of how many it is, and an unindexed repository
   joins only calls inside one file and says the callee was matched by name.
   Sanitizers are a short list of escaping, quoting, and narrowing calls, plus
   any function whose parameter reaches its `return` only through one; a
   suppressed flow is reported as suppressed rather than disappearing. Bounds are
   stated: 2 call hops by default, 400 joined functions, 8 rounds of
   assignment-chasing. See [flow](flow.md).
   A returned value is every explicit `return` plus a Rust tail expression,
   followed through the arms of a tail `if`/`match`, because the value a body
   ends on is what a caller receives.
   Not claimed: this is not security analysis. The data flow is syntactic and
   line-granular, aliasing through references, fields, and containers is not
   tracked in a callee any more than in a caller, a finding is a place to look
   rather than a proven vulnerability, and no findings is not evidence of
   safety. Reaching
   definitions over-approximates what may reach and under-approximates what
   does.

### P1 — complete workflows and heterogeneous graphs

- 6. **Closed.** First-class route, RPC, and tool intelligence: route maps,
   MCP/RPC tool maps, API consumer links, response-shape checks, and API impact,
   as `aag api routes|tools|shapes|impact` and the MCP tools `route_map`,
   `tool_map`, `shape_check`, and `api_impact`. See [api](api.md).
   Declarations stay evidence rather than being replaced by inference: a declared
   endpoint and a served one pair by shape (`/pets/{id}` and `/pets/:id` are one
   endpoint, and the contract's spelling is the published name), and the two
   mismatch states — declared-but-unimplemented, served-but-undeclared — are
   reported rather than resolved. An RPC/MCP tool is an endpoint whose method is
   `TOOL`, recognized from a registration call, a `@tool`/`#[tool]` marker, or a
   `ToolSpec { name: … }` table, so one contract vocabulary covers both.
   Outbound HTTP calls become `Calls` edges into the endpoint they request:
   EXTRACTED for a literal match, INFERRED once path parameters are flattened,
   AMBIGUOUS when several endpoints share that shape.
   The shape check compares dotted field paths, not just the top level: the
   declared schema is flattened through `$ref`s and array items, and the
   handler's response is followed to where it was assembled — a body bound to a
   local variable, a field holding another variable, an object spread — so
   `customer.name` is a finding and no longer silence. Recursion stops at the
   first name that resolves back to itself, and nesting is compared four levels
   deep. A tool invocation is linked to the definition it names by priority 7.
   A path assembled from literals is folded before matching, so
   `` fetch(`${BASE}/orders/${id}`) `` and `fetch(BASE + '/orders')` name the
   endpoints their literals name: a leading unreadable piece before a `/` is a
   base URL, and every other one must fill a whole segment to become a
   parameter.
   Not claimed: recognition is by name across a fixed set of frameworks and
   clients, so a house-built router or client is invisible until its shape is
   added. A path that only exists at runtime is skipped rather than guessed, and
   so is a partial segment (`'/orders' + suffix` could be `/ordersearch`). The shape
   check is still syntactic — a field copied out of a call, a model, or a
   serializer reads as missing, a key computed at runtime is not a name it can
   read, and a finding is a place to look.
- 7. **Closed.** Cross-repository protocol links without merging local
   databases: API producer to client, package export to import, event producer to
   consumer, schema to model, and MCP tool definition to invocation, as
   `aag group links <group>` and the MCP tool `group_links`. Each member's graph
   is opened read-only and matched by name across the boundary; nothing is
   unified, and every link carries the evidence that produced it. Package names
   come from the repository's own manifests, with no registry consulted. Inside
   one repository the same relations are ordinary edges: a publisher gets a
   `References` edge to every listener of that event name, and a tool invocation
   gets a `Calls` edge into the definition it names. See [federation](federation.md).
   Not claimed: a link is a name agreeing across an ownership boundary, which is
   evidence and not proof — two repositories can name one event and mean two
   things, a declared `Order` and a class `Order` may be unrelated, and a path or
   event name built at runtime is never matched, so a missing link is not evidence
   that no call exists.
- 8. **Closed.** Live PostgreSQL catalog ingestion (`src/database.rs`):
   schemas, tables, partitioned and foreign tables, views and materialized
   views, columns with type/nullability/default, primary keys, unique
   constraints, `CHECK` definitions, indexes with their column order, and
   foreign keys — as `aag db scan --url <url>`, with `aag db drift` and the MCP
   tool `db_drift` comparing the live schema against the DDL this repository
   declares. See [database](database.md).
   The vocabulary is the one already in the graph: a table is a
   `DatabaseTable`, a column is a `DatabaseColumn`, a foreign key is a
   `References` edge, DDL stays `Declared` and the catalog is `Observed`, so
   both halves answer one query.
   Credentials are kept out by construction rather than by filtering: nodes are
   filed under `postgres/<database>/<schema>`, a path with no host, user, or
   password, and a test greps the whole graph for every part of the connection
   string. Every message about a connection is redacted, and ingestion is
   CLI-only — a connection string passed through an MCP call is a credential in
   a transcript.
   Not claimed: PostgreSQL only, because the queries are `pg_catalog` queries.
   A catalog is a snapshot taken when `scan` ran; nothing watches the database.
   Drift is name-level — a table in both is "matched" even when its columns
   differ — and row data is never read.
- 9. **Closed.** Native text and metadata extraction (`src/extract.rs`), run by
   indexing before any agent is involved: PDF text layers, `.docx`, `.pptx`
   slide by slide, `.odt`/`.odp`, `.xlsx`/`.xlsm`/`.xls`/`.ods` sheet by sheet,
   `.csv`, `.svg` labels, image dimensions and authoring EXIF, and the
   `.srt`/`.vtt` transcript sitting beside a video. `.srt`, `.vtt`, `.rst`, and
   `.adoc` are indexed as text documents too. See [extract](extract.md).
   What comes back is the doc's description and goes through the same linking a
   `.md` file gets, so a design PDF naming `build_widget` gets an `Explains`
   edge into it in the first pass. Host-agent descriptions remain exactly what
   they were: `aag describe` still overwrites, because what a diagram *shows*
   beats the words printed on it.
   Not claimed: no OCR and no speech recognition. A scanned PDF, an unlabelled
   screenshot, and a video with no transcript beside it read as nothing — which
   leaves the empty description the host-agent path expects — and an image's
   metadata says where it came from, not what it depicts. Extraction is bounded
   at 40 000 characters and says so where it cuts, and a malformed document is
   skipped rather than fatal.
- 10. **Closed.** Shared MCP Streamable HTTP (`src/transport.rs`): sessions with
    `Mcp-Session-Id`, `DELETE` termination, and idle expiry; the same JSON-RPC
    answer framed as JSON or as one SSE `event: message` by `Accept`; a `GET`
    stream with keepalives; `--stateless` for a load balancer that will not pin a
    client; `--bind`, `--api-key`, `--max-body`, and `--rate-limit`; and container
    guidance in [transport](transport.md). Binding beyond loopback without a key
    refuses to start rather than exposing the repository, and the twelve HTTP
    behaviours are covered by tests that speak HTTP to a real socket.
    The `GET` stream is a real notification channel: the graph is published as
    the resource `aag://graph` (`resources/list`, `resources/read`,
    `subscribe: true`), and every reindex — watcher or reconcile — sends each
    open stream a `notifications/resources/updated` carrying the revision the
    client can read back, so a woken client can confirm which state it read.
    Not claimed: stdio has no such channel, because there is no stream to push
    into — a stdio client still asks again. Sessions live in the process that
    minted them, which is what `--stateless` is for.
- 11. **Closed.** Graph-backed pull-request workflow (`src/pr.rs`):
    `aag pr dashboard` ranks every open PR by what it reaches, `aag pr
    conflicts` reports the pairs about to collide, `aag pr worktrees` maps each
    local worktree to the PR on its branch, and `aag pr impact <n>` answers for
    one. MCP gained `pr_dashboard` and `pr_conflicts`. See [pr](pr.md).
    Risk is a stated table, not a judgement — +3 per touched hub symbol (10+
    dependents), +1 per 25 symbols of blast radius, +4 when affected tests exist
    and the PR changes none, +3 for failing checks, +2 for overlapping another
    open PR — and every point prints next to the rule that produced it.
    Overlap has three grades: same file (a merge conflict on the way), same
    symbol without the same file (the branches merge cleanly and still
    disagree — the one no diff shows), and same community (proximity, reported
    but not called a conflict).
    Not claimed: one `gh pr diff` per pull request, so 37 open PRs cost 37 round
    trips (~38s measured); attribution is by changed file rather than changed
    hunk, which over-reports on large files; and the graph describes the base
    tree, not each PR's head.
- 12. **Closed.** Outcome-backed work memory (`src/memory.rs`): question,
    answer, supporting nodes, outcome (`worked`/`wrong`/`open`), correction, and
    revision, as `aag memory save|correct|recall|lessons` and the MCP tools
    `memory_save`, `memory_recall`, `memory_lessons`. See [memory](memory.md).
    The gate's constraint is enforced rather than documented: every recalled entry
    is checked against the current graph and marked `stale` when the symbols it
    rested on are gone, an entry naming no symbols is stale because nothing can
    check it, an unrecognized outcome parses as `open` rather than as a success,
    and a lesson carries its entry count, how many the graph still supports, and
    its ids — a lesson about deleted code is labelled history. Both outputs open
    by saying memory is recorded experience and the graph wins any disagreement.
    Memory lives beside the graph and survives `--force`, because an index can be
    recomputed from source and what a session learned cannot.
    Not claimed: nothing is inferred beyond counting outcomes per symbol — no
    clustering, no embeddings, no model in the loop. A lesson needs two entries
    because one outcome is an anecdote, and memory is per repository.

### P2 — ecosystem breadth and distribution

- 13. **Closed.** Structural coverage extended and validated across the
    prioritized set — Vue, Svelte, Astro, Zig, PowerShell, Julia, Groovy,
    Verilog/SystemVerilog, Fortran, Pascal/Delphi, Salesforce Apex — plus
    Haskell, OCaml, Erlang, Clojure, Nim, Perl, and Solidity, taking the surface
    from 20 languages to 39.
    Validated is the operative word: a language is listed only once a test in
    `src/parse.rs` shows a snippet of it yields a declaration. A grammar existing
    in the pack is not coverage. Four grammars needed real work rather than a
    table entry: Vue/Svelte/Astro hand their script block back as one opaque
    node, so the block is extracted and handed to the JavaScript frontend with
    line numbers shifted; Clojure declares with a form (`(defn greet …)`) rather
    than a node kind; Groovy parses as a loose command soup; the rest declare
    with their own node kinds (Zig's `FnProto`, Fortran's `subroutine`, Perl's
    `subroutine_declaration_statement`, …) and their own name nodes.
    The markup half of a component file is indexed too, because a single-file
    component *is* a declaration: the file becomes a `Component` node, and every
    component element in its template becomes a call into the component that
    element names. That is the edge nothing else supplies — a globally
    registered child is rendered without an import, so parent and child look
    unrelated until the template is read. `PascalCase` or `kebab-case` is a
    component and `div` is not, which is the same rule the frameworks use.
    Groovy recognition is by shape rather than by keyword: a `func` followed by
    a brace is a method, so `String greet(who) { … }` and
    `static int add(a, b) { … }` are found alongside `def`, and `greet(1)` is
    not — the brace is what separates a declaration from a call.
    Not claimed: structural coverage, as the claim discipline below already
    says. An element whose name is built at runtime (`<component :is="x">`)
    names nothing and is skipped, and a Groovy declaration written without a
    parameter list is not a method to this rule.
- 14. **Closed.** Fourteen integrations, up from seven (`src/install.rs`): the
    original Claude Code, Cursor, Gemini CLI, Kiro, opencode, Codex, and
    Antigravity, plus VS Code / GitHub Copilot (`.vscode/mcp.json`, keyed
    `servers`), Windsurf (`~/.codeium/windsurf/mcp_config.json` + `.windsurf/
    rules/aag.md`), Zed (`.zed/settings.json`, keyed `context_servers`), Roo
    Code (`.roo/mcp.json` + `.roo/rules/aag.md`), Cline (`.clinerules/aag.md`),
    Crush (`.crush.json`, keyed `mcp`, `type: stdio`), and goose (a stdio
    extension in `~/.config/goose/config.yaml`). See
    [agent integration](agent-integration.md).
    Each shape is written as that agent documents it rather than assumed to be
    `mcpServers`, and every edit stays additive, idempotent, and reversible: a
    test installs all fourteen twice, asserts the second pass writes nothing,
    then uninstalls and asserts the neighbouring servers, extensions, themes,
    and provider settings are still there.
    Not claimed: Cline's MCP list lives in VS Code extension globalStorage,
    whose path varies by platform, VS Code flavour, and fork — writing there
    would be a guess, so only its rules file is written and the server is left
    to Cline's own UI. Antigravity stays UI-managed for the same reason. An
    `extensions:` map that only ever held `aag` is left behind empty rather
    than deleted, matching how the JSON configs are treated.
- 15. Generate repository-area skills from detected communities, entrypoints,
    processes, contracts, and cross-area links, with deterministic refresh.
- 16. Ship semantic search as an optional prebuilt distribution so npm users can
    enable embeddings without compiling Rust or packaging ONNX themselves.
- 17. Add branch-aware indexes and explicit comparisons between workspace,
    branch, commit, and PR graph states.
- 18. Improve public onboarding and proof: architecture docs, benchmark history,
   compatibility matrix, migration notes, example corpora, screenshots,
   multilingual quickstarts, and release automation for every supported
   platform.

## P0.1 empirical evaluation contract

### Scope and naming

AAG Protocol, AboveAllGraphs Engine, and an end-to-end AAG agent workflow are
separate evaluation subjects. A benchmark must identify which one it measures
and must not transfer a result from one layer to another.

- Protocol evaluation measures manifest validity, semantic conformance,
  portability, declared/observed separation, evidence, uncertainty, freshness,
  and consumer interpretation.
- Engine evaluation measures discovery, extraction, resolution, indexing,
  storage, queries, incremental reconciliation, compilation, and operational
  performance of a named implementation.
- End-to-end evaluation measures the complete producer to compiler to context
  to consumer path, including task quality and all generation, update, query,
  and consumption costs.
- A public result must use the terms protocol benchmark, engine benchmark, or
  end-to-end benchmark explicitly. The unqualified term AAG benchmark is not
  sufficient when the tested implementation could be misunderstood.

### Required evaluation tracks

- Track A, protocol conformance: validate schema and semantic rules, stable
  identifiers, ownership preservation, evidence references, uncertainty,
  freshness, exact version handling, and producer/consumer compatibility.
- Track B, engine extraction: measure entity and relationship precision and
  recall by type, caller and callee recall, resolution ambiguity, flow
  continuity, contract matching, impact false positives and false negatives,
  and affected-test accuracy.
- Track C, agent utility: compare raw-repository access against fixed reference
  manifests, AboveAllGraphs-produced manifests, LLM-only manifests, hybrid
  manifests, and task-specific context slices while holding the consumer model
  and task constant.
- Track D, end-to-end economics: include cold generation, incremental updates,
  manifest compilation, query execution, context tokens, consumer tokens,
  model calls, wall time, and the number of tasks required to break even.
- Track E, scale and operations: measure indexing time, incremental update time,
  query latency distributions, peak memory, database size, export size, graph
  UI preparation, and partial-analysis behavior across versioned repository
  tiers.

### Producer and consumer separation

The benchmark abstraction is a producer, not a builder model. A producer may
be the AboveAllGraphs Engine, an LLM-only agent, a deterministic reference
implementation, a hybrid engine plus model, or another conforming tool.

Every run must record:

```text
producer name, version, commit, configuration, and capabilities
optional builder model, provider, model ID, and parameters
consumer provider, model ID, tier, parameters, and tool permissions
engine and compiler versions when either contributes
protocol and manifest schema versions
repository identity, revision, working-tree state, and scope
task, ground-truth, manifest, configuration, and result hashes
run kind, timestamp, environment, random seed, and repetition index
```

The minimum transfer matrix compares raw repository access, a reference
manifest, an AboveAllGraphs Engine manifest, and an LLM-only manifest with at
least one compact and one frontier consumer. Changing a producer and consumer
in the same comparison without a factorial design does not isolate causality.

### Run classes and evidence isolation

- Empirical runs execute the declared producer and consumer against the pinned
  repository and retain their raw logs and artifacts.
- Pilot runs exercise benchmark infrastructure or dogfood on a repository
  closely related to the protocol or implementation under test.
- Simulated runs test schemas, aggregation, and reporting without executing the
  claimed system.
- Simulated, pilot, and empirical data must live in separate immutable paths
  and carry a machine-readable `run_kind`.
- Default aggregation and every public quality or performance claim must use
  empirical runs only. Including another class requires an explicit flag and a
  visibly separate report.
- A self-benchmark against AAG Protocol is a pilot. It is not evidence of
  external generalization, language breadth, or industrial scale.
- Raw run records are append-only. Corrected evaluation logic creates a new
  derived result rather than rewriting the original execution record.

### Repository tiers

Small, medium, large, and industrial tiers are corpus labels, not claims based
only on file count. Each repository profile must record source files, logical
lines, symbols, relationships, languages, packages, services, entrypoints,
tests, generated-code share, dependency topology, history depth, and any
private or unavailable dependency boundary.

- Every public tier contains external repositories or immutable fixtures that
  were not used to design the evaluated tasks or tune the engine.
- Industrial claims require a real qualifying corpus or a clearly labeled
  synthetic stress corpus. Synthetic scale cannot establish real-world task
  accuracy.
- Ground truth and tasks remain versioned independently from producer output.
  Task authors, ground-truth reviewers, producers, consumers, and evaluators
  must be separated sufficiently to prevent answer leakage.

### Statistics and release gates

- Run stochastic conditions repeatedly and publish sample count, failures,
  mean, median, dispersion, and confidence intervals where applicable.
- Report precision and recall per entity and relationship type before any
  macro or micro average so rare or difficult relationships cannot disappear
  inside one headline number.
- Report query and update latency as distributions including p50 and p95, not
  only a best or average time.
- Retain unsuccessful runs and classify infrastructure failure, producer
  failure, consumer failure, invalid output, timeout, and evaluator failure.
- Version benchmark schemas, adapters, corpora, tasks, and ground truth.
  Results from incompatible versions must not be merged silently.
- Completing schemas, mock adapters, or simulated runs does not complete P0.1.
  The gate closes only after reproducible empirical engine and end-to-end runs
  on external corpora are published with immutable raw evidence.

### AboveAllGraphs adapter requirements

The neutral AAG ScaleBench may live with the protocol, but an official
AboveAllGraphs producer adapter is required for engine claims. It must invoke a
released or commit-pinned binary, record the exact command and build features,
compile the resulting graph into the declared manifest version, retain query
and update logs, and expose engine-native metrics without asking an LLM to
reconstruct facts already produced by the graph.

LLM-only and hybrid producers remain valuable baselines. They are comparisons,
not substitutes for executing the AboveAllGraphs Engine.

## P0.3 graph experience redesign

### Product outcome

The graph must help a developer answer a code question, not merely prove that
the repository contains nodes and edges. Its default state must communicate
system shape, its focused states must remove irrelevant topology, and every
visible relationship must remain traceable to source evidence.

The redesign is successful when a first-time user can identify the principal
modules, inspect one symbol, trace a path, and understand a change's blast
radius without learning graph-layout controls or manually hiding most of the
repository.

### Existing baseline to preserve

- Keep the self-contained offline HTML export with vendored Sigma.js,
  Graphology, and layout code. The graph must not require a CDN or hosted AAG
  service.
- Preserve WebGL rendering, deterministic seeded placement, community
  detection, search, file and node-kind filters, confidence filters, hover
  highlighting, drag, camera controls, details, wiki links, deep-link focus,
  and configurable layout settings.
- Treat the current ForceAtlas2 plus no-overlap implementation as one available
  layout primitive, not the universal presentation for every question.
- Preserve source provenance and confidence values through every collapsed,
  bundled, or summarized view.

### Information architecture

- Put the current task at the top of the interface with an explicit mode
  switcher: Overview, Explore, Path, Impact, and Contracts.
- Keep global search, repository identity, current mode, active filters, and
  navigation history visible without competing with the canvas.
- Use one consistent inspector for nodes, edges, communities, and paths. The
  inspector must show kind, file and line, confidence, evidence, incoming and
  outgoing relation counts, community, and direct source/wiki actions.
- Replace the always-expanded control surface with progressive controls:
  common actions first, graph tuning and diagnostics second.
- Make every meaningful state shareable through the URL, including mode,
  focus, endpoints, direction, depth, relation filters, confidence filters,
  selected community, and pinned nodes.

### Overview mode

- Open large repositories as collapsed semantic communities or modules rather
  than rendering every symbol and edge immediately.
- Size communities by contained symbols and encode internal cohesion,
  externally connected modules, entrypoints, and risk concentration without
  relying on color alone.
- Expand and collapse communities in place while preserving the user's mental
  map and pinned positions.
- Summarize inter-community edges by relation kind, direction, confidence, and
  count. Expanding a summary must reveal the supporting relationships.
- Provide useful labels derived from directories, dominant files, exported
  symbols, and detected processes instead of opaque numeric community names.

### Explore mode

- Center a selected symbol, file, process, or community and initially show only
  the most relevant one-hop neighborhood.
- Allow independent upstream and downstream depth expansion with hard result
  limits and a preview of how many hidden nodes each expansion would add.
- Rank neighbors by relation semantics, confidence, process participation, and
  structural importance instead of treating every edge as equally useful.
- Preserve a breadcrumb and back/forward history so exploration never loses
  the previous context.
- Support pinning, multi-selection, compare, hide, isolate, and return-to-
  overview without recalculating unrelated positions.

### Path mode

- Render a selected source-to-target path as a directed layered sequence rather
  than a force graph.
- Distinguish calls, imports, contracts, implementations, schemas, and inferred
  hops by line style, arrow treatment, and labels.
- Show alternative shortest paths and explain why each hop exists, including
  file, line, confidence, and provenance.
- Permit controlled side-context expansion around any hop without destroying
  the primary path.
- Expose copyable CLI and MCP equivalents for reproducing the visible query.

### Impact mode

- Separate upstream dependents from downstream dependencies around the changed
  symbol, file, or diff.
- Group impact by depth, module, relation kind, confidence, production code,
  and test code.
- Mark directly changed, directly affected, transitively affected, and
  ambiguous nodes distinctly without depending only on color.
- Show affected tests, entrypoints, contracts, and public APIs before generic
  transitive nodes.
- Permit depth and confidence changes without resetting selection, camera, or
  expanded groups.

### Contracts mode

- Visually separate declared contract nodes from observed implementation nodes.
- Pair OpenAPI, SQL, Terraform, package, event, and MCP declarations with their
  matched implementations and consumers.
- Surface missing implementations, undocumented observations, conflicting
  evidence, and ambiguous matches as first-class states.
- Make provenance inspectable from every contract edge and preserve multiple
  sources when they disagree.

### Visual and interaction system

- Define a stable visual grammar for node kind, relation kind, direction,
  confidence, selection, search result, change state, and provenance. Do not
  encode two independent dimensions with the same color channel.
- Add semantic zoom: communities and file labels at distance, important
  symbols at medium zoom, complete symbol and edge detail only when focused.
- Prevent label collisions and suppress low-value edges until hover, selection,
  or sufficient zoom. Edge labels must remain readable in both directions.
- Use edge aggregation or bundling in overview states while retaining a direct
  drill-down to every original edge.
- Provide ranked search results with kind, path, signature, and community;
  keyboard navigation; exact and fuzzy matching; and recent queries.
- Add a command palette for mode changes, focus, path, impact, filters, reset,
  fit, and share actions.
- Support keyboard-only navigation, visible focus, reduced motion, sufficient
  contrast, non-color status cues, screen-reader labels for controls, and a
  usable inspector at narrow desktop widths.

### Performance and stability budgets

- Define public benchmark fixtures for small, medium, and large repositories
  and record browser, operating system, hardware, node count, and edge count.
- Keep camera, hover, and selection responsive while visible complexity is
  bounded; expensive neighborhood or layout work must not block input.
- Make the default large-repository overview render collapsed communities
  instead of sending the complete raw graph to the visible scene.
- Run layout, filtering, aggregation, and path preparation in workers when the
  benchmark shows main-thread stalls.
- Cache layout by graph fingerprint and mode so reloads and navigation preserve
  stable positions. Invalidation must be deterministic after graph changes.
- Establish measured release gates for initial interaction, search response,
  filter response, frame rate, layout duration, peak memory, and export size
  before calling the redesign complete.

### Delivery order

- 3A. Research and task model: test the current graph against the Overview,
  Explore, Path, Impact, and Contracts jobs; capture failure cases and choose
  representative benchmark repositories.
- 3B. Interaction and visual specification: produce wireframes, state diagrams,
  visual tokens, semantic-zoom rules, keyboard behavior, responsive behavior,
  and URL-state schema before rewriting the renderer.
- 3C. Graph presentation foundation: introduce mode state, layout adapters,
  community collapse, aggregation, stable layout caching, unified selection,
  history, inspector, and shareable state.
- 3D. Task views: ship Overview and Explore first, then Path and Impact, then
  Contracts. Each view must be useful independently and reuse the same graph
  semantics rather than duplicating query logic in the browser.
- 3E. Scale, accessibility, and hardening: add worker offloading, performance
  telemetry for local tests, keyboard and screen-reader coverage, responsive
  behavior, error and empty states, and visual regression fixtures.
- 3F. Public proof: publish screenshots, short task demonstrations, benchmark
  results, and an explicit comparison using the same repositories and tasks as
  competing graph tools.

### Definition of done

- A fresh load presents a comprehensible module overview rather than an
  unfiltered symbol hairball.
- Reloading the same graph produces stable placement, and switching modes does
  not unexpectedly discard focus, filters, history, or pinned context.
- A user can find a symbol, inspect evidence, expand its neighborhood, trace a
  path, and open impact analysis from the graph without returning to the CLI.
- Every aggregate, community, path hop, and impact result can be expanded to
  the original nodes, edges, source locations, confidence, and provenance.
- The five modes have automated interaction tests and visual regression
  baselines for light/dark behavior, empty data, small graphs, and benchmark
  large graphs.
- Performance budgets are measured in CI against versioned fixtures, and a
  regression blocks release rather than being documented after publication.
- Accessibility checks include keyboard completion of core tasks, visible
  focus, reduced motion, contrast, and status cues that do not rely on color.
- The offline export remains self-contained and usable with networking
  disabled.

## Current differentiators

- The AAG Protocol is an independent, language-agnostic contract rather than a
  serialization detail of one indexer.
- Declared contracts and observed implementation coexist with explicit
  provenance, evidence sources, and preserved uncertainty.
- File-level incremental updates and the native watcher keep local graphs
  fresh without requiring a full manual rebuild after every edit.
- The lightweight binary, offline static site, local SQLite source of truth,
  and MIT license make deployment and commercial adoption straightforward.
- Hierarchical groups federate independent repositories without requiring a
  central graph service or sacrificing per-repository ownership.

## Claim discipline

- Thirty-nine frontends means structural coverage, not equal semantic depth in
  all thirty-nine languages. Rust and JavaScript have the deepest extraction;
  flow analysis covers seven; the rest are declarations, calls, imports, and
  heritage.
- Protocol conformance or protocol utility does not prove AboveAllGraphs Engine
  extraction quality, performance, or scale.
- LLM-only manifests and simulated runs cannot substantiate engine claims.
- Dogfood against AAG Protocol is a pilot and does not prove external
  generalization.
- Engine and best-in-class claims require empirical runs that identify the
  producer, consumer, engine build, protocol version, corpus, task, ground
  truth, and immutable raw result.
- The query surface is a documented subset of Cypher with real pattern
  evaluation, not full Cypher and not a query planner. Every unsupported clause
  is an error naming what was expected — see [query](query.md).
- Receiver typing is syntactic — construction, annotation, and enclosing type.
  It is not type inference, and must not be described as one. Call edges stay
  INFERRED or AMBIGUOUS accordingly.
- Detected routes are what the implementation registers, not what a contract
  declares. An observed endpoint is evidence of code, not of a published API.
- Route, tool, and consumer detection is by name across a fixed set of
  frameworks and client libraries. "No endpoints found" means none matched those
  shapes, not that the repository serves none.
- Taint findings are syntactic and line-granular, and crossing a call does not
  change that. A finding is a place to look, no findings is not evidence of
  safety, and neither `aag taint` nor the `taint` tool is a security scanner.
- Binary Office/media files are graph nodes, but native content extraction is
  still a priority gate.
- Group queries aggregate independent graphs. Cross-repository links are name
  agreement across an ownership boundary — API, package, event, schema, and tool
  — not unified symbol resolution, and each link says what evidence produced it.
- Optional source-build embeddings must not be presented as part of the
  standard npm binary.
- The current graph UI is usable, not finished. Visual quality, navigation,
  and large-graph comprehension are tracked as P0 product requirements.
