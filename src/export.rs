//! Visualization/export — side-outputs of the same indexed graph, never a
//! separate pipeline. Per `SPEC.md` section 6: `graph.json`, `graph.html`,
//! the report, `graph.graphml`, `cypher.txt`, and the wiki are all written
//! by default on every `aag bigbang` (skippable with `--no-viz`); only the
//! Obsidian vault export is opt-in (it writes outside `.aag/`, into a vault
//! the caller names).
//!
//! `.aag/index.html` ties the rest together as one small site — a landing
//! page linking Graph/Wiki/Report/raw exports — rather than a pile of
//! unrelated files a user has to already know the names of. The wiki and
//! report render as real `.html` (via [`render_markdown`], a tiny renderer
//! covering only the markdown subset this module itself generates) so they
//! open straight in a browser; `GRAPH_REPORT.md` is also kept in raw
//! markdown alongside `report.html` for tools/agents that want it as text.
//!
//! `graph.html` embeds a real vendored copy of D3 (`assets/d3.v7.min.js`,
//! BSD-3-Clause, Mike Bostock) via `include_str!`, so the page renders
//! offline with no server and no CDN fetch.
//!
//! Community detection (clustering symbols into the wiki's per-community
//! pages) is not implemented in v1 — `SPEC.md`'s own risk note flags that a
//! naive clustering pass on a small codebase produces noise, not signal.
//! The wiki groups by file instead, which is a source of ground truth
//! `crate::resolve` already has, not a heuristic guess.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::storage::{Edge, Graph, Node, NodeKind};

const D3_JS: &str = include_str!("../assets/d3.v7.min.js");
/// Vendored sigma.js (WebGL renderer) + graphology (its graph model) — the
/// render layer scales to multi-thousand-node repos where canvas 2D chokes.
/// d3 stays vendored too: the ForceAtlas2/noverlap physics use `d3.quadtree`.
const SIGMA_JS: &str = include_str!("../assets/sigma.min.js");
const GRAPHOLOGY_JS: &str = include_str!("../assets/graphology.umd.min.js");
const GRAPH_HTML_TEMPLATE: &str = include_str!("../assets/graph.html.template");

/// Minimum distinct files indexed before the report's god-nodes and
/// suggested-questions sections render — below this, ranking connectivity
/// is noise, not signal (see `SPEC.md`'s risks section).
const MIN_FILES_FOR_REPORT_SIGNAL: usize = 5;

/// Writes every default export artifact into `aag_dir`, reading source
/// files under `root` for the `WHY:`/`HACK:` scan. Callers skip this
/// entirely for `--no-viz`.
///
/// # Errors
///
/// Returns an error if the graph can't be read or an artifact can't be written.
pub fn write_default(root: &Path, aag_dir: &Path, graph: &Graph) -> Result<()> {
    let nodes = graph.all_nodes()?;
    let edges = graph.all_edges()?;

    write_json(aag_dir, &nodes, &edges)?;
    write_html(root, aag_dir, &nodes, &edges)?;
    let report_md = report_markdown(root, &nodes, &edges);
    write_file(&aag_dir.join("GRAPH_REPORT.md"), &report_md)?;
    write_file(
        &aag_dir.join("report.html"),
        &page_shell("Report", "", &render_markdown(&report_md)),
    )?;
    write_graphml(aag_dir, &nodes, &edges)?;
    write_cypher(aag_dir, &nodes, &edges)?;
    write_wiki_html(&aag_dir.join("wiki"), graph)?;
    write_index(root, aag_dir, &nodes, &edges)?;
    Ok(())
}

fn write_index(root: &Path, aag_dir: &Path, nodes: &[Node], edges: &[Edge]) -> Result<()> {
    let distinct_files = nodes.iter().filter(|n| n.kind == NodeKind::File).count();
    let docs = nodes.iter().filter(|n| n.kind == NodeKind::Doc).count();
    let name = root
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "repository".to_string());

    // Standalone landing page (not `page_shell`): first thing anyone sees,
    // so it gets the same visual identity as graph.html — centered card,
    // brand mark, stat chips — instead of the generic doc shell.
    let html = format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>{name} — aag</title>
