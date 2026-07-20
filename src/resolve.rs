//! Cross-file resolution: turns each file's raw imports/calls (produced by
//! `crate::parse`) into graph edges, tagged with how confident the
//! resolution is — per `SPEC.md` section 3:
//!
//! - `EXTRACTED` — an import whose last path segment matches exactly one
//!   symbol name in the repo.
//! - `INFERRED` — a call whose callee identifier matches exactly one symbol
//!   name in the repo (name-only heuristic, no type checking).
//! - `AMBIGUOUS` — the same, but more than one symbol shares that name.
//!
//! Matches against nothing (e.g. a call into an external crate, or `std`)
//! are dropped rather than stored as a dangling edge.
//!
//! Doc/image files (`SPEC.md` section 5) are handled here too: text docs
//! (`.md`/`.txt`) are indexed immediately as `Doc` nodes with their full
//! content as `description` — no model call needed, same as any other
//! deterministic parse. Binary docs (images/PDFs) are inserted with
//! `description: None`, a "needs a vision pass" marker; `crate::docs`
//! lets the host agent fill that in later at zero cost to `aag` itself.
//! Either way, mentions of a known symbol name in a doc's text become
//! `Explains` edges, resolved the same name-matching way as calls/imports.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::error::Result;
use crate::parse::parse_file;
use crate::storage::{
    Confidence, Edge, EdgeKind, EvidenceKind, Graph, Node, NodeKind, Perspective, Provenance,
    RawReference,
};

/// Directory names skipped entirely while walking a repo for indexing —
/// shared by the watcher and `aag sync` so "what can affect the index"
/// has exactly one definition. `.playwright-mcp` holds browser-automation
/// artifacts (screenshots, snapshots) and `.claude`/`.cursor`/`.agents` hold agent
/// config (including the skill pack `aag install` writes) — all of which
/// would otherwise pollute the graph as doc nodes. `.venv`/`venv`/
/// `__pycache__`/`.tox` are a belt-and-suspenders net for repos whose
/// `.gitignore` doesn't (or doesn't yet) exclude their own virtualenv —
/// `walk_files` also honors `.gitignore` itself, so this list only matters
/// when that file is missing or incomplete.
pub(crate) const SKIP_DIRS: &[&str] = &[
    ".git",
    ".aag",
    "target",
    "node_modules",
    ".playwright-mcp",
    ".claude",
    ".cursor",
    ".agents",
    ".venv",
    "venv",
    "__pycache__",
    ".tox",
];

/// Generated root-level files that must never trigger or enter indexing.
pub(crate) const SKIP_FILES: &[&str] = &[".aag.lock"];

/// Text doc extensions, indexed immediately (no vision pass needed).
const TEXT_DOC_EXTENSIONS: &[&str] = &["md", "txt"];

/// Binary/image doc extensions — inserted unprocessed, described later by
/// the host agent via `crate::docs::describe`.
const BINARY_DOC_EXTENSIONS: &[&str] = &[
    "pdf", "png", "jpg", "jpeg", "gif", "webp", "svg", "docx", "pptx", "xlsx", "odt", "ods", "odp",
    "mp4", "mov", "avi", "mkv", "webm",
];

/// Counts from one `index_repo` pass — used for logging and tests.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct IndexSummary {
    /// Files parsed (only those with a registered `LanguageParser`).
    pub files: u32,
    /// Symbol nodes inserted (functions/structs/methods), excluding file nodes.
    pub nodes: u32,
    /// Doc nodes inserted (text docs indexed immediately, binary docs pending description).
    pub docs: u32,
    /// Operations declared by OpenAPI/Swagger contracts.
    pub contracts: u32,
    /// Database and infrastructure declarations.
    pub artifacts: u32,
    /// Edges resolved and inserted (imports + calls + explains).
    pub edges: u32,
}

/// Whether/how a file is a doc rather than code, by extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocKind {
    /// Plain text — indexed immediately, no model needed.
    Text,
    /// Image/PDF — needs a vision pass before it has a description.
    Binary,
}

