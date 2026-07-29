---
wiki: src/query.rs
---

# query.rs

A documented formal subset of Cypher, evaluated as real graph patterns. This is
what P0.4 of [capability coverage](capability-coverage.md) asks for, and it
replaces a surface that could not answer the questions it accepted.

## What it replaced, and why that mattered

The previous implementation sniffed strings. It checked that a query began with
`MATCH`, searched the whole text for the first `.name = '...'` and `.kind =
'...'`, and — if the query contained `-[` anywhere — returned *every edge in the
graph*, ignoring the relationship type, the direction, and both endpoints. So

```cypher
MATCH (f:Function)-[:CALLS]->(g:Function) WHERE f.file = 'src/parse.rs' RETURN g
```

returned imports, doc links, and schema references from every file, and looked
exactly like an answer. A query surface that cannot be wrong about what it
matched is worth less than no query surface at all, because a reader trusts it.

## The subset

```text
query      := select (UNION [ALL] select)*
select     := (MATCH | OPTIONAL MATCH) pattern (',' pattern)*
              ((MATCH | OPTIONAL MATCH) pattern (',' pattern)*)*
              [WHERE predicate]
              RETURN [DISTINCT] item (',' item)*
              [ORDER BY column [ASC|DESC] (',' ...)*]
              [SKIP int] [LIMIT int]

pattern    := node (relationship node)*
node       := '(' [variable] [':' Label] [ '{' key ':' literal (',' ...)* '}' ] ')'
relationship := '-' '[' [variable] [':' TYPE ('|' TYPE)*] [hops] ']' '->'   // outgoing
              | '<-' '[' ... ']' '-'                                         // incoming
              | '-'  '[' ... ']' '-'                                         // either way
hops       := '*' [int] ['..' [int]]

predicate  := predicate (AND|OR) predicate | NOT predicate | '(' predicate ')'
              | value comparison value
              | value IS [NOT] NULL
              | value IN '[' literal (',' literal)* ']'
comparison := '=' | '<>' | '<' | '<=' | '>' | '>=' | CONTAINS | STARTS WITH | ENDS WITH

item       := value [AS alias] | aggregate [AS alias]
aggregate  := count '(' (value | '*') ')' | (collect|min|max|sum|avg) '(' value ')'
value      := variable | variable '.' property | literal
literal    := 'text' | "text" | integer
```

Keywords and labels are case-insensitive; variables are not.

**Labels** are the graph's node kinds: `File`, `Function`, `Struct`, `Method`,
`Interface`, `Doc`, `Endpoint`, `Schema`, `DatabaseTable`, `InfraResource`,
`Component`.
`Fn`, `Class`, `Trait`, `Table`, and `Resource` are accepted spellings of the
ones a reader would expect them to mean.

**Relationship types** are the graph's edge kinds: `CALLS`, `IMPORTS`,
`INHERITS`, `IMPLEMENTS`, `EXPLAINS`, `REFERENCES`.

**Node properties**: `id`, `kind`, `name`, `file`, `line`, `end_line`.
**Relationship properties**: `type`, `confidence` — the confidence being
`EXTRACTED`, `INFERRED`, or `AMBIGUOUS`, so a query can ask only for edges the
graph is sure about.

## Examples

```cypher
-- Functions in one file, in source order
MATCH (f:Function) WHERE f.file = 'src/resolve.rs' RETURN f.name, f.line ORDER BY f.line

-- Who calls what, skipping the calls the graph could not narrow to one symbol
MATCH (a)-[r:CALLS]->(b) WHERE r.confidence <> 'AMBIGUOUS' RETURN a.name, b.name LIMIT 50

-- Which imports are stated in source rather than inferred
MATCH (f:File)-[r:IMPORTS]->(t) WHERE r.confidence = 'EXTRACTED' RETURN f.name, t.name

-- Everything reachable from one function within three calls
MATCH (f:Function {name: 'format_taint'})-[:CALLS*1..3]->(g) RETURN DISTINCT g.name

-- The busiest callers, by file
MATCH (a:Function)-[:CALLS]->(b) RETURN a.file, count(*) AS calls ORDER BY calls DESC LIMIT 10

-- A doc that explains a function that calls something else: two patterns, joined
MATCH (a)-[:CALLS]->(b), (d:Doc)-[:EXPLAINS]->(b) RETURN d.name, b.name, a.name

-- Every function, and what it calls — including the ones that call nothing
MATCH (f:Function) OPTIONAL MATCH (f)-[:CALLS]->(g)
RETURN f.name, collect(g.name) AS callees, count(g) AS calls ORDER BY calls DESC

-- Functions and structs in one list
MATCH (f:Function) WHERE f.file = 'src/query.rs' RETURN f.name AS name, f.line AS line
UNION
MATCH (s:Struct) WHERE s.file = 'src/query.rs' RETURN s.name AS name, s.line AS line

-- The span of a file's symbols
MATCH (f:Function) RETURN f.file, min(f.line) AS first, max(f.line) AS last, avg(f.line) AS middle
```

