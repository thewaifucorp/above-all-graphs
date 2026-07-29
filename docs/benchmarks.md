---
wiki: src/bench.rs
---

# Benchmarks

Results for every track of the evaluation contract in
[capability coverage](capability-coverage.md), each labelled with the subject it
measures. Track A is a **protocol benchmark**, Tracks B and E are **engine
benchmarks**, and Tracks C and D are **end-to-end benchmarks** — the contract
forbids transferring a result from one layer to another, so they are reported
separately and never averaged.

Every corpus is external. Raw records are in `bench/empirical/`, append-only.

| Track | Subject | Status |
|---|---|---|
| A — protocol conformance | manifests this engine compiles | 11/11 rules on 2 corpora |
| B — engine extraction | entities and calls, Python | P 1.000 / R 0.998 entities; P 0.974 / R 0.997 calls |
| C — agent utility | consumer accuracy by producer | graph 1.000 on both tiers; raw 0.857 / 0.982 |
| D — end-to-end economics | measured cost per condition | graph costs ¼ of raw search, in one turn |
| E — scale and operations | indexing, queries, memory, size | 4 corpora, 57 → 15 361 files |

## Track E — scale and operations

This is an **engine benchmark**: indexing, queries, memory, and artifact size.
It says nothing about agent task quality.

```bash
aag bench --repo /path/to/repo --repetitions 3   # measure and append a record
aag bench --report                                # print what was recorded
aag bench --repo big --skip-export                # every metric except the site
```

Records are appended to `bench/<run-kind>/runs.jsonl`, one JSON line each,
append-only. Nothing here ever rewrites a line: a corrected metric is a new
run.

## Run classes

`empirical`, `pilot`, and `simulated` live in separate directories and are
never averaged together. A run against this repository is recorded as **pilot**
whatever the command asked for — the harness detects its own source and
downgrades the class, because dogfood cannot substantiate an engine claim.

## Results

Release build, Linux x86-64, warm page cache, three repetitions unless noted.
Every corpus below is external: none of them was written to tune this engine.

| repository | revision | files | symbols | edges | cold index p50 | one-file resync p50 | search p95 | callers p95 | export | db | peak RSS |
|---|---|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|
| katsui-infra | `888e16ff` | 57 | 56 | 103 | 70 ms | 26 ms | 0.07 ms | 0.03 ms | 0.8 MB | 0.3 MB | 21 MB |
| katsui | `43d2842e` | 130 | 114 | 108 | 321 ms | 44 ms | 0.05 ms | 0.03 ms | 1.6 MB | 0.4 MB | 24 MB |
| gitnexus | `ba5de0bd` | 1836 | 4269 | 98 437 | 8.7 s | 3.5 s | 0.13 ms | 0.07 ms | 458 MB | 15.8 MB | 1252 MB |
| flutter | `00b0c91f` | 15 361 | 86 808 | 417 605 | 200 s | 95 s | 0.20 ms | 0.03 ms | not measured | 140 MB | 196 MB |

Pilot (this repository, not evidence of anything external): 107 files, 1493
symbols, 9065 edges, cold index 473 ms, resync 97 ms, export 11.1 MB, db 5.7 MB.

## What the numbers say

**Queries are not the problem.** Search and callers stay under a fifth of a
millisecond at every size measured, including 417 605 edges. SQLite with FTS5
and an index on both edge endpoints is enough; nothing here needs a graph
database.

**Incremental update scales with the repository, not with the edit.** 26 ms on
57 files, 3.5 s on 1836, 95 s on 15 361. `index_file` re-resolves cross-file
references globally after replacing one file's symbols, so the cost tracks
total graph size. That is fine for the repositories most people work in and
plainly wrong for a monorepo — on flutter the "incremental" path costs half a
cold index. It is a real limit of the current design, recorded here rather than
rounded off.

**The exported site grows with edges, not files.** gitnexus produces 458 MB of
export from 1836 files because it has 98 437 edges and the export writes a
per-file page plus the graph payload. `--skip-export` exists because of it, and
because a machine without that much disk should still be able to record every
other metric.

