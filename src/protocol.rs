//! Compiler and validator for the portable AAG Agent Protocol contract.
//!
//! `.aag/graph.db` remains the source of truth. The context manifest is a
//! deterministic interchange projection that can be consumed independently of
//! AAG's parser, storage engine, or query API.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::storage::{Confidence, Edge, EdgeKind, Graph, Node, NodeKind, Perspective, Provenance};

/// Exact AAG Protocol revision bundled into this binary.
pub const VERSION: &str = "0.2.0-proposal";

const MANIFEST_SCHEMA: &str = include_str!("../protocol/aag.manifest.schema.json");

/// Compile an existing repository graph into a context manifest.
///
/// # Errors
///
/// Returns an error if the repository has no index, the previous manifest is
/// malformed, compilation fails validation, or the output cannot be written.
pub fn run_export(root: &Path, output: Option<&Path>) -> Result<PathBuf> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let graph = Graph::open_existing(&root)?;
    let nodes = graph.all_nodes_with_provenance()?;
    let edges = graph.all_edges_with_provenance()?;
    let output = output.map_or_else(|| root.join(".aag/context.yaml"), Path::to_path_buf);
    write_manifest_with_provenance(&root, &output, &nodes, &edges)?;
    Ok(output)
}

/// Validate a YAML or JSON context manifest with the bundled contract.
///
/// # Errors
///
/// Returns a protocol diagnostic for malformed, schema-invalid, or
/// semantically-invalid input.
pub fn run_validate(path: &Path) -> Result<()> {
    let manifest = read_yaml(path)?;
    validate_manifest(&manifest)
}

/// Compile and write a manifest from an already-loaded graph snapshot.
///
/// Existing human-owned declarations and shared annotations are preserved;
/// tool-owned observations are regenerated from the graph.
///
/// # Errors
///
/// Returns an error if an existing manifest cannot be read, the new manifest
/// is not conforming, or the output cannot be written.
pub fn write_manifest(root: &Path, output: &Path, nodes: &[Node], edges: &[Edge]) -> Result<()> {
    let nodes = nodes
        .iter()
        .cloned()
        .map(|node| {
            let provenance = Provenance::for_node(&node);
            (node, provenance)
        })
        .collect::<Vec<_>>();
    let edges = edges
        .iter()
        .copied()
        .map(|edge| {
            let provenance = Provenance::for_edge(&edge);
            (edge, provenance)
        })
        .collect::<Vec<_>>();
    write_manifest_with_provenance(root, output, &nodes, &edges)
}

/// Compile a manifest while preserving declared-versus-observed provenance.
///
/// # Errors
/// Returns an error if compilation, validation, or writing fails.
pub fn write_manifest_with_provenance(
    root: &Path,
    output: &Path,
    nodes: &[(Node, Provenance)],
    edges: &[(Edge, Provenance)],
) -> Result<()> {
    let previous = output.is_file().then(|| read_yaml(output)).transpose()?;
    let plain_nodes = nodes
        .iter()
        .map(|(node, _)| node.clone())
        .collect::<Vec<_>>();
    let plain_edges = edges.iter().map(|(edge, _)| *edge).collect::<Vec<_>>();
    let manifest = build_manifest(
        root,
        &plain_nodes,
        &plain_edges,
        previous.as_ref(),
        Some(nodes),
        Some(edges),
    );
    validate_manifest(&manifest)?;
    let yaml = serde_yaml_ng::to_string(&manifest).map_err(|source| Error::Protocol {
        context: "serialization failed",
        detail: source.to_string(),
    })?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|source| Error::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(output, yaml).map_err(|source| Error::Write {
        path: output.to_path_buf(),
        source,
    })
}

fn read_yaml(path: &Path) -> Result<Value> {
    let text = fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_yaml_ng::from_str(&text).map_err(|source| Error::Protocol {
        context: "manifest parse failed",
        detail: source.to_string(),
    })
}

