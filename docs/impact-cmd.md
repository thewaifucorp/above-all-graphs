---
wiki: src/impact.rs
---

# impact — blast radius analysis

`src/impact.rs` backs `aag impact <symbol>`, per `SPEC.md` section 7 ("impact
analysis / blast radius before editing"). Given a symbol name, it answers:
"if I change this, what else in the codebase could break?" by walking the
knowledge graph outward from the target node to every caller/importer,
transitively.

## Entry points

`run` is what `Command::Impact { symbol, path }` in `main.rs` calls: it opens
the index at `root`, resolves `symbol`, and prints the result to stdout.

`format` does the actual work and returns a `String` instead of printing.
It exists so the CLI and the MCP `impact` tool (in `crate::mcp`) can share
one implementation and never drift in output shape — `run` is just
`println!("{}", format(root, symbol)?)`.

## Algorithm

`format` opens the graph via `Graph::open_existing`, looks up the target
node with `find_by_name`, and errors with `Error::SymbolNotFound` if the
name isn't in the index (or `Error::IndexMissing` if there's no index at
all under `root`).

From the target node's id it runs a breadth-first search using
`Graph::callers`, expanding one frontier at a time. Each visited node is
recorded in a `HashSet` so cycles in the call graph don't cause infinite
loops or duplicate entries. The BFS is capped by `MAX_DEPTH` (20 hops), a
safety valve against pathological graphs — the visited set already
prevents cycles from looping forever, but this bounds worst-case work on
very deep or densely connected chains.

Each hop records the caller node, the edge `kind`, a `confidence` level,
and the depth at which it was discovered. Confidence reflects how sure the
resolver was that the call/import edge is real (see `resolve.rs`), and both
`kind` and `confidence` are rendered via their `as_str()` methods.

## Output

If no callers/importers are found, `format` returns a short message noting
the symbol has no known dependents in the index. Otherwise it emits a
header line naming the symbol and its definition location, one line per
affected node showing its depth, name, file:line, edge kind, and
confidence, and a trailing summary counting distinct symbols and distinct
files touched.

## Notes

The `target.id` unwrap is documented as a non-panicking invariant: any
`Node` read back from `storage::Graph` always has `id: Some(_)`, so the
`expect` in `format` should never fire in practice.
