//! Route, RPC, and tool intelligence over the indexed graph — P1.6 of
//! `docs/capability-coverage.md`.
//!
//! Everything here reads the graph that indexing already built: endpoints
//! declared by a contract (`crate::openapi`), endpoints and tools observed in
//! code (`crate::resolve`), the handler each observed one is served by, and the
//! outbound calls that consume them. Nothing re-parses a repository except the
//! shape check, which has to look inside a handler body.
//!
//! The gate's own wording is the rule this module follows: **a declaration is
//! evidence, not a replacement for the implementation, and the reverse.** So a
//! declared endpoint with no implementation, an implemented endpoint no contract
//! declares, and a declared endpoint whose implementation returns a different
//! shape are all first-class results rather than omissions.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use serde_json::Value;

use crate::error::Result;
use crate::storage::{Confidence, EdgeKind, Graph, Node, NodeKind, Perspective};

/// One endpoint or tool, with everything the graph knows about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Surface {
    /// `METHOD /path`, or `TOOL name`.
    pub name: String,
    /// HTTP method, or `TOOL`.
    pub method: String,
    /// Path, or tool name.
    pub path: String,
    /// Where the declaration lives, when a contract declares it.
    pub declared_in: Option<String>,
    /// Where the registration lives, when code registers it.
    pub observed_in: Option<String>,
    /// Symbols that serve it, with `file:line`.
    pub handlers: Vec<(String, String)>,
    /// Symbols that call it, with `file:line` and how sure the link is.
    pub consumers: Vec<(String, String, Confidence)>,
}

impl Surface {
    /// What state this surface is in, as the one word a reader needs.
    ///
    /// A tool is never "undeclared": nothing publishes a contract for it, so
    /// code registering one is the whole story.
    #[must_use]
    pub fn state(&self) -> &'static str {
        match (
            self.method == "TOOL",
            self.declared_in.is_some(),
            self.observed_in.is_some(),
        ) {
            (true, _, _) => "exposed",
            (false, true, true) => "matched",
            (false, true, false) => "declared only",
            (false, false, true) => "undeclared",
            (false, false, false) => "unknown",
        }
    }
}

/// Every endpoint and tool in the repository, declared and observed, paired.
///
/// Pairing is by shape rather than by spelling: a contract's `/pets/{id}` and a
/// framework's `/pets/:id` are the same endpoint, and saying otherwise would
/// report every parameterized route twice, once as missing and once as
/// undeclared.
///
/// # Errors
/// Returns [`crate::error::Error::IndexMissing`] when `root` has no graph, or a
/// storage error when it cannot be read.
pub fn surfaces(root: &Path) -> Result<Vec<Surface>> {
    let graph = Graph::open_existing(root)?;
    let nodes = graph.all_nodes_with_provenance()?;
    let edges = graph.all_edges()?;
    let by_id: BTreeMap<i64, &Node> = nodes
        .iter()
        .filter_map(|(node, _)| node.id.map(|id| (id, node)))
        .collect();

    let mut grouped: BTreeMap<String, Surface> = BTreeMap::new();
    for (node, provenance) in &nodes {
        if node.kind != NodeKind::Endpoint {
            continue;
        }
        let (method, path) = split_surface(node);
        let key = shape_key(&method, &path);
        let entry = grouped.entry(key).or_insert_with(|| Surface {
            name: node.name.clone(),
            method: method.clone(),
            path: path.clone(),
            declared_in: None,
            observed_in: None,
            handlers: Vec::new(),
            consumers: Vec::new(),
        });
        // A declared name is the canonical one: it is what the contract
        // published, and the implementation's spelling is an implementation
        // detail.
        if provenance.perspective == Perspective::Declared {
            entry.declared_in = Some(node.file_path.clone());
            entry.name.clone_from(&node.name);
            entry.path = path;
        } else {
            entry.observed_in = Some(node.file_path.clone());
        }
        let id = node.id.unwrap_or_default();
        for edge in &edges {
            if edge.dst != id {
                continue;
            }
            let Some(source) = by_id.get(&edge.src) else {
                continue;
            };
            let location = format!("{}:{}", source.file_path, source.start_line);
            match edge.kind {
                EdgeKind::Implements => entry.handlers.push((source.name.clone(), location)),
                EdgeKind::Calls => {
                    entry
                        .consumers
                        .push((source.name.clone(), location, edge.confidence));
                }
                _ => {}
            }
        }
    }
    let mut out: Vec<Surface> = grouped.into_values().collect();
    for surface in &mut out {
        surface.handlers.sort_unstable();
        surface.handlers.dedup();
        surface
            .consumers
            .sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));
        surface.consumers.dedup();
    }
    out.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(out)
}

