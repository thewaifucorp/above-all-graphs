---
wiki: src/memory.rs
---

# memory.rs

Outcome-backed work memory: what was asked, what was answered, which symbols it
rested on, whether it held up, and what corrected it. P1.12 of
[capability coverage](capability-coverage.md).

## The constraint is the design

The gate asks for reviewable lessons **without letting stale experience override
current source evidence**. Two rules follow, and the module is built around them:

1. **Every recalled entry is checked against the graph as it is now.** An entry
   whose supporting symbols are gone comes back marked `stale`, with the missing
   names listed. An entry that names no symbols is stale too — nothing ties it to
   the repository, so nothing can check it.
2. **A lesson is a review candidate with its evidence attached.** It carries how
   many entries it came from, how many of those the graph still supports, and
   their ids. A lesson about deleted code is labelled history rather than
   repeated as advice.

Both outputs open by saying what memory is: recorded experience, not extracted
evidence. Where the two disagree, the graph is right.

```bash
aag memory save --question "how does call resolution pick a candidate" \
                --answer "the narrowing ladder in resolve_call" \
                --nodes resolve_call,candidates --outcome open
aag memory correct 3 --outcome wrong --correction "receiver typing runs first"
aag memory recall "call resolution"
aag memory lessons
```

Over MCP: `memory_save` (`question`, `answer`, and optionally `nodes`, `outcome`,
`correction`, `revision`), `memory_recall`, and `memory_lessons`.

## What is stored

| Field | Why it is there |
|---|---|
| question, answer | The work itself |
| nodes | What the answer rested on — this is what makes staleness checkable |
| outcome | `worked`, `wrong`, or `open` |
| correction | What replaced a wrong answer |
| revision | The commit the work landed in |
| recorded | When |

An unrecognized outcome parses as `open`, never as a success: an unverified
answer must not be counted as one that held up.

Memory lives in `.aag/memory.db`, beside the graph and separate from it. A
forced rebuild (`aag bigbang --force`) preserves it, because the index can be
recomputed from source and what a session learned cannot.

## Recall

Relevance is word overlap with the stored question, minus the words every
question contains. That is deliberately dumb: memory is a hint, and a clever
matcher would make it feel like an answer. At equal relevance a `wrong` entry
outranks an `open` one — knowing what failed is the more useful memory.

## Lessons

A lesson needs at least two entries about one symbol, because one outcome is an
anecdote. It says which way the outcomes went and quotes the corrections:

```text
answers about `resolve_call` were wrong 2 of 2 times; corrected to: receiver
typing runs first (from 2 entries, 2 still supported by the graph; ids 1, 2)
```

## Deliberate limits

- Nothing is inferred beyond counting outcomes per symbol. There is no
  clustering, no embedding, and no model in the loop — a lesson you cannot
  check in one glance is not reviewable.
- Memory is per repository. Nothing is shared across workspaces, and a group
  query does not read it.
- An entry is never used to answer a question on its own. It is returned
  alongside its staleness so the reader decides.