fn doc_kind(relative_path: &str) -> Option<DocKind> {
    let extension = relative_path.rsplit('.').next().unwrap_or_default();
    if TEXT_DOC_EXTENSIONS.contains(&extension) {
        Some(DocKind::Text)
    } else if BINARY_DOC_EXTENSIONS.contains(&extension) {
        Some(DocKind::Binary)
    } else {
        None
    }
}

/// Whether changing this path can add, remove, or alter graph facts.
#[must_use]
pub fn is_indexable_path(path: &Path) -> bool {
    let text = path.to_string_lossy();
    if crate::parse::supports_file(&text) || doc_kind(&text).is_some() {
        return true;
    }
    matches!(
        path.extension().and_then(|value| value.to_str()),
        Some("json" | "yaml" | "yml" | "sql" | "tf" | "hcl")
    )
}

/// Symbol names mentioned in `text`, restricted to names `by_name` already
/// knows about (so a doc's prose doesn't spuriously "mention" a symbol
/// that only shares a common English word). Requires more than 2
/// characters to cut noise from short tokens.
pub(crate) fn mentioned_names(
    text: &str,
    by_name: &HashMap<String, Vec<(i64, String)>>,
) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|token| token.len() > 2 && by_name.contains_key(*token))
        .filter(|token| seen.insert(*token))
        .map(str::to_string)
        .collect()
}

/// Clears `graph`, walks `root`, parses every recognized file, and resolves
/// cross-file imports/calls/doc-mentions into confidence-tagged edges.
/// Always a full rebuild rather than an incremental patch — callers (e.g.
/// `crate::watch` on every debounced change) rely on this being idempotent
/// and safe to call repeatedly as files change.
///
/// # Errors
///
/// Returns a storage error if a graph write fails. Individual files that
/// can't be read as UTF-8 (e.g. an unrecognized binary format) are skipped
/// with a warning rather than aborting the whole pass.
pub fn index_repo(graph: &Graph, root: &Path) -> Result<IndexSummary> {
    // One transaction for the whole clear+insert+resolve pass — one fsync
    // on commit instead of one per statement. See `Graph::transaction`.
    graph.transaction(|| {
        graph.clear()?;

        let mut summary = IndexSummary::default();
        let mut by_name: HashMap<String, Vec<(i64, String)>> = HashMap::new();
        let mut by_file_symbol: HashMap<(String, String), i64> = HashMap::new();
        let mut pending_imports: Vec<(i64, String)> = Vec::new();
        let mut pending_calls: Vec<(String, String, String)> = Vec::new();
        let mut pending_doc_mentions: Vec<(i64, String)> = Vec::new();
        let mut pending_operations = Vec::new();

        for path in walk_files(root) {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");

            if let Some(operations) = crate::openapi::index_contract(graph, &relative, &path)? {
                persist_operations(graph, &relative, &operations)?;
                summary.contracts += u32::try_from(operations.len()).unwrap_or(u32::MAX);
                pending_operations.extend(operations);
                continue;
            }
            if let Some(count) = crate::artifacts::index_artifact(graph, &relative, &path)? {
                summary.artifacts = summary.artifacts.saturating_add(count);
                continue;
            }

            if let Some(kind) = doc_kind(&relative) {
                index_doc_file(
                    graph,
                    &relative,
                    &path,
                    kind,
                    &mut by_name,
                    &mut pending_doc_mentions,
                    &mut summary,
                )?;
                continue;
            }

            let Ok(source) = fs::read_to_string(&path) else {
                tracing::warn!(path = %path.display(), "skipping unreadable file (likely binary)");
                continue;
            };
            let Some(parsed) = parse_file(&relative, &source)? else {
                continue;
            };

            index_code_file(
                graph,
                &relative,
                &source,
                parsed,
                &mut by_name,
                &mut by_file_symbol,
                &mut pending_imports,
                &mut pending_calls,
                &mut summary,
            )?;
        }

        resolve_doc_mentions(graph, pending_doc_mentions, &by_name, &mut summary)?;
        resolve_imports(graph, pending_imports, &by_name, &mut summary)?;
        resolve_calls(
            graph,
            pending_calls,
            &by_name,
            &by_file_symbol,
            &mut summary,
        )?;
        resolve_openapi_operations(graph, pending_operations, &by_name, &mut summary)?;
        graph.mark_incremental_ready()?;

        Ok(summary)
    })
}

