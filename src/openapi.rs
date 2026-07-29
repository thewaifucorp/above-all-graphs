//! Language-agnostic OpenAPI/Swagger contract ingestion, and emission of the
//! surface the code actually serves.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::Path,
};

use serde_json::{Map, Value, json};

use crate::{
    api::Surface,
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

/// Renders the API surface as an `OpenAPI` 3.1 document.
///
/// This is the inverse of the ingestion above, and it is deliberately narrow:
/// the graph knows which routes exist, which symbol serves each one, and where
/// both live. It does not know request or response schemas, because nothing in
/// the code declares them — a handler returning `Json<Value>` says nothing about
/// its shape. So the document carries paths, methods, parameters read from the
/// path itself, and provenance under `x-aag`, and it says in plain words that
/// response shapes are absent rather than inventing a `200 OK` nobody promised.
///
/// Endpoints a contract declares but no code serves are left out unless asked
/// for: they are already in whatever document declared them, and emitting them
/// here would present a promise as an implementation.
pub(crate) fn document(title: &str, surfaces: &[Surface], include_declared: bool) -> Value {
    let mut paths: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    let mut tools: Vec<Value> = Vec::new();
    let mut used_ids: BTreeSet<String> = BTreeSet::new();

    for surface in surfaces {
        if surface.method == "TOOL" {
            tools.push(json!({
                "name": surface.path,
                "handlers": handler_list(surface),
                "consumers": surface.consumers.len(),
                "registeredIn": surface.observed_in,
            }));
            continue;
        }
        if surface.observed_in.is_none() && !include_declared {
            continue;
        }
        let path = openapi_path(&surface.path);
        let method = surface.method.to_ascii_lowercase();
        if !VERBS.contains(&method.as_str()) {
            continue;
        }
        let entry = paths.entry(path.clone()).or_default();
        let mut operation = Map::new();
        operation.insert(
            "operationId".to_string(),
            json!(unique_id(surface, &method, &path, &mut used_ids)),
        );
        operation.insert("summary".to_string(), json!(surface.name));
        operation.insert("description".to_string(), json!(describe(surface)));
        let parameters = path_parameters(&path);
        if !parameters.is_empty() {
            operation.insert("parameters".to_string(), json!(parameters));
        }
        operation.insert(
            "responses".to_string(),
            json!({
                "default": {
                    "description": "Not declared in code. AAG reports the routes it observed; \
            response shapes are not inferred from handler bodies."
                }
            }),
        );
        operation.insert(
            "x-aag".to_string(),
            json!({
                "state": surface.state(),
                "observedIn": surface.observed_in,
                "declaredIn": surface.declared_in,
                "handlers": handler_list(surface),
                "consumers": surface.consumers.len(),
            }),
        );
        entry.insert(method, Value::Object(operation));
    }

    let mut document = Map::new();
    document.insert("openapi".to_string(), json!("3.1.0"));
    document.insert(
        "info".to_string(),
        json!({
            "title": title,
            "version": "0.0.0",
            "description": "Generated by aag from the routes observed in this repository. \
        Paths, methods and handlers come from the code; request and response schemas are absent \
        because the code does not declare them.",
        }),
    );
    document.insert("paths".to_string(), json!(paths));
    if !tools.is_empty() {
        // RPC and MCP tools are a real part of this surface and not HTTP, so
        // they are reported beside the paths instead of being dressed up as
        // routes.
        document.insert("x-aag-tools".to_string(), json!(tools));
    }
    Value::Object(document)
}

/// The HTTP methods `OpenAPI` allows under a path item. Anything else observed —
/// a router registered with a variable, say — is not spelled as a method here.
const VERBS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// Rewrites route parameters into the one spelling `OpenAPI` understands:
/// `:id` and `<id>` both become `{id}`.
fn openapi_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for segment in path.split('/') {
        if segment.is_empty() {
            if out.is_empty() {
                out.push('/');
            }
            continue;
        }
        if !out.ends_with('/') {
            out.push('/');
        }
        if let Some(name) = segment.strip_prefix(':') {
            out.push('{');
            out.push_str(name);
            out.push('}');
        } else if let Some(name) = segment.strip_prefix('<').and_then(|s| s.strip_suffix('>')) {
            // Flask and friends: `<int:id>` carries a converter we do not keep.
            let name = name.rsplit_once(':').map_or(name, |(_, name)| name);
            out.push('{');
            out.push_str(name);
            out.push('}');
        } else {
            out.push_str(segment);
        }
    }
    if out.is_empty() {
        out.push('/');
    }
    out
}

/// Every `{name}` in the path, as a required string parameter. A path parameter
/// is required by definition, and the type is the only honest guess available.
fn path_parameters(path: &str) -> Vec<Value> {
    let mut parameters = Vec::new();
    let mut rest = path;
    while let Some(start) = rest.find('{') {
        let Some(end) = rest[start..].find('}') else {
            break;
        };
        let name = &rest[start + 1..start + end];
        if !name.is_empty() {
            parameters.push(json!({
                "name": name,
                "in": "path",
                "required": true,
                "schema": { "type": "string" },
            }));
        }
        rest = &rest[start + end + 1..];
    }
    parameters
}

fn handler_list(surface: &Surface) -> Vec<Value> {
    surface
        .handlers
        .iter()
        .map(|(name, location)| json!({ "symbol": name, "at": location }))
        .collect()
}