**Peak memory follows edge density, not file count.** gitnexus (1836 files,
98 k edges) peaked at 1.25 GB while flutter (15 361 files, 418 k edges) peaked
at 196 MB — the difference is that the gitnexus run also built the export,
which holds the whole graph payload in memory. The flutter run skipped it.

## Track A — protocol conformance

A **protocol benchmark**: are the manifests this engine compiles valid, stable,
and interpretable. Eleven rules, checked on manifests compiled from external
repositories rather than on fixtures written to pass.

```bash
python3 scripts/track_a_conformance.py <repo> --json bench/empirical/track-a-<name>.json
```

| repository | entities | relationships | identifiers | rules passed |
|---|--:|--:|--:|---|
| gitnexus `ba5de0bd` | 5922 | 88 051 | 180 167 | 11 / 11 |
| katsui `43d2842e` | 173 | 95 | 317 | 11 / 11 |

The rules: schema valid, semantics valid, identifiers stable across two
independent compilations, identifiers unique, every reference resolves,
ownership preserved, evidence present on every entity, uncertainty present on
every relationship, freshness declared, versions exact, and a second reader
seeing the same document. Stability is checked by compiling twice and diffing
the identifiers — a manifest whose ids move between runs cannot be referenced
by anything.

## Track B — extraction quality, Python

An **engine benchmark** against an **independent oracle**: CPython's own `ast`
module, written by other people for another purpose, and the definition of what
a Python function, class, or call is. No ground truth was authored here, which
is the point — ground truth written by whoever wrote the extractor proves
nothing.

```bash
aag bigbang --path <repo> --no-viz --no-install
python3 scripts/track_b_python.py <repo> <repo>/.aag/graph.db --json out.json
```

| repository | Python files | entities P / R | calls P / R |
|---|--:|---|---|
| gitnexus `ba5de0bd` | 162 | 1.000 / 1.000 (486) | 0.974 / 0.997 (340) |
| flutter `00b0c91f` | 104 | 1.000 / 0.996 (545) | 0.987 / 0.990 (620) |

Records: `bench/empirical/track-b-python-*.json`.

Calls are compared at name level on both sides, restricted to callees the
repository itself defines: the oracle cannot resolve a receiver's type any
better than the engine can, and a finer comparison would measure the comparison
rather than the engine. The nine false-positive call edges out of 348 are the
resolver's AMBIGUOUS fan-out landing on a same-named function the oracle
attributes elsewhere — which is why those edges are labelled AMBIGUOUS in the
graph instead of being presented as facts.

**Both misses are one behavior, not noise.** flutter's two missing functions are
`main` in `.../fuchsia/flutter/build/asset_package.py` and
`.../build/gen_debug_wrapper_main.py`. `engine/src/flutter/.gitignore` contains
`*/**/build/`; git tracks those files anyway, because ignore rules do not apply
to files already tracked, while the engine's walker honors the ignore rule and
skips them. Precision is 1.000 on both corpora: the engine invented nothing.

Not covered by this oracle: contract matching, impact false positives, and
affected-test accuracy. Those need ground truth in a form CPython does not
supply.

## Tracks C and D — agent utility and what it costs

An **end-to-end benchmark**: the same question, the same consumer, five
different producers. Tasks and answers come from the CPython oracle, so nobody
here wrote the answers; each condition is one model call (the LLM-only producer
is two: one to compile its slice, one to answer from it), and the consumer CLI
reports the dollar cost of every call.

```bash
python3 scripts/track_cd_consumer.py <repo> --tasks 6 --repetitions 2 \
    --model claude-sonnet-5 --json bench/empirical/track-cd.json
```

The transfer matrix, five tasks per cell, both consumer tiers. Each task asks
for the callers of a symbol that has between five and fifteen of them — with
only two, every condition scored 1.000 and the benchmark measured nothing.

