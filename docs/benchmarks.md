---
wiki: src/bench.rs
---

# Engine benchmarks

This is an **engine benchmark**, Track E of the evaluation contract in
[capability coverage](capability-coverage.md): scale and operations of the
AboveAllGraphs Engine. It is not a protocol benchmark and not an end-to-end
benchmark, and none of the numbers below say anything about agent task quality.

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

## Track B, one slice: Python entity extraction

Entity precision and recall against an **independent oracle** — CPython's own
`ast` module, written by someone else, for another purpose, and the definition
of what a Python function or class is. No ground truth was authored here, which
is the point: ground truth written by whoever wrote the extractor proves
nothing.

```bash
aag bigbang --path <repo> --no-viz --no-install
python3 scripts/track_b_python.py <repo> <repo>/.aag/graph.db --json out.json
```

| repository | Python files | functions P / R | classes P / R | all entities P / R |
|---|--:|---|---|---|
| gitnexus `ba5de0bd` | 162 | 1.000 / 1.000 | 1.000 / 1.000 | 1.000 / 1.000 (486) |
| flutter `00b0c91f` | 104 | 1.000 / 0.996 | 1.000 / 1.000 | 1.000 / 0.996 (545) |

Records: `bench/empirical/track-b-python-*.json`.

**Both misses are one behavior, not noise.** flutter's two missing functions are
`main` in `.../fuchsia/flutter/build/asset_package.py` and
`.../build/gen_debug_wrapper_main.py`. `engine/src/flutter/.gitignore` contains
`*/**/build/`; git tracks those files anyway, because ignore rules do not apply
to files already tracked, while the engine's walker honors the ignore rule and
skips them. Precision is 1.000 on both corpora: the engine invented nothing.

This is a slice of Track B, not Track B. It covers entities in one language.
Relationship precision and recall by type, resolution ambiguity, contract
matching, impact false positives, and affected-test accuracy all still need
ground truth nobody here can supply without authoring the answers to their own
exam.

## What this harness does not measure

| Track | Status |
|---|---|
| A — protocol conformance | not implemented here; the protocol is a separate subject |
| B — engine extraction quality | partially measured: Python entities against CPython's `ast`, above. Relationships, ambiguity, contracts, and impact accuracy are not claimed |
| C — agent utility | needs a consumer model and a factorial design; no model is called |
| D — end-to-end economics | needs C; no token or call cost is measured |
| E — scale and operations | this harness |

Tracks B, C, and D stay open, and P0.1 stays open with them. Publishing Track E
numbers is not the same as closing the contract, and the harness prints these
caveats next to every report so a number is never read as more than it is.

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