fn describe(surface: &Surface) -> String {
    match (&surface.declared_in, &surface.observed_in) {
        (Some(declared), Some(observed)) => {
            format!("Declared in {declared} and served by code in {observed}.")
        }
        (None, Some(observed)) => {
            format!("Served by code in {observed}. No contract declares it.")
        }
        (Some(declared), None) => format!("Declared in {declared}. No code serves it."),
        (None, None) => "Neither declared nor observed.".to_string(),
    }
}

/// A stable `operationId`, taken from the handler when there is one because that
/// is the name a reader already knows the route by.
fn unique_id(surface: &Surface, method: &str, path: &str, used: &mut BTreeSet<String>) -> String {
    let base = surface.handlers.first().map_or_else(
        || {
            let slug: String = path
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect();
            format!("{method}{slug}")
        },
        |(name, _)| name.clone(),
    );
    if used.insert(base.clone()) {
        return base;
    }
    for suffix in 2..1000 {
        let candidate = format!("{base}_{suffix}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    base
}

#[cfg(test)]
mod emit_tests {
    use super::{Surface, document};

    fn observed(name: &str, method: &str, path: &str) -> Surface {
        Surface {
            name: name.to_string(),
            method: method.to_string(),
            path: path.to_string(),
            declared_in: None,
            observed_in: Some("src/routes.rs".to_string()),
            handlers: Vec::new(),
            consumers: Vec::new(),
        }
    }

    #[test]
    fn route_parameters_are_rewritten_and_declared_required() {
        let mut surface = observed("GET /pets/:id", "GET", "/pets/:id");
        surface
            .handlers
            .push(("get_pet".to_string(), "src/routes.rs:41".to_string()));
        let document = document("demo", &[surface], false);
        let operation = &document["paths"]["/pets/{id}"]["get"];
        assert_eq!(operation["operationId"], "get_pet");
        let parameter = &operation["parameters"][0];
        assert_eq!(parameter["name"], "id");
        assert_eq!(parameter["in"], "path");
        assert_eq!(parameter["required"], true);
        assert_eq!(operation["x-aag"]["state"], "undeclared");
    }

    #[test]
    fn a_framework_converter_is_not_part_of_the_parameter_name() {
        let document = document(
            "demo",
            &[observed("GET /pets/<int:id>", "GET", "/pets/<int:id>")],
            false,
        );
        assert_eq!(
            document["paths"]["/pets/{id}"]["get"]["parameters"][0]["name"],
            "id"
        );
    }

    #[test]
    fn no_response_shape_is_invented() {
        let document = document("demo", &[observed("GET /pets", "GET", "/pets")], false);
        let responses = &document["paths"]["/pets"]["get"]["responses"];
        assert!(
            responses.get("200").is_none(),
            "a 200 nobody promised must not appear"
        );
        assert!(
            responses["default"]["description"]
                .as_str()
                .unwrap_or_default()
                .contains("not inferred"),
            "the absence has to be stated: {responses}"
        );
    }

    #[test]
    fn a_declared_endpoint_no_code_serves_is_left_out_unless_asked_for() {
        let promise = Surface {
            name: "GET /pets".to_string(),
            method: "GET".to_string(),
            path: "/pets".to_string(),
            declared_in: Some("openapi.yaml".to_string()),
            observed_in: None,
            handlers: Vec::new(),
            consumers: Vec::new(),
        };
        let without = document("demo", std::slice::from_ref(&promise), false);
        assert!(
            without["paths"]
                .as_object()
                .is_some_and(serde_json::Map::is_empty),
            "a promise is not an implementation: {without}"
        );
        let with = document("demo", &[promise], true);
        assert_eq!(
            with["paths"]["/pets"]["get"]["x-aag"]["state"],
            "declared only"
        );
    }

    #[test]
    fn tools_are_reported_beside_the_paths_not_as_routes() {
        let mut tool = observed("TOOL explore", "TOOL", "explore");
        tool.handlers
            .push(("handle_explore".to_string(), "src/mcp.rs:12".to_string()));
        let document = document("demo", &[tool], false);
        assert!(
            document["paths"]
                .as_object()
                .is_some_and(serde_json::Map::is_empty)
        );
        assert_eq!(document["x-aag-tools"][0]["name"], "explore");
        assert_eq!(
            document["x-aag-tools"][0]["handlers"][0]["symbol"],
            "handle_explore"
        );
    }

    #[test]
    fn two_routes_sharing_a_handler_name_still_get_distinct_ids() {
        let mut first = observed("GET /pets", "GET", "/pets");
        first
            .handlers
            .push(("list".to_string(), "src/pets.rs:9".to_string()));
        let mut second = observed("GET /owners", "GET", "/owners");
        second
            .handlers
            .push(("list".to_string(), "src/owners.rs:9".to_string()));
        let document = document("demo", &[first, second], false);
        let one = document["paths"]["/pets"]["get"]["operationId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let two = document["paths"]["/owners"]["get"]["operationId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_ne!(one, two, "an operationId has to be unique in a document");
    }

    #[test]
    fn a_method_openapi_has_no_word_for_is_skipped() {
        let document = document(
            "demo",
            &[observed("LISTEN /events", "LISTEN", "/events")],
            false,
        );
        assert!(
            document["paths"]
                .as_object()
                .is_some_and(serde_json::Map::is_empty)
        );
    }

    #[test]
    fn several_methods_on_one_path_share_the_path_item() {
        let document = document(
            "demo",
            &[
                observed("GET /pets", "GET", "/pets"),
                observed("POST /pets", "POST", "/pets"),
            ],
            false,
        );
        let item = document["paths"]["/pets"].as_object().expect("path item");
        assert!(
            item.contains_key("get") && item.contains_key("post"),
            "{item:?}"
        );
    }
}