#[allow(clippy::too_many_lines)]
fn build_manifest(
    root: &Path,
    nodes: &[Node],
    edges: &[Edge],
    previous: Option<&Value>,
    node_provenance: Option<&[(Node, Provenance)]>,
    edge_provenance: Option<&[(Edge, Provenance)]>,
) -> Value {
    let revision = git(root, &["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let changed_files = changed_files(root);
    let dirty = !changed_files.is_empty();
    let timestamp = now_rfc3339();
    let id_by_row: HashMap<i64, String> = nodes
        .iter()
        .filter_map(|node| node.id.map(|row| (row, entity_id(node))))
        .collect();

    let observed_nodes = node_provenance.map_or_else(
        || nodes.iter().collect::<Vec<_>>(),
        |items| {
            items
                .iter()
                .filter(|(_, p)| p.perspective == Perspective::Observed)
                .map(|(n, _)| n)
                .collect()
        },
    );
    let declared_nodes = node_provenance.map_or_else(Vec::new, |items| {
        items
            .iter()
            .filter(|(_, p)| {
                p.perspective == Perspective::Declared
                    && p.evidence_kind.as_str() == "openapi_contract"
            })
            .collect::<Vec<_>>()
    });
    let declared_artifacts = node_provenance.map_or_else(Vec::new, |items| {
        items
            .iter()
            .filter(|(_, provenance)| {
                provenance.perspective == Perspective::Declared
                    && matches!(
                        provenance.evidence_kind,
                        crate::storage::EvidenceKind::SqlSchema
                            | crate::storage::EvidenceKind::Infrastructure
                    )
            })
            .collect::<Vec<_>>()
    });
    let entities = dedupe_by_id(
        observed_nodes
            .into_iter()
            .map(|node| entity(node, &revision)),
    );

    let relationships = dedupe_by_id(
        edges
            .iter()
            .filter(|edge| {
                edge_provenance.is_none_or(|items| {
                    items.iter().any(|(candidate, p)| {
                        candidate == *edge && p.perspective == Perspective::Observed
                    })
                })
            })
            .filter_map(|edge| relationship(edge, &id_by_row, nodes, &revision)),
    );
    let flows = detected_flows(nodes, edges, &id_by_row, &revision);

    let mut uncertainties = preserved_items(previous, "/uncertainties");
    uncertainties.extend(
        edges
            .iter()
            .filter(|edge| {
                edge_provenance.is_none_or(|items| {
                    items.iter().any(|(candidate, provenance)| {
                        candidate == *edge && provenance.perspective == Perspective::Observed
                    })
                })
            })
            .filter(|edge| edge.confidence == Confidence::Ambiguous)
            .filter_map(|edge| uncertainty(edge, &id_by_row, nodes, &revision)),
    );
    uncertainties.sort_by(|left, right| value_id(left).cmp(value_id(right)));
    uncertainties.dedup_by(|left, right| value_id(left) == value_id(right));

    let files_analyzed = nodes
        .iter()
        .filter(|node| node.kind == NodeKind::File)
        .count();
    let repo_name = root.file_name().map_or_else(
        || "repository".into(),
        |name| name.to_string_lossy().into_owned(),
    );
    let freshness = if dirty || revision == "unknown" {
        "unknown"
    } else {
        "current"
    };

    let mut extensions = merged_extensions(previous);
    if !declared_nodes.is_empty() {
        extensions["x-aag-declared-contracts"] = json!({
            "source": "openapi",
            "entities": declared_nodes.into_iter().map(|(node, provenance)| json!({
                "id": entity_id(node),
                "kind": entity_type(node.kind),
                "name": node.name,
                "location": location(node),
                "evidence_type": provenance.evidence_kind.as_str(),
                "evidence_source": provenance.evidence_source,
                "contract": node.description.as_deref().and_then(|value| serde_json::from_str::<Value>(value).ok()),
                "implementation_status": if node.kind != NodeKind::Endpoint { "not_applicable" } else if edges.iter().any(|edge| edge.dst == node.id.unwrap_or_default() && edge.kind == EdgeKind::Implements) { "matched" } else { "unmatched" }
            })).collect::<Vec<_>>(),
            "relationships": edge_provenance.into_iter().flatten()
                .filter(|(_, provenance)| provenance.perspective == Perspective::Declared && provenance.evidence_kind.as_str() == "openapi_contract")
                .filter_map(|(edge, _)| Some(json!({
                    "source_id": id_by_row.get(&edge.src)?,
                    "type": relationship_type(edge.kind),
                    "target_id": id_by_row.get(&edge.dst)?,
                    "confidence": confidence(edge.confidence)
                }))).collect::<Vec<_>>()
        });
    }
    if !declared_artifacts.is_empty() {
        extensions["x-aag-declared-artifacts"] = json!({
            "entities": declared_artifacts.into_iter().map(|(node, provenance)| json!({
                "id": entity_id(node),
                "kind": entity_type(node.kind),
                "name": node.name,
                "location": location(node),
                "evidence_type": provenance.evidence_kind.as_str(),
                "evidence_source": provenance.evidence_source,
                "declaration": node.description
            })).collect::<Vec<_>>()
        });
    }

    json!({
        "aag_manifest": {
            "version": VERSION,
            "protocol_version": VERSION,
            "generated_at": timestamp,
            "generator": {
                "name": "AboveAllGraphs",
                "version": env!("CARGO_PKG_VERSION"),
                "implementation_url": "https://github.com/thewaifucorp/above-all-graphs",
                "capabilities": ["static_analysis", "incremental_update", "semantic_validation"]
            }
        },
        "repository": {
            "name": repo_name,
            "root": ".",
            "repository_type": "unknown",
            "languages": languages(nodes),
            "analyzed_revision": revision,
            "dirty_worktree": dirty
        },
        "freshness": {
            "status": freshness,
            "analyzed_revision": revision,
            "current_revision": revision,
            "checked_at": timestamp,
            "working_tree_changed": dirty,
            "changed_files_since_analysis": changed_files,
            "notes": if dirty { "Working-tree changes exist; the compiler cannot prove that every change is present in the index." } else { "Compiled from the current AAG graph index." }
        },
        "perspectives": {
            "declared": previous_at(previous, "/perspectives/declared").unwrap_or_else(default_declared),
            "observed": {
                "ownership": "agent",
                "inventory": {
                    "files_analyzed": files_analyzed,
                    "files_ignored": 0,
                    "entities": entities.len(),
                    "relationships": relationships.len(),
                    "ignored_regions": [".git/**", ".aag/**", "target/**", "node_modules/**", "vendor/**"]
                },
                "entrypoints": [],
                "entities": entities,
                "relationships": relationships,
                "flows": flows,
                "side_effects": [],
                "architectural_findings": []
            },
            "historical": preserved_historical(previous)
        },
        "uncertainties": uncertainties,
        "task_recipes": previous_at(previous, "/task_recipes").unwrap_or_else(default_recipes),
        "extensions": extensions
    })
}

fn detected_flows(
    nodes: &[Node],
    edges: &[Edge],
    ids: &HashMap<i64, String>,
    revision: &str,
) -> Vec<Value> {
    crate::analysis::processes(nodes, edges)
        .into_iter()
        .filter_map(|process| {
            let entrypoint_id = ids.get(&process.entrypoint)?;
            let entrypoint = node_by_row(nodes, process.entrypoint)?;
            let flow_id = stable_id("flow", entrypoint_id);
            let step_ids = process
                .steps
                .iter()
                .map(|id| stable_id("flow-step", &(&flow_id, id)))
                .collect::<Vec<_>>();
            let steps = process
                .steps
                .iter()
                .enumerate()
                .filter_map(|(index, row)| {
                    let entity_id = ids.get(row)?;
                    let mut step = json!({"id": step_ids[index], "entity_id": entity_id});
                    if let Some(next) = step_ids.get(index + 1) {
                        step["next"] = json!([next]);
                    }
                    if index > 0
                        && let Some(edge) = edges.iter().find(|edge| {
                            edge.src == process.steps[index - 1]
                                && edge.dst == *row
                                && edge.kind == EdgeKind::Calls
                        })
                    {
                        let source_id = ids.get(&edge.src)?;
                        let target_id = ids.get(&edge.dst)?;
                        step["via_relationship_id"] = json!(stable_id(
                            "relationship",
                            &(source_id, edge.kind.as_str(), target_id)
                        ));
                    }
                    Some(step)
                })
                .collect::<Vec<_>>();
            Some(json!({
                "id": flow_id,
                "entrypoint_id": entrypoint_id,
                "summary": format!("Observed call flow rooted at {}", entrypoint.name),
                "steps": steps,
                "confidence": "medium",
                "evidence": [{
                    "type": "ast_call",
                    "origin": "tool",
                    "location": location(entrypoint),
                    "revision": revision
                }]
            }))
        })
        .collect()
}

fn entity(node: &Node, revision: &str) -> Value {
    let evidence_type = if node.kind == NodeKind::Doc {
        "text_reference"
    } else {
        "ast_definition"
    };
    let mut item = json!({
        "id": entity_id(node),
        "type": entity_type(node.kind),
        "name": node.name,
        "location": location(node),
        "confidence": "high",
        "evidence": [{
            "type": evidence_type,
            "origin": "tool",
            "location": location(node),
            "revision": revision
        }]
    });
    if let Some(description) = &node.description {
        item["summary"] = json!(description);
    }
    item
}

fn relationship(
    edge: &Edge,
    ids: &HashMap<i64, String>,
    nodes: &[Node],
    revision: &str,
) -> Option<Value> {
    let source_id = ids.get(&edge.src)?;
    let target_id = ids.get(&edge.dst)?;
    let source = node_by_row(nodes, edge.src)?;
    Some(json!({
        "id": stable_id("relationship", &(source_id, edge.kind.as_str(), target_id)),
        "source_id": source_id,
        "type": relationship_type(edge.kind),
        "target_id": target_id,
        "confidence": confidence(edge.confidence),
        "evidence": [{
            "type": edge_evidence(edge.kind, edge.confidence),
            "origin": "tool",
            "location": location(source),
            "revision": revision
        }]
    }))
}

fn uncertainty(
    edge: &Edge,
    ids: &HashMap<i64, String>,
    nodes: &[Node],
    revision: &str,
) -> Option<Value> {
    let source_id = ids.get(&edge.src)?;
    let target_id = ids.get(&edge.dst)?;
    let source = node_by_row(nodes, edge.src)?;
    Some(json!({
        "id": stable_id("uncertainty", &(source_id, edge.kind.as_str(), target_id)),
        "description": format!("The {} relationship from {} to {} could not be resolved with certainty.", edge.kind.as_str(), source_id, target_id),
        "reason": "The AAG resolver classified this relationship as AMBIGUOUS.",
        "affected_entity_ids": [source_id, target_id],
        "confidence": "unresolved",
        "recommended_verification": ["Inspect the source location or confirm the runtime dispatch target."],
        "origin": "tool",
        "evidence": [{
            "type": "inferred_relationship",
            "origin": "tool",
            "location": location(source),
            "revision": revision
        }]
    }))
}

fn node_by_row(nodes: &[Node], row: i64) -> Option<&Node> {
    nodes.iter().find(|node| node.id == Some(row))
}

fn entity_id(node: &Node) -> String {
    stable_id(
        "entity",
        &(
            node.kind.as_str(),
            &node.file_path,
            &node.name,
            node.start_line,
            node.end_line,
        ),
    )
}

fn stable_id(prefix: &str, value: &impl Hash) -> String {
    let mut hasher = Fnv1a::default();
    value.hash(&mut hasher);
    format!("{prefix}:{:016x}", hasher.finish())
}

#[derive(Default)]
struct Fnv1a(u64);

impl Hasher for Fnv1a {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut hash = if self.0 == 0 {
            0xcbf2_9ce4_8422_2325
        } else {
            self.0
        };
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.0 = hash;
    }
}

fn location(node: &Node) -> Value {
    json!({
        "kind": "source",
        "file": node.file_path,
        "symbol": node.name,
        "line_start": node.start_line.max(1),
        "line_end": node.end_line.max(node.start_line).max(1)
    })
}

const fn entity_type(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::File => "file",
        NodeKind::Function => "function",
        NodeKind::Struct | NodeKind::Schema => "type",
        NodeKind::Method => "method",
        NodeKind::Interface => "interface",
        NodeKind::Doc => "x-entity-document",
        NodeKind::Endpoint => "entrypoint",
        NodeKind::DatabaseTable => "x-entity-database-table",
        NodeKind::InfraResource => "x-entity-infrastructure-resource",
    }
}

