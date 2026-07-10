---
wiki: src/export.rs
---

# export.rs

This module is the visualization/export layer: it turns the same indexed graph `crate::storage::Graph` already holds into side-outputs, never a separate pipeline. Everything here is invoked from `write_default`, which `bigbang.rs` calls on every `aag bigbang` run unless the caller passes `--no-viz`. `write_default` writes `graph.json`, `graph.html`, `GRAPH_REPORT.md` plus `report.html`, `graph.graphml`, `cypher.txt`, and the rendered wiki, all into `.aag/`. It also writes `.aag/index.html` via `write_index`, a standalone landing page that links Graph/Wiki/Report/raw exports as one small offline site instead of a pile of files a user has to know the names of.

## Graph view

`write_html` builds `graph.html` from `GRAPH_HTML_TEMPLATE` (`assets/graph.html.template`), substituting placeholders for the graph JSON payload (`graph_data_json`), the full source text of every indexed file (`source_map_json`, so clicking a node shows the whole file with no server fetch), and the repo name. The page embeds vendored copies of D3 (`D3_JS`), sigma.js (`SIGMA_JS`, the WebGL renderer used at scale), and graphology (`GRAPHOLOGY_JS`, sigma's graph model) via `include_str!` — all bundled at compile time, no CDN, per the house rule that the site is 100% offline.

## Report

`report_markdown` builds `GRAPH_REPORT.md` (also rendered to `report.html`): file/symbol/edge counts, a confidence breakdown (`EXTRACTED`/`INFERRED`/`AMBIGUOUS`), a "god nodes" section ranking the most-connected symbols by degree, a few suggested `aag impact`/`aag explore` questions derived from those god nodes, and a `WHY:`/`HACK:` scan over indexed source files (`collect_why_hack`). The god-nodes and suggested-questions sections are skipped below `MIN_FILES_FOR_REPORT_SIGNAL` (5) distinct files — connectivity ranking on a tiny codebase is noise, not signal.

## GraphML and Cypher

`write_graphml` emits `graph.graphml`, a plain XML dump of nodes/edges with `name`/`kind`/`file` node attributes and `kind`/`confidence` edge attributes, for tools like Gephi. `write_cypher` emits `cypher.txt`, a Cypher import script (`CREATE`/`MATCH ... CREATE`) keyed by `nodeKey`; it is never executed by `aag` itself, only written for anyone who wants to load it into Neo4j/FalkorDB by hand.

## Wiki

The wiki groups symbols by file rather than by detected "community" — community detection isn't implemented in v1, and `crate::resolve` already has file grouping as ground truth rather than a clustering guess. `build_wiki_pages` is the shared builder: it reads all nodes, splits out `NodeKind::Doc` nodes, and parses each doc's optional frontmatter via `parse_doc_frontmatter` (`wiki: <path>` merges the doc's body into that source file's page via `crate::resolve`'s file identity; `title:` names a standalone page). For each remaining file it emits a page listing its symbols and, for each, its callers (via `Graph::callers`). This produces a `WikiPage` list plus an index, consumed by two writers:

- `write_wiki` writes raw `.md` files (one per `WikiPage`, slugified via `slugify`, plus `index.md`) — used by the Obsidian export because Obsidian needs real markdown, not HTML.
- `write_wiki_html` renders the same pages to `.html` via `render_markdown`, wraps each in `wiki_shell` (an Obsidian-style shell with a persistent `wiki_sidebar` file-tree and a "view in graph" deep link built by `query_encode`), and writes them under `.aag/wiki/`. This is what the default site links to.

`write_obsidian` is the one opt-in export (it writes outside `.aag/`, into a caller-named vault) — it calls `write_wiki` into `<vault_dir>/aag/`, never the vault root, so an existing vault's notes and `.obsidian` config are untouched.

## Markdown rendering

`render_markdown` is a small hand-rolled renderer, not a general CommonMark parser — it covers only the markdown subset this module itself generates (and that agent-authored wiki docs use): `#`/`##`/`###` headings, `- ` list items, fenced code blocks, `` `inline code` ``, `[text](x.md)` links (rewritten to `.html` targets), and a whole-line `_italic_` marker. `inline` checks the whole-line italic case first and recurses on the stripped text, so an underscore inside a link target like `a_rs.md` is never mistaken for an emphasis marker. `page_shell` and `wiki_shell` are the two page frames every generated `.html` page renders inside, sharing the same dark palette so the whole `.aag/` output reads as one site.
