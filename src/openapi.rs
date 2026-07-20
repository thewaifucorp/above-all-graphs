//! Language-agnostic OpenAPI/Swagger contract ingestion.

use std::{collections::HashMap, fs, path::Path};

use serde_json::{Map, Value, json};

use crate::{
    error::Result,
    storage::{
        Confidence, Edge, EdgeKind, EvidenceKind, Graph, Node, NodeKind, Perspective, Provenance,
    },
};

/// A declared API operation waiting to be linked to an observed code symbol.
#[derive(Debug)]
pub(crate) struct Operation {
    pub(crate) node_id: i64,
    pub(crate) node_name: String,
    pub(crate) candidate_names: Vec<String>,
}

/// Index an `OpenAPI` 2.x/3.x document, returning `None` for ordinary YAML/JSON.
pub(crate) fn index_contract(
    graph: &Graph,
    relative: &str,
    path: &Path,
) -> Result<Option<Vec<Operation>>> {
    let Some(document) = read_contract(path) else {
        return Ok(None);
    };
    let provenance = Provenance {
        perspective: Perspective::Declared,
        evidence_kind: EvidenceKind::OpenApi,
        evidence_source: Some(relative.to_string()),
    };
    let schemas = index_schemas(graph, relative, &document, &provenance)?;
    link_schema_references(graph, &document, &schemas, &provenance)?;
    let operations = index_operations(graph, relative, &document, &schemas, &provenance)?;
    Ok(Some(operations))
}

fn read_contract(path: &Path) -> Option<Value> {
    let extension = path.extension()?.to_str()?;
    if !matches!(extension, "json" | "yaml" | "yml") {
        return None;
    }
    let text = fs::read_to_string(path).ok()?;
    let value: Value = if extension == "json" {
        serde_json::from_str(&text).ok()?
    } else {
        serde_yaml_ng::from_str(&text).ok()?
    };
    (value.get("openapi").is_some() || value.get("swagger").is_some()).then_some(value)
}

fn schema_objects(document: &Value) -> Option<&Map<String, Value>> {
    document
        .pointer("/components/schemas")
        .or_else(|| document.get("definitions"))
        .and_then(Value::as_object)
}

fn index_schemas(
    graph: &Graph,
    relative: &str,
    document: &Value,
    provenance: &Provenance,
) -> Result<HashMap<String, i64>> {
    let mut schemas = HashMap::new();
    for (name, schema) in schema_objects(document).into_iter().flatten() {
        let id = graph.insert_node_with_provenance(
            &Node {
                id: None,
                kind: NodeKind::Schema,
                name: name.clone(),
                file_path: relative.to_string(),
                start_line: 1,
                end_line: 1,
                description: Some(schema.to_string()),
            },
            provenance,
        )?;
        schemas.insert(name.clone(), id);
    }
    Ok(schemas)
}

fn link_schema_references(
    graph: &Graph,
    document: &Value,
    schemas: &HashMap<String, i64>,
    provenance: &Provenance,
) -> Result<()> {
    for (name, schema) in schema_objects(document).into_iter().flatten() {
        let Some(&source) = schemas.get(name) else {
            continue;
        };
        insert_references(graph, source, schema, schemas, provenance)?;
    }
    Ok(())
}

fn index_operations(
    graph: &Graph,
    relative: &str,
    document: &Value,
    schemas: &HashMap<String, i64>,
    provenance: &Provenance,
) -> Result<Vec<Operation>> {
    let mut operations = Vec::new();
    let version = document
        .get("openapi")
        .or_else(|| document.get("swagger"))
        .cloned()
        .unwrap_or(Value::Null);
    let Some(paths) = document.get("paths").and_then(Value::as_object) else {
        return Ok(operations);
    };
    for (route, item) in paths {
        let Some(methods) = item.as_object() else {
            continue;
        };
        for (method, operation) in methods {
            if !is_http_method(method) {
                continue;
            }
            let operation_id = operation
                .get("operationId")
                .and_then(Value::as_str)
                .map(str::to_string);
            let details = json!({
                "contract_version": version,
                "method": method.to_uppercase(),
                "path": route,
                "path_parameters": item.get("parameters").cloned().unwrap_or_else(|| json!([])),
                "operation": operation
            });
            let node_name = format!("{} {route}", method.to_uppercase());
            let node_id = graph.insert_node_with_provenance(
                &Node {
                    id: None,
                    kind: NodeKind::Endpoint,
                    name: node_name.clone(),
                    file_path: relative.to_string(),
                    start_line: 1,
                    end_line: 1,
                    description: Some(details.to_string()),
                },
                provenance,
            )?;
            insert_references(graph, node_id, &details, schemas, provenance)?;
            let candidate_names = operation_id.map_or_else(
                || endpoint_candidate_names(method, route),
                |name| vec![name],
            );
            operations.push(Operation {
                node_id,
                node_name,
                candidate_names,
            });
        }
    }
    Ok(operations)
}

fn endpoint_candidate_names(method: &str, route: &str) -> Vec<String> {
    let resource = route
        .split('/')
        .rfind(|part| !part.is_empty() && !part.starts_with('{'))
        .unwrap_or_default();
    if resource.is_empty() {
        return Vec::new();
    }
    let singular = resource.strip_suffix('s').unwrap_or(resource);
    let title = |value: &str| {
        let mut characters = value.chars();
        characters.next().map_or_else(String::new, |first| {
            first.to_uppercase().collect::<String>() + characters.as_str()
        })
    };
    let resources = [title(resource), title(singular)];
    let verbs: &[&str] = match method {
        "get" if route.contains('{') => &["get", "find", "fetch", "show"],
        "get" => &["list", "get", "fetch"],
        "post" => &["create", "add", "post"],
        "put" | "patch" => &["update", "edit", "patch"],
        "delete" => &["delete", "remove"],
        _ => &[method],
    };
    verbs
        .iter()
        .flat_map(|verb| {
            resources
                .iter()
                .map(move |resource| format!("{verb}{resource}"))
        })
        .collect()
}

fn insert_references(
    graph: &Graph,
    source: i64,
    value: &Value,
    schemas: &HashMap<String, i64>,
    provenance: &Provenance,
) -> Result<()> {
    let mut references = Vec::new();
    collect_references(value, &mut references);
    references.sort_unstable();
    references.dedup();
    for name in references {
        let Some(&target) = schemas.get(name) else {
            continue;
        };
        graph.insert_edge_with_provenance(
            &Edge {
                src: source,
                dst: target,
                kind: EdgeKind::References,
                confidence: Confidence::Extracted,
            },
            provenance,
        )?;
    }
    Ok(())
}

fn collect_references<'a>(value: &'a Value, out: &mut Vec<&'a str>) {
    match value {
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(Value::as_str)
                && let Some(name) = reference.rsplit('/').next()
            {
                out.push(name);
            }
            for nested in object.values() {
                collect_references(nested, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_references(item, out);
            }
        }
        _ => {}
    }
}

fn is_http_method(method: &str) -> bool {
    matches!(
        method,
        "get" | "put" | "post" | "delete" | "options" | "head" | "patch" | "trace"
    )
}