/// `METHOD` and path for an endpoint node, from the details indexing stored
/// rather than by re-splitting the name.
fn split_surface(node: &Node) -> (String, String) {
    let details = node
        .description
        .as_deref()
        .and_then(|text| serde_json::from_str::<Value>(text).ok());
    let method = details
        .as_ref()
        .and_then(|details| details.get("method").and_then(Value::as_str))
        .map(str::to_ascii_uppercase);
    let path = details
        .as_ref()
        .and_then(|details| details.get("path").and_then(Value::as_str))
        .map(str::to_string);
    match (method, path) {
        (Some(method), Some(path)) => (method, path),
        _ => node.name.split_once(' ').map_or_else(
            || ("TOOL".to_string(), node.name.clone()),
            |(method, path)| (method.to_ascii_uppercase(), path.to_string()),
        ),
    }
}

/// The key two spellings of one endpoint share: path parameters flattened, and
/// a trailing slash ignored.
fn shape_key(method: &str, path: &str) -> String {
    if method == "TOOL" {
        return format!("TOOL {path}");
    }
    let flattened: Vec<&str> = path
        .trim_end_matches('/')
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

/// The `OpenAPI` 3.1 document for the surface this repository serves.
///
/// `include_declared` adds the endpoints a contract declares but no code
/// implements. They are off by default: emitting them would present a promise as
/// an implementation, and they are already in the document that declared them.
///
/// # Errors
///
/// Propagates failures reading the graph.
pub fn spec(root: &Path, filter: &str, include_declared: bool) -> Result<String> {
    let mut surfaces = surfaces(root)?;
    if !filter.is_empty() {
        let needle = filter.to_lowercase();
        surfaces.retain(|surface| surface.name.to_lowercase().contains(&needle));
    }
    let title = root
        .canonicalize()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "repository".to_string());
    let document = crate::openapi::document(&title, &surfaces, include_declared);
    // A `Value` this code built cannot fail to serialise; the empty document is
    // still a truthful answer if it somehow does.
    Ok(serde_json::to_string_pretty(&document).unwrap_or_else(|_| "{}".into()))
}

/// The HTTP surface as text: one line per endpoint, its state, handler, and
/// consumers.
///
/// # Errors
/// As [`surfaces`].
pub fn route_map(root: &Path, filter: &str) -> Result<String> {
    let surfaces = surfaces(root)?;
    let routes: Vec<&Surface> = surfaces
        .iter()
        .filter(|surface| surface.method != "TOOL" && surface.name.contains(filter))
        .collect();
    if routes.is_empty() {
        return Ok(
            "no HTTP endpoints found: none declared by a contract, none registered in code"
                .to_string(),
        );
    }
    let mut out = vec![format!("{} HTTP endpoints", routes.len())];
    out.push(String::new());
    for surface in routes {
        out.push(describe(surface));
    }
    out.extend(mismatch_summary(&surfaces, "endpoint"));
    Ok(out.join("\n"))
}

/// The RPC/MCP tool surface as text.
///
/// # Errors
/// As [`surfaces`].
pub fn tool_map(root: &Path, filter: &str) -> Result<String> {
    let surfaces = surfaces(root)?;
    let tools: Vec<&Surface> = surfaces
        .iter()
        .filter(|surface| surface.method == "TOOL" && surface.name.contains(filter))
        .collect();
    if tools.is_empty() {
        return Ok(
            "no RPC or MCP tools found. A tool is recognized from `server.tool(\"name\", …)`, \
             a `@tool`/`#[tool]` marker, or a `ToolSpec { name: … }` table entry."
                .to_string(),
        );
    }
    let mut out = vec![format!("{} tools", tools.len()), String::new()];
    for surface in tools {
        out.push(describe(surface));
    }
    Ok(out.join("\n"))
}