fn persist_operations(
    graph: &Graph,
    relative: &str,
    operations: &[crate::openapi::Operation],
) -> Result<()> {
    for operation in operations {
        graph.insert_raw_reference(&RawReference {
            file_path: relative.to_string(),
            kind: "operation".into(),
            owner: operation.node_name.clone(),
            target: serde_json::to_string(&operation.candidate_names)
                .unwrap_or_else(|_| "[]".into()),
        })?;
    }
    Ok(())
}

fn resolve_openapi_operations(
    graph: &Graph,
    pending: Vec<crate::openapi::Operation>,
    by_name: &HashMap<String, Vec<(i64, String)>>,
    summary: &mut IndexSummary,
) -> Result<()> {
    for operation in pending {
        let candidates = operation
            .candidate_names
            .iter()
            .filter_map(|name| by_name.get(name))
            .flatten()
            .collect::<Vec<_>>();
        let confidence = resolution_confidence(candidates.len(), Confidence::Inferred);
        for &&(implementation, _) in &candidates {
            graph.insert_edge_with_provenance(
                &Edge {
                    src: implementation,
                    dst: operation.node_id,
                    kind: EdgeKind::Implements,
                    confidence,
                },
                &Provenance {
                    perspective: Perspective::Declared,
                    evidence_kind: EvidenceKind::OpenApi,
                    evidence_source: None,
                },
            )?;
            summary.edges += 1;
        }
    }
    Ok(())
}

