# Changelog

All notable changes to AboveAllGraphs are documented here.

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