<style>
  :root {{
    --bg-0: #0a0a0a; --bg-1: #121212; --bg-2: #1c1c1c;
    --border: #262624; --border-bright: #373531;
    --text-primary: #f2f2ee; --text-secondary: #8a8a84; --text-dim: #63635e;
    --accent: #ffc600;
    --mono: "JetBrains Mono", ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    --spectrum: linear-gradient(90deg, #ff3b3b, #ffcf3b, #3bff6e, #3bd8ff, #7a3bff);
  }}
  html, body {{ margin: 0; min-height: 100%; background: var(--bg-0); color: var(--text-primary); font-family: var(--mono); }}
  body {{ display: flex; align-items: center; justify-content: center; padding: 40px 16px; box-sizing: border-box; min-height: 100vh; }}
  .wrap {{ width: min(560px, 100%); background: var(--bg-1); border: 1px solid var(--border); border-radius: 16px; padding: 36px 36px 28px; box-sizing: border-box; position: relative; overflow: hidden; }}
  .wrap::before {{ content: ""; position: absolute; top: 0; left: 0; right: 0; height: 2px; background: var(--spectrum); }}
  .brand {{ display: flex; align-items: center; justify-content: center; gap: 8px; color: var(--text-dim); font-size: 11px; letter-spacing: 0.22em; text-transform: uppercase; }}
  h1 {{ text-align: center; font-size: 22px; margin: 14px 0 6px; }}
  .sub {{ text-align: center; color: var(--text-secondary); font-size: 13px; margin: 0 0 22px; }}
  .chips {{ display: flex; justify-content: center; gap: 8px; flex-wrap: wrap; margin-bottom: 26px; }}
  .chip {{ background: var(--bg-2); border: 1px solid var(--border); border-radius: 999px; padding: 5px 12px; font-size: 12px; color: var(--text-secondary); }}
  .chip b {{ color: var(--text-primary); font-weight: 600; }}
  a.card {{ display: block; background: var(--bg-2); border: 1px solid var(--border); border-radius: 12px; padding: 14px 16px; margin-bottom: 10px; text-decoration: none; transition: border-color 0.12s; }}
  a.card:hover {{ border-color: var(--accent); }}
  a.card.primary {{ border-color: rgba(255, 198, 0, 0.45); }}
  a.card h2 {{ margin: 0 0 4px; font-size: 14px; color: var(--text-primary); }}
  a.card p {{ margin: 0; font-size: 12.5px; color: var(--text-secondary); line-height: 1.5; }}
  .divider {{ display: flex; align-items: center; gap: 12px; color: var(--text-dim); font-size: 10px; letter-spacing: 0.14em; text-transform: uppercase; margin: 22px 0 12px; }}
  .divider::before, .divider::after {{ content: ""; flex: 1; height: 1px; background: var(--border); }}
  .raw {{ display: flex; justify-content: center; gap: 14px; flex-wrap: wrap; font-size: 12px; }}
  .raw a {{ color: var(--text-secondary); text-decoration: none; border-bottom: 1px dotted var(--border-bright); }}
  .raw a:hover {{ color: var(--accent); }}
  .foot {{ text-align: center; color: var(--text-dim); font-size: 11px; margin-top: 24px; }}
</style>
</head>
<body>
<div class="wrap">
  <div class="brand">
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none"><path d="M12 5.5 6 18.5M12 5.5l6 13M6.8 15.5h10.4" stroke="#373531" stroke-width="1.3"/><circle cx="12" cy="5.5" r="3" fill="#ff3b3b"/><circle cx="6" cy="18.5" r="3" fill="#ffcf3b"/><circle cx="18" cy="18.5" r="3" fill="#3bd8ff"/></svg>
    aag &middot; above all graphs
  </div>
  <h1>{name}</h1>
  <p class="sub">Indexed code graph — explore it, read it, query it.</p>
  <div class="chips">
    <span class="chip"><b>{files}</b> files</span>
    <span class="chip"><b>{symbols}</b> symbols</span>
    <span class="chip"><b>{docs}</b> docs</span>
    <span class="chip"><b>{edges}</b> edges</span>
  </div>
  <a class="card primary" href="graph.html"><h2>&#11042; Graph</h2><p>Interactive map: modules by color, drag nodes, zoom reveals names, click a node to read the source.</p></a>
  <a class="card" href="wiki/index.html"><h2>&#128214; Wiki</h2><p>One page per file with callers and agent-written docs, vault-style sidebar.</p></a>
  <a class="card" href="report.html"><h2>&#128200; Report</h2><p>God nodes, cycles, confidence breakdown, WHY/HACK notes found in code.</p></a>
  <div class="divider">raw exports</div>
  <div class="raw">
    <a href="graph.json">graph.json</a>
    <a href="graph.graphml">graph.graphml</a>
    <a href="cypher.txt">cypher.txt</a>
  </div>
  <p class="foot">generated by aag &middot; fully offline &middot; nothing leaves your machine</p>
</div>
</body>
</html>
"##,
        files = distinct_files,
        symbols = nodes.len() - distinct_files - docs,
        docs = docs,
        edges = edges.len(),
    );
    write_file(&aag_dir.join("index.html"), &html)
}

fn write_file(path: &Path, contents: &str) -> Result<()> {
    fs::write(path, contents).map_err(|source| Error::Write {
        path: path.to_path_buf(),
        source,
    })
}

fn graph_data_json(nodes: &[Node], edges: &[Edge]) -> Value {
    json!({
        "nodes": nodes.iter().map(|n| json!({
            "id": n.id,
            "kind": n.kind.as_str(),
            "name": n.name,
            "file": n.file_path,
            "startLine": n.start_line,
            "endLine": n.end_line,
        })).collect::<Vec<_>>(),
        "edges": edges.iter().map(|e| json!({
            "source": e.src,
            "target": e.dst,
            "kind": e.kind.as_str(),
            "confidence": e.confidence.as_str(),
        })).collect::<Vec<_>>(),
    })
}

fn write_json(aag_dir: &Path, nodes: &[Node], edges: &[Edge]) -> Result<()> {
    let data = graph_data_json(nodes, edges);
    let pretty = serde_json::to_string_pretty(&data).unwrap_or_default();
    write_file(&aag_dir.join("graph.json"), &pretty)
}

fn write_html(root: &Path, aag_dir: &Path, nodes: &[Node], edges: &[Edge]) -> Result<()> {
    let data = graph_data_json(nodes, edges);
    let files = source_map_json(root, nodes);
    let name = root
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "repository".to_string());
    let html = GRAPH_HTML_TEMPLATE
        .replace("/*__D3_JS__*/", D3_JS)
        .replace("/*__GRAPHOLOGY_JS__*/", GRAPHOLOGY_JS)
        .replace("/*__SIGMA_JS__*/", SIGMA_JS)
        .replace(
            "/*__GRAPH_DATA__*/",
            &json_safe_for_script(&data.to_string()),
        )
        .replace(
            "/*__FILES_DATA__*/",
            &json_safe_for_script(&files.to_string()),
        )
        .replace("__REPO_NAME__", &json_safe_for_script(&name));
    write_file(&aag_dir.join("graph.html"), &html)
}

/// Escapes every literal `<` in `text` to its Unicode escape sequence
/// before splicing it into an inline `<script>` block.
///
/// Without this, any indexed file whose source contains a literal
/// `</script>` — trivially true for HTML/JS/template files, and, once
/// `aag` indexes its own repo, for `export.rs`/`graph.html.template`
/// themselves — prematurely closes the surrounding script element: the
/// HTML tokenizer's script-data state matches on raw bytes looking for
/// that tag, not on JS/JSON string boundaries, so correct JSON quoting
/// inside the string does nothing to protect it. The Unicode escape is
/// valid inside any JSON string or JS string literal and decodes back to
/// `<` at parse time, so the data is unchanged once parsed — only the
/// raw HTML byte stream is protected.
fn json_safe_for_script(text: &str) -> String {
    text.replace('<', "\\u003c")
}

/// Every distinct file's full text, keyed by path — embedded into
/// `graph.html` so clicking a node can show the whole file, not just its
/// snippet, with no server to fetch it from. Files that fail to read as
/// UTF-8 (binary docs) are simply omitted; the panel falls back to the
/// node's own snippet for those.
fn source_map_json(root: &Path, nodes: &[Node]) -> Value {
    let mut files: HashMap<&str, String> = HashMap::new();
    for node in nodes {
        let path = node.file_path.as_str();
        if files.contains_key(path) {
            continue;
        }
        if let Ok(content) = fs::read_to_string(root.join(path)) {
            files.insert(path, content);
        }
    }
    json!(files)
}

