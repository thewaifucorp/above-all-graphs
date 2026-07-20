---
title: Capability coverage
---

# Capability coverage

This page is the regression contract for the AAG surface that overlaps GitNexus and Graphify. A capability counts only when it is callable and tested; the MCP test `every_advertised_tool_is_implemented` prevents placeholder tools.

## Implemented core

- 20 structural language frontends with one language-neutral graph model
- confidence-tagged calls, imports, inheritance, implementation, documentation, contract, schema, and infrastructure relations
- graph-aware natural-language search, node context, incoming/outgoing neighbors, impact, affected tests, shortest path, god nodes, communities, and execution processes
- coordinated rename, read-only Cypher, diff change detection, wiki, report, GraphML, JSON, Cypher export, Obsidian export, and interactive WebGL UI
- OpenAPI/Swagger operations, parameters, bodies, responses, security, schemas, `$ref`, implementation matching, SQL DDL/foreign keys, and Terraform/HCL resources
- PR listing, triage, and graph impact through the read-only GitHub CLI
- multi-workspace query, status, contracts, and synchronization
- protocol compiler, structural validation, semantic validation, provenance, declared/observed separation, and automatic SQLite migration
- local MCP, hooks, skills, watcher reconciliation, and integrations for Claude Code, Cursor, Codex, Gemini CLI, Kiro, opencode, and Antigravity

## Remaining parity gates

- optional vector embeddings and semantic RRF in addition to current lexical/structural ranking
- file-level incremental graph mutation instead of correct full reconciliation
- Streamable HTTP MCP transport with API-key authentication
- direct live PostgreSQL catalog ingestion
- native text extraction from Office/media binaries instead of host-agent descriptions
- named hierarchical repository groups instead of the current all-workspaces federation

These gates must not be described as implemented until their tests and callable surfaces land.
