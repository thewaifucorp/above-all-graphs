//! Cross-file resolution: turns each file's imports/calls/type declarations
//! (produced by `crate::parse`) into graph edges, tagged with how confident
//! the resolution is — per `SPEC.md` section 3:
//!
//! - `EXTRACTED` — the source says so outright: an import resolved to the
//!   module it names, or a declared `extends`/`implements` relation.
//! - `INFERRED` — a call resolved through the narrowing ladder in
//!   [`narrow_call`] (import bindings, receivers, module qualifiers,
//!   enclosing file) rather than checked by a type checker.
//! - `AMBIGUOUS` — more than one candidate survived narrowing.
//!
//! Matches against nothing (e.g. a call into an external crate, or `std`)
//! are dropped rather than stored as a dangling edge. Import sources are
//! mapped onto repository files by `crate::bindings` first, so an import
//! that resolves outside the repo produces no edge at all instead of
//! name-matching a local symbol that happens to share its last segment.
//!
//! Doc/image files (`SPEC.md` section 5) are handled here too: text docs
//! (`.md`/`.txt`) are indexed immediately as `Doc` nodes with their full
//! content as `description` — no model call needed, same as any other
//! deterministic parse. Binary docs (images/PDFs) are inserted with
//! `description: None`, a "needs a vision pass" marker; `crate::docs`
//! lets the host agent fill that in later at zero cost to `aag` itself.
//! Either way, mentions of a known symbol name in a doc's text become
//! `Explains` edges, resolved the same name-matching way as calls/imports.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use crate::bindings::{Bindings, ImportTarget, ModuleIndex};
use crate::error::Result;
use crate::parse::{CallRef, ImportRef, InheritRef, LocalTypeRef, MemberRef, parse_file};
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
const TEXT_DOC_EXTENSIONS: &[&str] = &["md", "txt", "rst", "adoc", "srt", "vtt"];

/// Binary/image doc extensions. Text is extracted natively where the format
/// carries it (`crate::extract`); what has none — a scan, an unlabelled
/// screenshot, a video with no transcript beside it — is inserted unprocessed
/// and described later by the host agent via `crate::docs::describe`.
const BINARY_DOC_EXTENSIONS: &[&str] = &[
    "pdf", "png", "jpg", "jpeg", "gif", "webp", "svg", "docx", "pptx", "xlsx", "xlsm", "xls",
    "odt", "ods", "odp", "mp4", "mov", "avi", "mkv", "webm",
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
    if crate::parse::supports_file(&text)
        || doc_kind(&text).is_some()
        || crate::toolchain::is_manifest(&text)
    {
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
        let mut pending = Pending::default();

        for path in walk_files(root) {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");

            if crate::toolchain::is_manifest(&relative) {
                index_manifest(graph, &relative, &path, &mut pending)?;
                continue;
            }
            if let Some(operations) = crate::openapi::index_contract(graph, &relative, &path)? {
                persist_operations(graph, &relative, &operations)?;
                summary.contracts += u32::try_from(operations.len()).unwrap_or(u32::MAX);
                pending.operations.extend(operations);
                continue;
            }
            if let Some(count) = crate::artifacts::index_artifact(graph, &relative, &path)? {
                summary.artifacts = summary.artifacts.saturating_add(count);
                continue;
            }

            if let Some(kind) = doc_kind(&relative) {
                index_doc_file(graph, &relative, &path, kind, &mut pending, &mut summary)?;
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
                &mut pending,
                &mut summary,
            )?;
        }

        resolve_pending(graph, &pending, &mut summary)?;
        graph.mark_incremental_ready()?;

        Ok(summary)
    })
}

/// Everything one indexing pass collects before cross-file resolution runs.
///
/// Resolution needs the whole repository in hand — an import can only be
/// mapped to a file once every file is known — so the walk fills this and
/// [`resolve_pending`] turns it into edges.
#[derive(Debug, Default)]
struct Pending {
    /// Symbol name → every `(node id, file)` declaring it.
    by_name: HashMap<String, Vec<(i64, String)>>,
    /// `(file, symbol name)` → node id.
    by_file_name: HashMap<(String, String), i64>,
    /// File path → its `File` node id.
    file_nodes: HashMap<String, i64>,
    /// Parsed code files, in walk order — the module resolver's universe.
    code_files: Vec<String>,
    /// File path → imports written in it.
    imports: HashMap<String, Vec<ImportRef>>,
    /// `(file, call site)`.
    calls: Vec<(String, CallRef)>,
    /// `(file, declared type relation)`.
    inherits: Vec<(String, InheritRef)>,
    /// `(file, member declared by a type)`.
    members: Vec<(String, MemberRef)>,
    /// `(file, local binding with a known type)`.
    locals: Vec<(String, LocalTypeRef)>,
    /// Import aliases declared by the repository's build manifests.
    aliases: Vec<crate::toolchain::Alias>,
    /// `(file, handler symbol, endpoint node name)` for routes registered
    /// in code, and for tools exposed by name — the two link their handler the
    /// same way.
    routes: Vec<(String, Option<String>, String)>,
    /// `(file, calling symbol, method, path)` for outbound HTTP calls awaiting
    /// an endpoint to point at.
    consumers: Vec<(String, Option<String>, String, String)>,
    /// `(file, publishing symbol, event name)`.
    emitters: Vec<(String, Option<String>, String)>,
    /// `(file, listening symbol, event name)`.
    listeners: Vec<(String, Option<String>, String)>,
    /// `(file, calling symbol, tool name)`.
    tool_calls: Vec<(String, Option<String>, String)>,
    /// `(doc node id, doc text)`.
    doc_mentions: Vec<(i64, String)>,
    /// `OpenAPI` operations awaiting implementation matching.
    operations: Vec<crate::openapi::Operation>,
}

/// Name lookup tables plus the language-aware module and binding resolution
/// built on top of them.
struct SymbolIndex<'p> {
    by_name: &'p HashMap<String, Vec<(i64, String)>>,
    by_file_name: &'p HashMap<(String, String), i64>,
    file_nodes: &'p HashMap<String, i64>,
    modules: ModuleIndex,
    bindings: Bindings,
    /// `(file, type)` → the members that type declares there.
    type_members: HashMap<(String, String), HashSet<String>>,
    /// Every type name declared anywhere, so a receiver can be recognized
    /// as a type (`Graph::open`) rather than a variable.
    type_names: HashSet<String>,
    /// `(file, enclosing function, variable)` → the variable's type.
    locals: HashMap<(String, String, String), String>,
}

impl<'p> SymbolIndex<'p> {
    fn new(pending: &'p Pending) -> Self {
        let modules = ModuleIndex::with_toolchain(
            pending.code_files.iter().cloned(),
            crate::toolchain::Toolchain::new(pending.aliases.iter().cloned()),
        );
        let bindings = Bindings::build(&modules, &pending.imports);
        let mut type_members: HashMap<(String, String), HashSet<String>> = HashMap::new();
        let mut type_names = HashSet::new();
        for (file, member) in &pending.members {
            type_names.insert(member.owner_type.clone());
            type_members
                .entry((file.clone(), member.owner_type.clone()))
                .or_default()
                .insert(member.member.clone());
        }
        let locals = pending
            .locals
            .iter()
            .map(|(file, local)| {
                (
                    (file.clone(), local.scope.clone(), local.name.clone()),
                    local.type_name.clone(),
                )
            })
            .collect();
        Self {
            by_name: &pending.by_name,
            by_file_name: &pending.by_file_name,
            file_nodes: &pending.file_nodes,
            modules,
            bindings,
            type_members,
            type_names,
            locals,
        }
    }

    fn candidates(&self, name: &str) -> &[(i64, String)] {
        self.by_name.get(name).map_or(&[], Vec::as_slice)
    }

    /// Whether `file` declares `member` on `owner_type`.
    fn declares(&self, file: &str, owner_type: &str, member: &str) -> bool {
        self.type_members
            .get(&(file.to_string(), owner_type.to_string()))
            .is_some_and(|members| members.contains(member))
    }
}

