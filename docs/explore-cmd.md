---
wiki: src/explore.rs
---
# explore

`src/explore.rs` implements `aag explore`, the answer to "how does X work" —
it returns matching symbols' source verbatim, grouped by file, plus their
direct callers. Per `SPEC.md` section 4, `explore` is the one tool an agent
should reach for by default instead of picking between many granular tools.
`src/main.rs` wires `Command::Explore { query, path }` to [`run`], and
`src/mcp.rs` exposes the same behavior as its default-listed MCP tool.

## Entry points

- [`run`] — prints [`format`]'s output for `<query>` against the index under
  `root`.
- [`format`] — builds the full multi-match report as a `String`; used by both
  the CLI and the MCP `explore` tool so the two surfaces never drift.
- [`format_node`] — renders exactly one symbol by exact `name`, erroring
  instead of falling back to a prefix search; backs the MCP `node` tool.

Both open the graph via `Graph::open_existing(root)` and return
`Error::IndexMissing` if no index exists yet. `format_node` additionally
returns `Error::SymbolNotFound` when `name` has no exact match.

## Query resolution

[`search`] tries an exact name match first (`Graph::find_by_name`) — an
agent asking about a known symbol shouldn't get buried under unrelated FTS
noise — then falls back to `Graph::search` with a `<query>*` prefix pattern,
capped at 20 results.

## Answer shape

[`render_match`] formats each matching `Node` as a `## file:start-end (kind
name)` heading, the node's source lines read verbatim from disk via
[`source_snippet`] (using `node.start_line`/`node.end_line`, 1-based
inclusive), and a `called by:` list of direct callers from `Graph::callers`,
each shown with name, location, edge kind, and confidence. Read errors and
caller-lookup errors are embedded inline in the output rather than aborting
the whole report. `format` appends a trailing `N match(es) for `query``
summary line; no matches yields `no matches for `query`` instead.
