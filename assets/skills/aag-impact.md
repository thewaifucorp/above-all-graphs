---
name: aag-impact
description: Check what breaks before changing code — blast radius of a symbol, "is it safe to change X", "what depends on this". Run it BEFORE editing any widely-used function, struct, or public API; the graph returns every transitive caller/importer with file:line in milliseconds.
---

# Impact analysis with aag

Before editing a symbol that might be widely used, ask the graph — not your memory — what depends on it.

## How

- `aag impact <symbol>` (or MCP `impact`): transitive blast radius — every caller/importer, depth-tagged, with file:line and confidence per hop, plus a `N symbols across M files` summary.
- The PreToolUse hook already injects a one-line warning when you edit a file containing high-fan-in symbols. Treat that warning as a prompt to run the full `aag impact` before proceeding.

## Decision rules

- **0 callers** — safe to change; also a hint the symbol may be dead code or an external entry point.
- **Few callers, all `EXTRACTED`/`INFERRED`** — read each listed call site (they come with file:line) and update them in the same change.
- **Many callers or any `AMBIGUOUS` hops** — the edge may be a same-name false positive; verify the ambiguous ones in source before trusting the count, and prefer a staged change (add new, migrate, remove old) over an in-place break.
- Changing behavior (not signature)? Callers list = review checklist for silent breakage.

## Companions

- Tests affected by the change: `git diff --name-only | aag affected --stdin` — run those first.
- Renames specifically: use the `aag-refactoring` skill (`aag rename` handles all call sites atomically).
