# AAG Protocol Conformance

This document defines the normative boundary between the AAG Agent Protocol
and software that implements it. The key words **MUST**, **MUST NOT**,
**SHOULD**, **SHOULD NOT**, and **MAY** are to be interpreted as described in
RFC 2119 and RFC 8174 when, and only when, they appear in all capitals.

## Protocol boundary

The protocol defines portable artifacts and their semantics. It does not
prescribe a parser, agent, programming language, graph model, graph database,
storage engine, transport, compiler architecture, or query API.

An implementation owns the mechanisms used to discover facts, retain them,
resolve relationships, update them, and compile them into an AAG Context
Manifest. Implementations may consume the schemas as source files, packaged
resources, generated types, or a pinned external dependency.

## Producer conformance

A producer is any agent, compiler, exporter, or other tool that creates or
updates an AAG Context Manifest.

A conforming producer:

1. **MUST** emit a manifest valid against the exact
   `aag.manifest.schema.json` version declared by `aag_manifest.version`.
2. **MUST** set `aag_manifest.protocol_version` to the exact protocol version
   whose semantics it followed.
3. **MUST** identify itself with `generator.name`, `generator.version`, and
   `generator.capabilities`.
4. **MUST NOT** claim a capability that did not contribute to that manifest.
5. **MUST** attach evidence to every observed fact for which the schema
   requires evidence.
6. **MUST** preserve human-managed content and the origin of shared content.
7. **MUST NOT** present documentation, naming conventions, or plausible
   inference as directly observed implementation.
8. **MUST** apply the semantic validation rules below before reporting
   successful generation.
9. **MUST NOT** report `freshness.status: current` unless the declared scope,
   analyzed revision, working-tree state, and generated observations agree.
10. **SHOULD** generate deterministic ordering and stable identifiers when the
    underlying logical facts have not changed.

## Consumer conformance

A consumer is any tool that reads an AAG Context Manifest.

A conforming consumer:

1. **MUST** validate or otherwise verify the declared manifest version before
   interpreting fields.
2. **MUST NOT** silently interpret an unsupported version using different
   semantics.
3. **MUST** preserve the distinction between declared, observed, historical,
   and unresolved information.
4. **MUST** surface freshness and uncertainty when using facts for a decision.
5. **MUST NOT** treat custom `x-*` vocabulary as core vocabulary unless it
   explicitly implements that extension.
6. **SHOULD** reject invalid internal references instead of dropping them
   silently.

## Compiler conformance

A compiler is a producer that transforms implementation-specific observations
or a knowledge graph into an AAG Context Manifest. A compiler may be embedded
in a product or distributed independently.

In addition to producer requirements, a conforming compiler:

1. **MUST** keep implementation-specific identifiers from leaking into core
   semantics unless they are stable and satisfy the protocol ID convention.
2. **MUST** map unsupported domain concepts to valid `x-entity-*` or
   `x-relation-*` extensions rather than redefining core types.
3. **MUST** resolve duplicate facts deterministically or retain their distinct
   evidence without silently discarding conflicts.
4. **MUST** invalidate derived facts when their supporting observations become
   stale or disappear.
5. **SHOULD** produce a diff before overwriting an existing manifest.

The compiler's input representation is intentionally outside this version of
the protocol. This allows AST tools, agents, runtime tracers, relational stores,
property graphs, RDF stores, and in-memory graphs to implement the same output
contract without adopting a shared runtime.

## Semantic validation

JSON Schema validates document shape. A conforming producer additionally
checks all of the following:

- every ID is globally unique in the manifest;
- every entity, relationship, entrypoint, flow, side-effect, finding, and
  uncertainty reference resolves to an existing compatible object;
- flow step links resolve within the same flow and do not violate declared
  ordering or continuity;
- evidence source paths remain inside the configured repository scope and do
  not match forbidden secret patterns;
- evidence locations use valid line ordering when both line bounds exist;
- the manifest and protocol versions are compatible;
- `freshness` metadata agrees with the analyzed and current revisions;
- custom vocabulary uses the reserved `x-entity-*`, `x-relation-*`, or `x-*`
  extension namespaces;
- capability claims are consistent with the evidence present in the manifest.

Schema-valid output that fails any applicable semantic rule is not conforming.

## Capability vocabulary

Capabilities describe how a particular manifest was produced, not every
feature the implementation supports in general.

| Capability | Meaning |
| --- | --- |
| `static_analysis` | Source structure or behavior was extracted without executing application code. |
| `runtime_observation` | Authorized execution traces, logs, or runtime probes contributed evidence. |
| `history_analysis` | Version-control history contributed historical findings. |
| `incremental_update` | The producer reconciled changes against a previous analysis. |
| `semantic_validation` | The producer executed the semantic checks applicable to the manifest. |
| `task_context_selection` | The producer generated or evaluated task-oriented context slices. |

Experimental capabilities use the `x-*` namespace. They MUST NOT weaken core
conformance requirements.

## Version compatibility

During `0.x`, consumers and producers MUST use exact protocol and schema
versions unless a published migration explicitly declares compatibility.
Implementations SHOULD pin a release or immutable revision rather than follow a
moving branch.
