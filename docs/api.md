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
declared top-level field names are compared against the keys of the object
literals its handler returns or hands to a JSON responder (`res.json`,
`jsonify`, `send`, …). A `$ref` is followed into the schema it names, and an
array schema is unwrapped to its items.

```text
GET /pets — listPets
     declared but never returned: name, tag
     of those, required by the contract: name
     returned but not declared: colour
```

It is syntactic and top-level, which means:

- A handler that builds its response in a variable, spreads another object, or
  returns a serialized model reads as returning nothing, and is skipped rather
  than reported as missing everything.
- A nested field is not compared. Only the top level of the declared schema is.
- A field the handler copies from elsewhere reads as missing. That is a place to
  look, not a defect.

An endpoint with no declared schema produces no finding at all — there is
nothing to compare, and inventing a comparison would be worse than silence.

## Deliberate limits

- A path built at runtime (`fetch(url)`, an interpolated base) is not a target
  this can name, and is skipped rather than guessed.
- `api impact` reports the handler's blast radius through the existing symbol
  graph; it does not know about consumers outside this repository, and says so
  when a declared contract has no local caller.
- Tool *invocations* are not linked to tool definitions. A dispatcher that
  matches a name to a branch is a link only the string can make, and the graph
  does not claim it.
- Recognition is by name across a fixed set of frameworks and client libraries.
  A house-built router or client is invisible until its shape is added, and the
  route map saying "no HTTP endpoints found" means exactly that — not that the
  repository serves none.
