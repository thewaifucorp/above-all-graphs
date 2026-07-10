---
name: aag-wiki
description: Write documentation that shows up in the aag wiki (.aag/wiki/). Use whenever asked to document a module, file, or concept in this repo, or after implementing something worth explaining. Covers the frontmatter format, where files go, and how pages link together.
---

# Writing aag wiki documentation

`aag bigbang` indexes every `.md`/`.txt` in the repo (except `.git`, `.aag`, `target`, `node_modules`) as a **doc node** and renders the wiki at `.aag/wiki/`. A PostToolUse hook already regenerates the site after every Write/Edit — just write the markdown file and it appears.

## Format

Put docs in `docs/` (any path works, `docs/` keeps the repo tidy). Optional YAML frontmatter routes the content:

```markdown
---
wiki: src/export.rs
---

## What this module does

Prose explaining src/export.rs. Rendered at the TOP of that file's
wiki page, above the auto-generated symbol/caller sections.
```

Frontmatter keys (both optional):

- `wiki: <path>` — merge this doc's body into that source file's wiki page. The doc loses its standalone page. Path is repo-relative, exactly as indexed (e.g. `src/parse.rs`).
- `title: <text>` — heading for a **standalone** page (no `wiki:` key, or target not indexed). Without it, the file path is the heading.

No frontmatter at all = standalone page whose body is the whole file.

## What the renderer supports

`#`/`##`/`###` headings, `- ` lists, `[text](page.md)` links (auto-rewritten to `.html`), `` `inline code` ``, whole-line `_italic_`, and ``` fenced code blocks. Keep to this subset — it is NOT full CommonMark (no tables, no images, no bold).

## Linking

- Link other wiki pages by slug: `[parse](src_parse_rs.md)`. Slug = path with `/`, `\`, `.` replaced by `_` (e.g. `src/export.rs` → `src_export_rs`).
- **Mention symbol names in prose** (e.g. "the `build_wiki_pages` function"): the indexer creates `explains` edges from your doc to those symbols automatically, which surfaces the doc in the graph and in "Called by" lists.
- Every page gets a "view in graph" deep link automatically; you can hand-write one too: `../graph.html?focus=src/export.rs`.

## Workflow

1. Write/edit `docs/<topic>.md` with frontmatter.
2. Hook regenerates `.aag/` automatically (or run `aag sync`).
3. Verify: open `.aag/wiki/index.html` — merged docs appear at the top of the target file's page marked "_documented in docs/<topic>.md_".