fn resolve_pending(graph: &Graph, pending: &Pending, summary: &mut IndexSummary) -> Result<()> {
    let index = SymbolIndex::new(pending);
    resolve_doc_mentions(graph, &pending.doc_mentions, &index, summary)?;
    resolve_imports(graph, &pending.imports, &index, summary)?;
    resolve_calls(graph, &pending.calls, &index, summary)?;
    resolve_inheritance(graph, &pending.inherits, &index, summary)?;
    resolve_routes(graph, &pending.routes, &index, summary)?;
    resolve_consumers(graph, &pending.consumers, &index, summary)?;
    resolve_events(graph, pending, &index, summary)?;
    resolve_tool_calls(graph, &pending.tool_calls, &index, summary)?;
    resolve_openapi_operations(graph, &pending.operations, &index, summary)
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
    pending: &[crate::openapi::Operation],
    index: &SymbolIndex<'_>,
    summary: &mut IndexSummary,
) -> Result<()> {
    for operation in pending {
        let candidates = operation
            .candidate_names
            .iter()
            .filter_map(|name| index.by_name.get(name))
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
    pending: &mut Pending,
    summary: &mut IndexSummary,
) -> Result<()> {
    // A binary doc is read natively when its format carries text; what comes
    // back is indexed exactly like a text doc's contents, so it links to the
    // symbols it names without waiting for a vision pass. Nothing readable is
    // still `None`, which is what `aag describe` expects to find.
    let description = match kind {
        DocKind::Text => fs::read_to_string(path).ok(),
        DocKind::Binary => crate::extract::text(path),
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
    pending
        .by_name
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
        pending.doc_mentions.push((doc_id, text));
    }
    Ok(())
}

/// Reads a build manifest for the import aliases it declares and persists
/// them like any other unresolved reference, so an incremental pass keeps
/// resolving `@app/*` without re-reading the repository.
fn index_manifest(graph: &Graph, relative: &str, path: &Path, pending: &mut Pending) -> Result<()> {
    let Ok(contents) = fs::read_to_string(path) else {
        return Ok(());
    };
    let aliases = crate::toolchain::manifest_aliases(relative, &contents);
    for alias in aliases {
        graph.insert_raw_reference(&RawReference {
            file_path: relative.to_string(),
            kind: "alias".into(),
            owner: alias.pattern.clone(),
            target: serde_json::to_string(&alias.targets).unwrap_or_else(|_| "[]".into()),
        })?;
        pending.aliases.push(alias);
    }
    Ok(())
}

fn index_code_file(
    graph: &Graph,
    relative: &str,
    source: &str,
    parsed: crate::parse::ParsedFile,
    pending: &mut Pending,
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
    pending.file_nodes.insert(relative.to_string(), file_id);
    pending.code_files.push(relative.to_string());

    for node in parsed.nodes {
        let name = node.name.clone();
        let id = graph.insert_node(&node)?;
        summary.nodes += 1;
        pending
            .by_name
            .entry(name.clone())
            .or_default()
            .push((id, relative.to_string()));
        pending
            .by_file_name
            .insert((relative.to_string(), name), id);
    }

    for import in parsed.imports {
        graph.insert_raw_reference(&RawReference {
            file_path: relative.to_string(),
            kind: "import".into(),
            owner: relative.to_string(),
            target: encode_import(&import),
        })?;
        pending
            .imports
            .entry(relative.to_string())
            .or_default()
            .push(import);
    }
    for call in parsed.calls {
        graph.insert_raw_reference(&RawReference {
            file_path: relative.to_string(),
            kind: "call".into(),
            owner: call.caller.clone(),
            target: encode_call(&call),
        })?;
        pending.calls.push((relative.to_string(), call));
    }
    for inherit in parsed.inherits {
        graph.insert_raw_reference(&RawReference {
            file_path: relative.to_string(),
            kind: "inherit".into(),
            owner: inherit.child.clone(),
            target: encode_inherit(&inherit),
        })?;
        pending.inherits.push((relative.to_string(), inherit));
    }
    for member in parsed.members {
        graph.insert_raw_reference(&RawReference {
            file_path: relative.to_string(),
            kind: "member".into(),
            owner: member.owner_type.clone(),
            target: member.member.clone(),
        })?;
        pending.members.push((relative.to_string(), member));
    }
    index_routes(graph, relative, &parsed.routes, pending, summary)?;
    index_tools(graph, relative, &parsed.tools, pending, summary)?;
    index_consumers(graph, relative, &parsed.consumers, pending)?;
    index_events(graph, relative, &parsed.events, &parsed.tool_calls, pending)?;
    for local in parsed.locals {
        graph.insert_raw_reference(&RawReference {
            file_path: relative.to_string(),
            kind: "local".into(),
            owner: local.scope.clone(),
            target: encode_local(&local),
        })?;
        pending.locals.push((relative.to_string(), local));
    }
    Ok(())
}

/// Inserts an endpoint node per route the file registers, and persists the
/// registration so an incremental pass can relink the handler.
fn index_routes(
    graph: &Graph,
    relative: &str,
    routes: &[crate::parse::RouteRef],
    pending: &mut Pending,
    summary: &mut IndexSummary,
) -> Result<()> {
    // A route registered in code is an endpoint the implementation actually
    // serves — observed, as opposed to the declared endpoints a contract
    // file states (`crate::openapi`). Both belong in the graph.
    for route in routes {
        let endpoint = format!("{} {}", route.method, route.path);
        let endpoint_id = graph.insert_node_with_provenance(
            &Node {
                id: None,
                kind: NodeKind::Endpoint,
                name: endpoint.clone(),
                file_path: relative.to_string(),
                start_line: route.line,
                end_line: route.line,
                description: Some(
                    serde_json::json!({ "method": route.method, "path": route.path }).to_string(),
                ),
            },
            &Provenance {
                perspective: Perspective::Observed,
                evidence_kind: EvidenceKind::AstDefinition,
                evidence_source: Some(relative.to_string()),
            },
        )?;
        pending
            .by_file_name
            .insert((relative.to_string(), endpoint.clone()), endpoint_id);
        summary.contracts = summary.contracts.saturating_add(1);
        graph.insert_raw_reference(&RawReference {
            file_path: relative.to_string(),
            kind: "route".into(),
            owner: route.handler.clone().unwrap_or_default(),
            target: endpoint.clone(),
        })?;
        pending
            .routes
            .push((relative.to_string(), route.handler.clone(), endpoint));
    }
    Ok(())
}

/// Inserts an endpoint node per RPC/MCP tool the file exposes.
///
/// A tool is a callable contract in the same sense a route is, so it lands in
/// the same node kind with `TOOL` where a method would be. One vocabulary means
/// impact, contracts, and the graph UI treat both without special cases.
fn index_tools(
    graph: &Graph,
    relative: &str,
    tools: &[crate::parse::ToolRef],
    pending: &mut Pending,
    summary: &mut IndexSummary,
) -> Result<()> {
    for tool in tools {
        let endpoint = format!("TOOL {}", tool.name);
        let endpoint_id = graph.insert_node_with_provenance(
            &Node {
                id: None,
                kind: NodeKind::Endpoint,
                name: endpoint.clone(),
                file_path: relative.to_string(),
                start_line: tool.line,
                end_line: tool.line,
                description: Some(
                    serde_json::json!({ "method": "TOOL", "path": tool.name }).to_string(),
                ),
            },
            &Provenance {
                perspective: Perspective::Observed,
                evidence_kind: EvidenceKind::AstDefinition,
                evidence_source: Some(relative.to_string()),
            },
        )?;
        pending
            .by_file_name
            .insert((relative.to_string(), endpoint.clone()), endpoint_id);
        summary.contracts = summary.contracts.saturating_add(1);
        graph.insert_raw_reference(&RawReference {
            file_path: relative.to_string(),
            kind: "tool".into(),
            owner: tool.handler.clone().unwrap_or_default(),
            target: endpoint.clone(),
        })?;
        pending
            .routes
            .push((relative.to_string(), tool.handler.clone(), endpoint));
    }
    Ok(())
}

/// Persists the outbound HTTP calls a file makes, so resolution can point each
/// at the endpoint it consumes once every endpoint in the repository is known.
fn index_consumers(
    graph: &Graph,
    relative: &str,
    consumers: &[crate::parse::ConsumerRef],
    pending: &mut Pending,
) -> Result<()> {
    for consumer in consumers {
        graph.insert_raw_reference(&RawReference {
            file_path: relative.to_string(),
            kind: "consumer".into(),
            owner: consumer.owner.clone().unwrap_or_default(),
            target: format!("{} {}", consumer.method, consumer.path),
        })?;
        pending.consumers.push((
            relative.to_string(),
            consumer.owner.clone(),
            consumer.method.clone(),
            consumer.path.clone(),
        ));
    }
    Ok(())
}

/// Persists the events a file publishes or listens for, and the tools it
/// invokes by name. All three are name-keyed links whose other half is usually in
/// another file, and across a group of repositories in another repository.
fn index_events(
    graph: &Graph,
    relative: &str,
    events: &[crate::parse::EventRef],
    tool_calls: &[crate::parse::ToolCallRef],
    pending: &mut Pending,
) -> Result<()> {
    for event in events {
        let kind = if event.emitted {
            "event_emit"
        } else {
            "event_listen"
        };
        graph.insert_raw_reference(&RawReference {
            file_path: relative.to_string(),
            kind: kind.into(),
            owner: event.owner.clone().unwrap_or_default(),
            target: event.name.clone(),
        })?;
        let slot = if event.emitted {
            &mut pending.emitters
        } else {
            &mut pending.listeners
        };
        slot.push((
            relative.to_string(),
            event.owner.clone(),
            event.name.clone(),
        ));
    }
    for call in tool_calls {
        graph.insert_raw_reference(&RawReference {
            file_path: relative.to_string(),
            kind: "tool_call".into(),
            owner: call.owner.clone().unwrap_or_default(),
            target: call.name.clone(),
        })?;
        pending
            .tool_calls
            .push((relative.to_string(), call.owner.clone(), call.name.clone()));
    }
    Ok(())
}

/// Links a publisher to every listener of the same event name.
///
/// INFERRED, never EXTRACTED: the two sides agree on a string, and a string
/// match is evidence that they are talking about the same event, not proof. An
/// event with several listeners is normal, so every one is linked rather than the
/// link being dropped as ambiguous.
fn resolve_events(
    graph: &Graph,
    pending: &Pending,
    index: &SymbolIndex<'_>,
    summary: &mut IndexSummary,
) -> Result<()> {
    for (emit_file, emitter, name) in &pending.emitters {
        let Some(emitter) = emitter else { continue };
        let Some(&src) = index
            .by_file_name
            .get(&(emit_file.clone(), emitter.clone()))
        else {
            continue;
        };
        for (listen_file, listener, listened) in &pending.listeners {
            if listened != name {
                continue;
            }
            let Some(listener) = listener else { continue };
            let Some(&dst) = index
                .by_file_name
                .get(&(listen_file.clone(), listener.clone()))
            else {
                continue;
            };
            if src == dst {
                continue;
            }
            graph.insert_edge_with_provenance(
                &Edge {
                    src,
                    dst,
                    kind: EdgeKind::References,
                    confidence: Confidence::Inferred,
                },
                &Provenance {
                    perspective: Perspective::Observed,
                    evidence_kind: EvidenceKind::AstCall,
                    evidence_source: Some(emit_file.clone()),
                },
            )?;
            summary.edges += 1;
        }
    }
    Ok(())
}

/// Links a tool invocation to the tool definition it names.
///
/// This is the half of tool intelligence that a dispatcher hides: a definition
/// table says a tool exists, and only a call site naming it says who uses it.
fn resolve_tool_calls(
    graph: &Graph,
    pending: &[(String, Option<String>, String)],
    index: &SymbolIndex<'_>,
    summary: &mut IndexSummary,
) -> Result<()> {
    for (file_path, owner, name) in pending {
        let Some(owner) = owner else { continue };
        let Some(&src) = index.by_file_name.get(&(file_path.clone(), owner.clone())) else {
            continue;
        };
        let wanted = format!("TOOL {name}");
        for (_, &dst) in index
            .by_file_name
            .iter()
            .filter(|((_, node_name), _)| *node_name == wanted)
        {
            if src == dst {
                continue;
            }
            graph.insert_edge_with_provenance(
                &Edge {
                    src,
                    dst,
                    kind: EdgeKind::Calls,
                    confidence: Confidence::Extracted,
                },
                &Provenance {
                    perspective: Perspective::Observed,
                    evidence_kind: EvidenceKind::AstCall,
                    evidence_source: Some(file_path.clone()),
                },
            )?;
            summary.edges += 1;
        }
    }
    Ok(())
}

/// Points each outbound call at the endpoint it requests.
///
/// An exact `METHOD /path` match is EXTRACTED — the call literally names it. A
/// match that only holds once path parameters are treated as wildcards is
/// INFERRED (`/pets/42` calling `/pets/{id}`). Several endpoints matching is
/// AMBIGUOUS and every candidate is linked, the same fan-out a call with several
/// candidate symbols gets.
fn resolve_consumers(
    graph: &Graph,
    pending: &[(String, Option<String>, String, String)],
    index: &SymbolIndex<'_>,
    summary: &mut IndexSummary,
) -> Result<()> {
    for (file_path, owner, method, path) in pending {
        let Some(owner) = owner else { continue };
        let Some(&src) = index.by_file_name.get(&(file_path.clone(), owner.clone())) else {
            continue;
        };
        let wanted = format!("{method} {path}");
        // Endpoint nodes are registered per file rather than by bare name, so
        // the lookup walks that table: an endpoint name contains a space, which
        // no symbol name does.
        let exact: Vec<i64> = index
            .by_file_name
            .iter()
            .filter(|((_, name), _)| *name == wanted)
            .map(|(_, id)| *id)
            .collect();
        let (candidates, confidence) = if exact.is_empty() {
            let shape = endpoint_shape(&wanted);
            let matched: Vec<i64> = index
                .by_file_name
                .iter()
                .filter(|((_, name), _)| name.contains(' ') && endpoint_shape(name) == shape)
                .map(|(_, id)| *id)
                .collect();
            let confidence = if matched.len() > 1 {
                Confidence::Ambiguous
            } else {
                Confidence::Inferred
            };
            (matched, confidence)
        } else {
            (exact, Confidence::Extracted)
        };
        for dst in candidates {
            if dst == src {
                continue;
            }
            graph.insert_edge_with_provenance(
                &Edge {
                    src,
                    dst,
                    kind: EdgeKind::Calls,
                    confidence,
                },
                &Provenance {
                    perspective: Perspective::Observed,
                    evidence_kind: EvidenceKind::AstCall,
                    evidence_source: Some(file_path.clone()),
                },
            )?;
            summary.edges += 1;
        }
    }
    Ok(())
}

/// An endpoint name with its path parameters flattened, so the same route
/// written three ways compares equal: `GET /pets/{id}`, `GET /pets/:id`, and
/// `GET /pets/42` all become `GET /pets/*`.
///
/// Only a name that starts with a method and a `/` is a path — a tool name is
/// returned as-is, since flattening it would make unrelated tools equal.
fn endpoint_shape(name: &str) -> String {
    let Some((method, path)) = name.split_once(' ') else {
        return name.to_string();
    };
    if !path.starts_with('/') {
        return name.to_string();
    }
    let flattened: Vec<&str> = path
        .split('/')
        .map(|segment| {
            let parameter = segment.starts_with('{')
                || segment.starts_with(':')
                || segment.starts_with('<')
                || segment.starts_with('$')
                || segment == "*"
                || (!segment.is_empty() && segment.chars().all(|c| c.is_ascii_digit()));
            if parameter { "*" } else { segment }
        })
        .collect();
    format!("{method} {}", flattened.join("/"))
}

/// Structured references are persisted as JSON in `raw_references.target` so
/// an incremental pass can rebuild edges without reparsing the repository.
/// Decoding tolerates the pre-structured format (a bare path or callee name)
/// so a database written by an older build degrades instead of breaking.
fn encode_import(import: &ImportRef) -> String {
    serde_json::json!({
        "source": import.source,
        "name": import.name,
        "alias": import.alias,
        "glob": import.glob,
        "namespace": import.namespace,
    })
    .to_string()
}

fn decode_import(target: &str) -> ImportRef {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(target) else {
        return ImportRef::module(target);
    };
    ImportRef {
        source: value["source"].as_str().unwrap_or_default().to_string(),
        name: value["name"].as_str().map(str::to_string),
        alias: value["alias"].as_str().map(str::to_string),
        glob: value["glob"].as_bool().unwrap_or_default(),
        namespace: value["namespace"].as_bool().unwrap_or_default(),
    }
}

fn encode_call(call: &CallRef) -> String {
    serde_json::json!({
        "callee": call.callee,
        "receiver": call.receiver,
        "caller_type": call.caller_type,
    })
    .to_string()
}

fn decode_call(owner: &str, target: &str) -> CallRef {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(target) else {
        return CallRef {
            caller: owner.to_string(),
            caller_type: None,
            callee: target.to_string(),
            receiver: None,
        };
    };
    CallRef {
        caller: owner.to_string(),
        caller_type: value["caller_type"].as_str().map(str::to_string),
        callee: value["callee"].as_str().unwrap_or_default().to_string(),
        receiver: value["receiver"].as_str().map(str::to_string),
    }
}

fn encode_local(local: &LocalTypeRef) -> String {
    serde_json::json!({ "name": local.name, "type": local.type_name }).to_string()
}

fn decode_local(owner: &str, target: &str) -> Option<LocalTypeRef> {
    let value = serde_json::from_str::<serde_json::Value>(target).ok()?;
    Some(LocalTypeRef {
        scope: owner.to_string(),
        name: value["name"].as_str()?.to_string(),
        type_name: value["type"].as_str()?.to_string(),
    })
}

fn encode_inherit(inherit: &InheritRef) -> String {
    serde_json::json!({ "parent": inherit.parent, "implements": inherit.implements }).to_string()
}

fn decode_inherit(owner: &str, target: &str) -> Option<InheritRef> {
    let value = serde_json::from_str::<serde_json::Value>(target).ok()?;
    Some(InheritRef {
        child: owner.to_string(),
        parent: value["parent"].as_str()?.to_string(),
        implements: value["implements"].as_bool().unwrap_or_default(),
    })
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
            let mut pending = Pending::default();
            if crate::toolchain::is_manifest(&relative) {
                index_manifest(graph, &relative, file, &mut pending)?;
            } else if let Some(operations) = crate::openapi::index_contract(graph, &relative, file)?
            {
                persist_operations(graph, &relative, &operations)?;
            } else if crate::artifacts::index_artifact(graph, &relative, file)?.is_some() {
            } else if let Some(kind) = doc_kind(&relative) {
                index_doc_file(graph, &relative, file, kind, &mut pending, &mut summary)?;
            } else if let Ok(source) = fs::read_to_string(file)
                && let Some(parsed) = parse_file(&relative, &source)?
            {
                index_code_file(
                    graph,
                    &relative,
                    &source,
                    parsed,
                    &mut pending,
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
    let mut pending = Pending::default();
    for node in &nodes {
        let Some(id) = node.id else { continue };
        if node.kind == NodeKind::File {
            pending.file_nodes.insert(node.file_path.clone(), id);
            pending.code_files.push(node.file_path.clone());
        } else {
            pending
                .by_name
                .entry(node.name.clone())
                .or_default()
                .push((id, node.file_path.clone()));
            pending
                .by_file_name
                .insert((node.file_path.clone(), node.name.clone()), id);
        }
    }
    load_raw_references(graph, &mut pending)?;
    let mut summary = IndexSummary::default();
    resolve_pending(graph, &pending, &mut summary)?;
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

/// Rehydrates every persisted unresolved reference into `pending`, so an
/// incremental pass resolves from the same inputs a full pass would.
fn load_raw_references(graph: &Graph, pending: &mut Pending) -> Result<()> {
    for reference in graph.raw_references()? {
        match reference.kind.as_str() {
            "import" => pending
                .imports
                .entry(reference.file_path)
                .or_default()
                .push(decode_import(&reference.target)),
            "call" => pending.calls.push((
                reference.file_path,
                decode_call(&reference.owner, &reference.target),
            )),
            "inherit" => {
                if let Some(inherit) = decode_inherit(&reference.owner, &reference.target) {
                    pending.inherits.push((reference.file_path, inherit));
                }
            }
            "member" => pending.members.push((
                reference.file_path,
                MemberRef {
                    owner_type: reference.owner,
                    member: reference.target,
                },
            )),
            "local" => {
                if let Some(local) = decode_local(&reference.owner, &reference.target) {
                    pending.locals.push((reference.file_path, local));
                }
            }
            "route" | "tool" => pending.routes.push((
                reference.file_path,
                Some(reference.owner).filter(|owner| !owner.is_empty()),
                reference.target,
            )),
            "event_emit" => pending.emitters.push((
                reference.file_path,
                Some(reference.owner).filter(|owner| !owner.is_empty()),
                reference.target,
            )),
            "event_listen" => pending.listeners.push((
                reference.file_path,
                Some(reference.owner).filter(|owner| !owner.is_empty()),
                reference.target,
            )),
            "tool_call" => pending.tool_calls.push((
                reference.file_path,
                Some(reference.owner).filter(|owner| !owner.is_empty()),
                reference.target,
            )),
            "consumer" => {
                if let Some((method, path)) = reference.target.split_once(' ') {
                    pending.consumers.push((
                        reference.file_path,
                        Some(reference.owner).filter(|owner| !owner.is_empty()),
                        method.to_string(),
                        path.to_string(),
                    ));
                }
            }
            "alias" => pending.aliases.push(crate::toolchain::Alias {
                pattern: reference.owner,
                targets: serde_json::from_str(&reference.target).unwrap_or_default(),
            }),
            "doc" => {
                if let Some(&id) = pending
                    .by_file_name
                    .get(&(reference.file_path, reference.owner))
                {
                    pending.doc_mentions.push((id, reference.target));
                }
            }
            "operation" => {
                if let Some(&node_id) = pending
                    .by_file_name
                    .get(&(reference.file_path, reference.owner.clone()))
                {
                    let candidate_names =
                        serde_json::from_str(&reference.target).unwrap_or_default();
                    pending.operations.push(crate::openapi::Operation {
                        node_id,
                        node_name: reference.owner,
                        candidate_names,
                    });
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// A doc naming a symbol that exists in more than this many places is not
/// "explaining" any one of them — it's using a common word (`run`, `list`).
/// Linking all of them buries the graph in AMBIGUOUS Explains edges.
const MAX_DOC_MENTION_CANDIDATES: usize = 2;

fn resolve_doc_mentions(
    graph: &Graph,
    pending: &[(i64, String)],
    index: &SymbolIndex<'_>,
    summary: &mut IndexSummary,
) -> Result<()> {
    for (doc_id, text) in pending {
        let doc_id = *doc_id;
        for name in mentioned_names(text, index.by_name) {
            let Some(candidates) = index.by_name.get(&name) else {
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

/// Turns each import into an edge from the importing file to what it
/// actually pulled in.
///
/// The import statement already names the module, so the module resolver
/// gets first say: `use crate::b::Widget` points at the `Widget` declared in
/// `b.rs`, not at every `Widget` in the repo. A source with module structure
/// that resolves to no repository file (`std::fs`, `react`, a vendored SDK)
/// is an external dependency and gets no edge at all — the old name-only
/// match happily linked `use std::fs::File` to a local `File` type.
fn resolve_imports(
    graph: &Graph,
    pending: &HashMap<String, Vec<ImportRef>>,
    index: &SymbolIndex<'_>,
    summary: &mut IndexSummary,
) -> Result<()> {
    let mut files: Vec<&String> = pending.keys().collect();
    files.sort_unstable();
    for file in files {
        let Some(&file_id) = index.file_nodes.get(file) else {
            continue;
        };
        for import in &pending[file] {
            let (targets, member) = match index.modules.resolve_import(import, file) {
                ImportTarget::External => continue,
                ImportTarget::Member { files, name } => (files, Some(name)),
                ImportTarget::Module { files } => (files, None),
            };
            // A named member resolves to that member's node; anything else
            // (a module, a namespace, a wildcard, a member the target file
            // does not declare) is recorded as a file-level dependency.
            let members: Vec<i64> = member
                .map(|name| {
                    targets
                        .iter()
                        .filter_map(|target| {
                            index
                                .by_file_name
                                .get(&(target.clone(), name.clone()))
                                .copied()
                        })
                        .collect()
                })
                .unwrap_or_default();
            let destinations: Vec<i64> = if members.is_empty() {
                targets
                    .iter()
                    .filter_map(|target| index.file_nodes.get(target).copied())
                    .collect()
            } else {
                members
            };
            let confidence = resolution_confidence(destinations.len(), Confidence::Extracted);
            for dst in destinations {
                if dst == file_id {
                    continue;
                }
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
    pending: &[(String, CallRef)],
    index: &SymbolIndex<'_>,
    summary: &mut IndexSummary,
) -> Result<()> {
    for (file_path, call) in pending {
        let Some(&src) = index
            .by_file_name
            .get(&(file_path.clone(), call.caller.clone()))
        else {
            continue;
        };
        let narrowed = narrow_call(index, file_path, call);
        let confidence = resolution_confidence(narrowed.len(), Confidence::Inferred);
        for (dst, _) in narrowed {
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

/// Picks the nodes a call site can reach, best evidence first.
///
/// Each rung uses information a bare name match throws away, so a repo full
/// of same-named `run`s still resolves to one edge:
///
/// 1. The receiver's type is known — from `self`/`this`, from a typed local
///    (`let s = Store::new()`), or because the receiver *is* a type
///    (`Graph::open`) — so only members that type declares can match.
/// 2. The receiver is a name the file imported (`store.get()` where `store`
///    came from `./store`) — look the callee up in that module.
/// 3. The callee itself is an imported name, possibly under an alias.
/// 4. `self`/`this` keeps the call inside the caller's own file.
/// 5. A module-qualified call (`bigbang::run`) picks the matching file.
/// 6. A wildcard import brings the whole module's surface into scope.
/// 7. A definition in the caller's own file beats a same-named one elsewhere.
/// 8. Otherwise every candidate, AMBIGUOUS when more than one — the honest
///    fallback, since over-warning on impact beats missing a caller.
fn narrow_call(index: &SymbolIndex<'_>, file_path: &str, call: &CallRef) -> Vec<(i64, String)> {
    if let Some(owner_type) = receiver_type(index, file_path, call) {
        let hits: Vec<(i64, String)> = index
            .candidates(&call.callee)
            .iter()
            .filter(|(_, file)| index.declares(file, owner_type, &call.callee))
            .cloned()
            .collect();
        if !hits.is_empty() {
            return hits;
        }
    }

    if let Some(receiver) = call.receiver.as_deref()
        && let Some(binding) = binding_keys(receiver)
            .into_iter()
            .find_map(|key| index.bindings.lookup(file_path, key))
    {
        let hits = in_files(index.candidates(&call.callee), &binding.files);
        if !hits.is_empty() {
            return hits;
        }
    }

    if let Some(binding) = index.bindings.lookup(file_path, &call.callee) {
        let target = binding.symbol.as_deref().unwrap_or(&call.callee);
        let hits = in_files(index.candidates(target), &binding.files);
        if !hits.is_empty() {
            return hits;
        }
    }

    let candidates = index.candidates(&call.callee);
    if candidates.is_empty() {
        return Vec::new();
    }

    let same_file: Vec<(i64, String)> = candidates
        .iter()
        .filter(|(_, file)| file == file_path)
        .cloned()
        .collect();
    if is_self_receiver(call.receiver.as_deref()) {
        if !same_file.is_empty() {
            return same_file;
        }
    } else if let Some(receiver) = call.receiver.as_deref()
        && let Some(by_module) = Some(narrow_by_module(candidates, module_qualifier(receiver)))
            .filter(|hits| hits.len() == 1 && hits.len() < candidates.len())
    {
        return by_module;
    }

    let globs = index.bindings.glob_targets(file_path);
    if !globs.is_empty() {
        let hits = in_files(candidates, globs);
        if hits.len() == 1 {
            return hits;
        }
    }

    if same_file.len() == 1 {
        return same_file;
    }
    candidates.to_vec()
}

/// The type a call was made on, when the syntax settles it: `self`/`this`
/// is the caller's own type, a local declared by construction carries its
/// constructed type, and a receiver that is itself a declared type name is
/// a static/associated call on that type.
fn receiver_type<'i>(
    index: &'i SymbolIndex<'_>,
    file_path: &str,
    call: &'i CallRef,
) -> Option<&'i str> {
    let receiver = call.receiver.as_deref()?;
    if is_self_receiver(Some(receiver)) {
        return call.caller_type.as_deref();
    }
    if receiver.contains(['.', ':', '(', '[']) {
        // A chain (`self.db.query()`, `a::b::c()`) needs real type flow;
        // the later rungs handle it rather than guessing here.
        return None;
    }
    if let Some(local) = index.locals.get(&(
        file_path.to_string(),
        call.caller.clone(),
        receiver.to_string(),
    )) {
        return Some(local);
    }
    index.type_names.get(receiver).map(String::as_str)
}

/// Candidates declared in one of `files`.
fn in_files(candidates: &[(i64, String)], files: &[String]) -> Vec<(i64, String)> {
    candidates
        .iter()
        .filter(|(_, file)| files.iter().any(|target| target == file))
        .cloned()
        .collect()
}

/// Names a receiver expression could have been bound under: the head of a
/// member chain (`store` in `store.items`) and the tail of a module path
/// (`sync` in `crate::sync`).
fn binding_keys(receiver: &str) -> Vec<&str> {
    let head = receiver
        .split(['.', ':', '-', '>', '('])
        .next()
        .unwrap_or(receiver)
        .trim();
    let tail = receiver.rsplit("::").next().unwrap_or(receiver).trim();
    let mut keys = Vec::new();
    if !head.is_empty() {
        keys.push(head);
    }
    if !tail.is_empty() && tail != head {
        keys.push(tail);
    }
    keys
}

/// Whether a call was made on the enclosing instance.
fn is_self_receiver(receiver: Option<&str>) -> bool {
    receiver.is_some_and(|receiver| {
        matches!(receiver, "self" | "this" | "Self" | "@")
            || receiver.starts_with("self.")
            || receiver.starts_with("this.")
    })
}

/// Links each handler to the endpoint its framework registration serves.
///
/// The registration is explicit code, so a handler that resolves in its own
/// file is EXTRACTED. A route whose handler is an inline closure still gets
/// an endpoint node — the endpoint exists whether or not a named symbol
/// serves it.
fn resolve_routes(
    graph: &Graph,
    pending: &[(String, Option<String>, String)],
    index: &SymbolIndex<'_>,
    summary: &mut IndexSummary,
) -> Result<()> {
    for (file_path, handler, endpoint) in pending {
        let Some(&dst) = index
            .by_file_name
            .get(&(file_path.clone(), endpoint.clone()))
        else {
            continue;
        };
        let Some(handler) = handler else { continue };
        let Some(&src) = index
            .by_file_name
            .get(&(file_path.clone(), handler.clone()))
        else {
            continue;
        };
        graph.insert_edge_with_provenance(
            &Edge {
                src,
                dst,
                kind: EdgeKind::Implements,
                confidence: Confidence::Extracted,
            },
            &Provenance {
                perspective: Perspective::Observed,
                evidence_kind: EvidenceKind::AstCall,
                evidence_source: Some(file_path.clone()),
            },
        )?;
        summary.edges += 1;
    }
    Ok(())
}

/// Turns `class A extends B` / `impl Trait for Type` into graph edges.
///
/// The declaration is explicit in source, so a uniquely resolved parent is
/// EXTRACTED — the uncertainty is only ever *which* same-named type was
/// meant, never whether the relation exists.
fn resolve_inheritance(
    graph: &Graph,
    pending: &[(String, InheritRef)],
    index: &SymbolIndex<'_>,
    summary: &mut IndexSummary,
) -> Result<()> {
    for (file_path, inherit) in pending {
        let Some(&src) = index
            .by_file_name
            .get(&(file_path.clone(), inherit.child.clone()))
        else {
            continue;
        };
        let candidates = index.candidates(&inherit.parent);
        if candidates.is_empty() {
            continue;
        }
        let narrowed = index
            .bindings
            .lookup(file_path, &inherit.parent)
            .map(|binding| {
                let target = binding.symbol.as_deref().unwrap_or(&inherit.parent);
                in_files(index.candidates(target), &binding.files)
            })
            .filter(|hits| !hits.is_empty())
            .or_else(|| {
                let same_file: Vec<(i64, String)> = candidates
                    .iter()
                    .filter(|(_, file)| file == file_path)
                    .cloned()
                    .collect();
                (same_file.len() == 1).then_some(same_file)
            })
            .unwrap_or_else(|| candidates.to_vec());
        let confidence = resolution_confidence(narrowed.len(), Confidence::Extracted);
        for (dst, _) in narrowed {
            if dst == src {
                continue;
            }
            graph.insert_edge(&Edge {
                src,
                dst,
                kind: if inherit.implements {
                    EdgeKind::Implements
                } else {
                    EdgeKind::Inherits
                },
                confidence,
            })?;
            summary.edges += 1;
        }
    }
    Ok(())
}

/// The module a qualified call was routed through — the last real segment
/// of the receiver path (`crate::bigbang` → `bigbang`). `None` when the
/// receiver names no module of its own (`self`, `crate`, `super`).
fn module_qualifier(receiver: &str) -> Option<&str> {
    let qualifier = receiver.rsplit("::").next().map(str::trim)?;
    if qualifier.is_empty() || matches!(qualifier, "self" | "crate" | "super" | "Self") {
        return None;
    }
    Some(qualifier)
}

/// Candidates whose file corresponds to `qualifier` (`sync` matches
/// `src/sync.rs`, `sync/mod.rs`, or anything under a `sync/` directory).
/// No qualifier, or nothing matching, returns everything — narrowing must
/// never drop a real candidate, only prefer a provably better one.
fn narrow_by_module(candidates: &[(i64, String)], qualifier: Option<&str>) -> Vec<(i64, String)> {
    let Some(qualifier) = qualifier else {
        return candidates.to_vec();
    };
    let matched: Vec<(i64, String)> = candidates
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
        .cloned()
        .collect();
    if matched.is_empty() {
        candidates.to_vec()
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
    fn external_import_no_longer_matches_a_same_named_local_type() {
        let root = scratch_root();
        fs::write(root.join("a.rs"), "use std::fs::File;\nfn caller() {}").unwrap();
        fs::write(root.join("b.rs"), "struct File;").unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        let local = graph.find_by_name("File").unwrap().unwrap();
        assert!(
            graph.callers(local.id.unwrap()).unwrap().is_empty(),
            "`use std::fs::File` must not link to the repo's own File"
        );
    }

    #[test]
    fn import_binding_resolves_a_call_that_name_matching_would_split() {
        let root = scratch_root();
        fs::write(
            root.join("app.js"),
            "import { run } from './engine.js';\nfunction go() { run(); }",
        )
        .unwrap();
        fs::write(root.join("engine.js"), "export function run() {}").unwrap();
        fs::write(root.join("other.js"), "export function run() {}").unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        for node in graph.search("run", 10).unwrap() {
            let callers = graph.callers(node.id.unwrap()).unwrap();
            let called_by_go = callers
                .iter()
                .any(|(caller, kind, _)| caller.name == "go" && *kind == EdgeKind::Calls);
            if node.file_path == "engine.js" {
                assert!(called_by_go, "the import says `run` lives in engine.js");
                assert!(
                    callers
                        .iter()
                        .any(|(_, kind, confidence)| *kind == EdgeKind::Calls
                            && *confidence == Confidence::Inferred)
                );
            } else {
                assert!(!called_by_go, "other.js's run was never imported");
            }
        }
    }

    #[test]
    fn aliased_import_resolves_to_the_original_symbol() {
        let root = scratch_root();
        fs::write(
            root.join("app.js"),
            "import { run as go } from './engine.js';\nfunction start() { go(); }",
        )
        .unwrap();
        fs::write(root.join("engine.js"), "export function run() {}").unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        let run = graph.find_by_name("run").unwrap().unwrap();
        let callers = graph.callers(run.id.unwrap()).unwrap();
        assert!(
            callers
                .iter()
                .any(|(caller, kind, _)| caller.name == "start" && *kind == EdgeKind::Calls),
            "the alias `go` must resolve back to `run` in engine.js"
        );
    }

    #[test]
    fn namespace_receiver_resolves_the_member_call() {
        let root = scratch_root();
        fs::write(
            root.join("app.js"),
            "import * as engine from './engine.js';\nfunction go() { engine.run(); }",
        )
        .unwrap();
        fs::write(root.join("engine.js"), "export function run() {}").unwrap();
        fs::write(root.join("other.js"), "export function run() {}").unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        for node in graph.search("run", 10).unwrap() {
            let called_by_go = graph
                .callers(node.id.unwrap())
                .unwrap()
                .iter()
                .any(|(caller, _, _)| caller.name == "go");
            assert_eq!(
                called_by_go,
                node.file_path == "engine.js",
                "`engine.run()` must resolve through the namespace import"
            );
        }
    }

    #[test]
    fn class_extends_becomes_an_inherits_edge() {
        let root = scratch_root();
        fs::write(
            root.join("store.js"),
            "import { Base } from './base.js';\nclass Store extends Base {}",
        )
        .unwrap();
        fs::write(root.join("base.js"), "export class Base {}").unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        let base = graph.find_by_name("Base").unwrap().unwrap();
        let parents = graph.callers(base.id.unwrap()).unwrap();
        assert!(
            parents
                .iter()
                .any(|(node, kind, confidence)| node.name == "Store"
                    && *kind == EdgeKind::Inherits
                    && *confidence == Confidence::Extracted),
            "expected Store -[inherits]-> Base, got {parents:?}"
        );
    }

    #[test]
    fn impl_of_trait_becomes_an_implements_edge() {
        let root = scratch_root();
        fs::write(root.join("shape.rs"), "trait Draw {}").unwrap();
        fs::write(
            root.join("square.rs"),
            "use crate::shape::Draw;\nstruct Square;\nimpl Draw for Square {}",
        )
        .unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        let draw = graph.find_by_name("Draw").unwrap().unwrap();
        let implementers = graph.callers(draw.id.unwrap()).unwrap();
        assert!(
            implementers
                .iter()
                .any(|(node, kind, _)| node.name == "Square" && *kind == EdgeKind::Implements),
            "expected Square -[implements]-> Draw, got {implementers:?}"
        );
    }

    #[test]
    fn python_relative_import_binds_the_call() {
        let root = scratch_root();
        fs::write(
            root.join("app.py"),
            "from .engine import run\n\ndef go():\n    run()\n",
        )
        .unwrap();
        fs::write(root.join("engine.py"), "def run():\n    pass\n").unwrap();
        fs::write(root.join("other.py"), "def run():\n    pass\n").unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        for node in graph.search("run", 10).unwrap() {
            let called_by_go = graph
                .callers(node.id.unwrap())
                .unwrap()
                .iter()
                .any(|(caller, _, _)| caller.name == "go");
            assert_eq!(called_by_go, node.file_path == "engine.py");
        }
    }

    /// Every `(caller, callee file)` pair for a callee name — the shape most
    /// resolution assertions want.
    fn call_targets(graph: &Graph, callee: &str) -> Vec<(String, String)> {
        let mut found = Vec::new();
        for node in graph.search(callee, 20).unwrap() {
            if node.name != callee {
                continue;
            }
            for (caller, kind, _) in graph.callers(node.id.unwrap()).unwrap() {
                if kind == EdgeKind::Calls {
                    found.push((caller.name, node.file_path.clone()));
                }
            }
        }
        found.sort_unstable();
        found
    }

    #[test]
    fn associated_call_resolves_through_the_type_that_declares_it() {
        let root = scratch_root();
        fs::write(
            root.join("app.rs"),
            "use crate::store::Store;\nfn go() { Store::open(); }",
        )
        .unwrap();
        fs::write(
            root.join("store.rs"),
            "struct Store;\nimpl Store { fn open() {} }",
        )
        .unwrap();
        fs::write(
            root.join("cache.rs"),
            "struct Cache;\nimpl Cache { fn open() {} }",
        )
        .unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        assert_eq!(
            call_targets(&graph, "open"),
            vec![("go".to_string(), "store.rs".to_string())],
            "`Store::open()` must not also link Cache::open"
        );
    }

    #[test]
    fn typed_local_resolves_the_method_call_on_it() {
        let root = scratch_root();
        fs::write(
            root.join("app.js"),
            "import { Store } from './store.js';\nfunction go() { const s = new Store(); s.get(); }",
        )
        .unwrap();
        fs::write(root.join("store.js"), "export class Store { get() {} }").unwrap();
        fs::write(root.join("cache.js"), "export class Cache { get() {} }").unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        assert_eq!(
            call_targets(&graph, "get"),
            vec![("go".to_string(), "store.js".to_string())],
            "`s` was constructed as a Store, so `s.get()` is Store's get"
        );
    }

    #[test]
    fn self_call_resolves_through_the_enclosing_type() {
        let root = scratch_root();
        fs::write(
            root.join("store.rs"),
            "struct Store;\nimpl Store { fn run(&self) { self.step(); } fn step(&self) {} }",
        )
        .unwrap();
        fs::write(
            root.join("cache.rs"),
            "struct Cache;\nimpl Cache { fn step(&self) {} }",
        )
        .unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        assert_eq!(
            call_targets(&graph, "step"),
            vec![("run".to_string(), "store.rs".to_string())]
        );
    }

    #[test]
    fn tsconfig_path_alias_resolves_the_import() {
        let root = scratch_root();
        fs::create_dir_all(root.join("src/lib")).unwrap();
        fs::write(
            root.join("tsconfig.json"),
            r#"{"compilerOptions": {"baseUrl": ".", "paths": {"@lib/*": ["src/lib/*"]}}}"#,
        )
        .unwrap();
        fs::write(
            root.join("src/app.ts"),
            "import { format } from '@lib/format';\nfunction go(): void { format(); }",
        )
        .unwrap();
        fs::write(
            root.join("src/lib/format.ts"),
            "export function format(): void {}",
        )
        .unwrap();
        fs::write(root.join("other.ts"), "export function format(): void {}").unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        assert_eq!(
            call_targets(&graph, "format"),
            vec![("go".to_string(), "src/lib/format.ts".to_string())],
            "`@lib/*` is declared by tsconfig.json and must resolve"
        );
    }

    #[test]
    fn go_module_prefix_resolves_the_package_import() {
        let root = scratch_root();
        fs::create_dir_all(root.join("internal/store")).unwrap();
        fs::write(root.join("go.mod"), "module github.com/acme/app\n").unwrap();
        fs::write(
            root.join("main.go"),
            "package main\nimport \"github.com/acme/app/internal/store\"\nfunc run() { store.Open() }\n",
        )
        .unwrap();
        fs::write(
            root.join("internal/store/store.go"),
            "package store\nfunc Open() {}\n",
        )
        .unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        assert_eq!(
            call_targets(&graph, "Open"),
            vec![("run".to_string(), "internal/store/store.go".to_string())]
        );
    }

    #[test]
    fn workspace_package_name_resolves_across_packages() {
        let root = scratch_root();
        fs::create_dir_all(root.join("packages/utils/src")).unwrap();
        fs::create_dir_all(root.join("apps/web/src")).unwrap();
        fs::write(
            root.join("packages/utils/package.json"),
            r#"{"name": "@acme/utils", "main": "src/index.js"}"#,
        )
        .unwrap();
        fs::write(
            root.join("packages/utils/src/index.js"),
            "export function slugify() {}",
        )
        .unwrap();
        fs::write(
            root.join("apps/web/src/app.js"),
            "import { slugify } from '@acme/utils';\nfunction go() { slugify(); }",
        )
        .unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        assert_eq!(
            call_targets(&graph, "slugify"),
            vec![("go".to_string(), "packages/utils/src/index.js".to_string())]
        );
    }

    /// `(endpoint name, handler)` pairs the graph observed in code.
    fn observed_routes(graph: &Graph) -> Vec<(String, String)> {
        let mut found = Vec::new();
        for (node, provenance) in graph.all_nodes_with_provenance().unwrap() {
            if node.kind != NodeKind::Endpoint || provenance.perspective != Perspective::Observed {
                continue;
            }
            for (handler, kind, _) in graph.callers(node.id.unwrap()).unwrap() {
                if kind == EdgeKind::Implements {
                    found.push((node.name.clone(), handler.name));
                }
            }
        }
        found.sort_unstable();
        found
    }

    #[test]
    fn express_route_registration_becomes_an_observed_endpoint() {
        let root = scratch_root();
        fs::write(
            root.join("server.js"),
            "function listPets() {}\nfunction addPet() {}\nfunction wire(app) { app.get('/pets', listPets); app.post('/pets', addPet); }\n",
        )
        .unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        assert_eq!(
            observed_routes(&graph),
            vec![
                ("GET /pets".to_string(), "listPets".to_string()),
                ("POST /pets".to_string(), "addPet".to_string())
            ]
        );
    }

    #[test]
    fn a_tool_definition_becomes_an_observed_endpoint_with_its_handler() {
        let root = scratch_root();
        fs::write(
            root.join("server.js"),
            "function searchDocs() {}\nfunction wire(server) { server.tool('search', searchDocs); }\n",
        )
        .unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        assert_eq!(
            observed_routes(&graph),
            vec![("TOOL search".to_string(), "searchDocs".to_string())]
        );
    }

    #[test]
    fn a_tool_table_entry_is_a_tool_even_without_a_handler() {
        let root = scratch_root();
        fs::write(
            root.join("mcp.rs"),
            "struct ToolSpec { name: &'static str }\nconst SPECS: &[ToolSpec] = &[ToolSpec { name: \"explore\" }];\n",
        )
        .unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        let tools: Vec<String> = graph
            .all_nodes()
            .unwrap()
            .into_iter()
            .filter(|node| node.kind == NodeKind::Endpoint)
            .map(|node| node.name)
            .collect();
        assert_eq!(tools, vec!["TOOL explore".to_string()]);
    }

    #[test]
    fn an_outbound_call_is_linked_to_the_endpoint_it_requests() {
        let root = scratch_root();
        fs::write(
            root.join("server.js"),
            "function listPets() {}\nfunction wire(app) { app.get('/pets', listPets); }\n",
        )
        .unwrap();
        fs::write(
            root.join("client.js"),
            "function loadPets() { return fetch('/pets'); }\n",
        )
        .unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        let endpoint = graph.find_by_name("GET /pets").unwrap().unwrap();
        let callers = graph.callers(endpoint.id.unwrap()).unwrap();
        assert!(
            callers
                .iter()
                .any(|(node, kind, confidence)| node.name == "loadPets"
                    && *kind == EdgeKind::Calls
                    && *confidence == Confidence::Extracted),
            "the literal path matches the endpoint exactly: {callers:?}"
        );
    }

    #[test]
    fn a_parameterized_path_matches_the_route_that_declares_the_parameter() {
        let root = scratch_root();
        fs::write(
            root.join("server.js"),
            "function getPet() {}\nfunction wire(app) { app.get('/pets/:id', getPet); }\n",
        )
        .unwrap();
        fs::write(
            root.join("client.js"),
            "function loadPet(client) { return client.get('/pets/42'); }\n",
        )
        .unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        let endpoint = graph.find_by_name("GET /pets/:id").unwrap().unwrap();
        let callers = graph.callers(endpoint.id.unwrap()).unwrap();
        assert!(
            callers
                .iter()
                .any(|(node, _, confidence)| node.name == "loadPet"
                    && *confidence == Confidence::Inferred),
            "a match that needs the parameter flattened is INFERRED, not EXTRACTED: {callers:?}"
        );
    }

    #[test]
    fn an_event_publisher_is_linked_to_every_listener_of_that_name() {
        let root = scratch_root();
        fs::write(
            root.join("producer.js"),
            "function placeOrder(bus) { bus.emit('order.created', {}); }\n",
        )
        .unwrap();
        fs::write(
            root.join("consumer.js"),
            "function sendReceipt(bus) { bus.on('order.created', () => {}); }\n             function updateStock(bus) { bus.subscribe('order.created', () => {}); }\n             function unrelated(bus) { bus.on('user.created', () => {}); }\n",
        )
        .unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        let publisher = graph.find_by_name("placeOrder").unwrap().unwrap();
        let listeners: Vec<(String, Confidence)> = graph
            .callees(publisher.id.unwrap())
            .unwrap()
            .into_iter()
            .filter(|(_, kind, _)| *kind == EdgeKind::References)
            .map(|(node, _, confidence)| (node.name, confidence))
            .collect();
        assert!(
            listeners
                .iter()
                .any(|(name, confidence)| name == "sendReceipt"
                    && *confidence == Confidence::Inferred),
            "{listeners:?}"
        );
        assert!(
            listeners.iter().any(|(name, _)| name == "updateStock"),
            "an event with two listeners has two links: {listeners:?}"
        );
        assert!(
            !listeners.iter().any(|(name, _)| name == "unrelated"),
            "a different event name is a different event: {listeners:?}"
        );
    }

    #[test]
    fn a_tool_invocation_is_linked_to_the_tool_it_names() {
        let root = scratch_root();
        fs::write(
            root.join("server.js"),
            "function searchDocs() {}\nfunction wire(server) { server.tool('search', searchDocs); }\n",
        )
        .unwrap();
        fs::write(
            root.join("agent.js"),
            "function ask(mcp) { return mcp.call_tool('search'); }\n",
        )
        .unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        let tool = graph.find_by_name("TOOL search").unwrap().unwrap();
        let callers = graph.callers(tool.id.unwrap()).unwrap();
        assert!(
            callers
                .iter()
                .any(|(node, kind, _)| node.name == "ask" && *kind == EdgeKind::Calls),
            "a dispatcher hides this link; the call site is what reveals it: {callers:?}"
        );
    }

    #[test]
    fn an_incremental_file_sync_keeps_the_endpoints_that_file_registers() {
        let root = scratch_root();
        fs::write(
            root.join("server.js"),
            "function listPets() {}\nfunction wire(app) { app.get('/pets', listPets); }\n",
        )
        .unwrap();
        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();
        assert_eq!(observed_routes(&graph).len(), 1);

        // The same file changes and is reindexed on its own, which is what the
        // post-edit hook does on every keystroke-sized edit.
        fs::write(
            root.join("server.js"),
            "function listPets() {}\nfunction addPet() {}\nfunction wire(app) { app.get('/pets', listPets); app.post('/pets', addPet); }\n",
        )
        .unwrap();
        index_file(&graph, &root, &root.join("server.js")).unwrap();

        assert_eq!(
            observed_routes(&graph),
            vec![
                ("GET /pets".to_string(), "listPets".to_string()),
                ("POST /pets".to_string(), "addPet".to_string())
            ],
            "an endpoint is not collateral damage of reindexing its own file"
        );
    }

    #[test]
    fn a_route_registration_is_not_mistaken_for_a_consumer() {
        let root = scratch_root();
        fs::write(
            root.join("server.js"),
            "function wire(app) { app.get('/pets', (req, res) => res.json([])); }\n",
        )
        .unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        let endpoint = graph.find_by_name("GET /pets").unwrap().unwrap();
        assert!(
            graph.callers(endpoint.id.unwrap()).unwrap().is_empty(),
            "`app.get` with an inline handler serves the route, it does not consume it"
        );
    }

    #[test]
    fn http_client_call_is_not_mistaken_for_a_route() {
        let root = scratch_root();
        fs::write(
            root.join("client.js"),
            "function load(http) { return http.get('/pets'); }\n",
        )
        .unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        assert!(observed_routes(&graph).is_empty());
        assert!(
            graph
                .all_nodes()
                .unwrap()
                .iter()
                .all(|node| node.kind != NodeKind::Endpoint)
        );
    }

    #[test]
    fn inline_closure_route_gets_an_endpoint_but_no_wrong_handler() {
        let root = scratch_root();
        fs::write(
            root.join("server.js"),
            "function wire(app) { app.get('/pets', (req, res) => res.send()); }\nfunction unrelated() {}\n",
        )
        .unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        assert!(
            graph
                .all_nodes()
                .unwrap()
                .iter()
                .any(|node| node.kind == NodeKind::Endpoint && node.name == "GET /pets"),
            "the endpoint exists even with an anonymous handler"
        );
        assert!(
            observed_routes(&graph).is_empty(),
            "an inline closure must not be attributed to the next declaration"
        );
    }

    #[test]
    fn python_decorator_route_attaches_to_the_function_below_it() {
        let root = scratch_root();
        fs::write(
            root.join("api.py"),
            "@app.get(\"/pets\")\ndef list_pets():\n    pass\n",
        )
        .unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        assert_eq!(
            observed_routes(&graph),
            vec![("GET /pets".to_string(), "list_pets".to_string())]
        );
    }

    #[test]
    fn axum_route_registration_names_its_handler() {
        let root = scratch_root();
        fs::write(
            root.join("server.rs"),
            "fn list_pets() {}\nfn build() { Router::new().route(\"/pets\", get(list_pets)); }\n",
        )
        .unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        assert_eq!(
            observed_routes(&graph),
            vec![("GET /pets".to_string(), "list_pets".to_string())]
        );
    }

    #[test]
    fn spring_annotation_route_attaches_to_its_method() {
        let root = scratch_root();
        fs::write(
            root.join("PetController.java"),
            "class PetController {\n  @GetMapping(\"/pets\")\n  public void listPets() {}\n}\n",
        )
        .unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();

        assert_eq!(
            observed_routes(&graph),
            vec![("GET /pets".to_string(), "listPets".to_string())]
        );
    }

    #[test]
    fn detected_entrypoints_root_the_traced_processes() {
        let root = scratch_root();
        fs::write(
            root.join("main.rs"),
            "fn main() { start(); }\nfn start() { helper(); }\nfn helper() {}\nfn unused_public() { helper(); }\n",
        )
        .unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();
        let nodes = graph.all_nodes().unwrap();
        let edges = graph.all_edges().unwrap();
        let by_id: HashMap<i64, &Node> = nodes
            .iter()
            .filter_map(|node| node.id.map(|id| (id, node)))
            .collect();

        let roots: Vec<&str> = crate::analysis::processes(&nodes, &edges)
            .iter()
            .filter_map(|process| {
                by_id
                    .get(&process.entrypoint)
                    .map(|node| node.name.as_str())
            })
            .collect();
        assert_eq!(
            roots,
            vec!["main"],
            "`main` is the entrypoint; `unused_public` merely has no callers"
        );
    }

    #[test]
    fn incremental_rebuild_preserves_binding_resolution() {
        let root = scratch_root();
        fs::write(
            root.join("app.js"),
            "import { run } from './engine.js';\nfunction go() { run(); }",
        )
        .unwrap();
        fs::write(root.join("engine.js"), "export function run() {}").unwrap();
        fs::write(root.join("other.js"), "export function run() {}").unwrap();

        let graph = Graph::open_in_memory().unwrap();
        index_repo(&graph, &root).unwrap();
        // Reindexing one untouched file must re-resolve from persisted
        // references without losing what the import statement established.
        index_file(&graph, &root, &root.join("other.js")).unwrap();

        for node in graph.search("run", 10).unwrap() {
            let called_by_go = graph
                .callers(node.id.unwrap())
                .unwrap()
                .iter()
                .any(|(caller, _, _)| caller.name == "go");
            assert_eq!(called_by_go, node.file_path == "engine.js");
        }
    }
}
