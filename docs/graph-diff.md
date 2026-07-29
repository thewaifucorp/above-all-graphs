---
wiki: src/refs.rs
---

# refs.rs

Branch-aware indexes and graph-state comparison. This is P1.17 of
[capability coverage](capability-coverage.md).

`.aag/graph.db` describes one thing: the working tree, right now. That is the
right default and the wrong answer to "what did this branch change", "what does
merging this PR do to the graph", or "which symbol became a hub this week".

```bash
aag graph-diff                    # HEAD → workspace: your uncommitted work
aag graph-diff main workspace     # what this branch has done so far
aag graph-diff v0.1.0 main        # a release's worth of structural change
aag graph-diff pr/42              # a pull request's head against the workspace
```

MCP: `graph_diff`, taking `before..after` or a single state.

## How a ref gets indexed

`git worktree add --detach` checks the commit out into a scratch directory
under `.aag/refs/`, that tree is indexed into `.aag/refs/<commit>.db`, and the
worktree is removed — whether or not indexing succeeded, so a failure leaves no
stray checkout. Your working tree is never touched, never stashed, and never
checked out to something else; uncommitted work is never at risk.

Snapshots are keyed by resolved commit, so `main` picks up new commits instead
of serving a stale answer, and asking the same question twice costs one index.
The cache keeps the eight most recently used and drops the rest.

## What a comparison reports

```text
HEAD~1 → HEAD

2 symbol(s) added, 0 removed, 0 moved; 5 edge(s) added, 0 removed
1 file(s) added, 0 removed

added:
  function assetFor
  function wantsSemantic

fan-in changed:
  method default: 87 → 88 ↑
  function install: 24 → 25 ↑
  function assetFor: 0 → 1 ↑
```

Node ids are per-database, so everything is keyed by `kind name` instead. That
is what makes a symbol that changed file read as **moved** rather than as one
deletion plus one unrelated addition — the thing a text diff cannot tell you
about a refactor.

The fan-in section is the one worth reading last: a symbol whose dependent
count jumped is one the rest of the code just started leaning on.

## Deliberate limits

- **Same-name symbols merge.** Two `new` methods on different types are one
  entry, because the key is `kind name`. A rename plus a move in one commit
  reads as an add and a remove.
- **A snapshot is a full index.** The first `graph-diff` against a large
  repository costs one full indexing pass — seconds here, longer on a
  thousand-file monorepo. Subsequent runs hit the cache.
- **`pr/<n>` needs `gh`** and network: the head commit is resolved through
  `gh pr view`, then indexed like any other commit. Everything else is local
  git.
- **A shallow clone cannot check out what it does not have.** `git worktree
  add` fails on a missing commit, and the error says so rather than reporting
  an empty diff.
- **Comparison is structural, not semantic.** Two states with the same symbols
  and edges compare as identical even when a function body changed completely.
