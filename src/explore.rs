//! `aag explore` — answer "how does X work" by returning matching symbols'
//! source verbatim, grouped by file, plus their direct callers. Per
//! `SPEC.md` section 4, this is the one tool an agent should reach for by
//! default instead of picking between many granular tools.
//!
//! [`format`] and [`format_node`] build the same text `crate::mcp`'s
//! `explore`/`node` tools return, so the CLI and MCP surfaces never drift.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use crate::error::{Error, Result};
use crate::storage::{Graph, Node};

/// Runs `aag explore <query>` against the index under `root`, printing
/// matches grouped by file with source verbatim and direct callers.
///
/// # Errors
///
/// Returns [`Error::IndexMissing`] if `root` has no index yet.
pub fn run(root: &Path, query: &str) -> Result<()> {
    println!("{}", format(root, query)?);
    Ok(())
}

/// Builds the same output `run` prints, as a string.
///
/// # Errors
///
/// Returns [`Error::IndexMissing`] if `root` has no index yet.
pub fn format(root: &Path, query: &str) -> Result<String> {
    let graph = Graph::open_existing(root)?;
    let matches = search(&graph, query)?;

    if matches.is_empty() {
        return Ok(format!("no matches for `{query}`"));
    }

    let mut out = String::new();
    for node in &matches {
        out.push_str(&render_match(root, &graph, node));
    }
    let _ = writeln!(out, "\n{} match(es) for `{query}`", matches.len());
    Ok(out)
}

/// Source verbatim plus direct callers for exactly one symbol — errors
/// instead of falling back to a prefix search when there's no exact match.
///
/// # Errors
///
/// Returns [`Error::IndexMissing`] if `root` has no index yet, or
/// [`Error::SymbolNotFound`] if `name` isn't in the graph.
pub fn format_node(root: &Path, name: &str) -> Result<String> {
    let graph = Graph::open_existing(root)?;
    let node = graph
        .find_by_name(name)?
        .ok_or_else(|| Error::SymbolNotFound {
            name: name.to_string(),
        })?;
    Ok(render_match(root, &graph, &node))
}

/// Exact name match first (an agent asking about a known symbol shouldn't
/// get buried under unrelated FTS noise); falls back to a prefix search.
fn search(graph: &Graph, query: &str) -> Result<Vec<Node>> {
    if let Some(exact) = graph.find_by_name(query)? {
        return Ok(vec![exact]);
    }
    let tokens = query
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .map(str::to_ascii_lowercase)
        .filter(|token| token.len() > 2 && !STOP_WORDS.contains(&token.as_str()))
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return graph.search(&fts_prefix_query(query), 20);
    }
    let mut ranked: HashMap<i64, (Node, u32)> = HashMap::new();
    for token in &tokens {
        for node in graph.search(&fts_prefix_query(token), 40)? {
            let Some(id) = node.id else { continue };
            let name = node.name.to_ascii_lowercase();
            let score = if name == *token {
                20
            } else if name.contains(token) {
                10
            } else {
                3
            };
            ranked
                .entry(id)
                .and_modify(|(_, total)| *total += score)
                .or_insert((node, score));
        }
    }
    // Reciprocal-rank fusion: semantic results can introduce candidates that
    // lexical search misses, while exact/prefix matches retain more weight.
    for (rank, (node, _similarity)) in crate::semantic::search(graph, query, 40)?
        .into_iter()
        .enumerate()
    {
        let Some(id) = node.id else { continue };
        let score = u32::try_from(1200 / (60 + rank)).unwrap_or(0);
        ranked
            .entry(id)
            .and_modify(|(_, total)| *total += score)
            .or_insert((node, score));
    }
    let mut degree = HashMap::<i64, u32>::new();
    for edge in graph.all_edges()? {
        *degree.entry(edge.src).or_default() += 1;
        *degree.entry(edge.dst).or_default() += 1;
    }
    let mut rows = ranked.into_values().collect::<Vec<_>>();
    rows.sort_by(|(left_node, left_score), (right_node, right_score)| {
        let left = *left_score
            + degree
                .get(&left_node.id.unwrap_or_default())
                .copied()
                .unwrap_or(0)
                .min(10);
        let right = *right_score
            + degree
                .get(&right_node.id.unwrap_or_default())
                .copied()
                .unwrap_or(0)
                .min(10);
        right
            .cmp(&left)
            .then_with(|| left_node.name.cmp(&right_node.name))
    });
    Ok(rows.into_iter().take(20).map(|(node, _)| node).collect())
}

