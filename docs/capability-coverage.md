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
- Twenty structural language frontends sharing one graph model. Rust and
  JavaScript have dedicated extraction paths; the remaining languages use the
  tree-sitter language pack plus AAG declaration and call extraction.
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
  handlers, alongside the endpoints a contract file declares.
- Graph-aware search, node context, neighbors, impact, affected tests,
  shortest path, god nodes, communities, detected entrypoints, and execution
  processes rooted at them.
- Coordinated whole-word rename, a limited read-only Cypher-shaped query
  surface, diff change detection, wiki, report, GraphML, JSON, Cypher export,
  Obsidian export, and an offline WebGL graph.
- OpenAPI and Swagger operations, parameters, bodies, responses, security,
  schemas, references, implementation matching, SQL DDL and foreign keys, and
  Terraform/HCL resources.
- Basic PR listing, filtering, and graph impact through the read-only GitHub
  CLI.
- Multi-workspace query and named slash-hierarchical repository groups while
  keeping each repository graph independent.
- AAG Protocol compilation, structural and semantic validation, provenance,
  declared/observed separation, uncertainty preservation, and automatic
  SQLite migration.
- Optional local embeddings through fastembed/ONNX with lexical, semantic,
  and structural reciprocal-rank fusion. The standard prebuilt npm binary is
  intentionally lightweight and does not include this feature yet.
- True file-level incremental parsing with persisted unresolved references,
  global edge re-resolution, watcher reconciliation, and agent hooks.
- MCP over stdio and authenticated loopback HTTP request/response. The HTTP
  transport is not yet a shared Streamable HTTP/SSE service.
- Integrations for Claude Code, Cursor, Codex, Gemini CLI, Kiro, OpenCode, and
  Antigravity.

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
- 3. Complete the graph experience redesign specified below. Preserve the
   fast offline renderer, but replace the single hairball-oriented interaction
   model with task-specific overview, explore, path, impact, and contract
   views. The current graph is functional but not yet a competitive visual
   product.
- 4. Replace the current Cypher-shaped shim with either a documented formal
   subset and real pattern evaluation or a genuine graph query engine. Do not
   advertise arbitrary Cypher until query semantics match the claim.
- 5. Add statement-level control-flow and data-flow foundations: basic blocks,
   CFG, def-use, control/data dependence, PDG queries, and provenance-aware
   source-to-sink taint findings, starting with TypeScript and JavaScript.

### P1 — complete workflows and heterogeneous graphs

- 6. Build first-class route, RPC, and tool intelligence: route maps, MCP/RPC
   tool maps, API consumer links, response-shape checks, and API impact. Use
   OpenAPI declarations as evidence instead of replacing them with framework
   inference.
- 7. Resolve cross-repository protocol links without merging local databases:
   API producer to client, package export to import, event producer to
   consumer, schema to model, and MCP tool definition to invocation.
- 8. Ingest live PostgreSQL catalogs, including schemas, tables, columns,
   constraints, indexes, views, and foreign keys, with credentials kept out of
   graph exports and logs.
- 9. Add native text and metadata extraction for PDF, DOCX, XLSX, presentations,
   images, audio/video transcripts, URLs, papers, and workspace documents.
   Host-agent descriptions remain valid additional evidence, not the only
   extraction path.
- 10. Implement shared MCP Streamable HTTP with session management, SSE and JSON
    responses, stateless mode, configurable bind address, authentication,
    rate/size limits, and container deployment guidance.
- 11. Turn PR primitives into a full graph-backed workflow: dashboard, worktree
    mapping, conflict detection through shared communities, review queue,
    risk-ranked findings, and plan to work to review gates.
- 12. Add outcome-backed work memory: save a question, answer, supporting nodes,
    result, correction, and code revision; derive reviewable lessons without
    allowing stale experience to override current source evidence.

### P2 — ecosystem breadth and distribution

- 13. Expand and validate structural coverage to the wider language set used by
    Graphify, prioritizing Vue, Svelte, Astro, Zig, PowerShell, Julia, Groovy,
    Verilog/SystemVerilog, Fortran, Pascal/Delphi, and Salesforce Apex.
- 14. Expand agent installers and uninstallers from seven integrations to the
    broader cross-framework surface, while keeping every edit additive,
    idempotent, and reversible.
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

- Twenty frontends means structural coverage, not equal semantic depth in all
  twenty languages.
- Protocol conformance or protocol utility does not prove AboveAllGraphs Engine
  extraction quality, performance, or scale.
- LLM-only manifests and simulated runs cannot substantiate engine claims.
- Dogfood against AAG Protocol is a pilot and does not prove external
  generalization.
- Engine and best-in-class claims require empirical runs that identify the
  producer, consumer, engine build, protocol version, corpus, task, ground
  truth, and immutable raw result.
- The current query surface is not full Cypher.
- Receiver typing is syntactic — construction, annotation, and enclosing type.
  It is not type inference, and must not be described as one. Call edges stay
  INFERRED or AMBIGUOUS accordingly.
- Detected routes are what the implementation registers, not what a contract
  declares. An observed endpoint is evidence of code, not of a published API.
- Binary Office/media files are graph nodes, but native content extraction is
  still a priority gate.
- Group queries aggregate independent graphs; cross-repository symbol and
  contract resolution is not complete until priority 7 lands.
- Optional source-build embeddings must not be presented as part of the
  standard npm binary.
- The current graph UI is usable, not finished. Visual quality, navigation,
  and large-graph comprehension are tracked as P0 product requirements.
