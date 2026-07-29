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

## Emitting a document from the code

`aag api spec` is the inverse of the ingestion above: it writes an OpenAPI 3.1
document for the surface this repository actually serves.

```
aag api spec                      # to standard output
aag api spec --out openapi.json   # to a file
aag api spec --include-declared    # add operations no code serves
```

What the graph knows becomes the document: paths, methods, the symbol serving
each route with its `file:line`, how many callers it has, and whether a contract
also declares it — under `x-aag` on each operation.

What the graph does not know is left out and said so. Nothing in a handler
declares its response shape, so every operation carries a single `default`
response whose description states that the shape is not inferred; there is no
invented `200 OK`. Route parameters are the exception, because the path itself
declares them: `:id`, `<int:id>` and `{id}` all become `{id}`, required, typed as
a string, which is the only honest guess available.

Two more choices worth knowing. Operations a contract declares but no code
serves are excluded unless `--include-declared` is passed, since emitting them
would present a promise as an implementation — and they are already in the
document that declared them. RPC and MCP tools are not HTTP and are not dressed
up as routes: they are listed under a top-level `x-aag-tools` array with their
handlers.

`operationId` is the handler's name when there is one, because that is what a
reader already calls the route; collisions get a numeric suffix so the document
stays valid.