From the CLI, `aag cypher "<query>"` prints a table and `--json` prints rows;
over MCP, the `cypher` tool returns the JSON form:

```json
{"columns": ["a.name", "calls"], "rows": [["caller", 2]], "truncated": false}
```

Rows are arrays, not objects, because column order is part of the answer and a
JSON object's key order is not. `truncated` says a page was returned rather than
the whole result — silence about paging is how a partial answer gets read as a
complete one.

## What it refuses, and how

Everything outside the subset is an error that names what was expected, at the
line and column where it was found. Nothing is silently reinterpreted.

| Query | Answer |
|---|---|
| `MATCH (n) DELETE n` | ``line 1, column 11: `DELETE` writes to the graph; this surface is read-only`` |
| `MATCH (n) WITH n RETURN n` | ``line 1, column 11: `WITH` is outside the supported subset`` |
| `MATCH (n) RETURN toUpper(n.name)` | ``unknown function `toUpper` — this subset evaluates count, collect, min, max, sum, avg`` |
| `OPTIONAL MATCH (n) RETURN n` | `OPTIONAL MATCH extends rows another pattern produced; this query has no MATCH to extend` |
| `MATCH (f:Function) RETURN f.name UNION MATCH (s:Struct) RETURN s.name` | `UNION needs the same columns in each arm: this one returns s.name, the first returns f.name` |
| `MATCH (n:Widget) RETURN n` | ``unknown label `Widget` — the graph has: File, Function, …`` |
| `MATCH (a)-[:USES]->(b) RETURN a` | ``unknown relationship type `USES` — the graph has: CALLS, IMPORTS, …`` |
| `MATCH (n) RETURN n.colour` | ``unknown property `colour` — a node has id, kind, name, file, line, end_line`` |
| `MATCH (n:Function RETURN n` | ``line 1, column 19: expected `)` (found `RETURN`)`` |
| `MATCH (n) RETURN n.name ORDER BY n.line` | `ORDER BY must name a returned column; this query returns: n.name` |

Writes (`CREATE`, `MERGE`, `DELETE`, `SET`, `REMOVE`, `DETACH`, `DROP`, `LOAD`,
`FOREACH`, `CALL`) are rejected by name before parsing, so the message is about
intent rather than syntax. The graph is read-only through this surface.

## Bounds, stated rather than discovered

- `LIMIT` defaults to 100 and is clamped to 1000; a clamped or paged answer
  reports `truncated`.
- A hop range is at most 5, and a bare `*` means `*1..5` rather than unbounded —
  a query surface that can hang is not usable from a hook.
- One query may hold 20 000 intermediate rows, checked while paths expand as
  well as while rows accumulate. Beyond that the query is rejected with the
  advice that would make it answerable, rather than answered slowly.
- A variable-length path never repeats an edge, so a cycle terminates.
- A table cell is cut at 60 characters — `collect()` can gather hundreds of
  names, and a table one cell wide is not a table. The cut is announced under
  the table, and the JSON form carries the whole value.

## Deliberate limits

These are limits, not bugs:

- **Not a planner.** Evaluation loads the graph and matches in memory. Labels
  and literal property maps are pushed down before a node is bound, `WHERE` runs
  on rows afterward, and nothing else is optimized.
- **Six functions, all aggregates**: `count`, `collect`, `min`, `max`, `sum`,
  `avg`. Grouping is by the non-aggregate returned columns — Cypher's rule — and
  every aggregate ignores nulls, so `count(x)` counts values while `count(*)`
  counts rows, and `sum` over anything but numbers is null rather than a
  guess. There are no scalar functions, no arithmetic, no `CASE`, no `exists`;
  anything else in call position is rejected by name.
- **No `WITH`, `UNWIND`, or subqueries.** Each is rejected by name. A pipeline
  needs an intermediate row shape that is not a graph match, and this evaluator
  has one stage on purpose.
- **`OPTIONAL MATCH` extends, it does not start.** A row that cannot satisfy the
  optional pattern survives with that pattern's variables null. A query whose
  only clause is optional is rejected: there is nothing to extend.
- **`UNION` arms must return the same columns in the same order**, as in Cypher,
  and each arm carries its own `ORDER BY`, `SKIP`, and `LIMIT`. `UNION` drops
  duplicate rows; `UNION ALL` keeps them.
- **`ORDER BY` names returned columns**, not arbitrary expressions: with `count`
  in the projection, ordering by anything else has no defined meaning here.
- **String literals have no escapes** — a literal is the text between two
  quotes.
- **A relationship property on a multi-hop path is null**, because the value
  would have to belong to one edge of it and picking one silently is a wrong
  answer. Return the path itself to see every edge, its type, and its
  confidence.
- **Mismatched types never compare equal and never order.** `n.line = '10'` is
  false rather than coerced, so a typo does not read as a real answer.

This is a documented subset with real pattern evaluation. It is not full Cypher,
and [capability coverage](capability-coverage.md) says so in the same words.
