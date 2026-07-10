---
wiki: src/error.rs
---

# Errors

`src/error.rs` defines the crate-wide error type for `aag`'s domain layer. It has no dependents inside itself — every other module that can fail returns the `Result` alias defined here instead of hand-rolling its own error enum.

`Result<T>` is a type alias for `std::result::Result<T, Error>`. `src/lib.rs` re-exports both `Error` and `Result` so the rest of the crate imports them straight from the crate root rather than reaching into `error` directly.

`Error` is a `#[non_exhaustive]` enum built with `thiserror::Error`, so each variant carries its own `#[error(...)]` display message and callers outside the crate cannot exhaustively match on it — new variants can be added without breaking downstream matches. The variants map to the failure modes of the core pipeline:

- `CreateDir` and `RemoveDir` wrap `std::io::Error` for failures managing the `.aag/` index directory, including the forced-rebuild remove path.
- `NotImplemented` marks a subcommand that has no implementation yet; it carries only the command name.
- `Storage` wraps a `rusqlite::Error` together with a `context` string describing what graph operation was attempted, used by `storage.rs`.
- `Parse` reports a source file that `parse.rs` could not extract via tree-sitter, with a `file` path and a `reason` string.
- `Io` covers read failures while walking and indexing files, distinct from `CreateDir`/`RemoveDir`/`Write` by direction (read vs. write vs. directory management).
- `IndexMissing` fires when a query command runs against a repo that has never had `aag bigbang` run — its message tells the user exactly what to run.
- `SymbolNotFound` and `NotADoc` back the lookup commands (`aag describe`, `aag explore`, etc.): the former for a name absent from the graph, the latter for a name present but not a doc node.
- `AmbiguousRename` guards `aag rename`: it fires when the requested name matches more than one symbol, carrying both the `name` and the `count` of colliding symbols, since a safe rename needs a unique target.

Because every variant renders its own message via `thiserror`, callers generally just propagate `Error` with `?` and let the CLI's top-level handler print the `Display` output — there's no separate error-formatting layer elsewhere in the codebase.