fn report_markdown(root: &Path, nodes: &[Node], edges: &[Edge]) -> String {
    let by_id: HashMap<i64, &Node> = nodes
        .iter()
        .filter_map(|n| n.id.map(|id| (id, n)))
        .collect();
    let distinct_files: HashSet<&str> = nodes
        .iter()
        .filter(|n| n.kind == NodeKind::File)
        .map(|n| n.file_path.as_str())
        .collect();

    let mut out = String::new();
    let _ = writeln!(out, "# Graph Report\n");
    let _ = writeln!(
        out,
        "- files indexed: {}\n- symbols: {}\n- edges: {}\n",
        distinct_files.len(),
        nodes.len(),
        edges.len()
    );

    let mut confidence_counts: HashMap<&str, u32> = HashMap::new();
    for edge in edges {
        *confidence_counts
            .entry(edge.confidence.as_str())
            .or_default() += 1;
    }
    let _ = writeln!(out, "## Confidence\n");
    for label in ["EXTRACTED", "INFERRED", "AMBIGUOUS"] {
        let _ = writeln!(
            out,
            "- {label}: {}",
            confidence_counts.get(label).copied().unwrap_or(0)
        );
    }
    out.push('\n');

    if distinct_files.len() < MIN_FILES_FOR_REPORT_SIGNAL {
        let _ = writeln!(
            out,
            "## God nodes\n\n_Skipped: fewer than {MIN_FILES_FOR_REPORT_SIGNAL} files indexed — not \
             enough signal yet for a meaningful connectivity ranking._\n"
        );
    } else {
        let mut degree: HashMap<i64, u32> = HashMap::new();
        for edge in edges {
            *degree.entry(edge.src).or_default() += 1;
            *degree.entry(edge.dst).or_default() += 1;
        }
        let mut ranked: Vec<(i64, u32)> = degree.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

        let _ = writeln!(out, "## God nodes (most connected)\n");
        for &(id, count) in ranked.iter().take(10) {
            if let Some(node) = by_id.get(&id) {
                let _ = writeln!(
                    out,
                    "- `{}` ({}:{}) — {count} edge(s)",
                    node.name, node.file_path, node.start_line
                );
            }
        }
        out.push('\n');

        let _ = writeln!(out, "## Suggested questions\n");
        for &(id, _) in ranked.iter().take(3) {
            if let Some(node) = by_id.get(&id) {
                let _ = writeln!(
                    out,
                    "- What breaks if `{}` changes? (`aag impact {}`)",
                    node.name, node.name
                );
                let _ = writeln!(
                    out,
                    "- Who calls or imports `{}`? (`aag explore {}`)",
                    node.name, node.name
                );
            }
        }
        out.push('\n');
    }

    let why_hack = collect_why_hack(root, &distinct_files);
    let _ = writeln!(out, "## WHY / HACK notes\n");
    if why_hack.is_empty() {
        let _ = writeln!(out, "_None found._\n");
    } else {
        for (file, line, text) in &why_hack {
            let _ = writeln!(out, "- `{file}:{line}` — {text}");
        }
    }

    out
}

fn collect_why_hack(root: &Path, files: &HashSet<&str>) -> Vec<(String, u32, String)> {
    let mut out = Vec::new();
    for &file in files {
        let Ok(source) = fs::read_to_string(root.join(file)) else {
            continue;
        };
        for (i, line) in source.lines().enumerate() {
            if line.contains("WHY:") || line.contains("HACK:") {
                out.push((
                    file.to_string(),
                    u32::try_from(i).unwrap_or(u32::MAX) + 1,
                    line.trim().to_string(),
                ));
            }
        }
    }
    out.sort();
    out
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn write_graphml(aag_dir: &Path, nodes: &[Node], edges: &[Edge]) -> Result<()> {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<graphml xmlns=\"http://graphml.graphdrawing.org/xmlns\">\n");
    out.push_str("  <key id=\"name\" for=\"node\" attr.name=\"name\" attr.type=\"string\"/>\n");
    out.push_str("  <key id=\"kind\" for=\"node\" attr.name=\"kind\" attr.type=\"string\"/>\n");
    out.push_str("  <key id=\"file\" for=\"node\" attr.name=\"file\" attr.type=\"string\"/>\n");
    out.push_str("  <key id=\"ekind\" for=\"edge\" attr.name=\"kind\" attr.type=\"string\"/>\n");
    out.push_str(
        "  <key id=\"econfidence\" for=\"edge\" attr.name=\"confidence\" attr.type=\"string\"/>\n",
    );
    out.push_str("  <graph id=\"aag\" edgedefault=\"directed\">\n");

    for node in nodes {
        let Some(id) = node.id else { continue };
        let _ = writeln!(out, "    <node id=\"n{id}\">");
        let _ = writeln!(
            out,
            "      <data key=\"name\">{}</data>",
            xml_escape(&node.name)
        );
        let _ = writeln!(
            out,
            "      <data key=\"kind\">{}</data>",
            node.kind.as_str()
        );
        let _ = writeln!(
            out,
            "      <data key=\"file\">{}</data>",
            xml_escape(&node.file_path)
        );
        out.push_str("    </node>\n");
    }
    for (i, edge) in edges.iter().enumerate() {
        let _ = writeln!(
            out,
            "    <edge id=\"e{i}\" source=\"n{}\" target=\"n{}\">",
            edge.src, edge.dst
        );
        let _ = writeln!(
            out,
            "      <data key=\"ekind\">{}</data>",
            edge.kind.as_str()
        );
        let _ = writeln!(
            out,
            "      <data key=\"econfidence\">{}</data>",
            edge.confidence.as_str()
        );
        out.push_str("    </edge>\n");
    }
    out.push_str("  </graph>\n</graphml>\n");
    write_file(&aag_dir.join("graph.graphml"), &out)
}