const fn relationship_type(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::Imports => "imports",
        EdgeKind::Calls => "calls",
        EdgeKind::Inherits => "extends",
        EdgeKind::Implements => "implements",
        EdgeKind::Explains => "x-relation-explains",
        EdgeKind::References => "x-relation-references-schema",
    }
}

const fn confidence(value: Confidence) -> &'static str {
    match value {
        Confidence::Extracted => "high",
        Confidence::Inferred => "medium",
        Confidence::Ambiguous => "unresolved",
    }
}

const fn edge_evidence(kind: EdgeKind, confidence: Confidence) -> &'static str {
    match confidence {
        Confidence::Ambiguous => "inferred_relationship",
        Confidence::Extracted | Confidence::Inferred => match kind {
            EdgeKind::Imports => "ast_import",
            EdgeKind::Calls => "ast_call",
            EdgeKind::Inherits | EdgeKind::Implements => "ast_inheritance",
            EdgeKind::Explains => "text_reference",
            EdgeKind::References => "openapi_contract",
        },
    }
}

fn languages(nodes: &[Node]) -> Vec<String> {
    let mut found = BTreeSet::new();
    for node in nodes.iter().filter(|node| node.kind == NodeKind::File) {
        let language = match Path::new(&node.file_path)
            .extension()
            .and_then(|value| value.to_str())
        {
            Some("rs") => "Rust",
            Some("js" | "jsx" | "mjs" | "cjs") => "JavaScript",
            Some("ts" | "tsx") => "TypeScript",
            Some("py") => "Python",
            Some("go") => "Go",
            Some("java") => "Java",
            Some("rb") => "Ruby",
            Some("c" | "h") => "C",
            Some("cc" | "cpp" | "cxx" | "hpp") => "C++",
            Some("cs") => "C#",
            Some("php" | "phtml") => "PHP",
            Some("swift") => "Swift",
            Some("kt" | "kts") => "Kotlin",
            Some("dart") => "Dart",
            Some("scala" | "sc") => "Scala",
            Some("sh" | "bash" | "zsh") => "Shell",
            Some("lua") => "Lua",
            Some("r" | "R") => "R",
            Some("ex" | "exs") => "Elixir",
            Some("m" | "mm") => "Objective-C",
            _ => continue,
        };
        found.insert(language.to_string());
    }
    if found.is_empty() {
        found.insert("unknown".into());
    }
    found.into_iter().collect()
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string()
    })
}

