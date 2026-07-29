# Example corpora

Repositories used to measure this engine, and how to reproduce a measurement on
one. Every corpus here is **external**: none of them was written by this
project, and none was used to design the extraction rules or tune the resolver.

The tier labels below are corpus labels, not claims derived from a file count.
Each row carries the profile the benchmark records: tracked files, files the
engine parsed, symbols, and relationships.

| Tier | Repository | Revision measured | Tracked | Parsed | Symbols | Edges | Dominant languages |
|---|---|---|--:|--:|--:|--:|---|
| small | `katsui-infra` | `888e16ff` | 57 | 13 | 56 | 103 | HCL, YAML, shell |
| medium | `katsui` | `43d2842e` | 130 | 59 | 114 | 108 | TypeScript, Markdown |
| large | `gitnexus` | `ba5de0bd` | 1836 | 1624 | 4269 | 98 437 | TypeScript, test fixtures in 12 languages |
| industrial | `flutter` | `00b0c91f` | 15 361 | 10 766 | 86 808 | 417 605 | Dart, C++, Java, Objective-C |

Results are in [benchmarks](benchmarks.md); the raw records are
`bench/empirical/runs.jsonl`, one append-only JSON line per run.

## Running one yourself

```bash
git clone <repo> /tmp/corpus && git -C /tmp/corpus checkout <revision>
cargo build --release
./target/release/aag bench --repo /tmp/corpus --repetitions 3
./target/release/aag bench --report
```

`--skip-export` records every metric except the exported site, which is what
you want on a machine that cannot spare a few hundred megabytes: the gitnexus
export is 458 MB.

## Choosing a corpus

- **External, or it proves nothing.** A benchmark against this repository is
  recorded as a `pilot` by the harness itself, no matter what the command asked
  for, and pilots are never averaged with empirical runs.
- **Pin the revision.** The record stores the commit and whether the working
  tree was dirty; a dirty tree makes a run unreproducible and says so in the
  record rather than hiding it.
- **Report the profile, not the adjective.** "Large" means the numbers in the
  row above, not a feeling about the repository.

## What these corpora cannot tell you

They measure the engine's scale and operations — Track E. They say nothing
about extraction *accuracy*, which needs ground truth authored by someone other
than the person writing the extractor, and nothing about whether an agent using
the graph does better work, which needs a consumer model and a factorial
design. Both remain open, and
[capability coverage](capability-coverage.md) records them as open.