const STOP_WORDS: &[&str] = &[
    "how", "what", "where", "which", "does", "work", "from", "this", "that", "with", "into",
    "code", "flow",
];

/// Wraps `query` as a quoted FTS5 phrase with a trailing prefix `*`, so
/// arbitrary user input (dots, quotes, `AND`/`NOT`, column filters, ...)
/// can never be parsed as FTS5 query syntax and trip a syntax error.
fn fts_prefix_query(query: &str) -> String {
    format!("\"{}\"*", query.replace('"', "\"\""))
}

fn render_match(root: &Path, graph: &Graph, node: &Node) -> String {
    let mut out = format!(
        "\n## {}:{}-{} ({} {})\n",
        node.file_path,
        node.start_line,
        node.end_line,
        node.kind.as_str(),
        node.name
    );

    match source_snippet(root, node) {
        Ok(snippet) => {
            out.push_str(&snippet);
            out.push('\n');
        }
        Err(error) => {
            let _ = writeln!(out, "(could not read source: {error})");
        }
    }

    if let Some(id) = node.id {
        match graph.callers(id) {
            Ok(callers) if !callers.is_empty() => {
                out.push_str("called by:\n");
                for (caller, kind, confidence) in callers {
                    let _ = writeln!(
                        out,
                        "  - {} ({}:{}) [{} {}]",
                        caller.name,
                        caller.file_path,
                        caller.start_line,
                        kind.as_str(),
                        confidence.as_str()
                    );
                }
            }
            Ok(_) => {}
            Err(error) => {
                let _ = writeln!(out, "(could not load callers: {error})");
            }
        }

        match graph.callees(id) {
            Ok(callees) if !callees.is_empty() => {
                out.push_str("calls / references:\n");
                for (callee, kind, confidence) in callees {
                    let _ = writeln!(
                        out,
                        "  - {} ({}:{}) [{} {}]",
                        callee.name,
                        callee.file_path,
                        callee.start_line,
                        kind.as_str(),
                        confidence.as_str()
                    );
                }
            }
            Ok(_) => {}
            Err(error) => {
                let _ = writeln!(out, "(could not load callees: {error})");
            }
        }

        if let (Ok(nodes), Ok(edges)) = (graph.all_nodes(), graph.all_edges()) {
            let by_id: HashMap<i64, &Node> = nodes
                .iter()
                .filter_map(|item| item.id.map(|row| (row, item)))
                .collect();
            let flows = crate::analysis::processes(&nodes, &edges)
                .into_iter()
                .filter(|process| process.steps.contains(&id))
                .take(3)
                .collect::<Vec<_>>();
            if !flows.is_empty() {
                out.push_str("participates in processes:\n");
                for process in flows {
                    let names = process
                        .steps
                        .iter()
                        .filter_map(|row| by_id.get(row).map(|item| item.name.as_str()))
                        .take(12)
                        .collect::<Vec<_>>();
                    let _ = writeln!(out, "  - {}", names.join(" -> "));
                }
            }
        }
    }

    out
}

fn source_snippet(root: &Path, node: &Node) -> std::io::Result<String> {
    let source = fs::read_to_string(root.join(&node.file_path))?;
    let start = node.start_line.saturating_sub(1) as usize;
    let end = node.end_line as usize;
    let snippet: Vec<&str> = source
        .lines()
        .enumerate()
        .filter(|(i, _)| *i >= start && *i < end)
        .map(|(_, line)| line)
        .collect();
    Ok(snippet.join("\n"))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::format;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn indexed_root() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("aag-explore-test-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("bigbang.rs"), "fn helper() {}").unwrap();
        crate::bigbang::run(&dir, &crate::bigbang::Options::default()).unwrap();
        dir
    }

    #[test]
    fn dotted_query_does_not_break_fts_syntax() {
        let root = indexed_root();
        let out = format(&root, "bigbang.rs").unwrap();
        assert!(out.contains("fn helper"));
    }

    #[test]
    fn multi_word_query_with_dot_does_not_break_fts_syntax() {
        let root = indexed_root();
        let out = format(&root, "what does bigbang.rs do").unwrap();
        assert!(out.contains("no matches for") || out.contains("fn helper"));
    }
}