fn index_doc_file(
    graph: &Graph,
    relative: &str,
    path: &Path,
    kind: DocKind,
    by_name: &mut HashMap<String, Vec<(i64, String)>>,
    pending_doc_mentions: &mut Vec<(i64, String)>,
    summary: &mut IndexSummary,
) -> Result<()> {
    let description = match kind {
        DocKind::Text => fs::read_to_string(path).ok(),
        DocKind::Binary => None,
    };
    let doc_id = graph.insert_node(&Node {
        id: None,
        kind: NodeKind::Doc,
        name: relative.to_string(),
        file_path: relative.to_string(),
        start_line: 1,
        end_line: 1,
        description: description.clone(),
    })?;
    summary.docs += 1;
    by_name
        .entry(relative.to_string())
        .or_default()
        .push((doc_id, relative.to_string()));
    if let Some(text) = description {
        graph.insert_raw_reference(&RawReference {
            file_path: relative.to_string(),
            kind: "doc".into(),
            owner: relative.to_string(),
            target: text.clone(),
        })?;
        pending_doc_mentions.push((doc_id, text));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn index_code_file(
    graph: &Graph,
    relative: &str,
    source: &str,
    parsed: crate::parse::ParsedFile,
    by_name: &mut HashMap<String, Vec<(i64, String)>>,
    by_file_symbol: &mut HashMap<(String, String), i64>,
    pending_imports: &mut Vec<(i64, String)>,
    pending_calls: &mut Vec<(String, String, String)>,
    summary: &mut IndexSummary,
) -> Result<()> {
    summary.files += 1;
    let file_id = graph.insert_node(&Node {
        id: None,
        kind: NodeKind::File,
        name: relative.to_string(),
        file_path: relative.to_string(),
        start_line: 1,
        end_line: u32::try_from(source.lines().count())
            .unwrap_or(u32::MAX)
            .max(1),
        description: None,
    })?;

    for node in parsed.nodes {
        let name = node.name.clone();
        let id = graph.insert_node(&node)?;
        summary.nodes += 1;
        by_name
            .entry(name.clone())
            .or_default()
            .push((id, relative.to_string()));
        by_file_symbol.insert((relative.to_string(), name), id);
    }

    for raw in parsed.imports {
        graph.insert_raw_reference(&RawReference {
            file_path: relative.to_string(),
            kind: "import".into(),
            owner: relative.to_string(),
            target: raw.clone(),
        })?;
        pending_imports.push((file_id, raw));
    }
    for (caller, callee) in parsed.calls {
        graph.insert_raw_reference(&RawReference {
            file_path: relative.to_string(),
            kind: "call".into(),
            owner: caller.clone(),
            target: callee.clone(),
        })?;
        pending_calls.push((relative.to_string(), caller, callee));
    }
    Ok(())
}

/// Reindexes exactly one changed file, then re-resolves cross-file edges from
/// persisted raw references. Unchanged files are neither read nor parsed.
///
/// # Errors
/// Returns an error when the changed file cannot be parsed or the graph cannot be updated.
pub fn index_file(graph: &Graph, root: &Path, file: &Path) -> Result<IndexSummary> {
    let relative = file
        .strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/");
    graph.transaction(|| {
        graph.remove_file(&relative)?;
        if file.is_file() {
            let mut summary = IndexSummary::default();
            let mut by_name = HashMap::new();
            let mut by_file_symbol = HashMap::new();
            let mut pending_imports = Vec::new();
            let mut pending_calls = Vec::new();
            let mut pending_docs = Vec::new();
            if let Some(operations) = crate::openapi::index_contract(graph, &relative, file)? {
                persist_operations(graph, &relative, &operations)?;
            } else if crate::artifacts::index_artifact(graph, &relative, file)?.is_some() {
            } else if let Some(kind) = doc_kind(&relative) {
                index_doc_file(
                    graph,
                    &relative,
                    file,
                    kind,
                    &mut by_name,
                    &mut pending_docs,
                    &mut summary,
                )?;
            } else if let Ok(source) = fs::read_to_string(file)
                && let Some(parsed) = parse_file(&relative, &source)?
            {
                index_code_file(
                    graph,
                    &relative,
                    &source,
                    parsed,
                    &mut by_name,
                    &mut by_file_symbol,
                    &mut pending_imports,
                    &mut pending_calls,
                    &mut summary,
                )?;
            }
        }
        rebuild_resolved_edges(graph)
    })
}

/// Rebuilds name-resolved relations using stored parser output only.
///
/// # Errors
/// Returns an error when persisted references cannot be read or edges cannot be written.
pub fn rebuild_resolved_edges(graph: &Graph) -> Result<IndexSummary> {
    graph.clear_resolved_edges()?;
    let nodes = graph.all_nodes()?;
    let mut by_name: HashMap<String, Vec<(i64, String)>> = HashMap::new();
    let mut by_file_symbol = HashMap::new();
    let mut by_file_node = HashMap::new();
    let mut by_file_name = HashMap::new();
    for node in &nodes {
        let Some(id) = node.id else { continue };
        if node.kind == NodeKind::File {
            by_file_node.insert(node.file_path.clone(), id);
        } else {
            by_name
                .entry(node.name.clone())
                .or_default()
                .push((id, node.file_path.clone()));
            by_file_name.insert((node.file_path.clone(), node.name.clone()), id);
            if matches!(
                node.kind,
                NodeKind::Function | NodeKind::Method | NodeKind::Struct | NodeKind::Interface
            ) {
                by_file_symbol.insert((node.file_path.clone(), node.name.clone()), id);
            }
        }
    }
    let mut imports = Vec::new();
    let mut calls = Vec::new();
    let mut docs = Vec::new();
    let mut operations = Vec::new();
    for reference in graph.raw_references()? {
        match reference.kind.as_str() {
            "import" => {
                if let Some(&id) = by_file_node.get(&reference.file_path) {
                    imports.push((id, reference.target));
                }
            }
            "call" => calls.push((reference.file_path, reference.owner, reference.target)),
            "doc" => {
                if let Some(&id) = by_file_name.get(&(reference.file_path, reference.owner)) {
                    docs.push((id, reference.target));
                }
            }
            "operation" => {
                if let Some(&node_id) =
                    by_file_name.get(&(reference.file_path, reference.owner.clone()))
                {
                    let candidate_names =
                        serde_json::from_str(&reference.target).unwrap_or_default();
                    operations.push(crate::openapi::Operation {
                        node_id,
                        node_name: reference.owner,
                        candidate_names,
                    });
                }
            }
            _ => {}
        }
    }
    let mut summary = IndexSummary::default();
    resolve_doc_mentions(graph, docs, &by_name, &mut summary)?;
    resolve_imports(graph, imports, &by_name, &mut summary)?;
    resolve_calls(graph, calls, &by_name, &by_file_symbol, &mut summary)?;
    resolve_openapi_operations(graph, operations, &by_name, &mut summary)?;
    summary.files = u32::try_from(
        nodes
            .iter()
            .filter(|node| node.kind == NodeKind::File)
            .count(),
    )
    .unwrap_or(u32::MAX);
    summary.docs = u32::try_from(
        nodes
            .iter()
            .filter(|node| node.kind == NodeKind::Doc)
            .count(),
    )
    .unwrap_or(u32::MAX);
    summary.contracts = u32::try_from(
        nodes
            .iter()
            .filter(|node| node.kind == NodeKind::Endpoint)
            .count(),
    )
    .unwrap_or(u32::MAX);
    summary.artifacts = u32::try_from(
        nodes
            .iter()
            .filter(|node| matches!(node.kind, NodeKind::DatabaseTable | NodeKind::InfraResource))
            .count(),
    )
    .unwrap_or(u32::MAX);
    summary.nodes = u32::try_from(nodes.len())
        .unwrap_or(u32::MAX)
        .saturating_sub(summary.files)
        .saturating_sub(summary.docs);
    summary.edges = u32::try_from(graph.all_edges()?.len()).unwrap_or(u32::MAX);
    Ok(summary)
}

/// A doc naming a symbol that exists in more than this many places is not
/// "explaining" any one of them — it's using a common word (`run`, `list`).
/// Linking all of them buries the graph in AMBIGUOUS Explains edges.
const MAX_DOC_MENTION_CANDIDATES: usize = 2;

fn resolve_doc_mentions(
    graph: &Graph,
    pending: Vec<(i64, String)>,
    by_name: &HashMap<String, Vec<(i64, String)>>,
    summary: &mut IndexSummary,
) -> Result<()> {
    for (doc_id, text) in pending {
        for name in mentioned_names(&text, by_name) {
            let Some(candidates) = by_name.get(&name) else {
                continue;
            };
            if candidates.len() > MAX_DOC_MENTION_CANDIDATES {
                continue;
            }
            let confidence = resolution_confidence(candidates.len(), Confidence::Inferred);
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
                summary.edges += 1;
            }
        }
    }
    Ok(())
}

fn resolve_imports(
    graph: &Graph,
    pending: Vec<(i64, String)>,
    by_name: &HashMap<String, Vec<(i64, String)>>,
    summary: &mut IndexSummary,
) -> Result<()> {
    for (file_id, raw) in pending {
        for path in expand_import(&raw) {
            let Some(target) = last_segment(&path) else {
                continue;
            };
            let Some(candidates) = by_name.get(target) else {
                continue;
            };
            // The import path names its module (`crate::sync::run` → the
            // candidate living in sync.rs) — same narrowing calls get.
            let narrowed = narrow_by_module(candidates, module_qualifier(&path));
            let confidence = resolution_confidence(narrowed.len(), Confidence::Extracted);
            for &(dst, _) in narrowed {
                graph.insert_edge(&Edge {
                    src: file_id,
                    dst,
                    kind: EdgeKind::Imports,
                    confidence,
                })?;
                summary.edges += 1;
            }
        }
    }
    Ok(())
}

fn resolve_calls(
    graph: &Graph,
    pending: Vec<(String, String, String)>,
    by_name: &HashMap<String, Vec<(i64, String)>>,
    by_file_symbol: &HashMap<(String, String), i64>,
    summary: &mut IndexSummary,
) -> Result<()> {
    for (file_path, caller, callee) in pending {
        let Some(&src) = by_file_symbol.get(&(file_path.clone(), caller)) else {
            continue;
        };
        let name = last_segment(&callee).unwrap_or(callee.as_str());
        let Some(candidates) = by_name.get(name) else {
            continue;
        };

        // Narrowing ladder — each rung uses information the raw name-match
        // ignores, so a repo full of same-named `run`s still resolves:
        // 1. Qualified call (`bigbang::run`): candidate in the file whose
        //    module matches the qualifier.
        // 2. Unqualified (or unmatched): candidate defined in the caller's
        //    own file — what an unqualified call in Rust usually means.
        // 3. Otherwise: every candidate, AMBIGUOUS when more than one
        //    (the honest fallback; better to over-warn on impact).
        let by_module = narrow_by_module(candidates, module_qualifier(&callee));
        let narrowed = if by_module.len() == 1 {
            by_module
        } else {
            let same_file: Vec<&(i64, String)> = candidates
                .iter()
                .filter(|(_, file)| *file == file_path)
                .collect();
            if same_file.len() == 1 {
                same_file
            } else {
                candidates.iter().collect()
            }
        };

        let confidence = resolution_confidence(narrowed.len(), Confidence::Inferred);
        for &(dst, _) in narrowed {
            graph.insert_edge(&Edge {
                src,
                dst,
                kind: EdgeKind::Calls,
                confidence,
            })?;
            summary.edges += 1;
        }
    }
    Ok(())
}

/// The module segment right before the final name in a qualified path
/// (`crate::bigbang::run` → `bigbang`), or `None` for unqualified names
/// and non-module qualifiers (`self`, `crate`, `super` alone).
fn module_qualifier(path: &str) -> Option<&str> {
    let mut segments = path.rsplit("::");
    segments.next()?;
    let qualifier = segments.next().map(str::trim)?;
    if qualifier.is_empty() || matches!(qualifier, "self" | "crate" | "super" | "Self") {
        return None;
    }
    Some(qualifier)
}

/// Candidates whose file corresponds to `qualifier` (`sync` matches
/// `src/sync.rs`, `sync/mod.rs`, or anything under a `sync/` directory).
/// No qualifier, or nothing matching, returns everything — narrowing must
/// never drop a real candidate, only prefer a provably better one.
fn narrow_by_module<'c>(
    candidates: &'c [(i64, String)],
    qualifier: Option<&str>,
) -> Vec<&'c (i64, String)> {
    let Some(qualifier) = qualifier else {
        return candidates.iter().collect();
    };
    let matched: Vec<&(i64, String)> = candidates
        .iter()
        .filter(|(_, file)| {
            let stem = file
                .rsplit('/')
                .next()
                .unwrap_or(file)
                .trim_end_matches(".rs");
            stem == qualifier || file.contains(&format!("/{qualifier}/")) || {
                stem == "mod"
                    && file
                        .rsplit('/')
                        .nth(1)
                        .is_some_and(|parent| parent == qualifier)
            }
        })
        .collect();
    if matched.is_empty() {
        candidates.iter().collect()
    } else {
        matched
    }
}

