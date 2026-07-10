---
name: aag-pr-review
description: Review a pull request or diff using the aag knowledge graph — what the change touches transitively, risk of merging, call sites the diff forgot to update, and which tests must run. Use for "review this PR", "is this diff safe", "what does this change affect".
---

# PR / diff review with aag

A diff shows what changed; the graph shows what the change REACHES. Review both.

## Method

1. **Changed files**: `git diff --name-only <base>...<head>` (or the PR's file list).
2. **Blast radius per changed symbol**: for each function/struct the diff modifies (not just touches — signature or behavior changes), run `aag impact <symbol>`. Callers OUTSIDE the diff are the risk surface: they run new behavior nobody edited.
3. **Missed call sites**: a renamed/re-signatured symbol whose `impact` still lists callers not present in the diff = incomplete change. Flag each with file:line from the impact output.
4. **Test coverage**: `git diff --name-only <base>...<head> | aag affected --stdin` — test files transitively affected. Tests listed there but not updated/run in the PR = coverage gap worth flagging.
5. **Context per hunk**: `aag explore <symbol>` gives the full source + callers of anything the diff touches — cheaper than opening each file, and shows the docs (`explains` edges) that may now be stale.

## Risk verdict heuristics

- Diff-internal changes only (all affected callers are inside the diff) — low risk.
- Callers outside the diff, all `EXTRACTED`/`INFERRED` — medium; list them for the author.
- `AMBIGUOUS` edges on a changed symbol — resolution may be wrong in either direction; verify manually before trusting "no impact".
- Changed symbol has 0 callers in the graph — either dead code (flag it) or an external entry point (check route/CLI/MCP registration).

## Output shape

Per finding: `file:line — symbol — what breaks / what's missing — evidence (impact/affected output)`. End with an overall verdict: safe / safe-with-listed-follow-ups / needs-changes.
