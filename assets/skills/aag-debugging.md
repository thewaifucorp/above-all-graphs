---
name: aag-debugging
description: Trace bugs and errors using the aag knowledge graph — "why does X fail", "where does this error come from", following an execution path from symptom to cause. The graph gives you the callers/callees chain instantly instead of grepping call sites one by one.
---

# Debugging with aag

When tracing a failure, work the call chain through the graph instead of hand-grepping each hop.

## Method

1. **Locate the symptom**: take the function/symbol from the error message or stack trace and run MCP `explore` (CLI: `aag explore <symbol>`) — you get its source, everything that calls it, and everything it calls, in one shot.
2. **Walk upstream**: the bug is often in a caller passing bad state. `callers` (or the callers section of `explore`) lists every entry path, transitively. Check each caller's source — `explore` already returned it verbatim.
3. **Walk downstream**: if the symbol itself looks fine, `callees` shows what it depends on; a failing dependency surfaces there.
4. **Check the blast radius of your fix** before editing: `aag impact <symbol>` — if the fix touches something with many transitive callers, look at those call sites for the same misuse pattern (one bug often repeats).

## Signals worth trusting

- `AMBIGUOUS` edges: more than one symbol shares the name — the call may resolve to a different definition than you assume. Verify which one actually runs.
- A doc node with an `explains` edge to the failing symbol: the repo documented intent there — compare intended vs. actual behavior.
- No callers at all in the graph for something that "should" be called: dead code, dynamic dispatch, or an external entry point — all three are leads.

## When NOT to use

- The file changed in the last few seconds (sync may be in flight — read directly).
- Runtime-only issues (env vars, network, data) with no code-path question.