/// Confident tag for a name-based match: `unique` when exactly one candidate
/// resolved, `AMBIGUOUS` when more than one did.
fn resolution_confidence(candidate_count: usize, unique: Confidence) -> Confidence {
    if candidate_count == 1 {
        unique
    } else {
        Confidence::Ambiguous
    }
}

/// Walks `root` for indexing, honoring the repo's `.gitignore`/`.ignore`
/// (via the `ignore` crate — same rules ripgrep uses) plus the hardcoded
/// `SKIP_DIRS` net for repos whose ignore files don't cover their own
/// vendor/build directories. `hidden(false)` keeps walking into dotdirs
/// not explicitly named in `SKIP_DIRS` (e.g. `.github`) — only gitignore
/// rules and the explicit list prune anything.
fn walk_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .hidden(false)
        .filter_entry(|entry| match entry.file_type() {
            Some(file_type) if file_type.is_dir() => {
                !SKIP_DIRS.contains(&entry.file_name().to_string_lossy().as_ref())
            }
            _ => !SKIP_FILES.contains(&entry.file_name().to_string_lossy().as_ref()),
        });
    builder
        .build()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|ft| ft.is_file()))
        .map(|entry| entry.path().to_path_buf())
        .collect()
}

/// Expands a raw grouped import (`std::collections::{HashMap, HashSet}`)
/// into one path per name. Ungrouped imports pass through unchanged.
fn expand_import(raw: &str) -> Vec<String> {
    let Some(brace_start) = raw.find('{') else {
        return vec![raw.to_string()];
    };
    let prefix = &raw[..brace_start];
    let inner = raw
        .get(brace_start + 1..raw.rfind('}').unwrap_or(raw.len()))
        .unwrap_or_default();
    inner
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| format!("{prefix}{name}"))
        .collect()
}

