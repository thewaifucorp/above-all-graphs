//! `aag impact` — blast radius of changing a symbol: every caller/importer,
//! transitively, tagged with how confident each hop's resolution is. Per
//! `SPEC.md` section 7 ("impact analysis / blast radius before editing").
//!
//! [`format`] builds the same text `crate::mcp`'s `impact` tool returns, so
//! the CLI and MCP surfaces never drift.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::Path;

use crate::error::{Error, Result};
use crate::storage::Graph;

/// Caps BFS depth as a safety valve on pathological graphs; cycles are
/// already broken by the visited set, this just bounds worst-case work.
const MAX_DEPTH: u32 = 20;

/// Runs `aag impact <symbol>` against the index under `root`, printing the
/// transitive blast radius of changing `symbol`.
///
/// # Errors
///
/// Returns [`Error::IndexMissing`] if `root` has no index yet, or
/// [`Error::SymbolNotFound`] if `symbol` isn't in the graph.
pub fn run(root: &Path, symbol: &str) -> Result<()> {
    println!("{}", format(root, symbol)?);
    Ok(())
}

/// Builds the same output `run` prints, as a string.
///
/// # Errors
///
/// Returns [`Error::IndexMissing`] if `root` has no index yet, or
/// [`Error::SymbolNotFound`] if `symbol` isn't in the graph.
///
/// # Panics
///
/// Never in practice: a [`Node`](crate::storage::Node) read back from
/// storage always has `id: Some(_)`.
pub fn format(root: &Path, symbol: &str) -> Result<String> {
    let graph = Graph::open_existing(root)?;
    let target = graph
        .find_by_name(symbol)?
        .ok_or_else(|| Error::SymbolNotFound {
            name: symbol.to_string(),
        })?;
    let target_id = target
        .id
        .expect("node loaded from storage always has an id");

    let mut visited = HashSet::new();
    visited.insert(target_id);
    let mut frontier = vec![target_id];
    let mut affected = Vec::new();
    let mut depth = 0;

    while !frontier.is_empty() && depth < MAX_DEPTH {
        depth += 1;
        let mut next = Vec::new();
        for node_id in frontier {
            for (caller, kind, confidence) in graph.callers(node_id)? {
                let Some(caller_id) = caller.id else { continue };
                if visited.insert(caller_id) {
                    next.push(caller_id);
                    affected.push((caller, kind, confidence, depth));
                }
            }
        }
        frontier = next;
    }

    if affected.is_empty() {
        return Ok(format!(
            "`{symbol}` ({}:{}) has no known callers/importers in the index",
            target.file_path, target.start_line
        ));
    }

    let mut out = format!(
        "blast radius of `{symbol}` ({}:{}):\n",
        target.file_path, target.start_line
    );
    for (node, kind, confidence, hop) in &affected {
        let _ = writeln!(
            out,
            "  depth {hop}: {} ({}:{}) via {} [{}]",
            node.name,
            node.file_path,
            node.start_line,
            kind.as_str(),
            confidence.as_str()
        );
    }

    let files: HashSet<&str> = affected
        .iter()
        .map(|(n, ..)| n.file_path.as_str())
        .collect();
    let _ = writeln!(
        out,
        "\n{} symbol(s) across {} file(s) affected",
        affected.len(),
        files.len()
    );
    Ok(out)
}
