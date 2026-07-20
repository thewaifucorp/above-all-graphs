---
title: OpenAPI contracts and provenance
---

# OpenAPI contracts and provenance

During `index_repo`, AAG recognizes OpenAPI 3.x and Swagger 2.x JSON or YAML files. Each HTTP operation becomes a declared graph fact with `openapi_contract` evidence. Source symbols extracted from ASTs remain observed facts.

When an operation has an `operationId`, `resolve_openapi_operations` looks for a source symbol with the same name. Without one, method/resource candidates such as `listPets`, `getPet`, `createPet`, and `deletePet` are tried. A match creates an `Implements` edge; no match is retained as an unmatched contract operation instead of being silently discarded.

`Graph` stores `Perspective`, `EvidenceKind`, and the evidence source on every node and edge. Existing SQLite indexes migrate automatically with observed defaults.

The protocol compiler exports implementation entities under `perspectives.observed`. OpenAPI operations are emitted under `extensions.x-aag-declared-contracts`, including `matched` or `unmatched` implementation status. This keeps the current protocol schema conforming while preserving the declared-versus-observed distinction.

Run:

```
aag sync --path .
aag export --path .
aag validate .aag/context.yaml
```

The comparison retains the complete operation object and its referenced schemas. Framework-specific decorator extraction and runtime traces remain additional evidence sources rather than requirements for contract ingestion.