fn changed_files(root: &Path) -> Vec<String> {
    git(root, &["status", "--porcelain"])
        .map(|status| {
            status
                .lines()
                .filter_map(|line| line.get(3..))
                .map(|path| {
                    path.rsplit_once(" -> ")
                        .map_or(path, |(_, destination)| destination)
                        .to_string()
                })
                .filter(|path| {
                    !path.starts_with(".aag/")
                        && !path.contains("/.aag/")
                        && !path.ends_with(".aag.lock")
                })
                .collect()
        })
        .unwrap_or_default()
}

fn previous_at(previous: Option<&Value>, pointer: &str) -> Option<Value> {
    previous?.pointer(pointer).cloned()
}

fn preserved_items(previous: Option<&Value>, pointer: &str) -> Vec<Value> {
    previous_at(previous, pointer)
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter(|item| {
            matches!(
                item.get("origin").and_then(Value::as_str),
                Some("human" | "mixed")
            )
        })
        .collect()
}

fn preserved_historical(previous: Option<&Value>) -> Value {
    let findings = previous_at(previous, "/perspectives/historical/findings")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter(|finding| {
            finding
                .get("evidence")
                .and_then(Value::as_array)
                .is_some_and(|evidence| {
                    evidence.iter().any(|item| {
                        matches!(
                            item.get("origin").and_then(Value::as_str),
                            Some("human" | "mixed")
                        )
                    })
                })
        })
        .collect::<Vec<_>>();
    json!({ "ownership": "shared", "analyzed": false, "findings": findings })
}