/// The rightmost identifier in a `::`-separated path, ignoring a trailing
/// `as alias` and skipping glob imports (`use foo::*`).
fn last_segment(path: &str) -> Option<&str> {
    let path = path.split(" as ").next().unwrap_or(path).trim();
    if path.is_empty() || path.ends_with('*') {
        return None;
    }
    path.rsplit("::")
        .next()
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn scratch_root() -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("aag-resolve-test-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn resolves_unique_call_across_files_as_inferred() {
        let root = scratch_root();
        fs::write(root.join("a.rs"), "fn caller() { helper(); }").unwrap();
        fs::write(root.join("b.rs"), "fn helper() {}").unwrap();

        let graph = Graph::open_in_memory().unwrap();
        let summary = index_repo(&graph, &root).unwrap();

        assert_eq!(summary.files, 2);
        let helper = graph.find_by_name("helper").unwrap().unwrap();
        let callers = graph.callers(helper.id.unwrap()).unwrap();
        assert_eq!(callers.len(), 1);
        assert_eq!(callers[0].0.name, "caller");
        assert_eq!(callers[0].1, EdgeKind::Calls);
        assert_eq!(callers[0].2, Confidence::Inferred);
    }

    #[test]
    fn resolves_import_across_files_as_extracted() {
        let root = scratch_root();
        fs::write(root.join("a.rs"), "use crate::b::Widget;").unwrap();
        fs::write(root.join("b.rs"), "struct Widget;").unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        let widget = graph.find_by_name("Widget").unwrap().unwrap();
        let importers = graph.callers(widget.id.unwrap()).unwrap();
        assert_eq!(importers.len(), 1);
        assert_eq!(importers[0].1, EdgeKind::Imports);
        assert_eq!(importers[0].2, Confidence::Extracted);
    }

    #[test]
    fn indexes_openapi_as_declared_and_links_operation_id() {
        let root = scratch_root();
        fs::write(root.join("api.rs"), "fn listPets() {}").unwrap();
        fs::write(
            root.join("openapi.yaml"),
            "openapi: 3.1.0\ninfo: {title: Pets, version: 1.0.0}\npaths:\n  /pets:\n    get:\n      operationId: listPets\n      parameters:\n        - {name: limit, in: query, schema: {type: integer}}\n      responses:\n        '200':\n          description: ok\n          content:\n            application/json:\n              schema: {$ref: '#/components/schemas/Pet'}\ncomponents:\n  schemas:\n    Pet:\n      type: object\n      required: [name]\n      properties: {name: {type: string}}\n",
        ).unwrap();

        let graph = Graph::open_in_memory().unwrap();
        let summary = index_repo(&graph, &root).unwrap();
        assert_eq!(summary.contracts, 1);

        let items = graph.all_nodes_with_provenance().unwrap();
        let (contract, provenance) = items
            .iter()
            .find(|(node, _)| node.name == "GET /pets")
            .unwrap();
        assert_eq!(contract.kind, NodeKind::Endpoint);
        assert_eq!(provenance.perspective, Perspective::Declared);
        assert_eq!(provenance.evidence_kind, EvidenceKind::OpenApi);
        let schema = items.iter().find(|(node, _)| node.name == "Pet").unwrap();
        assert_eq!(schema.0.kind, NodeKind::Schema);
        let implementers = graph.callers(contract.id.unwrap()).unwrap();
        assert!(
            implementers
                .iter()
                .any(|(node, kind, _)| node.name == "listPets" && *kind == EdgeKind::Implements)
        );
        let references = graph.callees(contract.id.unwrap()).unwrap();
        assert!(
            references
                .iter()
                .any(|(node, kind, _)| node.name == "Pet" && *kind == EdgeKind::References)
        );
    }

    #[test]
    fn same_name_in_two_files_resolves_call_as_ambiguous() {
        let root = scratch_root();
        fs::write(root.join("a.rs"), "fn caller() { run(); }").unwrap();
        fs::write(root.join("b.rs"), "fn run() {}").unwrap();
        fs::write(root.join("c.rs"), "fn run() {}").unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        let by_name = graph.search("run", 10).unwrap();
        assert_eq!(by_name.len(), 2);
        for node in by_name {
            let callers = graph.callers(node.id.unwrap()).unwrap();
            assert_eq!(callers.len(), 1);
            assert_eq!(callers[0].2, Confidence::Ambiguous);
        }
    }

    #[test]
    fn same_file_definition_beats_same_name_elsewhere() {
        let root = scratch_root();
        fs::write(
            root.join("a.rs"),
            "fn caller() { helper(); }\nfn helper() {}",
        )
        .unwrap();
        fs::write(root.join("b.rs"), "fn helper() {}").unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        // The unqualified call in a.rs must resolve only to a.rs's helper —
        // one INFERRED edge, not an AMBIGUOUS fan-out to b.rs too.
        let hits = graph.search("helper", 10).unwrap();
        let mut edges = 0;
        for node in hits {
            for (caller, _, confidence) in graph.callers(node.id.unwrap()).unwrap() {
                assert_eq!(caller.file_path, "a.rs");
                assert_eq!(confidence, Confidence::Inferred);
                edges += 1;
            }
        }
        assert_eq!(edges, 1, "exactly one resolved call edge");
    }

    #[test]
    fn qualified_call_resolves_to_matching_module() {
        let root = scratch_root();
        fs::write(root.join("main.rs"), "fn go() { bigbang::run(); }").unwrap();
        fs::write(root.join("bigbang.rs"), "fn run() {}").unwrap();
        fs::write(root.join("other.rs"), "fn run() {}").unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        let hits = graph.search("run", 10).unwrap();
        for node in hits {
            let callers = graph.callers(node.id.unwrap()).unwrap();
            if node.file_path == "bigbang.rs" {
                assert_eq!(callers.len(), 1, "qualifier must pick bigbang.rs");
                assert_eq!(callers[0].2, Confidence::Inferred);
            } else {
                assert!(
                    callers.is_empty(),
                    "other.rs's run must get no edge from a bigbang::-qualified call"
                );
            }
        }
    }

    #[test]
    fn call_with_no_match_is_dropped_not_stored_dangling() {
        let root = scratch_root();
        fs::write(root.join("a.rs"), "fn caller() { println!(\"x\"); }").unwrap();

        let graph = Graph::open_in_memory().unwrap();
        let summary = index_repo(&graph, &root).unwrap();

        assert_eq!(summary.edges, 0);
    }

    #[test]
    fn expand_import_splits_grouped_use() {
        let expanded = expand_import("std::collections::{HashMap, HashSet}");
        assert_eq!(
            expanded,
            vec![
                "std::collections::HashMap".to_string(),
                "std::collections::HashSet".to_string()
            ]
        );
    }

    #[test]
    fn last_segment_ignores_alias() {
        assert_eq!(last_segment("std::fs::File as F"), Some("File"));
    }

    #[test]
    fn last_segment_skips_glob() {
        assert_eq!(last_segment("std::collections::*"), None);
    }
}
