---
wiki: src/parse.rs
---

`src/parse.rs` is the tree-sitter based structural parsing layer. It turns one file's source text into a `ParsedFile`: the symbols the file declares plus *raw*, unresolved cross-references. "Raw" means the module never turns a call to `bar` into an edge pointing at a specific node id — that resolution, along with tagging edges `EXTRACTED`/`INFERRED`/`AMBIGUOUS`, is `crate::resolve`'s job. This split keeps each language's parser dumb and swappable without touching the storage layer.

The default frontend covers Rust, JavaScript, TypeScript, Python, Java, C, C++, C#, Go, PHP, Ruby, Swift, Kotlin, Dart, Scala, Shell, Lua, R, Elixir, and Objective-C. Rust and JavaScript keep dedicated high-precision walkers. The other 18 use the shared Tree-sitter language pack plus an AST fallback, producing the same `ParsedFile` contract for structure, imports, and calls. Grammars download on first use and remain cached for offline runs.

`ParsedFile` holds three fields:

- `nodes` — `Vec<Node>` of symbols declared directly in the file (functions, structs, methods), not yet inserted, so each has no `id` yet.
- `imports` — raw `use`/import paths as written in source, e.g. `std::fs::File`.
- `calls` — `(caller_symbol_name, callee_name)` pairs found inside function/method bodies.

The `LanguageParser` trait is the extension point: `extensions` reports which file extensions (without the dot) a parser handles, and `parse` turns `file_path` and `source` into a `ParsedFile`, returning `Error::Parse` on failure. `parse_file` is the entry point callers use: it picks a registered parser by matching the file's extension against each parser's `extensions`, runs it, and returns `Ok(None)` for files with no registered parser — callers skip those rather than treating them as an error.

Currently only `RustParser` is registered, backed by `tree_sitter_rust`. It handles the `rs` extension. Its `parse` method builds a tree-sitter `Parser`, parses the source into a tree, then walks the tree with the internal `walk` function starting from `tree.root_node()`.

`walk` is a recursive-descent traversal that threads two bits of state through the tree: `in_impl`, which marks whether the walk is inside an `impl` block so nested `function_item` nodes get tagged `NodeKind::Method` instead of `NodeKind::Function`; and `current_owner`, the enclosing function/method name that any `call_expression` found inside its body gets attributed to. It matches on node kind:

- `struct_item` — emits a `Node` of kind `NodeKind::Struct`, named from the node's `name` field, with line range from `line_range`.
- `impl_item` — recurses into children with `in_impl` set to `true`, without emitting a node itself.
- `function_item` — emits a `Node` (`Method` or `Function` depending on `in_impl`), then recurses into the function body with `current_owner` set to the function's name so calls inside get attributed correctly.
- `use_declaration` — strips the `use`/`;` wrapper and pushes the raw import path string.
- `call_expression` — resolves the callee name via `callee_name` (handling plain `identifier`, `field_expression` for method calls, and `scoped_identifier` for path-qualified calls) and, if there's a `current_owner`, pushes a `(caller, callee)` pair.

Helper functions `text` (extracts a node's UTF-8 source slice), `line_range` (1-based start/end line numbers), and `children` (iterates a node's direct children) support the walk. Line numbers are computed via `u32::try_from`, saturating to `u32::MAX` on overflow rather than panicking.

The module's unit tests cover top-level function extraction, struct extraction, method-vs-function tagging inside `impl` blocks, raw import capture, call attribution to the enclosing function, method-call attribution by field name, and the `Ok(None)` fallback for unknown extensions like `.md`.
