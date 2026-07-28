---
wiki: src/api.rs
---

# api.rs

Route, RPC, and tool intelligence: what this repository serves, what serves it,
and who consumes it. This is P1.6 of
[capability coverage](capability-coverage.md).

The gate's own constraint is the rule the module follows: **a declaration is
evidence, not a replacement for the implementation — and the reverse.** So three
things are first-class results rather than omissions:

- a contract declares an endpoint and no code implements it,
- code serves an endpoint and no contract declares it,
- both exist, and the handler returns a different shape than the contract
  promises.

## Where the data comes from

Nothing here re-indexes a repository. It reads what indexing already produced:

| Fact | Produced by |
|---|---|
| Declared endpoints, schemas, response shapes | [openapi](openapi.md) — `Endpoint`/`Schema` nodes, perspective `Declared` |
| Served endpoints and their handler | [resolve](resolve.md) — `Endpoint` nodes, perspective `Observed`, `Implements` edge from the handler |
| RPC/MCP tools | `resolve` — `Endpoint` nodes whose method is `TOOL` |
| Consumers | `resolve` — `Calls` edge from the calling symbol into the endpoint |

Only the response-shape check re-parses anything, because the fields a handler
returns are inside its body and no edge carries them.

## Tools are endpoints

An MCP or JSON-RPC tool is a callable contract in the same sense an HTTP route
is: something outside the process invokes it by name. So a tool is an `Endpoint`
node whose method is `TOOL` and whose path is the tool name, rather than a second
vocabulary — impact, contracts, the query surface, and the graph UI then treat
both without special cases.

Three definition forms are recognized:

```js
server.tool('search', searchDocs);        // a registration call
```
```python
@mcp.tool()                                # a decorator; the name defaults to
def search_docs(query): ...                # the handler's own
```
```rust
const SPECS: &[ToolSpec] = &[ToolSpec { name: "explore", … }];  // a tool table
```

A table entry names no handler — a dispatcher routes the name at runtime — and
the output says exactly that instead of leaving the field blank.

## Consumers

An outbound HTTP call is an edge into the endpoint it requests, which is what
makes "who calls this API" the same question as "who calls this function".

```js
fetch('/pets')                      // GET, from the literal
fetch('/pets', {method: 'POST'})    // POST, from the options object
client.get('/pets/42')              // GET
```

A call and a registration are told apart by receiver first and argument count
second: `app.get('/x', handler)` serves, `client.get('/x')` consumes. That order
matters because `app.get('/x', (req, res) => …)` and `axios.get('/x', config)`
have the same shape.

The link's confidence says how it was made:

- **EXTRACTED** — the call names the endpoint exactly (`/pets` → `GET /pets`).
- **INFERRED** — it matches once path parameters are flattened (`/pets/42` →
  `GET /pets/{id}`).
- **AMBIGUOUS** — several endpoints match that shape, and every candidate is
  linked, the same fan-out an ambiguous call between symbols gets.

## The four views

```bash
aag api routes [filter]      # every endpoint, its state, handler, consumers
aag api tools [filter]       # every RPC/MCP tool and its handler
aag api shapes [filter]      # declared response shapes vs what handlers return
aag api impact "GET /pets"   # who is on the other side of one contract
```

Over MCP the same four are `route_map`, `tool_map`, `shape_check`, and
`api_impact`.

`routes` ends with the two mismatch lists, because a reader scanning the map
should not have to notice an absence:

```text
1 declared endpoints no code implements: GET /archived
1 served endpoints no contract declares: GET /health
```

Endpoints pair by *shape*, not by spelling: a contract's `/pets/{id}` and a
framework's `/pets/:id` are one endpoint. Reporting them separately would list
every parameterized route twice — once as unimplemented, once as undeclared —
which is the kind of noise that trains a reader to ignore the output. The
contract's spelling is the name that shows, since that is what was published.

## The shape check

For each endpoint a contract declares with a success-response schema, the
declared field names are compared against the keys of the object literals its
handler returns or hands to a JSON responder (`res.json`, `jsonify`, `send`, …).
A `$ref` is followed into the schema it names, and an array schema is unwrapped
to its items.

Both sides are flattened to dotted paths, so nesting is compared rather than
skipped:

```text
GET /orders — listOrders
     declared but never returned: customer.name
     of those, required by the contract: customer.name
```

A handler rarely writes its response as one literal in the `return`, so the
body is followed to where it was assembled:

- A body bound to a local variable is followed through the binding
  (`const body = {…}; res.json(body)`).
- A field whose value is another local variable is followed too, and its keys
  land under that field's path (`{ customer }` gives `customer.name`).
- A spread merges the other object's fields at the same level
  (`{ ...base, id }`).
- Recursion stops at the first name that resolves back to itself, and nesting
  is compared four levels deep — a contract that nests deeper is compared down
  to the cut and no further.

It is still syntactic, which means:

- A field the handler copies from a call, a model, or a serializer reads as
  missing. That is a place to look, not a defect.
- A key computed at runtime (`{ [name]: value }`) is not a name this can read.

An endpoint with no declared schema produces no finding at all — there is
nothing to compare, and inventing a comparison would be worse than silence.

## Deliberate limits

- A path assembled from literals is folded before it is matched: a template
  (`` fetch(`${BASE}/orders/${id}`) ``) and a concatenation
  (`fetch(BASE + '/orders')`) name the same endpoints as the literals do. A
  leading unreadable piece followed by `/` is a base URL — host, not path — and
  every other unreadable piece must fill a whole segment to become `{param}`.
  `fetch('/orders' + suffix)` could be `/ordersearch` as easily as `/orders/1`,
  so it is skipped rather than resolved to the wrong endpoint, and so is
  `fetch(url)`, which names nothing at all.
- `api impact` reports the handler's blast radius through the existing symbol
  graph; it does not know about consumers outside this repository, and says so
  when a declared contract has no local caller.
- A tool invocation is linked to the definition it names — `mcp.call_tool("x")`
  gets a `Calls` edge into `TOOL x` (see [federation](federation.md)) — but the
  link is the string agreeing. A dispatcher that builds the name at runtime
  matches nothing, so a missing link is not evidence that no call exists.
- Recognition is by name across a fixed set of frameworks and client libraries.
  A house-built router or client is invisible until its shape is added, and the
  route map saying "no HTTP endpoints found" means exactly that — not that the
  repository serves none.
