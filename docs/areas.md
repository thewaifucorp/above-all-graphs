---
wiki: src/areas.rs
---

# areas.rs

Repository-area skills, generated from the graph. This is P1.15 of
[capability coverage](capability-coverage.md).

The seven skills in `assets/skills/` teach an agent how to *use* `aag`. They
are identical in every repository, because they have to be. What they cannot
say is what *this* repository is made of: which areas exist, what runs first in
each, which symbols the rest of the code leans on, and which areas are coupled
to which.

All of that is already in the graph. This module turns it into one `SKILL.md`
per area, so an agent that has never seen the repository starts oriented
instead of grepping for a map.

```bash
aag areas          # what was detected, and what each area leans on
```

Generation runs inside `aag bigbang` and `aag sync` — no separate step, and
`aag uninstall` removes every generated page.

## What one page says

```text
---
name: aag-area-src-core-ingestion
description: The `src/core/ingestion` area — 179 symbols across 74 file(s)…
---
## Starts here            entrypoints declared in the area
## What the rest of the code leans on   its hubs, by dependent count
## Contracts it serves    HTTP endpoints declared or implemented here
## Flows rooted here      call chains from `aag processes`
## Coupled to             other areas, by edges across the boundary
## Files                  where it lives
```

Everything comes from [`analysis`](../src/analysis.rs) — the same community
detection, entrypoints, and processes the site and CLI already use.

## Determinism

Same graph in, byte-identical files out. No timestamps, no hash-order lists,
every section sorted. Refresh compares content and rewrites only what changed,
so `aag sync` on an unrelated edit touches nothing and the watcher never sees a
spurious write. Areas that stop existing have their pages pruned — a merged or
deleted module must not leave a page behind claiming it is still there.

## Naming

An area is named for the deepest directory holding a majority of its symbols:
`src/core/ingestion`, not `src`. Two communities in one directory would collide,
so the larger one keeps the directory name and the smaller is named for the file
that holds most of it (`src` and `src/storage.rs`). Slugs follow the name, which
is what makes the filename stable across runs.

## Deliberate limits

- **Communities are as good as label propagation is.** A flat crate whose
  modules all call each other comes back as one area — this repository detects
  two, one of them 1147 symbols wide. On a structured tree the split is what you
  would draw by hand: 12 areas on a 1000-file monorepo, down to
  `field-extractors/configs`.
- **A community can cross a boundary that means nothing.** Test fixtures in
  twelve languages cluster together because they look alike, and the area gets
  named after whichever file holds most of them.
- **Vendored bundles are excluded** by path (`.min.`, `vendor/`,
  `third_party/`): a minified library is a large cluster of symbols nobody here
  wrote, and a page about it would be a page about someone else's code.
- **Under 8 symbols is not an area**, and at most 12 areas are generated. Fifty
  pages would compete with each other for the agent's attention.
- **Generated pages are not user-editable.** They say so at the top, and the
  next `bigbang` overwrites them. The static pack stays write-if-missing, so
  edits there survive.
