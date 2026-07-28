---
wiki: src/flow.rs
---

# flow.rs

Statement-level control and data flow: basic blocks, a control-flow graph,
definitions and uses, reaching definitions, def-use chains, and control
dependence. This is the foundation P0.5 of
[capability coverage](capability-coverage.md) asks for.

Everything else in `aag` works at symbol granularity — this function calls that
function. That cannot answer "what guards this statement" or "where does this
value come from", because both questions live *inside* a function body.

## What it produces

`analyze(file_path, source)` returns one `Cfg` per function found in the file.
Each carries:

- **Blocks** — a straight-line run of statements with a single entry, plus the
  reason it ended (`Fallthrough`, `Branch`, `Loop`, `Return`, `Break`,
  `Continue`, and the synthetic `Exit`). Each block keeps the first line of its
  terminating statement verbatim, so a guard is readable without opening the
  file.
- **Edges** — `Sequential`, `True`, `False`, `Back`. A loop body's fall-through
  becomes a `Back` edge to the header; a `continue` jumps to the header; a
  `break` becomes a pending edge onto whatever follows the loop.
- **Definitions and uses** — every syntactic write and read, by name, block, and
  line. The left-hand side of a binding is a write, not a read.
- **`reaching_definitions`** — classic iterative dataflow: a block kills earlier
  definitions of the names it writes and generates its own.
- **`def_use_chains`** — which definitions may supply each use, with a nearer
  definition in the same block shadowing anything that reached the block entry.
- **`control_dependence`** — which branch decides whether a block runs,
  computed from post-dominance over this function's CFG.
- **Calls** — each call site with the identifiers passed to it.
- **`dependences`** — the program dependence graph as `(dependent line, source
  line, control|data)`, plus `dependences_of(line)` for the transitive backward
  slice of one line.
- **`taint_findings`** — source-to-sink flows: a known input (`req.query`,
  `process.env`, `argv`, `stdin`, …) reaching a known sink (`exec`, `query`,
  `innerHTML`, `writeFile`, …), with the assignments that carried it and
  whether a branch decides that the sink runs at all.

Three surfaces: `aag flow <file> [--function name]`, `aag pdg <file>
[--line N]`, `aag taint <file>`, and the same two as MCP tools `pdg_query`
(accepting `path` or `path:line`) and `taint`.

## Statement shape, per grammar

Two wrappers had to be walked through transparently, and getting them wrong
means silently finding no branches at all: Rust puts every expression-statement
inside an `expression_statement`, and Go puts a function body's statements
inside a `statement_list`. `STATEMENT_CONTAINERS` and `unwrap_statement` handle
both, so `if`, loops, and `return` are found in Rust, Go, JavaScript,
TypeScript, Python, Java, and C# alike.

Nested functions are not folded into their parent: a closure assigned to a
local has its own flow, and `FUNCTION_KINDS` stops the walk at its boundary.

## Deliberate limits

These are limits, not bugs, and none of them should be described as anything
else:

- Blocks are cut where a reader would cut them — at branches, loops, and jumps
  — not at every expression with a side effect.
- A definition is a *syntactic* assignment or binding. Aliasing through a
  reference, a field, or a container is not tracked, so reaching definitions is
  an over-approximation of what may reach and an under-approximation of what
  does.
- Control dependence is intraprocedural. An exception unwinding past a caller
  is not modelled.
- Languages without a flow frontend return nothing rather than failing, so a
  mixed repository still analyses.

## What the taint analysis is not

It is intraprocedural and syntactic. Taint spreads when a definition's *line*
reads an already-tainted name, which is line-granular rather than
expression-granular, and it cannot follow a value through a call, a field, or a
container. So:

- A finding is a place to look, never a proven vulnerability.
- No findings is **not** evidence of safety, and the CLI says so in its own
  output rather than leaving a reader to assume otherwise.
- A flow marked "guarded by a branch" means only that a branch decides whether
  the sink runs. Whether that branch actually validates anything is not
  something this analysis can know.

The source and sink lists are deliberately short and specific. A long fuzzy
list produces findings nobody reads.

## Not yet built

Interprocedural flow — following a value from a caller into a callee and back —
and sanitizer recognition. Both need the call graph the rest of `aag` already
has to be joined to these per-function graphs, which is not done.
