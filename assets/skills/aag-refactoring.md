---
name: aag-refactoring
description: Rename, extract, move, or restructure code safely using the aag knowledge graph — coordinated multi-file renames, finding every usage before moving code, verifying nothing dangles after. Use for "rename X", "extract this into a module", "move this function", "restructure this".
---

# Refactoring with aag

The graph knows every caller, importer, and doc mention of a symbol — use it so a refactor never misses a usage.

## Rename

1. `aag rename <old> <new>` — previews every change (file:line, before/after) WITHOUT writing. Refuses ambiguous names (two symbols sharing `<old>`) — safe by construction.
2. Review the preview. Then `aag rename <old> <new> --write` — applies all edits and reindexes in one shot.
3. Doc mentions: prose in `.md` files that names the symbol is NOT rewritten by `rename` — check `explains` edges via `aag explore <new>` and update docs that still say `<old>`.

## Extract / move / split

No dedicated tool — the graph de-risks the manual work:

1. **Before**: `aag impact <symbol>` for each symbol being moved — the caller list is exactly the set of files whose imports must change.
2. `aag explore <symbol>` returns the current source verbatim — cut from there, don't retype.
3. **After**: run `aag sync`, then `aag impact <symbol>` again — caller count should match the before picture; a drop means a call site now resolves elsewhere (or broke).
4. Affected tests: `git diff --name-only | aag affected --stdin` — run those before claiming done.

## Rules of thumb

- Any `AMBIGUOUS` edge touching the refactor target: verify by reading the call site — name-based resolution can point at the wrong same-named symbol.
- Wide blast radius (10+ callers)? Prefer staged: introduce new name, migrate call sites, remove old — `impact` on the old name reaching 0 callers proves migration is complete.
