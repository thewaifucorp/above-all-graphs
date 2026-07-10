---
wiki: src/watch.rs
---

# watch.rs

Native filesystem watcher that keeps the graph fresh while `aag mcp` is
running. It is the only path that reindexes without an explicit command:
there is no manual "reindex" step in normal operation. It is not a
`Command` variant in `main.rs`; it is invoked programmatically by
`mcp.rs` (on MCP server startup) and by `hook.rs` (the `session-start`
hook), not by users directly.

## Key functions

`reconcile` opens (creating if needed) `.aag/graph.db` via `Graph::open`
and runs one synchronous `resolve::index_repo` pass immediately. This is
the reconciliation-on-connect step: it absorbs edits made while no
watcher was running — a `git pull`, another editor, a previous session
that ended uncleanly. Both `mcp.rs`'s connection handler and `hook.rs`'s
`session_start` call `reconcile` before doing anything else.

`spawn` takes the repo root and starts a background thread running
`watch_loop`, returning immediately. The watcher lives for the process
lifetime; `watch_loop` failures are logged via `tracing::warn!` rather
than propagated, since a dead watcher shouldn't crash the MCP server.

`watch_loop` builds an `mpsc` channel and a `notify_debouncer_mini`
debouncer over `notify`'s OS-native backend (FSEvents, inotify, or
`ReadDirectoryChangesW`), watching the root recursively. Events collapse
across the `DEBOUNCE` window (2 seconds, matching `SPEC.md` section 2's
"Debounce configurável (default ~2s)") so a burst of saves triggers one
reindex, not one per file. Each debounced batch is filtered through
`is_relevant`; if any event in the batch touches a relevant path,
`reindex` runs.

`is_relevant` strips the event path down to a root-relative path and
rejects it if any path component matches `SKIP_DIRS`, imported directly
from `crate::resolve` so the watcher and the indexer walk agree on what
to ignore. `.aag` itself is in that list — otherwise writing `graph.db`
during a reindex would immediately retrigger the next one, a feedback
loop. `.git` is skipped for the same reason it's skipped everywhere else
in this codebase.

`reindex` opens the graph and runs `resolve::index_repo` again — a full
whole-repo pass, not a per-file patch. The comment at the top of the
file explains why: `crate::resolve`'s cross-file name resolution
recomputes from the whole repo's symbol table regardless, so patching
just the changed file's nodes/edges would still require that same
whole-repo pass to stay correct. Errors opening the graph or reindexing
are logged and swallowed, matching the house rule that hooks and
background paths must never take the process down.

## Relation to sync.rs

`watch.rs` and `sync.rs` are two different freshness mechanisms serving
different lifecycles. `watch.rs` is for the long-lived `aag mcp` process:
a background thread with a real OS filesystem watcher and a debounce
window, active for as long as the server runs. `sync.rs` is hook-driven:
`hook.rs`'s `pre-edit`/`post-edit` hooks spawn a short-lived `aag sync`
subprocess per edit, doing a full pass or a per-file relevance
short-circuit, for agents that aren't necessarily running `aag mcp` at
all. They don't call each other directly, but both ultimately call into
`resolve::index_repo` and both respect the same `SKIP_DIRS`.

## Tests

The unit tests in this module exercise `is_relevant` only: a source file
change under `/repo/src` is relevant, a write to `/repo/.aag/graph.db`
is not, and a change under `/repo/.git` is not.
