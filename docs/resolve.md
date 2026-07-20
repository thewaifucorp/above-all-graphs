---
wiki: src/resolve.rs
---

`resolve.rs` is the walk-parse-resolve stage of the indexer: it turns raw per-file data (imports, calls, doc text) produced by `crate::parse` into confidence-tagged graph edges. This is the module CLAUDE.md calls "walk + parse + cross-file resolution" — the code confirms that and adds the doc-indexing responsibility on top.

## Entry point

`index_repo` is the public entry point. It clears the graph, walks `root` with `walk_files`, and for every file either routes it to `index_doc_file` (docs) or reads it as UTF-8 and hands it to `index_code_file` (code, via `parse_file`). Files that fail to parse as UTF-8 are skipped with a warning rather than aborting the pass — binary source files are simply not indexed as code. After the walk, three resolution passes run in order: `resolve_doc_mentions`, `resolve_imports`, `resolve_calls`. `index_repo` is always a full rebuild, never an incremental patch; callers such as the watcher rely on it being idempotent and safe to call repeatedly as files change.

## SKIP_DIRS

`SKIP_DIRS` is a `pub(crate)` constant listing directory names excluded entirely from the walk: `.git`, `.aag`, `target`, `node_modules`, `.playwright-mcp`, `.claude`, `.cursor`. It is deliberately the single definition of "what can affect the index," shared by both the file watcher and `aag sync` so the two never disagree about what counts as project content versus noise. `.playwright-mcp` holds browser-automation artifacts and `.claude`/`.cursor` hold agent config (including the skill pack `aag install` writes); without the exclusion these would otherwise be picked up and indexed as `Doc` nodes, polluting the graph with the tool's own scaffolding. `walk_files` applies `SKIP_DIRS` via `WalkDir`'s `filter_entry`, pruning matching directories before descending into them.

## Confidence model

Every resolved edge carries a `Confidence` from `crate::storage`, computed by the small helper `resolution_confidence`: exactly one name match yields the caller-supplied "unique" confidence, more than one yields `Confidence::Ambiguous`. This backs the three levels documented in `SPEC.md` section 3: `EXTRACTED` for imports whose last path segment (`last_segment`) matches exactly one symbol, `INFERRED` for calls whose callee identifier matches exactly one symbol, and `AMBIGUOUS` whenever more than one symbol shares that name. Matches against nothing — a call into `std` or an external crate — are dropped rather than stored as dangling edges; `resolve_calls`, `resolve_imports`, and `resolve_doc_mentions` all skip silently when `by_name` has no candidates for a token.

## Docs as first-class nodes

Per `SPEC.md` section 5, `resolve.rs` also owns doc/image ingestion. `doc_kind` classifies a relative path by extension into `DocKind::Text` (`TEXT_DOC_EXTENSIONS`: `md`, `txt`) or `DocKind::Binary` (`BINARY_DOC_EXTENSIONS`: `pdf`, `png`, `jpg`, `jpeg`, `gif`, `webp`, `svg`). `index_doc_file` inserts text docs immediately as `NodeKind::Doc` nodes with their full file content as `description` — no model call is needed since this is deterministic parsing like everything else here. Binary docs are inserted with `description: None`, a marker meaning "needs a vision pass"; `crate::docs` is expected to fill that in later, at zero cost to `aag` itself. Whichever kind, the doc's text (once available) is scanned by `mentioned_names` for known symbol names, and each hit becomes an `EdgeKind::Explains` edge resolved through the same name-matching/confidence machinery as calls and imports, via `resolve_doc_mentions`.

## Supporting pieces

- `mentioned_names` tokenizes doc text on non-alphanumeric/non-underscore boundaries, keeps tokens longer than 2 characters, deduplicates, and restricts results to names already present in `by_name` — so a doc's prose can't spuriously "mention" a symbol that merely shares a common English word.
- `expand_import` unpacks a grouped Rust `use` path like `std::collections::{HashMap, HashSet}` into one fully-qualified path per name; ungrouped imports pass through unchanged.
- `last_segment` extracts the rightmost `::`-separated identifier from an import path, stripping a trailing `as alias` and returning `None` for glob imports (`use foo::*`), since those have no single name to resolve against.
- `index_code_file` inserts the file's own `NodeKind::File` node plus one node per parsed symbol, and populates two lookup maps used by later passes: `by_name` (name to node ids, for imports/calls/doc-mentions) and `by_file_symbol` (file+symbol-name to node id, used to find the calling symbol's own node when resolving a call's source).
- `IndexSummary` is the return value of `index_repo`: counts of files parsed, symbol nodes inserted, doc nodes inserted, and edges resolved, used for logging and asserted against directly in tests.

## Invariants worth knowing

- Resolution is purely name-based ("no type checking"); a symbol name shared across unrelated files is legitimately ambiguous, not a bug.
- `index_repo` performs the initial full build and persists unresolved references. `index_file` replaces one file atomically; `rebuild_resolved_edges` resolves the cached references against the current symbol table without reparsing unchanged files.
- Self-edges are explicitly excluded in `resolve_doc_mentions` (a doc never `Explains` itself) but no such guard exists for calls/imports, since a symbol cannot import or call itself by construction of how `pending_calls`/`pending_imports` are built.