fn merged_extensions(previous: Option<&Value>) -> Value {
    let mut extensions = previous_at(previous, "/extensions")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    extensions.insert(
        "x-aag".into(),
        json!({
            "source_of_truth": ".aag/graph.db",
            "protocol_revision": "a2a1666"
        }),
    );
    Value::Object(extensions)
}

fn default_declared() -> Value {
    json!({ "ownership": "human", "purpose": "", "invariants": [], "constraints": [] })
}

fn default_recipes() -> Value {
    json!([{
        "id": "recipe.change-impact",
        "description": "Select the smallest graph slice needed to assess a code change.",
        "start_from": ["changed entity"],
        "expand": ["incoming and outgoing relationships"],
        "must_include": ["affected tests", "ambiguous relationships"],
        "max_depth": 8
    }])
}

fn value_id(value: &Value) -> &str {
    value.get("id").and_then(Value::as_str).unwrap_or("")
}

fn dedupe_by_id(items: impl IntoIterator<Item = Value>) -> Vec<Value> {
    let mut by_id = BTreeMap::new();
    for item in items {
        by_id.entry(value_id(&item).to_string()).or_insert(item);
    }
    by_id.into_values().collect()
}

/// Validate schema shape plus the cross-reference and freshness rules that
/// JSON Schema cannot express.
///
/// # Errors
///
/// Returns the first concise schema or semantic diagnostic.
pub fn validate_manifest(manifest: &Value) -> Result<()> {
    let schema: Value =
        serde_json::from_str(MANIFEST_SCHEMA).map_err(|source| Error::Protocol {
            context: "bundled schema is invalid",
            detail: source.to_string(),
        })?;
    let validator = jsonschema::validator_for(&schema).map_err(|source| Error::Protocol {
        context: "bundled schema did not compile",
        detail: source.to_string(),
    })?;
    if let Some(error) = validator.iter_errors(manifest).next() {
        return Err(Error::Protocol {
            context: "schema validation failed",
            detail: format!("{}: {}", error.instance_path(), error),
        });
    }
    validate_semantics(manifest)
}

