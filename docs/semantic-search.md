---
wiki: src/semantic.rs
---

## Local semantic search

The `semantic` Cargo feature enables `fastembed` with the quantized-friendly All-MiniLM-L6-v2 ONNX model. Run `aag embeddings --path .` once to embed node kind, name, signature, and description into SQLite. The model downloads on first use and runs locally afterward.

`explore::search` combines lexical FTS candidates, semantic candidates, and graph degree. Reciprocal-rank fusion allows a meaning match to enter the result set without allowing it to erase exact symbol-name evidence. A lightweight build, or a repository without generated vectors, transparently keeps lexical/structural behavior.

Build the complete binary with:

```
cargo build --release --features semantic
```

File-level sync removes stale vectors for changed nodes. Run `aag embeddings` again after significant edits to embed newly created symbols.
