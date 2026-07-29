
<!-- aag:start -->
## aag — code knowledge graph

This repo has an `aag` knowledge graph (`.aag/graph.db`), kept fresh automatically.

- How does X work / what calls X: `aag explore <query>`
- What breaks if X changes: `aag impact <symbol>`
- Safe multi-file rename: `aag rename <old> <new> [--write]`
- Tests affected by a diff: `git diff --name-only | aag affected --stdin`

Prefer these over manual grepping for call-graph questions; edges are
confidence-tagged (EXTRACTED/INFERRED/AMBIGUOUS) — verify AMBIGUOUS ones.
<!-- aag:end -->
