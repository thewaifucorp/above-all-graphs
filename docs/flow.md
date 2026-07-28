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
- **Parameters and returns** — declared parameter names in order (`self`/`this`
  excluded, since a caller does not pass it at a position) and the names each
  explicit `return` hands back. These are what a caller's argument and
  assignment are matched against.
- **Calls** — each call site with the identifiers passed to it, grouped per
  positional argument. `f(a.b, 2, c)` gives `[["a"], [], ["c"]]`: a literal
  keeps its slot, so an argument can be matched to the parameter at the same
  position.
- **`dependences`** — the program dependence graph as `(dependent line, source
  line, control|data)`, plus `dependences_of(line)` for the transitive backward
  slice of one line.
- **`taint_findings`** — source-to-sink flows inside one function: a known input
  (`req.query`, `process.env`, `argv`, `stdin`, …) reaching a known sink
  (`exec`, `query`, `innerHTML`, `writeFile`, …), with the assignments that
  carried it and whether a branch decides that the sink runs at all.

Three surfaces: `aag flow <file> [--function name]`, `aag pdg <file>
[--line N]`, `aag taint <file> [--depth hops]`, and the same two as MCP tools
`pdg_query` (accepting `path` or `path:line`) and `taint` (accepting `path` or
`path:hops`).

## Across calls

`program(file, depth)` joins one file's functions with the ones they call and
returns a `Program`; `Program::findings` is the taint analysis over the join.
The mechanism is a per-function `Summary` — what a function does to the values
passed into it:

- which parameter *positions* reach a sink, and through which further calls,
- which positions reach an explicit `return`, so a tainted argument taints what
  the caller assigns from the call,
- whether the function reads an input of its own and returns it, which taints a
  caller's assignment with no tainted argument at all,
- whether it neutralizes what it is given, which makes it a sanitizer to every
  caller.

A caller does not re-analyze its callee; it reads that summary. Summaries are
recomputed a bounded number of rounds so a callee summarized in one round
informs its caller in the next, which is what carries a sink several calls up a
chain. Both the summaries and the resolved call targets are keyed by file *and*
name: a repository with two functions called `run` is ordinary, and letting one
file's `run` answer for another's is a wrong answer that reads like a right one.

Resolution is not reimplemented here. A call site in a body carries the tail
identifier only (`crate::bigbang::run` and a local `run` are the same string),
so the callee comes from the indexed `calls` edge — the language-aware ladder
already applied by [resolve](resolve.md). Where the graph is ambiguous, every
candidate is followed and the finding says which of how many it is. Where there
is no index, only calls inside the entry file are joined, and a finding says the
callee was matched by name rather than by a resolved edge.

Bounds are stated rather than discovered: `--depth` call hops (2 by default),
400 joined functions, and eight rounds of assignment-chasing per function.

## Sanitizers

A call whose name is in the sanitizer list — escaping, quoting, or narrowing to
a type that cannot carry an injection — stops taint at that line. A function is
recognized as a sanitizer when a parameter reaches its `return` only through
one, so a repository's own `clean(value)` counts without being listed anywhere.

Suppression is reported, not silent: `stopped at a sanitizer` lines name what
stopped the flow and where, including when it was stopped inside a callee. "No
findings" and "a flow was found and escaped" must not read the same.

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

It is syntactic. Taint spreads when a definition's *line* reads an
already-tainted name, which is line-granular rather than expression-granular,
and it cannot follow a value through a field or a container. Crossing a call
inherits every one of those limits rather than escaping them. So:

- A finding is a place to look, never a proven vulnerability.
- No findings is **not** evidence of safety, and the CLI says so in its own
  output rather than leaving a reader to assume otherwise.
- A flow marked "guarded by a branch" means only that a branch decides whether
  the sink runs. Whether that branch actually validates anything is not
  something this analysis can know.

- A sink takes any tainted name on its line, because a chain like
  `Command::new(sh).arg(cmd).spawn()` carries the value in the receiver rather
  than in the sink call's own arguments. Crossing into a callee is stricter: it
  needs the argument's position, since that is what a parameter is matched by.
- Sanitizer recognition is line-granular too, so `escape(a) + b` reads as
  sanitized even though `b` is not. That direction is deliberate — a false
  negative costs one missed place to look, a false positive costs trust in the
  whole list.

The source, sink, and sanitizer lists are deliberately short and specific. A
long fuzzy list produces findings nobody reads.

## Not yet built

- A Rust tail expression is not a `return` statement and is not recorded as one,
  so a function that returns its parameter without writing `return` has no
  return-value summary.
- Nothing is field- or container-sensitive, in a callee any more than in a
  caller, and dynamic dispatch is only as narrow as the call graph made it.
- A parameter's flow is summarized one position at a time; a value that only
  becomes dangerous through *two* arguments together is not modelled.