fn validate_semantics(manifest: &Value) -> Result<()> {
    let mut ids = HashSet::new();
    let mut duplicates = BTreeSet::new();
    collect_ids(manifest, &mut ids, &mut duplicates);
    if let Some(id) = duplicates.first() {
        return semantic_error(format!("duplicate id `{id}`"));
    }

    let mut references = Vec::new();
    collect_references(manifest, &mut references);
    if let Some(reference) = references
        .iter()
        .find(|reference| !ids.contains(*reference))
    {
        return semantic_error(format!("reference `{reference}` does not resolve"));
    }

    validate_locations(manifest)?;
    validate_freshness(manifest)?;
    validate_capabilities(manifest)
}

fn collect_ids(value: &Value, ids: &mut HashSet<String>, duplicates: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            if let Some(id) = object.get("id").and_then(Value::as_str)
                && !ids.insert(id.to_string())
            {
                duplicates.insert(id.to_string());
            }
            for child in object.values() {
                collect_ids(child, ids, duplicates);
            }
        }
        Value::Array(array) => {
            for child in array {
                collect_ids(child, ids, duplicates);
            }
        }
        _ => {}
    }
}

fn collect_references(value: &Value, references: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                if key.ends_with("_id") && key != "trace_id" {
                    if let Some(reference) = child.as_str() {
                        references.push(reference.to_string());
                    }
                } else if (key.ends_with("_ids") || key == "next")
                    && let Some(items) = child.as_array()
                {
                    references.extend(
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(ToString::to_string),
                    );
                }
                collect_references(child, references);
            }
        }
        Value::Array(array) => {
            for child in array {
                collect_references(child, references);
            }
        }
        _ => {}
    }
}