| producer | compact F1 | frontier F1 | compact $/task | frontier $/task | compact turns | frontier seconds |
|---|--:|--:|--:|--:|--:|--:|
| none (floor) | 0.000 | 0.000 | 0.0482 | 0.2348 | 4.0 | 16.5 |
| reference manifest (oracle) | 1.000 | 0.800 | 0.0122 | 0.0816 | 1.0 | 8.7 |
| **aag graph slice** | **1.000** | **1.000** | **0.0142** | **0.0639** | **1.0** | **5.3** |
| LLM-only manifest | 0.500 | 0.674 | 0.0556 | 0.2195 | 1.0 | 4.7 |
| raw repository access | 0.857 | 0.982 | 0.0573 | 0.1681 | 5.4 | 19.5 |

Compact is `claude-haiku-4-5`, frontier is `claude-sonnet-5`. Records:
`bench/empirical/track-cd-matrix-*-v2.json`.

**What it says.** The graph answers every task exactly on both tiers, in one
turn, for a quarter of what searching the repository costs and in a quarter to
a third of the time. Searching still gets most of the way there — 0.857 and
0.982 — but takes five to six turns to do it. The floor is zero, so the tasks
are not guessable. An LLM-compiled manifest is the worst working condition:
half to two-thirds accuracy at the highest price on the frontier tier, because
a model asked to compile context drops findings it just made.

The reference producer scoring 0.800 on the frontier tier while the graph scores
1.000 is not a real ranking: it is one task where the consumer was handed the
answer and talked itself out of it. With five tasks per cell, one such event
moves a cell by 0.2.

### A correction, and what caused it

The first version of this matrix reported the graph *losing* to raw repository
access (0.445 / 0.611 against 0.611 / 0.667). That was a defect in the
benchmark, not a finding about the engine.

The oracle attributed each call to the nearest enclosing function **and to every
enclosing class**, so a call inside a method counted as a call by the method and
by its class. Every answer key gained a phantom caller, and a producer that
answered correctly scored 2 × 1 × 0.5 / 1.5 = 0.667 — which is exactly the value
that kept recurring across unrelated conditions. The engine had been right and
the benchmark had been wrong, in the direction that flattered nobody.

Per the contract's rule that raw records are append-only and a corrected
evaluation creates a new derived result rather than rewriting an execution
record, the original run stays in `bench/empirical/track-cd-matrix-haiku.json`
and `track-cd-matrix-sonnet.json`, superseded by the `-v2` files. The
`track-cd-gitnexus.json` run (8 tasks × 2 repetitions) used the same broken
oracle and is superseded too; its dispersion figures should not be quoted.

The lesson is in the harness now: an oracle that punishes the correct answer is
worse than no oracle, and the tell was a suspiciously constant score.

**Costs are dollars, not estimates.** Every figure above is what the provider
charged, per call, recorded per task. The corrected matrix cost $4.86; all
Tracks C and D runs together, including the superseded ones, cost $20.64.

## What this harness does not measure

Every track now has empirical results on external corpora, and every result
carries its own limits:

| Track | Measured | Not claimed |
|---|---|---|
| A | 11 conformance rules, 2 corpora | one producer — no second implementation to test interoperability against |
| B | entities and calls, Python, 2 corpora | contract matching, impact false positives, affected-test accuracy; one language |
| C | 5 producers × 2 consumer tiers | one task family, one vendor's models, 5 tasks per matrix cell |
| D | dollars per call, per task | no amortization model, no break-even count across a real workload |
| E | 4 corpora, 57 → 15 361 files | one machine, one OS, warm cache |

The sample sizes are the weakest part and are stated rather than hidden: five
tasks per matrix cell, one task family (`who calls X`), one language for the
oracle, one model vendor for the consumer, and no repetitions in the corrected
matrix — the repeated run that provided dispersion used the broken oracle and
was superseded. Widening any of those is more of the same work, not new
machinery.

## Reproducing

```bash
cargo build --release
./target/release/aag bench --repo <external-repo> --repetitions 3
./target/release/aag bench --report
```

Each record carries the producer name, version, build features and profile, the
repository name, revision, and dirty state, the corpus profile (tracked files,
parsed files, symbols, docs, relationships, per-extension counts, test files),
the repetition count, and every distribution as min/p50/p95/max/mean. Records
from a different `schema_version` are refused rather than averaged in.
