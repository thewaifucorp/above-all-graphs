# Migration notes

`aag` upgrades by replacing one binary. There is no migration script, no
server, and no state outside the repository's `.aag/` directory and the agent
config files `install` wrote. This page exists so you know what changes
underneath you and what you have to do about it — usually nothing.

## What is on disk

| Path | What it is | Safe to delete |
|---|---|---|
| `.aag/graph.db` | the index | yes — rebuilt by `aag bigbang` |
| `.aag/refs/*.db` | per-ref snapshots for `graph-diff` | yes — recomputed on demand |
| `.aag/memory.db` | work memory (`aag memory`) | **no** — not derived from the repository |
| `.aag/index.html`, `graph.html`, `wiki/`, `report.html`, `graph.json`, `graph.graphml`, `cypher.txt` | the offline site and exports | yes |
| `.aag.lock` | rebuild/watcher mutual exclusion | yes when nothing is running |
| agent config files | what `install` wrote | use `aag uninstall`, which removes exactly those |

`bigbang --force` deletes `.aag/` and rebuilds — and deliberately preserves
work memory across the rebuild, because an index can be recomputed from source
and what a session learned cannot.

## Index format changes

The index records a `raw_references` marker in `index_metadata`. When the
binary expects a newer marker than the database carries, the index reads as
not-ready and the next run does a full rebuild by itself. There is nothing to
run and nothing to convert; the first command after an upgrade is slower once.

| Marker | Introduced | What changed |
|---|---|---|
| `1` | 0.1.0 | unresolved references stored as plain strings |
| `2` | 0.2.0 | structured JSON references — import source, alias, glob, receiver, declared type — which is what language-aware resolution needs |

## 0.1.x → 0.2.0

- **The index rebuilds itself** on first use, per the marker above.
- **Resolution got stricter.** An import that resolves outside the repository
  (`std::fs::File`, `react`) no longer produces an edge to a same-named local
  symbol. Some edges that existed in 0.1 are gone because they were wrong; the
  AMBIGUOUS count drops sharply on any real repository.
- **New commands**, none of which change existing behavior: `aag areas`,
  `aag bench`, `aag graph-diff`, `aag pr *`, `aag db scan|drift`.
- **New MCP tools**, still unlisted by default: `graph_diff`, `pr_dashboard`,
  `pr_conflicts`, `db_drift`. `explore` remains the only listed tool unless
  `AAG_MCP_TOOLS` says otherwise.
- **Generated area skills** appear as `aag-area-*` in each agent's skill
  directory. They are regenerated on every `bigbang` and `sync`, removed by
  `aag uninstall`, and never merged with the hand-written pack.
- **`install` covers fourteen agents** instead of seven. The new ones are only
  touched when their config directory exists; nothing is created speculatively.

## Downgrading

Install the older binary and run `aag bigbang --force`. The older binary will
not understand a newer index marker, and reacts the way it reacts to any index
it cannot read: by rebuilding one it can. Agent config written by a newer
version is removed by that newer version's `aag uninstall` — run it before
downgrading if you want a clean slate.

## What never changes without saying so

- Config edits stay additive and reversible. An upgrade never rewrites a hook,
  a rule file, or an MCP entry you edited; skills are write-if-missing.
- The exported site stays fully offline.
- Benchmark records carry a `schema_version` and are refused, not silently
  averaged, when the harness moves on.