fn cypher_string(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Generates a Cypher import script — never pushed live in v1 (see
/// `SPEC.md`'s "fica de fora do v1" section), just written to `cypher.txt`
/// for whoever wants to load it into Neo4j/FalkorDB by hand.
fn write_cypher(aag_dir: &Path, nodes: &[Node], edges: &[Edge]) -> Result<()> {
    let mut out = String::new();
    out.push_str("// Generated by `aag`. Import via cypher-shell or similar; never pushed live by aag itself.\n\n");

    for node in nodes {
        let Some(id) = node.id else { continue };
        let _ = writeln!(
            out,
            "CREATE (:{} {{nodeKey: {id}, name: {}, file: {}, startLine: {}}});",
            capitalize(node.kind.as_str()),
            cypher_string(&node.name),
            cypher_string(&node.file_path),
            node.start_line
        );
    }
    out.push('\n');
    for edge in edges {
        let _ = writeln!(
            out,
            "MATCH (a {{nodeKey: {}}}), (b {{nodeKey: {}}}) CREATE (a)-[:{} {{confidence: {}}}]->(b);",
            edge.src,
            edge.dst,
            edge.kind.as_str().to_uppercase(),
            cypher_string(edge.confidence.as_str())
        );
    }
    write_file(&aag_dir.join("cypher.txt"), &out)
}

fn slugify(path: &str) -> String {
    path.replace(['/', '\\', '.'], "_")
}

/// One wiki page's markdown, keyed by the file it covers. Shared source of
/// truth for both [`write_wiki`] (raw `.md`, for Obsidian) and
/// [`write_wiki_html`] (rendered `.html`, for the default `.aag/` site) —
/// same content, different container.
struct WikiPage {
    slug: String,
    file_path: String,
    markdown: String,
}

/// A doc file's contribution to the wiki, split out of its raw text.
/// `target` comes from `wiki:` frontmatter and names the source file whose
/// page the body should be merged into; without it the doc gets its own page.
struct DocEntry {
    source: String,
    target: Option<String>,
    title: Option<String>,
    body: String,
}

/// Parses the optional `---` frontmatter block agent-authored docs carry
/// (SPEC section 5 / the `aag-wiki` skill): `wiki: <file>` routes the body
/// into that source file's wiki page, `title:` names standalone pages.
/// Unknown keys are ignored; text without a leading `---` line is all body.
fn parse_doc_frontmatter(text: &str) -> (Option<String>, Option<String>, &str) {
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return (None, None, text);
    };
    let Some(end) = rest.find("\n---").map(|i| {
        let after = &rest[i + 4..];
        (i, after.strip_prefix('\r').unwrap_or(after))
    }) else {
        return (None, None, text);
    };
    let (block_end, body) = end;
    let body = body.strip_prefix('\n').unwrap_or(body);
    let mut wiki = None;
    let mut title = None;
    for line in rest[..block_end].lines() {
        if let Some(v) = line.strip_prefix("wiki:") {
            wiki = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("title:") {
            title = Some(v.trim().to_string());
        }
    }
    (wiki, title, body)
}

fn build_wiki_pages(graph: &Graph) -> Result<(String, Vec<WikiPage>)> {
    let nodes = graph.all_nodes()?;
    let mut by_file: HashMap<&str, Vec<&Node>> = HashMap::new();
    let mut docs: Vec<DocEntry> = Vec::new();
    for node in &nodes {
        if node.kind == NodeKind::Doc {
            let text = node.description.as_deref().unwrap_or("");
            let (target, title, body) = parse_doc_frontmatter(text);
            docs.push(DocEntry {
                source: node.file_path.clone(),
                target,
                title,
                body: body.to_string(),
            });
        } else if node.kind != NodeKind::File {
            by_file
                .entry(node.file_path.as_str())
                .or_default()
                .push(node);
        }
    }

    // Docs whose `wiki:` target is an indexed source file merge into that
    // file's page and lose their standalone page; everything else (no
    // frontmatter, or a target that matches nothing) stands alone.
    let mut merged: HashMap<&str, Vec<&DocEntry>> = HashMap::new();
    let mut standalone: Vec<&DocEntry> = Vec::new();
    for doc in &docs {
        match doc.target.as_deref() {
            Some(target) if by_file.contains_key(target) => {
                merged.entry(target).or_default().push(doc);
            }
            _ => standalone.push(doc),
        }
    }

    let mut files: Vec<&str> = by_file.keys().copied().collect();
    files.sort_unstable();

    let mut pages = Vec::new();
    for &file in &files {
        let mut page = format!("# {file}\n\n");
        for doc in merged.get(file).into_iter().flatten() {
            let _ = writeln!(page, "_documented in {}_\n", doc.source);
            page.push_str(doc.body.trim());
            page.push_str("\n\n");
        }
        for node in &by_file[file] {
            let _ = writeln!(page, "## {} `{}`\n", node.kind.as_str(), node.name);
            let Some(id) = node.id else { continue };
            let callers = graph.callers(id)?;
            if callers.is_empty() {
                continue;
            }
            let _ = writeln!(page, "Called by:\n");
            for (caller, kind, confidence) in callers {
                let _ = writeln!(
                    page,
                    "- [{}]({}.md) [{} {}]",
                    caller.name,
                    slugify(&caller.file_path),
                    kind.as_str(),
                    confidence.as_str()
                );
            }
            page.push('\n');
        }
        pages.push(WikiPage {
            slug: slugify(file),
            file_path: file.to_string(),
            markdown: page,
        });
    }

    for doc in standalone {
        let heading = doc.title.as_deref().unwrap_or(doc.source.as_str());
        let mut page = format!("# {heading}\n\n");
        if doc.body.trim().is_empty() {
            page.push_str("_no description yet — run `aag describe` for binary docs_\n");
        } else {
            page.push_str(doc.body.trim());
            page.push('\n');
        }
        pages.push(WikiPage {
            slug: slugify(&doc.source),
            file_path: doc.source.clone(),
            markdown: page,
        });
    }

    pages.sort_by(|a, b| a.file_path.cmp(&b.file_path));

    let mut index = String::from("# Wiki\n\n");
    for page in &pages {
        let _ = writeln!(index, "- [{}]({}.md)", page.file_path, page.slug);
    }

    Ok((index, pages))
}

/// Writes one markdown page per file (grouping proxy for "community" — see
/// module docs) plus an `index.md`, into `out_dir`. Used for the Obsidian
/// export, which needs real markdown files, not HTML — see [`write_wiki_html`]
/// for the rendered version the default `.aag/` site uses.
///
/// # Errors
///
/// Returns an error if the graph can't be read or a page can't be written.
pub fn write_wiki(out_dir: &Path, graph: &Graph) -> Result<()> {
    fs::create_dir_all(out_dir).map_err(|source| Error::Write {
        path: out_dir.to_path_buf(),
        source,
    })?;
    let (index, pages) = build_wiki_pages(graph)?;
    for page in &pages {
        write_file(&out_dir.join(format!("{}.md", page.slug)), &page.markdown)?;
    }
    write_file(&out_dir.join("index.md"), &index)
}

/// Same pages as [`write_wiki`], rendered to `.html` and wrapped in an
/// Obsidian-style shell — persistent file-tree sidebar, backlink styling,
/// and a "view in graph" deep link per page (`graph.html?focus=<file>`) —
/// so `.aag/wiki/` reads like a vault, not a pile of bare pages.
///
/// # Errors
///
/// Returns an error if the graph can't be read or a page can't be written.
pub(crate) fn write_wiki_html(out_dir: &Path, graph: &Graph) -> Result<()> {
    fs::create_dir_all(out_dir).map_err(|source| Error::Write {
        path: out_dir.to_path_buf(),
        source,
    })?;
    let (index, pages) = build_wiki_pages(graph)?;
    for page in &pages {
        let sidebar = wiki_sidebar(&pages, Some(&page.slug));
        let graph_link = format!(
            r#"<p class="graph-link"><a href="../graph.html?focus={}">&#11041; view in graph</a></p>"#,
            query_encode(&page.file_path)
        );
        let body = format!("{graph_link}{}", render_markdown(&page.markdown));
        write_file(
            &out_dir.join(format!("{}.html", page.slug)),
            &wiki_shell(&page.file_path, &sidebar, &body),
        )?;
    }
    let sidebar = wiki_sidebar(&pages, None);
    write_file(
        &out_dir.join("index.html"),
        &wiki_shell("Wiki", &sidebar, &render_markdown(&index)),
    )
}

/// Percent-encodes just the characters that would break a query-string
/// value in the `?focus=` deep link — file paths are otherwise URL-safe.
fn query_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '%' => out.push_str("%25"),
            '&' => out.push_str("%26"),
            '#' => out.push_str("%23"),
            '?' => out.push_str("%3F"),
            '+' => out.push_str("%2B"),
            ' ' => out.push_str("%20"),
            _ => out.push(c),
        }
    }
    out
}

