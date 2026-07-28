---
wiki: src/federation.rs
---

# federation.rs

Named hierarchical repository groups, federated queries, and cross-repository
protocol links. This page covers the links — P1.7 of
[capability coverage](capability-coverage.md); grouping itself is in
[workspaces](workspaces.md).

## Selection, not unification

Each repository keeps its own `.aag/graph.db`, its own node ids, and its own
ownership. A link is computed by reading each member's graph separately and
matching names across the boundary — never by merging databases into one. That is
the whole design constraint of the gate, and it is why a link carries the evidence
that produced it instead of appearing as an ordinary edge.

```bash
aag group links platform/backend    # or `all` for every registered workspace
```

Over MCP: the `group_links` tool, same argument.

## The five kinds

| Kind | Producing half | Consuming half | Evidence |
|---|---|---|---|
| `api` | An endpoint a repository serves or declares | An outbound call in another | The paths match, exactly or once parameters are flattened |
| `package` | A package name in a repository's own manifest | An import naming it | `@shop/service` is published here, imported there |
| `event` | `emit('order.created')` | `subscribe('order.created')` | Publisher and listener name the same event |
| `schema` | A schema a contract declares | A type of the same name | A declared `Order` and a class `Order` |
| `tool` | A tool definition | `call_tool('lookupOrder')` | The call names a tool defined there |

Package names come from the repository's own `package.json`, `Cargo.toml`, or
`go.mod`. No registry is consulted and nothing is fetched.

## What a link is, and is not

Every one of these is **a name agreeing across an ownership boundary**. That is
real evidence and it is not proof:

- Two repositories can name the same event and mean different things.
- A declared `Order` schema and a class called `Order` may be unrelated types
  that happen to share a noun.
- A path built from literals is folded and matched (see [api](api.md)), but one
  that only exists at runtime is not, so an absent link is not evidence that no
  call exists. Event and tool names are matched strictly: a folded event name
  would link two halves that may never meet, and a wrong link is worse than a
  missing one.
- The `api` link's evidence line says which of the two matches it was: an exact
  path or a flattened one.

Locally — inside one repository — the same three name-keyed relations are
ordinary graph edges, resolved during indexing: a publisher gets a `References`
edge to every listener of that event (INFERRED, because a string match is
evidence rather than certainty), and a tool invocation gets a `Calls` edge into
the tool definition it names. That second one matters because a dispatcher hides
the link: a definition table says a tool exists, and only a call site naming it
says who uses it.

A member whose graph cannot be read is reported in `unreadable` rather than
failing the group — one un-indexed repository must not take the answer down.