fn validate_locations(value: &Value) -> Result<()> {
    match value {
        Value::Object(object) => {
            if object.get("kind").and_then(Value::as_str) == Some("source") {
                let file = object.get("file").and_then(Value::as_str).unwrap_or("");
                let path = Path::new(file);
                if path.is_absolute()
                    || path
                        .components()
                        .any(|component| component == Component::ParentDir)
                {
                    return semantic_error(format!(
                        "source evidence path `{file}` leaves repository scope"
                    ));
                }
                if let (Some(start), Some(end)) = (
                    object.get("line_start").and_then(Value::as_u64),
                    object.get("line_end").and_then(Value::as_u64),
                ) && start > end
                {
                    return semantic_error(format!(
                        "source evidence `{file}` has reversed line bounds"
                    ));
                }
            }
            for child in object.values() {
                validate_locations(child)?;
            }
        }
        Value::Array(array) => {
            for child in array {
                validate_locations(child)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_freshness(manifest: &Value) -> Result<()> {
    let Some(freshness) = manifest.get("freshness") else {
        return Ok(());
    };
    if freshness.get("status").and_then(Value::as_str) == Some("current") {
        let analyzed = freshness.get("analyzed_revision");
        let current = freshness.get("current_revision");
        let dirty = freshness
            .get("working_tree_changed")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if analyzed != current || current.is_none() || dirty {
            return semantic_error(
                "freshness cannot be current when revisions differ or the working tree changed",
            );
        }
    }
    Ok(())
}

fn validate_capabilities(manifest: &Value) -> Result<()> {
    let capabilities = manifest
        .pointer("/aag_manifest/generator/capabilities")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    let evidence_types = evidence_types(manifest);
    if evidence_types.iter().any(|kind| {
        matches!(
            kind.as_str(),
            "execution_trace"
                | "log_observation"
                | "test_execution"
                | "database_trace"
                | "network_trace"
        )
    }) && !capabilities.contains("runtime_observation")
    {
        return semantic_error("runtime evidence requires the `runtime_observation` capability");
    }
    let historical = manifest
        .pointer("/perspectives/historical/analyzed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if historical && !capabilities.contains("history_analysis") {
        return semantic_error("analyzed history requires the `history_analysis` capability");
    }
    Ok(())
}

fn evidence_types(manifest: &Value) -> BTreeSet<String> {
    let mut types = BTreeSet::new();
    collect_evidence_types(manifest, &mut types);
    types
}

fn collect_evidence_types(value: &Value, types: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            if object.contains_key("origin")
                && object.contains_key("location")
                && let Some(kind) = object.get("type").and_then(Value::as_str)
            {
                types.insert(kind.to_string());
            }
            for child in object.values() {
                collect_evidence_types(child, types);
            }
        }
        Value::Array(array) => {
            for child in array {
                collect_evidence_types(child, types);
            }
        }
        _ => {}
    }
}

fn semantic_error(detail: impl Into<String>) -> Result<()> {
    Err(Error::Protocol {
        context: "semantic validation failed",
        detail: detail.into(),
    })
}

fn now_rfc3339() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        });
    let days = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = day_seconds / 3_600;
    let minute = day_seconds % 3_600 / 60;
    let second = day_seconds % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{Confidence, EdgeKind};

    fn node(id: i64, kind: NodeKind, name: &str, file: &str, line: u32) -> Node {
        Node {
            id: Some(id),
            kind,
            name: name.into(),
            file_path: file.into(),
            start_line: line,
            end_line: line,
            description: None,
        }
    }

    fn graph_fixture() -> (Vec<Node>, Vec<Edge>) {
        (
            vec![
                node(1, NodeKind::Function, "caller", "src/lib.rs", 1),
                node(2, NodeKind::Function, "callee", "src/lib.rs", 2),
            ],
            vec![Edge {
                src: 1,
                dst: 2,
                kind: EdgeKind::Calls,
                confidence: Confidence::Inferred,
            }],
        )
    }

    #[test]
    fn compiles_graph_to_conforming_manifest() {
        let (nodes, edges) = graph_fixture();
        let manifest = build_manifest(Path::new("."), &nodes, &edges, None, None, None);
        validate_manifest(&manifest).unwrap();
        assert_eq!(
            manifest.pointer("/perspectives/observed/relationships/0/confidence"),
            Some(&json!("medium"))
        );
    }

    #[test]
    fn compiles_openapi_contract_graph_without_mixing_perspectives() {
        let endpoint = Node {
            description: Some(json!({"method": "GET", "path": "/pets", "operation": {"responses": {"200": {"description": "ok"}}}}).to_string()),
            ..node(10, NodeKind::Endpoint, "GET /pets", "openapi.yaml", 1)
        };
        let schema = Node {
            description: Some(json!({"type": "object"}).to_string()),
            ..node(11, NodeKind::Schema, "Pet", "openapi.yaml", 1)
        };
        let provenance = Provenance {
            perspective: Perspective::Declared,
            evidence_kind: crate::storage::EvidenceKind::OpenApi,
            evidence_source: Some("openapi.yaml".into()),
        };
        let nodes = vec![(endpoint, provenance.clone()), (schema, provenance.clone())];
        let edges = vec![(
            Edge {
                src: 10,
                dst: 11,
                kind: EdgeKind::References,
                confidence: Confidence::Extracted,
            },
            provenance,
        )];
        let plain_nodes = nodes
            .iter()
            .map(|(node, _)| node.clone())
            .collect::<Vec<_>>();
        let plain_edges = edges.iter().map(|(edge, _)| *edge).collect::<Vec<_>>();
        let manifest = build_manifest(
            Path::new("."),
            &plain_nodes,
            &plain_edges,
            None,
            Some(&nodes),
            Some(&edges),
        );

        validate_manifest(&manifest).unwrap();
        assert_eq!(
            manifest.pointer("/perspectives/observed/inventory/entities"),
            Some(&json!(0))
        );
        assert_eq!(
            manifest.pointer("/extensions/x-aag-declared-contracts/entities/0/kind"),
            Some(&json!("entrypoint"))
        );
        assert_eq!(
            manifest.pointer("/extensions/x-aag-declared-contracts/relationships/0/type"),
            Some(&json!("x-relation-references-schema"))
        );
    }

    #[test]
    fn ambiguous_edges_create_uncertainties() {
        let (nodes, mut edges) = graph_fixture();
        edges[0].confidence = Confidence::Ambiguous;
        let manifest = build_manifest(Path::new("."), &nodes, &edges, None, None, None);
        validate_manifest(&manifest).unwrap();
        assert_eq!(manifest["uncertainties"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn preserves_human_owned_content() {
        let previous = json!({
            "perspectives": {
                "declared": { "ownership": "human", "purpose": "keep", "invariants": [], "constraints": [] },
                "historical": { "ownership": "shared", "analyzed": false, "findings": [] }
            },
            "uncertainties": [{
                "id": "uncertainty.human", "description": "keep", "reason": "unknown",
                "affected_entity_ids": [], "confidence": "unresolved",
                "recommended_verification": ["ask"], "origin": "human"
            }],
            "task_recipes": [],
            "extensions": { "x-team": true }
        });
        let manifest = build_manifest(Path::new("."), &[], &[], Some(&previous), None, None);
        assert_eq!(
            manifest.pointer("/perspectives/declared/purpose"),
            Some(&json!("keep"))
        );
        assert_eq!(manifest.pointer("/extensions/x-team"), Some(&json!(true)));
        assert_eq!(
            manifest.pointer("/uncertainties/0/id"),
            Some(&json!("uncertainty.human"))
        );
    }

    #[test]
    fn semantic_validation_rejects_broken_reference() {
        let (nodes, edges) = graph_fixture();
        let mut manifest = build_manifest(Path::new("."), &nodes, &edges, None, None, None);
        manifest["perspectives"]["observed"]["relationships"][0]["target_id"] =
            json!("entity:missing");
        let error = validate_manifest(&manifest).unwrap_err().to_string();
        assert!(error.contains("does not resolve"), "{error}");
    }

    #[test]
    fn generated_ids_are_stable() {
        let node = node(99, NodeKind::Function, "same", "src/lib.rs", 7);
        assert_eq!(entity_id(&node), entity_id(&node));
        assert_eq!(entity_id(&node), "entity:9f11120fc2edeb55");
    }

    #[test]
    fn utc_conversion_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_654), (2026, 7, 20));
    }

    #[test]
    fn validates_protocol_reference_example() {
        let example: Value =
            serde_yaml_ng::from_str(include_str!("../protocol/context.example.yaml")).unwrap();
        validate_manifest(&example).unwrap();
    }
}
