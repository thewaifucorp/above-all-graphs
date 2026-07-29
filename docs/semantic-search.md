---
wiki: src/semantic.rs
---

## Local semantic search

The `semantic` Cargo feature enables `fastembed` with the quantized-friendly All-MiniLM-L6-v2 ONNX model. Run `aag embeddings --path .` once to embed node kind, name, signature, and description into SQLite. The model downloads on first use and runs locally afterward.

`explore::search` combines lexical FTS candidates, semantic candidates, and graph degree. Reciprocal-rank fusion allows a meaning match to enter the result set without allowing it to erase exact symbol-name evidence. A lightweight build, or a repository without generated vectors, transparently keeps lexical/structural behavior.

## Getting a build that has it

Nobody has to compile Rust or package ONNX to get embeddings. Every release
ships two prebuilt assets per platform, and the npm wrapper picks between them:

```bash
npm i -g @waifucorp/aag                    # standard build, 25 MB
AAG_SEMANTIC=1 npm i -g @waifucorp/aag     # with local embeddings, 55 MB
npm i -g @waifucorp/aag --aag-semantic     # same, as an npm flag
```

`onnxruntime` is linked statically, so the semantic asset is still one
self-contained file — no sidecar library, no `LD_LIBRARY_PATH`, nothing else to
install. Measured on Linux x86-64: 25.7 MB against 54.6 MB (9.1 MB against 18.3
MB compressed), which is why it is a separate asset rather than the default.

From source, unchanged:

```
cargo build --release --features semantic
```

## What still happens at runtime

The embedding model (All-MiniLM-L6-v2, ~90 MB) is downloaded on first
`aag embeddings` run and cached by `fastembed`. Shipping it inside the binary
would triple the asset for a feature most users never enable, and no query
leaves the machine either way — but the first run does need network, and an
air-gapped machine needs the `fastembed` cache primed by hand.

File-level sync removes stale vectors for changed nodes. Run `aag embeddings` again after significant edits to embed newly created symbols.