fn describe(surface: &Surface) -> String {
    let handlers = if surface.handlers.is_empty() {
        // A table entry names no handler, and a dispatcher that routes by name
        // is a link only the string can make — saying so beats an empty field.
        "no handler named at the definition (dispatch routes it by name)".to_string()
    } else {
        surface
            .handlers
            .iter()
            .map(|(name, location)| format!("{name} ({location})"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let consumers = if surface.consumers.is_empty() {
        String::new()
    } else {
        let listed = surface
            .consumers
            .iter()
            .map(|(name, location, confidence)| {
                format!("{name} ({location}) [{}]", confidence.as_str())
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("\n     consumed by {listed}")
    };
    let places = [
        surface
            .declared_in
            .as_ref()
            .map(|file| format!("declared in {file}")),
        surface
            .observed_in
            .as_ref()
            .map(|file| format!("served from {file}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(", ");
    format!(
        "{} [{}] — {places}\n     handled by {handlers}{consumers}",
        surface.name,
        surface.state()
    )
}

/// The mismatches a reader should see even when scanning: a contract nobody
/// implements, and an implementation no contract declares.
fn mismatch_summary(surfaces: &[Surface], noun: &str) -> Vec<String> {
    let declared_only: Vec<&str> = surfaces
        .iter()
        .filter(|surface| surface.state() == "declared only")
        .map(|surface| surface.name.as_str())
        .collect();
    let undeclared: Vec<&str> = surfaces
        .iter()
        .filter(|surface| surface.state() == "undeclared" && surface.method != "TOOL")
        .map(|surface| surface.name.as_str())
        .collect();
    let mut out = Vec::new();
    if !declared_only.is_empty() {
        out.push(String::new());
        out.push(format!(
            "{} declared {noun}s no code implements: {}",
            declared_only.len(),
            declared_only.join(", ")
        ));
    }
    if !undeclared.is_empty() {
        out.push(String::new());
        out.push(format!(
            "{} served {noun}s no contract declares: {}",
            undeclared.len(),
            undeclared.join(", ")
        ));
    }
    out
}

/// One difference between a declared response shape and what the handler
/// returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeFinding {
    /// The endpoint the check is about.
    pub endpoint: String,
    /// Handler the shape was read from.
    pub handler: String,
    /// Fields the contract declares that the handler never puts in a response.
    pub missing: Vec<String>,
    /// Fields the handler returns that the contract does not declare.
    pub extra: Vec<String>,
    /// Declared fields marked required that are missing — a stronger signal
    /// than an optional one being absent.
    pub missing_required: Vec<String>,
}

/// Compares each declared response shape with what its handler returns.
///
/// Syntactic on purpose, but not shallow: the declared schema is flattened to
/// dotted field paths (`data.id`) through `$ref`s and array items, and the
/// handler's response is flattened the same way, following a body it assembled
/// in a local variable before answering with it. A finding is still a place to
/// look rather than a defect.
///
/// # Errors
/// As [`surfaces`], plus a parse error when a handler's file cannot be read.
pub fn shape_check(root: &Path, filter: &str) -> Result<Vec<ShapeFinding>> {
    let graph = Graph::open_existing(root)?;
    let nodes = graph.all_nodes_with_provenance()?;
    let schemas: BTreeMap<&str, &Node> = nodes
        .iter()
        .filter(|(node, _)| node.kind == NodeKind::Schema)
        .map(|(node, _)| (node.name.as_str(), node))
        .collect();
    let mut findings = Vec::new();
    for surface in surfaces(root)? {
        if !filter.is_empty() && !surface.name.contains(filter) {
            continue;
        }
        let Some(declared) = surface.declared_in.as_ref() else {
            continue;
        };
        let Some(declared_node) = nodes.iter().find(|(node, provenance)| {
            node.kind == NodeKind::Endpoint
                && provenance.perspective == Perspective::Declared
                && node.file_path == *declared
                && node.name == surface.name
        }) else {
            continue;
        };
        let expected = response_fields(declared_node.0.description.as_deref(), &schemas);
        if expected.is_empty() {
            continue;
        }
        for (handler, location) in &surface.handlers {
            let Some((file, _)) = location.rsplit_once(':') else {
                continue;
            };
            let returned = returned_fields(&root.join(file), handler)?;
            if returned.is_empty() {
                continue;
            }
            let missing: Vec<String> = expected
                .keys()
                .filter(|field| !returned.contains(field.as_str()))
                .cloned()
                .collect();
            let extra: Vec<String> = returned
                .iter()
                .filter(|field| !expected.contains_key(field.as_str()))
                .cloned()
                .collect();
            let missing_required = missing
                .iter()
                .filter(|field| expected.get(field.as_str()).copied().unwrap_or_default())
                .cloned()
                .collect();
            if missing.is_empty() && extra.is_empty() {
                continue;
            }
            findings.push(ShapeFinding {
                endpoint: surface.name.clone(),
                handler: handler.clone(),
                missing,
                extra,
                missing_required,
            });
        }
    }
    Ok(findings)
}

/// Top-level fields a declared success response carries, each with whether the
/// contract marks it required.
fn response_fields(
    description: Option<&str>,
    schemas: &BTreeMap<&str, &Node>,
) -> BTreeMap<String, bool> {
    let Some(details) = description.and_then(|text| serde_json::from_str::<Value>(text).ok())
    else {
        return BTreeMap::new();
    };
    let responses = details.pointer("/operation/responses");
    let success = responses.and_then(|responses| {
        responses.as_object().and_then(|codes| {
            codes
                .iter()
                .find(|(code, _)| code.starts_with('2'))
                .map(|(_, body)| body)
        })
    });
    let Some(schema) = success.and_then(|body| {
        body.pointer("/content")
            .and_then(Value::as_object)
            .and_then(|content| content.values().next())
            .and_then(|media| media.get("schema"))
            .or_else(|| body.get("schema"))
    }) else {
        return BTreeMap::new();
    };
    schema_properties(schema, schemas)
}

/// How deep a nested schema or response body is flattened. A contract that
/// nests deeper than this is compared down to the cut and no further, which
/// keeps a recursive `$ref` from turning into an infinite field list.
const SHAPE_DEPTH: usize = 4;

/// Resolves a schema — inline, a `$ref`, or an array of either — to the dotted
/// field paths it declares, each with whether the contract marks it required.
fn schema_properties(schema: &Value, schemas: &BTreeMap<&str, &Node>) -> BTreeMap<String, bool> {
    let mut out = BTreeMap::new();
    let mut visiting = BTreeSet::new();
    flatten_schema(schema, schemas, "", 0, &mut visiting, &mut out);
    out
}

/// Walks a schema into `out`, keyed by dotted path. `visiting` holds the
/// `$ref` names on the current branch, so a schema that refers to itself stops
/// at the cycle instead of unrolling forever.
fn flatten_schema(
    schema: &Value,
    schemas: &BTreeMap<&str, &Node>,
    prefix: &str,
    depth: usize,
    visiting: &mut BTreeSet<String>,
    out: &mut BTreeMap<String, bool>,
) {
    if depth > SHAPE_DEPTH {
        return;
    }
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let name = reference.rsplit('/').next().unwrap_or_default().to_string();
        if !visiting.insert(name.clone()) {
            return;
        }
        if let Some(body) = schemas
            .get(name.as_str())
            .and_then(|target| target.description.as_deref())
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
        {
            flatten_schema(&body, schemas, prefix, depth, visiting, out);
        }
        visiting.remove(&name);
        return;
    }
    // An array of objects declares the item's fields at the same path: a list
    // response and a single one disagree about cardinality, not about names.
    if let Some(items) = schema.get("items") {
        flatten_schema(items, schemas, prefix, depth, visiting, out);
        return;
    }
    let required: BTreeSet<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return;
    };
    for (name, property) in properties {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        out.insert(path.clone(), required.contains(name.as_str()));
        flatten_schema(property, schemas, &path, depth + 1, visiting, out);
    }
}

/// Keys of the object literals a handler returns, or hands to a JSON responder.
fn returned_fields(file: &Path, handler: &str) -> Result<BTreeSet<String>> {
    let Ok(source) = std::fs::read_to_string(file) else {
        return Ok(BTreeSet::new());
    };
    let name = file.to_string_lossy();
    let Some(language) = crate::flow::language_of(&name) else {
        return Ok(BTreeSet::new());
    };
    let mut parser = tree_sitter_language_pack::get_parser(language).map_err(|error| {
        crate::error::Error::Parse {
            file: name.to_string(),
            reason: error.to_string(),
        }
    })?;
    let Some(tree) = parser.parse(&source) else {
        return Ok(BTreeSet::new());
    };
    let mut locals = BTreeMap::new();
    let mut fields = BTreeSet::new();
    collect_returned(
        &tree.root_node(),
        &source,
        handler,
        false,
        &mut locals,
        &mut fields,
    );
    Ok(fields)
}

/// Source text of a node.
fn node_text<'a>(node: &tree_sitter_language_pack::Node, source: &'a str) -> Option<&'a str> {
    let range = node.byte_range();
    source.get(range.start..range.end)
}

/// Responder calls whose argument is the response body.
const RESPONDERS: &[&str] = &[
    "json",
    "send",
    "jsonify",
    "respond",
    "ok",
    "reply",
    "write",
    "return_json",
];

/// Statement kinds that bind a value to a name, with the fields holding the
/// name and the value. A handler that assembles its body before answering with
/// it binds it through one of these.
const BINDING_KINDS: &[(&str, &str, &str)] = &[
    ("variable_declarator", "name", "value"),
    ("let_declaration", "pattern", "value"),
    ("assignment", "left", "right"),
    ("assignment_expression", "left", "right"),
    ("short_var_declaration", "left", "right"),
    ("augmented_assignment", "left", "right"),
];

/// Walks for the response a handler produces. `inside` becomes true once the
/// walk is within the named function, so an object literal elsewhere in the file
/// is not read as this handler's response. Bindings are recorded as the walk
/// passes them, so `const body = {…}` is already known by the time
/// `res.json(body)` asks what `body` holds.
fn collect_returned(
    node: &tree_sitter_language_pack::Node,
    source: &str,
    handler: &str,
    inside: bool,
    locals: &mut BTreeMap<String, tree_sitter_language_pack::Node>,
    fields: &mut BTreeSet<String>,
) {
    let mut inside = inside;
    if !inside
        && node
            .child_by_field_name("name")
            .and_then(|name| node_text(&name, source))
            .is_some_and(|name| name == handler)
    {
        inside = true;
    }
    if inside {
        record_binding(node, source, locals);
        let responded = matches!(
            node.kind().as_str(),
            "return_statement" | "return_expression"
        ) || matches!(
            node.kind().as_str(),
            "call_expression" | "call" | "invocation_expression"
        ) && ["function", "callee", "name"]
            .into_iter()
            .find_map(|field| node.child_by_field_name(field))
            .and_then(|target| node_text(&target, source))
            .and_then(|text| text.rsplit(['.', ':']).next())
            .is_some_and(|tail| RESPONDERS.contains(&tail.trim().to_ascii_lowercase().as_str()));
        if responded {
            let mut seen = BTreeSet::new();
            // The body is what a `return` holds, or what a responder is passed —
            // never the responder's own name, so `res.json(body)` asks about
            // `body` and not about `res`.
            let carrier = ["arguments", "argument_list", "parameters"]
                .into_iter()
                .find_map(|field| node.child_by_field_name(field))
                .unwrap_or_else(|| node.clone());
            for index in 0..u32::try_from(carrier.named_child_count()).unwrap_or(u32::MAX) {
                if let Some(child) = carrier.named_child(index) {
                    follow_value(&child, source, "", 0, locals, &mut seen, fields);
                }
            }
        }
    }
    for index in 0..u32::try_from(node.named_child_count()).unwrap_or(u32::MAX) {
        if let Some(child) = node.named_child(index) {
            collect_returned(&child, source, handler, inside, locals, fields);
        }
    }
}

/// Remembers `name = value` when the name is a plain identifier. A destructured
/// or indexed target is skipped rather than guessed at.
fn record_binding(
    node: &tree_sitter_language_pack::Node,
    source: &str,
    locals: &mut BTreeMap<String, tree_sitter_language_pack::Node>,
) {
    let Some((_, name_field, value_field)) = BINDING_KINDS
        .iter()
        .find(|(kind, _, _)| *kind == node.kind())
    else {
        return;
    };
    let (Some(name), Some(value)) = (
        node.child_by_field_name(name_field),
        node.child_by_field_name(value_field),
    ) else {
        return;
    };
    let Some(name) = node_text(&name, source).map(str::trim) else {
        return;
    };
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return;
    }
    locals.insert(name.to_string(), value);
}

/// Dotted paths of every object/dictionary/struct literal under `node`.
///
/// A nested literal extends the path (`data.id`), which is what makes the
/// comparison mean something beyond the top level, and a value that is a plain
/// identifier is followed through the binding that produced it, once — a name
/// that resolves to itself stops rather than looping.
fn collect_object_keys(
    node: &tree_sitter_language_pack::Node,
    source: &str,
    prefix: &str,
    depth: usize,
    locals: &BTreeMap<String, tree_sitter_language_pack::Node>,
    seen: &mut BTreeSet<String>,
    fields: &mut BTreeSet<String>,
) {
    if depth > SHAPE_DEPTH {
        return;
    }
    if matches!(
        node.kind().as_str(),
        "pair" | "field_initializer" | "keyword_argument"
    ) {
        let key = ["key", "field", "name"]
            .into_iter()
            .find_map(|field| node.child_by_field_name(field))
            .and_then(|key| node_text(&key, source))
            .map(clean_key);
        if let Some(key) = key.filter(|key| is_field_name(key)) {
            let path = join_path(prefix, &key);
            fields.insert(path.clone());
            if let Some(value) = ["value", "right", "expression"]
                .into_iter()
                .find_map(|field| node.child_by_field_name(field))
            {
                follow_value(&value, source, &path, depth + 1, locals, seen, fields);
            }
        }
        return;
    }
    // `{ ...base, id }`: a spread merges the other object's fields at this
    // level, so the binding behind it is walked with the same prefix.
    if matches!(
        node.kind().as_str(),
        "spread_element" | "dictionary_splat" | "base_field_initializer"
    ) {
        if let Some(inner) = node.named_child(0) {
            follow_value(&inner, source, prefix, depth, locals, seen, fields);
        }
        return;
    }
    // `{ id }` and Rust's `Pet { id }`: the key is the whole node, and the
    // binding behind that name is what it carries.
    if matches!(
        node.kind().as_str(),
        "shorthand_property_identifier" | "shorthand_field_initializer"
    ) {
        let key = node_text(node, source).map(clean_key);
        if let Some(key) = key.filter(|key| is_field_name(key)) {
            let path = join_path(prefix, &key);
            fields.insert(path.clone());
            if let Some(bound) = locals.get(&key).filter(|_| seen.insert(key.clone())) {
                collect_object_keys(bound, source, &path, depth + 1, locals, seen, fields);
            }
        }
        return;
    }
    for index in 0..u32::try_from(node.named_child_count()).unwrap_or(u32::MAX) {
        if let Some(child) = node.named_child(index) {
            collect_object_keys(&child, source, prefix, depth, locals, seen, fields);
        }
    }
}

/// A field's value: an identifier is resolved through its binding, anything
/// else is walked for the literal it may contain.
fn follow_value(
    node: &tree_sitter_language_pack::Node,
    source: &str,
    prefix: &str,
    depth: usize,
    locals: &BTreeMap<String, tree_sitter_language_pack::Node>,
    seen: &mut BTreeSet<String>,
    fields: &mut BTreeSet<String>,
) {
    if node.kind() == "identifier" {
        let Some(name) = node_text(node, source).map(str::to_string) else {
            return;
        };
        if let Some(bound) = locals.get(&name).filter(|_| seen.insert(name.clone())) {
            collect_object_keys(bound, source, prefix, depth, locals, seen, fields);
        }
        return;
    }
    collect_object_keys(node, source, prefix, depth, locals, seen, fields);
}

fn clean_key(text: &str) -> String {
    text.trim().trim_matches(['"', '\'', '`']).to_string()
}

fn is_field_name(key: &str) -> bool {
    !key.is_empty() && !key.contains(char::is_whitespace)
}

fn join_path(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_string()
    } else {
        format!("{prefix}.{key}")
    }
}

/// The shape check as text.
///
/// # Errors
/// As [`shape_check`].
pub fn format_shape_check(root: &Path, filter: &str) -> Result<String> {
    let findings = shape_check(root, filter)?;
    if findings.is_empty() {
        return Ok(
            "no response-shape differences found. The check is syntactic — a field \
                   copied out of a call or a serializer is not read — so this is not \
                   proof the responses match."
                .to_string(),
        );
    }
    let mut out = vec![
        "Declared response shapes against what the handler returns, compared by \
         field path. Syntactic: a finding is a place to look."
            .to_string(),
        String::new(),
    ];
    for finding in &findings {
        let mut line = format!("{} — {}", finding.endpoint, finding.handler);
        if !finding.missing.is_empty() {
            let _ = write!(
                line,
                "\n     declared but never returned: {}",
                finding.missing.join(", ")
            );
        }
        if !finding.missing_required.is_empty() {
            let _ = write!(
                line,
                "\n     of those, required by the contract: {}",
                finding.missing_required.join(", ")
            );
        }
        if !finding.extra.is_empty() {
            let _ = write!(
                line,
                "\n     returned but not declared: {}",
                finding.extra.join(", ")
            );
        }
        out.push(line);
    }
    Ok(out.join("\n"))
}

/// What changing one endpoint, tool, or schema reaches.
///
/// The answer a reader needs first is not "how many nodes" but "who is on the
/// other side of this contract": the handler that serves it, the code that
/// consumes it, and the blast radius of the handler itself.
///
/// # Errors
/// As [`surfaces`].
pub fn impact(root: &Path, target: &str) -> Result<String> {
    let matches: Vec<Surface> = surfaces(root)?
        .into_iter()
        .filter(|surface| {
            surface.name == target || surface.path == target || surface.name.contains(target)
        })
        .collect();
    if matches.is_empty() {
        return Ok(format!(
            "no endpoint or tool matches `{target}`. `aag api routes` and `aag api tools` list \
             what the graph has."
        ));
    }
    let mut out = Vec::new();
    for surface in &matches {
        out.push(describe(surface));
        for (handler, _) in &surface.handlers {
            let blast = crate::impact::format(root, handler)?;
            let summary = blast
                .lines()
                .find(|line| line.contains("affected") || line.contains("no callers"))
                .unwrap_or("no transitive callers");
            out.push(format!("     changing {handler}: {summary}"));
        }
        if surface.consumers.is_empty() && surface.declared_in.is_some() {
            out.push(
                "     no code in this repository consumes it — a contract with no local caller \
                 may still have external ones"
                    .to_string(),
            );
        }
        out.push(String::new());
    }
    Ok(out.join("\n").trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// A repository with an `OpenAPI` contract, a server that serves part of it,
    /// a tool, and a client that consumes an endpoint.
    fn indexed_root() -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("aag-api-test-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("openapi.yaml"),
            "openapi: 3.0.0\ninfo:\n  title: Pets\n  version: '1'\npaths:\n  /pets:\n    get:\n      operationId: listPets\n      responses:\n        '200':\n          content:\n            application/json:\n              schema:\n                $ref: '#/components/schemas/Pet'\n  /pets/{id}:\n    get:\n      operationId: getPet\n      responses:\n        '200':\n          description: one pet\n  /archived:\n    get:\n      operationId: listArchived\n      responses:\n        '200':\n          description: never built\n  /orders/{id}:\n    get:\n      operationId: getOrder\n      responses:\n        '200':\n          description: one order\n  /orders:\n    get:\n      operationId: listOrders\n      responses:\n        '200':\n          content:\n            application/json:\n              schema:\n                $ref: '#/components/schemas/Order'\ncomponents:\n  schemas:\n    Pet:\n      type: object\n      required: [id, name]\n      properties:\n        id:\n          type: integer\n        name:\n          type: string\n        tag:\n          type: string\n    Order:\n      type: object\n      required: [id, customer]\n      properties:\n        id:\n          type: integer\n        customer:\n          type: object\n          required: [name]\n          properties:\n            name:\n              type: string\n            email:\n              type: string\n",
        )
        .unwrap();
        fs::write(
            root.join("server.js"),
            "function listPets(req, res) { return res.json({ id: 1, colour: 'red' }); }\n\
             function getPet(req, res) { return res.json({ id: 1 }); }\n\
             function health(req, res) { return res.json({ ok: true }); }\n\
             function listOrders(req, res) {\n  const customer = { email: 'ada@example.com' };\n  const base = { id: 1 };\n  const body = { ...base, customer };\n  return res.json(body);\n}\n\
             function getOrder(req, res) { return res.json({ id: 1 }); }\n\
             function wire(app) {\n  app.get('/pets', listPets);\n  app.get('/pets/:id', getPet);\n  app.get('/health', health);\n  app.get('/orders', listOrders);\n  app.get('/orders/:id', getOrder);\n}\n",
        )
        .unwrap();
        fs::write(
            root.join("client.js"),
            "const BASE = 'https://api.example.com';\n\
             function loadPets() { return fetch('/pets'); }\n\
             function loadOrder(id) { return fetch(`${BASE}/orders/${id}`); }\n\
             function loadOrders() { return fetch(BASE + '/orders'); }\n\
             function loadSuffixed(suffix) { return fetch('/orders' + suffix); }\n",
        )
        .unwrap();
        fs::write(
            root.join("tools.js"),
            "function searchDocs() {}\nfunction wire(server) { server.tool('search', searchDocs); }\n",
        )
        .unwrap();
        crate::bigbang::run(
            &root,
            &crate::bigbang::Options {
                no_viz: true,
                no_install: true,
                ..crate::bigbang::Options::default()
            },
        )
        .unwrap();
        root
    }

    #[test]
    fn a_declared_and_a_served_endpoint_pair_into_one_surface() {
        let root = indexed_root();

        let surfaces = surfaces(&root).unwrap();

        let pets = surfaces
            .iter()
            .find(|surface| surface.name == "GET /pets")
            .expect("GET /pets");
        assert_eq!(pets.state(), "matched");
        assert_eq!(pets.declared_in.as_deref(), Some("openapi.yaml"));
        assert_eq!(pets.observed_in.as_deref(), Some("server.js"));
        assert!(
            pets.handlers.iter().any(|(name, _)| name == "listPets"),
            "{:?}",
            pets.handlers
        );
        assert!(
            pets.consumers.iter().any(|(name, _, _)| name == "loadPets"),
            "{:?}",
            pets.consumers
        );
    }

    #[test]
    fn a_parameterized_path_pairs_across_its_two_spellings() {
        let root = indexed_root();

        let surfaces = surfaces(&root).unwrap();

        let parameterized: Vec<&Surface> = surfaces
            .iter()
            .filter(|surface| surface.path.starts_with("/pets/"))
            .collect();
        assert_eq!(
            parameterized.len(),
            1,
            "`/pets/{{id}}` and `/pets/:id` are one endpoint: {parameterized:?}"
        );
        assert_eq!(parameterized[0].state(), "matched");
        assert_eq!(
            parameterized[0].name, "GET /pets/{id}",
            "the contract's spelling is the published one"
        );
    }

    #[test]
    fn the_route_map_names_both_kinds_of_mismatch() {
        let root = indexed_root();

        let text = route_map(&root, "").unwrap();

        assert!(
            text.contains("declared endpoints no code implements: GET /archived"),
            "{text}"
        );
        assert!(
            text.contains("served endpoints no contract declares") && text.contains("GET /health"),
            "{text}"
        );
    }

    #[test]
    fn the_tool_map_lists_a_tool_with_its_handler() {
        let root = indexed_root();

        let text = tool_map(&root, "").unwrap();

        assert!(text.contains("TOOL search"), "{text}");
        assert!(text.contains("searchDocs"), "{text}");
        assert!(
            !route_map(&root, "").unwrap().contains("TOOL search"),
            "a tool is not an HTTP endpoint"
        );
    }

    #[test]
    fn the_shape_check_reports_a_field_the_handler_never_returns() {
        let root = indexed_root();

        let findings = shape_check(&root, "").unwrap();

        let pets = findings
            .iter()
            .find(|finding| finding.endpoint == "GET /pets")
            .unwrap_or_else(|| panic!("a finding for GET /pets: {findings:?}"));
        assert!(pets.missing.contains(&"name".to_string()), "{pets:?}");
        assert!(
            pets.missing_required.contains(&"name".to_string()),
            "the contract marks `name` required: {pets:?}"
        );
        assert!(pets.extra.contains(&"colour".to_string()), "{pets:?}");
        assert!(
            !pets.missing.contains(&"id".to_string()),
            "`id` is returned, so it is not missing: {pets:?}"
        );
    }

    #[test]
    fn a_url_built_from_a_base_and_a_parameter_still_names_its_endpoint() {
        let root = indexed_root();

        let surfaces = surfaces(&root).unwrap();

        let orders = surfaces
            .iter()
            .find(|surface| surface.name == "GET /orders")
            .expect("GET /orders");
        assert!(
            orders
                .consumers
                .iter()
                .any(|(name, _, _)| name == "loadOrders"),
            "`BASE + '/orders'` is a call to /orders: {:?}",
            orders.consumers
        );
        let parameterized = surfaces
            .iter()
            .find(|surface| surface.path.starts_with("/orders/"))
            .map(|surface| surface.consumers.clone())
            .unwrap_or_default();
        assert!(
            parameterized.iter().any(|(name, _, _)| name == "loadOrder"),
            "an interpolated id is a path parameter, and the base URL is host: {parameterized:?}"
        );
        assert!(
            surfaces.iter().all(|surface| !surface
                .consumers
                .iter()
                .any(|(name, _, _)| name == "loadSuffixed")),
            "`'/orders' + suffix` could be /ordersearch; a partial segment is skipped"
        );
    }

    #[test]
    fn the_shape_check_follows_a_body_built_in_a_variable_and_compares_nested_fields() {
        let root = indexed_root();

        let findings = shape_check(&root, "").unwrap();

        let orders = findings
            .iter()
            .find(|finding| finding.endpoint == "GET /orders")
            .unwrap_or_else(|| panic!("a finding for GET /orders: {findings:?}"));
        assert!(
            orders.missing.contains(&"customer.name".to_string()),
            "the nested required field is never set: {orders:?}"
        );
        assert!(
            orders
                .missing_required
                .contains(&"customer.name".to_string()),
            "and the contract marks it required: {orders:?}"
        );
        assert!(
            !orders.missing.contains(&"id".to_string())
                && !orders.missing.contains(&"customer".to_string()),
            "`id` arrives by spread, `customer` through a variable of its own, \
             and the body itself is a variable: {orders:?}"
        );
        assert!(
            !orders.extra.contains(&"customer.email".to_string()),
            "a declared nested field is not an extra one: {orders:?}"
        );
    }

    #[test]
    fn an_endpoint_with_no_declared_schema_is_not_a_finding() {
        let root = indexed_root();

        let findings = shape_check(&root, "").unwrap();

        assert!(
            findings
                .iter()
                .all(|finding| finding.endpoint != "GET /pets/{id}"),
            "nothing was declared to compare against: {findings:?}"
        );
    }

    #[test]
    fn api_impact_names_the_handler_and_the_consumer() {
        let root = indexed_root();

        let text = impact(&root, "GET /pets").unwrap();

        assert!(text.contains("listPets"), "{text}");
        assert!(text.contains("loadPets"), "{text}");
        assert!(text.contains("changing listPets"), "{text}");
        assert!(
            impact(&root, "GET /nowhere")
                .unwrap()
                .contains("no endpoint or tool matches"),
            "an unknown target says so"
        );
    }

    #[test]
    fn a_contract_nobody_calls_locally_says_that_rather_than_nothing() {
        let root = indexed_root();

        let text = impact(&root, "GET /archived").unwrap();

        assert!(text.contains("declared only"), "{text}");
        assert!(
            text.contains("no code in this repository consumes it"),
            "{text}"
        );
    }
}
