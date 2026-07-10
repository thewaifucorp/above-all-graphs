---
name: aag-exploring
description: Understand how code works using the aag knowledge graph — architecture questions, execution flows, "how does X work", "how does X reach Y", or surveying an unfamiliar area. Much cheaper and more complete than grepping around, because call paths and cross-file edges are precomputed.
---

# Exploring code with aag

When the user asks how something works, how two parts connect, or wants a map of an area, query the graph BEFORE manually opening files — it already knows the call graph, imports, and which docs explain which symbols.

## How

1. **One question, one call**: MCP tool `explore` with the symbol/term (CLI twin: `aag explore <query>`). Returns matching symbols with source verbatim grouped by file, callers/callees, and a blast-radius summary — usually enough to answer without further reads.
2. Need just one symbol's source + callers? MCP `node` (or `aag explore <name>` and take the first section).
3. Fuzzy name? MCP `search` (FTS5) first, then `explore` the best hit.
4. Follow a chain: repeat `explore` on the next hop, or use `callers`/`callees` for a single direction.

## Reading the output

- Edges are tagged `EXTRACTED` / `INFERRED` / `AMBIGUOUS`. Treat `AMBIGUOUS` hops as "verify in source" — several symbols share that name.
- File nodes and doc nodes appear alongside symbols; a doc node linked by `explains` is prose the repo already wrote about that symbol — read it, it is free context.
- For a visual answer, point the user at `.aag/graph.html?focus=<symbol-or-file>` (offline, clickable).

## When NOT to use

- File was edited seconds ago (index may still be syncing — read the file directly).
- Pure text/config questions with no symbols involved — plain grep is fine.