/// The wiki's file-tree sidebar: pages grouped by directory (Obsidian-style
/// vault explorer), with the current page highlighted. Pages live flat in
/// `wiki/` (slug filenames), so grouping is presentational only.
fn wiki_sidebar(pages: &[WikiPage], current: Option<&str>) -> String {
    let mut out = String::from(r#"<a class="root" href="index.html">Wiki</a>"#);
    let mut last_dir: Option<&str> = None;
    for page in pages {
        let (dir, name) = match page.file_path.rfind('/') {
            Some(i) => (&page.file_path[..=i], &page.file_path[i + 1..]),
            None => ("./", page.file_path.as_str()),
        };
        if last_dir != Some(dir) {
            let _ = write!(out, r#"<div class="dir">{}</div>"#, xml_escape(dir));
            last_dir = Some(dir);
        }
        let active = if current == Some(page.slug.as_str()) {
            " active"
        } else {
            ""
        };
        let _ = write!(
            out,
            r#"<a class="f{active}" href="{}.html">{}</a>"#,
            page.slug,
            xml_escape(name)
        );
    }
    out
}

/// The wiki page frame: same dark palette as the rest of the `.aag/` site,
/// but with a persistent left sidebar and violet internal links — the
/// Obsidian look, per the user-facing goal of the wiki being a vault you
/// browse, not a directory listing.
fn wiki_shell(title: &str, sidebar: &str, body: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>aag wiki — {title}</title>
<style>
  :root {{ --surface: #0a0a0a; --surface-2: #121212; --border: #262624; --text-primary: #f2f2ee; --text-secondary: #8a8a84; --accent: #ffc600; --wikilink: #ffc600; --mono: "JetBrains Mono", ui-monospace, "SF Mono", Menlo, Consolas, monospace; }}
  html, body {{ margin: 0; background: var(--surface); color: var(--text-primary); font-family: var(--mono); }}
  body {{ display: flex; min-height: 100vh; }}
  nav {{ position: fixed; top: 0; left: 0; right: 0; height: 41px; box-sizing: border-box; padding: 10px 20px; background: var(--surface-2); border-bottom: 1px solid var(--border); border-top: 2px solid transparent; border-image: linear-gradient(90deg, #ff3b3b, #ffcf3b, #3bff6e, #3bd8ff, #7a3bff) 1; border-image-width: 2px 0 0 0; display: flex; gap: 16px; font-size: 13px; z-index: 5; }}
  nav a {{ color: var(--text-secondary); text-decoration: none; }}
  nav a:hover {{ color: var(--text-primary); }}
  aside {{ position: fixed; top: 41px; left: 0; bottom: 0; width: 250px; box-sizing: border-box; background: var(--surface-2); border-right: 1px solid var(--border); padding: 14px 10px; overflow-y: auto; font-size: 13px; }}
  aside .root {{ display: block; font-weight: 600; color: var(--text-primary); text-decoration: none; padding: 4px 8px; margin-bottom: 8px; }}
  aside .dir {{ color: var(--text-secondary); font-size: 11px; text-transform: none; padding: 8px 8px 2px; opacity: 0.8; }}
  aside a.f {{ display: block; color: var(--text-secondary); text-decoration: none; padding: 3px 8px 3px 18px; border-radius: 4px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }}
  aside a.f:hover {{ background: var(--surface); color: var(--text-primary); }}
  aside a.f.active {{ background: var(--surface); color: var(--wikilink); }}
  main {{ margin: 41px 0 0 250px; padding: 32px 40px 80px; line-height: 1.6; max-width: 820px; box-sizing: border-box; flex: 1; }}
  h1, h2, h3 {{ color: var(--text-primary); }}
  h1 {{ margin-top: 0; }}
  h2 {{ font-size: 16px; border-bottom: 1px solid var(--border); padding-bottom: 4px; }}
  a {{ color: var(--wikilink); }}
  .graph-link a {{ color: var(--accent); text-decoration: none; font-size: 13px; }}
  .graph-link a:hover {{ text-decoration: underline; }}
  code {{ background: var(--surface-2); border: 1px solid var(--border); border-radius: 3px; padding: 1px 5px; font-size: 0.9em; }}
  ul {{ padding-left: 20px; }}
  li {{ margin-bottom: 4px; }}
</style>
</head>
<body>
<nav>
  <a href="../index.html">aag</a>
  <a href="../graph.html">Graph</a>
  <a href="index.html">Wiki</a>
  <a href="../report.html">Report</a>
</nav>
<aside>{sidebar}</aside>
<main>{body}</main>
</body>
</html>
"#
    )
}

/// Writes the same pages as [`write_wiki`] into `<vault_dir>/aag/` — never
/// the vault root — so an existing vault's notes and `.obsidian` config are
/// never touched, per `SPEC.md` section 6.
///
/// # Errors
///
/// Returns an error if the graph can't be read or a page can't be written.
pub fn write_obsidian(vault_dir: &Path, graph: &Graph) -> Result<()> {
    write_wiki(&vault_dir.join("aag"), graph)
}

/// The shared page frame every generated `.html` page (index/report/wiki)
/// renders inside — same dark palette as `graph.html` so the whole `.aag/`
/// output reads as one small site rather than a pile of unrelated files.
/// `base` is `""` for root-level pages and `"../"` for pages one directory
/// down (the wiki), so nav links resolve either way.
fn page_shell(title: &str, base: &str, body: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>aag — {title}</title>
<style>
  :root {{ --surface: #0a0a0a; --surface-2: #121212; --border: #262624; --text-primary: #f2f2ee; --text-secondary: #8a8a84; --accent: #ffc600; --mono: "JetBrains Mono", ui-monospace, "SF Mono", Menlo, Consolas, monospace; }}
  html, body {{ margin: 0; background: var(--surface); color: var(--text-primary); font-family: var(--mono); }}
  body {{ min-height: 100vh; }}
  nav {{ position: sticky; top: 0; padding: 10px 20px; background: var(--surface-2); border-bottom: 1px solid var(--border); border-top: 2px solid transparent; border-image: linear-gradient(90deg, #ff3b3b, #ffcf3b, #3bff6e, #3bd8ff, #7a3bff) 1; border-image-width: 2px 0 0 0; display: flex; gap: 16px; font-size: 13px; }}
  nav a {{ color: var(--text-secondary); text-decoration: none; }}
  nav a:hover {{ color: var(--text-primary); }}
  main {{ max-width: 860px; margin: 0 auto; padding: 32px 24px 80px; line-height: 1.6; }}
  h1, h2, h3 {{ color: var(--text-primary); }}
  h1 {{ margin-top: 0; }}
  p.stats {{ color: var(--text-secondary); }}
  a {{ color: var(--accent); }}
  code {{ background: var(--surface-2); border: 1px solid var(--border); border-radius: 3px; padding: 1px 5px; font-size: 0.9em; }}
  ul {{ padding-left: 20px; }}
  li {{ margin-bottom: 4px; }}
  .cards {{ display: flex; gap: 14px; flex-wrap: wrap; margin: 20px 0; }}
  .card {{ display: block; flex: 1 1 200px; background: var(--surface-2); border: 1px solid var(--border); border-radius: 8px; padding: 16px; text-decoration: none; color: var(--text-primary); }}
  .card:hover {{ border-color: var(--accent); }}
  .card h2 {{ margin: 0 0 6px; font-size: 15px; }}
  .card p {{ margin: 0; color: var(--text-secondary); font-size: 13px; }}
  /* Inside the `aag ui` shell (an iframe), the shell's own bar already
     carries the lib-level brand — hide only that link. The page tabs
     (Graph/Wiki/Report) are workspace-level navigation and stay: they
     navigate within the iframe. */
  html.embedded nav a.home {{ display: none; }}
</style>
<script>if (window.self !== window.top) document.documentElement.classList.add("embedded");</script>
</head>
<body>
<nav>
  <a class="home" href="{base}index.html">aag</a>
  <a href="{base}graph.html">Graph</a>
  <a href="{base}wiki/index.html">Wiki</a>
  <a href="{base}report.html">Report</a>
</nav>
<main>{body}</main>
</body>
</html>
"#
    )
}

/// Converts the markdown subset this module itself generates
/// (`#`/`##`/`###` headers, `- ` list items, `[text](x.md)` links,
/// `` `code` ``, and a whole-line `_italic_` note) plus triple-backtick
/// fenced code blocks (agent-authored wiki docs use them) into HTML. Not a general
/// `CommonMark` parser — deliberately only covers what `export.rs` and the
/// `aag-wiki` authoring format produce, since that's the only input it sees.
fn render_markdown(markdown: &str) -> String {
    let mut html = String::new();
    let mut in_list = false;
    let mut in_fence = false;
    for raw_line in markdown.lines() {
        let line = raw_line.trim_end();
        if line.starts_with("```") {
            close_list(&mut html, &mut in_list);
            html.push_str(if in_fence {
                "</code></pre>\n"
            } else {
                "<pre><code>"
            });
            in_fence = !in_fence;
        } else if in_fence {
            html.push_str(&xml_escape(raw_line));
            html.push('\n');
        } else if let Some(rest) = line.strip_prefix("### ") {
            close_list(&mut html, &mut in_list);
            let _ = writeln!(html, "<h3>{}</h3>", inline(rest));
        } else if let Some(rest) = line.strip_prefix("## ") {
            close_list(&mut html, &mut in_list);
            let _ = writeln!(html, "<h2>{}</h2>", inline(rest));
        } else if let Some(rest) = line.strip_prefix("# ") {
            close_list(&mut html, &mut in_list);
            let _ = writeln!(html, "<h1>{}</h1>", inline(rest));
        } else if let Some(item) = line.strip_prefix("- ") {
            if !in_list {
                html.push_str("<ul>\n");
                in_list = true;
            }
            let _ = writeln!(html, "<li>{}</li>", inline(item));
        } else if line.trim().is_empty() {
            close_list(&mut html, &mut in_list);
        } else {
            close_list(&mut html, &mut in_list);
            let _ = writeln!(html, "<p>{}</p>", inline(line));
        }
    }
    if in_fence {
        html.push_str("</code></pre>\n");
    }
    close_list(&mut html, &mut in_list);
    html
}

fn close_list(html: &mut String, in_list: &mut bool) {
    if *in_list {
        html.push_str("</ul>\n");
        *in_list = false;
    }
}

/// Inline span rendering for one line of [`render_markdown`]'s input.
/// Order matters: escape, then links (needs the raw `[`/`]`/`(`/`)`
/// `xml_escape` leaves untouched), then code — a whole-line `_..._` wrap is
/// checked first and recurses on its stripped inner text, rather than
/// toggling on every underscore, so an underscore inside a link's `.md`
/// target (e.g. `a_rs.md`) is never mistaken for an emphasis marker.
fn inline(raw: &str) -> String {
    if let Some(stripped) = raw.strip_prefix('_').and_then(|s| s.strip_suffix('_')) {
        return format!("<em>{}</em>", inline(stripped));
    }
    replace_code(&replace_links(&xml_escape(raw)))
}

fn replace_links(escaped: &str) -> String {
    let mut out = String::new();
    let mut rest = escaped;
    while let Some(start) = rest.find('[') {
        let Some(mid_offset) = rest[start..].find("](") else {
            out.push_str(rest);
            return out;
        };
        let mid = start + mid_offset;
        let Some(end_offset) = rest[mid..].find(')') else {
            out.push_str(rest);
            return out;
        };
        let end = mid + end_offset;
        out.push_str(&rest[..start]);
        let text = &rest[start + 1..mid];
        let href = &rest[mid + 2..end];
        let href = href
            .strip_suffix(".md")
            .map_or_else(|| href.to_string(), |base| format!("{base}.html"));
        let _ = write!(out, "<a href=\"{href}\">{text}</a>");
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

fn replace_code(s: &str) -> String {
    let mut out = String::new();
    let mut open = false;
    for (i, part) in s.split('`').enumerate() {
        if i > 0 {
            out.push_str(if open { "</code>" } else { "<code>" });
            open = !open;
        }
        out.push_str(part);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{Confidence, EdgeKind, NodeKind};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn scratch_dir() -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("aag-export-test-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn seeded_graph() -> (Graph, Vec<Node>) {
        let graph = Graph::open_in_memory().unwrap();
        let file_a = graph
            .insert_node(&Node {
                id: None,
                kind: NodeKind::File,
                name: "a.rs".into(),
                file_path: "a.rs".into(),
                start_line: 1,
                end_line: 2,
                description: None,
            })
            .unwrap();
        let from_id = graph
            .insert_node(&Node {
                id: None,
                kind: NodeKind::Function,
                name: "caller".into(),
                file_path: "a.rs".into(),
                start_line: 1,
                end_line: 1,
                description: None,
            })
            .unwrap();
        let to_id = graph
            .insert_node(&Node {
                id: None,
                kind: NodeKind::Function,
                name: "callee".into(),
                file_path: "a.rs".into(),
                start_line: 2,
                end_line: 2,
                description: None,
            })
            .unwrap();
        graph
            .insert_edge(&Edge {
                src: from_id,
                dst: to_id,
                kind: EdgeKind::Calls,
                confidence: Confidence::Inferred,
            })
            .unwrap();

        let nodes = vec![
            Node {
                id: Some(file_a),
                kind: NodeKind::File,
                name: "a.rs".into(),
                file_path: "a.rs".into(),
                start_line: 1,
                end_line: 2,
                description: None,
            },
            Node {
                id: Some(from_id),
                kind: NodeKind::Function,
                name: "caller".into(),
                file_path: "a.rs".into(),
                start_line: 1,
                end_line: 1,
                description: None,
            },
            Node {
                id: Some(to_id),
                kind: NodeKind::Function,
                name: "callee".into(),
                file_path: "a.rs".into(),
                start_line: 2,
                end_line: 2,
                description: None,
            },
        ];
        (graph, nodes)
    }

    #[test]
    fn json_safe_for_script_escapes_angle_brackets() {
        let escaped = json_safe_for_script("</script>alert(1)</script>");
        assert!(!escaped.contains('<'), "escaped was: {escaped}");
        assert_eq!(escaped, "\\u003c/script>alert(1)\\u003c/script>");
    }

    /// Regression test for a real bug: `aag` indexing its own repo (or any
    /// repo containing HTML/JS/template source) embeds that file's raw
    /// text into `graph.html`'s inline `<script>` block via `FILES`. A
    /// literal `</script>` inside that embedded text closes the HTML
    /// `<script>` element early — the browser's HTML tokenizer matches on
    /// raw bytes, not JS/JSON string boundaries — corrupting the rest of
    /// the page and breaking the graph with a `SyntaxError` at load time.
    #[test]
    fn embedded_file_content_cannot_break_out_of_script_tag() {
        let root = scratch_dir();
        let aag_dir = root.join(".aag");
        fs::create_dir_all(&aag_dir).unwrap();
        fs::write(
            root.join("a.rs"),
            "// </script><script>alert('escaped')</script>\nfn caller() {}\nfn callee() {}\n",
        )
        .unwrap();
        let (_graph, nodes) = seeded_graph();

        write_html(&root, &aag_dir, &nodes, &[]).unwrap();
        let html = fs::read_to_string(aag_dir.join("graph.html")).unwrap();

        assert!(
            !html.contains("</script><script>alert"),
            "malicious file content must not survive as a literal script boundary"
        );
        assert!(
            html.contains("alert(\\'escaped\\')") || html.contains("alert('escaped')"),
            "file content should still be present, just HTML-tokenizer-safe"
        );
    }

    #[test]
    fn write_default_produces_all_artifacts() {
        let (graph, _) = seeded_graph();
        let root = scratch_dir();
        let aag_dir = root.join(".aag");
        fs::create_dir_all(&aag_dir).unwrap();

        write_default(&root, &aag_dir, &graph).unwrap();

        for name in [
            "graph.json",
            "graph.html",
            "GRAPH_REPORT.md",
            "report.html",
            "index.html",
            "graph.graphml",
            "cypher.txt",
        ] {
            assert!(aag_dir.join(name).is_file(), "{name} should exist");
        }
        assert!(aag_dir.join("wiki").join("index.html").is_file());
        assert!(aag_dir.join("wiki").join("a_rs.html").is_file());
    }

    #[test]
    fn index_html_links_to_the_rest_of_the_site() {
        let (graph, _) = seeded_graph();
        let root = scratch_dir();
        let aag_dir = root.join(".aag");
        fs::create_dir_all(&aag_dir).unwrap();

        write_default(&root, &aag_dir, &graph).unwrap();

        let index = fs::read_to_string(aag_dir.join("index.html")).unwrap();
        assert!(index.contains(r#"href="graph.html""#));
        assert!(index.contains(r#"href="wiki/index.html""#));
        assert!(index.contains(r#"href="report.html""#));
    }

    #[test]
    fn render_markdown_rewrites_md_links_to_html_and_renders_code() {
        let html = render_markdown("- [caller](a_rs.md) [calls INFERRED]\n\n`literal`");
        assert!(html.contains(r#"href="a_rs.html""#));
        assert!(html.contains("<code>literal</code>"));
    }

    #[test]
    fn render_markdown_wraps_whole_line_italic_without_touching_link_underscores() {
        let html = render_markdown("_Skipped: see [x](a_rs.md)._");
        assert!(html.contains("<p><em>"));
        assert!(html.contains(r#"href="a_rs.html""#));
    }

    #[test]
    fn wiki_html_page_renders_caller_link_as_html() {
        let (graph, _) = seeded_graph();
        let out_dir = scratch_dir().join("wiki");

        write_wiki_html(&out_dir, &graph).unwrap();

        let page = fs::read_to_string(out_dir.join("a_rs.html")).unwrap();
        assert!(page.contains(r#"href="a_rs.html">caller</a>"#));
    }

    #[test]
    fn frontmatter_parses_wiki_target_title_and_body() {
        let (wiki, title, body) =
            parse_doc_frontmatter("---\nwiki: src/a.rs\ntitle: My Doc\n---\n\nBody here.\n");
        assert_eq!(wiki.as_deref(), Some("src/a.rs"));
        assert_eq!(title.as_deref(), Some("My Doc"));
        assert_eq!(body.trim(), "Body here.");
    }

    #[test]
    fn frontmatter_absent_is_all_body() {
        let (wiki, title, body) = parse_doc_frontmatter("# Plain doc\n\ntext\n");
        assert_eq!(wiki, None);
        assert_eq!(title, None);
        assert!(body.starts_with("# Plain doc"));
    }

    #[test]
    fn doc_with_wiki_target_merges_into_file_page_and_loses_own_page() {
        let (graph, _) = seeded_graph();
        graph
            .insert_node(&Node {
                id: None,
                kind: NodeKind::Doc,
                name: "docs/a.md".into(),
                file_path: "docs/a.md".into(),
                start_line: 1,
                end_line: 3,
                description: Some(
                    "---\nwiki: a.rs\n---\nAgent-written overview of module a.\n".into(),
                ),
            })
            .unwrap();

        let (index, pages) = build_wiki_pages(&graph).unwrap();

        let file_page = pages.iter().find(|p| p.file_path == "a.rs").unwrap();
        assert!(file_page.markdown.contains("Agent-written overview"));
        assert!(file_page.markdown.contains("_documented in docs/a.md_"));
        assert!(
            !pages.iter().any(|p| p.file_path == "docs/a.md"),
            "merged doc must not keep a standalone page"
        );
        assert!(!index.contains("docs/a.md"));
    }

    #[test]
    fn doc_without_target_gets_standalone_page_with_body() {
        let (graph, _) = seeded_graph();
        graph
            .insert_node(&Node {
                id: None,
                kind: NodeKind::Doc,
                name: "NOTES.md".into(),
                file_path: "NOTES.md".into(),
                start_line: 1,
                end_line: 2,
                description: Some("---\ntitle: Design Notes\n---\nStandalone content.\n".into()),
            })
            .unwrap();

        let (index, pages) = build_wiki_pages(&graph).unwrap();

        let page = pages.iter().find(|p| p.file_path == "NOTES.md").unwrap();
        assert!(page.markdown.contains("# Design Notes"));
        assert!(page.markdown.contains("Standalone content."));
        assert!(index.contains("NOTES.md"));
    }

    #[test]
    fn render_markdown_handles_fenced_code_blocks() {
        let html = render_markdown("Intro\n\n```\nlet x = <y>;\n```\n\nAfter\n");
        assert!(html.contains("<pre><code>let x = &lt;y&gt;;\n</code></pre>"));
        assert!(html.contains("<p>After</p>"));
    }

    #[test]
    fn graph_html_embeds_d3_and_data() {
        let (graph, _) = seeded_graph();
        let root = scratch_dir();
        let aag_dir = root.join(".aag");
        fs::create_dir_all(&aag_dir).unwrap();

        write_default(&root, &aag_dir, &graph).unwrap();

        let html = fs::read_to_string(aag_dir.join("graph.html")).unwrap();
        assert!(html.contains("d3js.org"), "vendored D3 must be embedded");
        assert!(html.contains("\"caller\""), "graph data must be inlined");
        assert!(
            !html.contains("__GRAPH_DATA__"),
            "placeholder must be substituted"
        );
    }

    #[test]
    fn report_skips_god_nodes_below_file_floor() {
        let (graph, _) = seeded_graph();
        let root = scratch_dir();
        let aag_dir = root.join(".aag");
        fs::create_dir_all(&aag_dir).unwrap();

        write_default(&root, &aag_dir, &graph).unwrap();

        let report = fs::read_to_string(aag_dir.join("GRAPH_REPORT.md")).unwrap();
        assert!(
            report.contains("Skipped"),
            "one file indexed must skip god-nodes ranking"
        );
    }

    #[test]
    fn report_extracts_why_and_hack_markers() {
        let (graph, _) = seeded_graph();
        let root = scratch_dir();
        fs::write(
            root.join("a.rs"),
            "fn caller() {}\n// HACK: retry because flaky upstream\nfn callee() {}\n",
        )
        .unwrap();
        let aag_dir = root.join(".aag");
        fs::create_dir_all(&aag_dir).unwrap();

        write_default(&root, &aag_dir, &graph).unwrap();

        let report = fs::read_to_string(aag_dir.join("GRAPH_REPORT.md")).unwrap();
        assert!(report.contains("HACK: retry because flaky upstream"));
    }

    #[test]
    fn cypher_script_matches_edges_by_node_key() {
        let (graph, _) = seeded_graph();
        let root = scratch_dir();
        let aag_dir = root.join(".aag");
        fs::create_dir_all(&aag_dir).unwrap();

        write_default(&root, &aag_dir, &graph).unwrap();

        let cypher = fs::read_to_string(aag_dir.join("cypher.txt")).unwrap();
        assert!(cypher.contains("CREATE (:Function"));
        assert!(cypher.contains("CREATE (a)-[:CALLS"));
    }

    #[test]
    fn wiki_groups_symbols_by_file_and_links_callers() {
        let (graph, _) = seeded_graph();
        let out_dir = scratch_dir().join("wiki");

        write_wiki(&out_dir, &graph).unwrap();

        let index = fs::read_to_string(out_dir.join("index.md")).unwrap();
        assert!(index.contains("a.rs"));
        let page = fs::read_to_string(out_dir.join("a_rs.md")).unwrap();
        assert!(page.contains("callee"));
        assert!(page.contains("caller"));
        assert!(page.contains("[caller](a_rs.md)"));
    }

    #[test]
    fn obsidian_writes_under_aag_subfolder_only() {
        let (graph, _) = seeded_graph();
        let vault = scratch_dir();
        fs::write(vault.join("existing-note.md"), "keep me").unwrap();

        write_obsidian(&vault, &graph).unwrap();

        assert!(vault.join("aag").join("index.md").is_file());
        assert!(
            vault.join("existing-note.md").is_file(),
            "must not touch existing vault notes"
        );
    }
}
