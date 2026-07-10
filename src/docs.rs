//! `aag describe` — the host-agent write-back for multimodal docs.
//!
//! `aag` has no vision model of its own and doesn't ask for an API key by
//! default — per `SPEC.md` section 5, that's an explicit fallback for
//! standalone use, not the default path (and not implemented here yet).
//! Instead, the *calling* agent — already looking at the file because it
//! just read or viewed it — submits what it saw through this function.
//! That's the "custo zero" trick: the vision pass runs on the host's
//! model, never on a budget `aag` itself has to pay for.
//!
//! [`format`] builds the same text `crate::mcp`'s `describe_doc` tool
//! returns, so the CLI and MCP surfaces never drift.

use std::collections::HashMap;
use std::path::Path;

use crate::error::{Error, Result};
use crate::resolve::mentioned_names;
use crate::storage::{Confidence, Edge, EdgeKind, Graph, NodeKind};

/// Runs `aag describe <doc> <description>` against the index under `root`,
/// printing what it linked.
///
/// # Errors
///
/// See [`format`].
///
/// # Panics
///
/// See [`format`].
pub fn run(root: &Path, doc_path: &str, description: &str) -> Result<()> {
    println!("{}", format(root, doc_path, description)?);
    Ok(())
}

/// Records `description` for the `Doc` node at `doc_path` (relative to
/// `root`, matching how `crate::resolve` names doc nodes — their `name` is
/// their file path), then links it to any currently-known symbol its text
/// mentions by name. Builds the same message `run` prints, as a string.
///
/// # Errors
///
/// Returns [`Error::IndexMissing`] if `root` has no index yet,
/// [`Error::SymbolNotFound`] if `doc_path` isn't in the graph,
/// [`Error::NotADoc`] if it exists but isn't a doc node, or a storage
/// error if the write fails.
///
/// # Panics
///
/// Never in practice: a [`Node`](crate::storage::Node) read back from
/// storage always has `id: Some(_)`.
pub fn format(root: &Path, doc_path: &str, description: &str) -> Result<String> {
    let graph = Graph::open_existing(root)?;
    let doc = graph
        .find_by_name(doc_path)?
        .ok_or_else(|| Error::SymbolNotFound {
            name: doc_path.to_string(),
        })?;
    if doc.kind != NodeKind::Doc {
        return Err(Error::NotADoc {
            name: doc_path.to_string(),
        });
    }
    let doc_id = doc.id.expect("node loaded from storage always has an id");

    graph.set_description(doc_id, description)?;

    let by_name = name_index(&graph)?;
    let mut linked = 0u32;
    for name in mentioned_names(description, &by_name) {
        let Some(candidates) = by_name.get(&name) else {
            continue;
        };
        let confidence = if candidates.len() == 1 {
            Confidence::Inferred
        } else {
            Confidence::Ambiguous
        };
        for &(dst, _) in candidates {
            if dst == doc_id {
                continue;
            }
            graph.insert_edge(&Edge {
                src: doc_id,
                dst,
                kind: EdgeKind::Explains,
                confidence,
            })?;
            linked += 1;
        }
    }

    Ok(format!(
        "described `{doc_path}` — linked to {linked} mentioned symbol(s)"
    ))
}

fn name_index(graph: &Graph) -> Result<HashMap<String, Vec<(i64, String)>>> {
    let mut by_name: HashMap<String, Vec<(i64, String)>> = HashMap::new();
    for node in graph.all_nodes()? {
        if let Some(id) = node.id {
            by_name
                .entry(node.name)
                .or_default()
                .push((id, node.file_path));
        }
    }
    Ok(by_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn indexed_root() -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("aag-docs-test-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("widget.rs"), "struct Widget;").unwrap();
        fs::write(dir.join("diagram.png"), [0x89, 0x50, 0x4e, 0x47]).unwrap();
        crate::bigbang::run(&dir, &crate::bigbang::Options::default()).unwrap();
        dir
    }

    #[test]
    fn binary_doc_is_indexed_unprocessed() {
        let root = indexed_root();
        let graph = Graph::open_existing(&root).unwrap();

        let doc = graph.find_by_name("diagram.png").unwrap().unwrap();

        assert_eq!(doc.kind, NodeKind::Doc);
        assert_eq!(doc.description, None);
    }

    #[test]
    fn describe_sets_description_and_links_mentioned_symbol() {
        let root = indexed_root();

        let message = format(
            &root,
            "diagram.png",
            "This diagram shows how Widget is constructed.",
        )
        .unwrap();

        assert!(message.contains("linked to 1 mentioned symbol"));
        let graph = Graph::open_existing(&root).unwrap();
        let doc = graph.find_by_name("diagram.png").unwrap().unwrap();
        assert_eq!(
            doc.description.as_deref(),
            Some("This diagram shows how Widget is constructed.")
        );
        let widget = graph.find_by_name("Widget").unwrap().unwrap();
        let explainers = graph.callers(widget.id.unwrap()).unwrap();
        assert!(
            explainers
                .iter()
                .any(|(node, kind, _)| node.name == "diagram.png" && *kind == EdgeKind::Explains)
        );
    }

    #[test]
    fn describe_non_doc_node_errors() {
        let root = indexed_root();

        let result = format(&root, "Widget", "not a doc");

        assert!(matches!(result, Err(Error::NotADoc { .. })));
    }

    #[test]
    fn text_doc_is_indexed_and_linked_immediately_without_describe() {
        let root = indexed_root();
        fs::write(
            root.join("README.md"),
            "The Widget type is the main entry point.",
        )
        .unwrap();
        crate::bigbang::run(
            &root,
            &crate::bigbang::Options {
                force: true,
                ..crate::bigbang::Options::default()
            },
        )
        .unwrap();

        let graph = Graph::open_existing(&root).unwrap();
        let widget = graph.find_by_name("Widget").unwrap().unwrap();
        let explainers = graph.callers(widget.id.unwrap()).unwrap();
        assert!(
            explainers
                .iter()
                .any(|(node, kind, _)| node.name == "README.md" && *kind == EdgeKind::Explains)
        );
    }
}
