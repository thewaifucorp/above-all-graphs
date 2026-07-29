---
wiki: src/pr.rs
---

# pr.rs

Graph-backed pull-request workflow. This is P1.11 of
[capability coverage](capability-coverage.md).

GitHub already says what a pull request *is*: title, branch, checks, review
state. `gh` prints that fine, and repeating it adds nothing. What GitHub cannot
say is what the change *reaches* — which symbols the rest of the repository
depends on, which tests have to run, and which other open PR is about to
collide with this one. That is the graph's half, and it is the only reason this
module exists.

## Commands

```bash
aag pr dashboard              # every open PR, highest risk first, with reasons
aag pr conflicts              # PRs that share a file or a symbol
aag pr worktrees              # local worktrees, mapped to the PR on each branch
aag pr impact 42              # one PR's blast radius and score, as JSON
```

MCP: `pr_dashboard` and `pr_conflicts`, plus the older `list_prs`,
`get_pr_impact`, and `triage_prs`.

## The risk model is a table, not a judgement

| Points | Rule |
|---|---|
| +3 each | a touched symbol with 10 or more dependents |
| +1 per 25 | symbols in the transitive blast radius |
| +4 | affected tests exist and the PR changes none of them |
| +3 | required checks are failing |
| +2 | the PR overlaps another open PR |

Every point is printed next to the rule that produced it, so a score can be
argued with instead of believed. Bands are `low` (0–3), `medium` (4–9), `high`
(10+). Drafts sink below ready work at equal risk.

Run against a repository with 37 open pull requests, the top entry reads:

```text
#2208 [high, risk 41] security: harden input validation
     4 file(s), 72 symbol(s), blast radius 147
     +27  touches 9 hub symbol(s) with 10+ dependents: …LocalBackend, …ensureInitialized, …
     +5   147 symbols transitively depend on this change
     +4   26 affected test file(s) and none of them changed: …
     +3   required checks are failing
     +2   overlaps 21 other open pull request(s)
```

Four changed files. The diff is small; the reach is not.

## Three kinds of overlap

- **`conflict`** — both PRs change the same file. A merge conflict on the way,
  and the only one `git` will warn you about.
- **`semantic`** — both reach the same symbol without sharing a file. This is
  the one a diff cannot show: the branches merge cleanly and still disagree.
- **`adjacent`** — they land in the same community and share nothing else.
  Proximity, not a problem, so `aag pr conflicts` leaves it out and only the
  dashboard mentions it.

## Where the seams are

Everything that talks to GitHub is a thin `gh` call at the edge —
`gh pr list --json …` for the metadata, `gh pr diff --name-only` for the files.
The analysis is pure functions over `(graph, changed files)`, which is what
makes the tests hermetic: they build a small repository where one symbol has
twelve dependents and assert that it does not score like a leaf.

## Deliberate limits

- **One `gh pr diff` per pull request.** Thirty-seven open PRs cost thirty-seven
  round trips (about 38 seconds on that repository). Fine for a review sitting,
  not for a hook.
- **Changed files, not changed lines.** A PR that edits one line of a file gets
  that file's symbols attributed to it. This over-reports on large files and is
  the honest cost of not parsing hunks.
- **The base branch is the index.** The graph describes the working tree, not
  each PR's head. A PR that adds a symbol shows up through the files it touches,
  not through what it introduces.
- **`worktrees` needs no PR to be useful.** A branch with nothing open is
  reported as such rather than dropped.
