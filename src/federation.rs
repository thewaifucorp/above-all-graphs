//! Federated queries plus persistent named, hierarchical repository groups.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::workspaces;

fn groups_path() -> Option<PathBuf> {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|home| PathBuf::from(home).join(".config"))
        })?;
    Some(config_home.join("aag").join("groups.json"))
}

fn read_groups(path: &Path) -> BTreeMap<String, BTreeSet<String>> {
    let value = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or_else(|| json!({}));
    value
        .get("groups")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(name, members)| {
            let members = members
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            (name.clone(), members)
        })
        .collect()
}

fn write_groups(path: &Path, groups: &BTreeMap<String, BTreeSet<String>>) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let value = json!({"groups": groups});
    std::fs::write(
        path,
        serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".into()) + "\n",
    )
    .map_err(|source| Error::Write {
        path: path.to_path_buf(),
        source,
    })
}

fn validate_group(name: &str) -> Result<()> {
    if name.is_empty()
        || name == "all"
        || name.split('/').any(|part| {
            part.is_empty()
                || !part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        })
    {
        return Err(Error::Protocol {
            context: "invalid group name",
            detail: "use hierarchical names such as `platform/backend`".into(),
        });
    }
    Ok(())
}

/// Creates an empty named group. Slash-separated names form a hierarchy.
/// # Errors
/// Returns an error for invalid names or an unwritable registry.
pub fn create(name: &str) -> Result<String> {
    validate_group(name)?;
    let path = groups_path().ok_or_else(|| Error::Protocol {
        context: "group storage unavailable",
        detail: "no config/home directory".into(),
    })?;
    let mut groups = read_groups(&path);
    groups.entry(name.to_string()).or_default();
    write_groups(&path, &groups)?;
    Ok(format!("created group {name}"))
}

/// Adds a registered workspace (by unique name or absolute path) to a group.
/// # Errors
/// Returns an error when the workspace cannot be resolved or storage fails.
pub fn add(name: &str, repository: &str) -> Result<String> {
    validate_group(name)?;
    let resolved = resolve_workspace(repository)?;
    let path = groups_path().ok_or_else(|| Error::Protocol {
        context: "group storage unavailable",
        detail: "no config/home directory".into(),
    })?;
    let mut groups = read_groups(&path);
    groups
        .entry(name.to_string())
        .or_default()
        .insert(resolved.to_string_lossy().to_string());
    write_groups(&path, &groups)?;
    Ok(format!("added {} to {name}", resolved.display()))
}

/// Removes a workspace from a group without deleting its graph.
/// # Errors
/// Returns an error when the group/workspace is missing or storage fails.
pub fn remove(name: &str, repository: &str) -> Result<String> {
    let resolved = resolve_workspace(repository)?;
    let path = groups_path().ok_or_else(|| Error::Protocol {
        context: "group storage unavailable",
        detail: "no config/home directory".into(),
    })?;
    let mut groups = read_groups(&path);
    let Some(members) = groups.get_mut(name) else {
        return group_missing(name);
    };
    members.remove(&resolved.to_string_lossy().to_string());
    write_groups(&path, &groups)?;
    Ok(format!("removed {} from {name}", resolved.display()))
}

/// Lists groups, or repositories in one group including all descendants.
/// # Errors
/// Returns an error when a selected group does not exist.
pub fn list_group(name: Option<&str>) -> Result<String> {
    let groups = groups_path().map_or_else(BTreeMap::new, |path| read_groups(&path));
    if let Some(name) = name {
        return serde_json::to_string_pretty(&entries_for(name, &groups)?).map_err(json_error);
    }
    let rows = groups
        .iter()
        .map(|(name, members)| json!({"name": name, "direct_members": members.len()}))
        .collect::<Vec<_>>();
    serde_json::to_string_pretty(&rows).map_err(json_error)
}

/// Lists all workspaces in the default federation.
#[must_use]
pub fn list() -> String {
    serde_json::to_string_pretty(&workspaces::live_entries()).unwrap_or_else(|_| "[]".into())
}

/// Queries all repositories selected by `group` (`all` selects the federation).
/// # Errors
/// Returns an error when the group is missing or all repository queries fail.
pub fn query_group(group: &str, question: &str) -> Result<String> {
    let mut results = Vec::new();
    let mut errors = Vec::new();
    for (name, path) in selected_entries(group)? {
        match crate::explore::format(&path, question) {
            Ok(text) if !text.starts_with("no matches") => {
                results.push(json!({"repository": name, "path": path, "result": text}));
            }
            Ok(_) => {}
            Err(error) => errors.push(error.to_string()),
        }
    }
    if results.is_empty() && !errors.is_empty() {
        return Err(Error::Protocol {
            context: "federated query failed",
            detail: errors.join("; "),
        });
    }
    serde_json::to_string_pretty(&results).map_err(json_error)
}

/// Backwards-compatible query over every workspace.
/// # Errors
/// Returns an error when every workspace query fails.
pub fn query(question: &str) -> Result<String> {
    query_group("all", question)
}

/// Validates index and protocol-manifest status for a selected group.
/// # Errors
/// Returns an error when the group does not exist.
pub fn status_group(group: &str) -> Result<String> {
    let rows = selected_entries(group)?.into_iter().map(|(name, path)| {
        let manifest = path.join(".aag/context.yaml");
        let valid = manifest.is_file() && crate::protocol::run_validate(&manifest).is_ok();
        json!({"repository": name, "path": path, "indexed": path.join(".aag/graph.db").is_file(), "manifest_valid": valid})
    }).collect::<Vec<_>>();
    serde_json::to_string_pretty(&rows).map_err(json_error)
}

/// Backwards-compatible status over every workspace.
#[must_use]
pub fn status() -> String {
    status_group("all").unwrap_or_else(|_| "[]".into())
}

/// Collects declared contracts from a selected group.
/// # Errors
/// Returns an error for missing groups or unreadable/malformed manifests.
pub fn contracts_group(group: &str) -> Result<String> {
    let mut rows = Vec::new();
    for (name, path) in selected_entries(group)? {
        let manifest = path.join(".aag/context.yaml");
        if !manifest.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&manifest).map_err(|source| Error::Io {
            path: manifest.clone(),
            source,
        })?;
        let value: Value = serde_yaml_ng::from_str(&text).map_err(|error| Error::Protocol {
            context: "federated manifest parse failed",
            detail: error.to_string(),
        })?;
        rows.push(json!({"repository": name, "path": path, "api": value.pointer("/extensions/x-aag-declared-contracts").cloned(), "artifacts": value.pointer("/extensions/x-aag-declared-artifacts").cloned()}));
    }
    serde_json::to_string_pretty(&rows).map_err(json_error)
}

/// Backwards-compatible contracts over every workspace.
/// # Errors
/// Returns an error for unreadable or malformed manifests.
pub fn contracts() -> Result<String> {
    contracts_group("all")
}

/// Synchronizes all repositories selected by a group.
/// # Errors
/// Returns an error when the group is missing or a workspace fails to sync.
pub fn sync_group(group: &str) -> Result<String> {
    let mut synced = Vec::new();
    let mut errors = Vec::new();
    for (name, path) in selected_entries(group)? {
        match crate::sync::run(&path, None, false) {
            Ok(()) => synced.push(name),
            Err(error) => errors.push(format!("{}: {error}", path.display())),
        }
    }
    if errors.is_empty() {
        Ok(format!(
            "synced {} workspace(s): {}",
            synced.len(),
            synced.join(", ")
        ))
    } else {
        Err(Error::Protocol {
            context: "federated sync failed",
            detail: errors.join("; "),
        })
    }
}

/// Backwards-compatible sync over every workspace.
/// # Errors
/// Returns an error when a workspace fails to synchronize.
pub fn sync() -> Result<String> {
    sync_group("all")
}

fn selected_entries(group: &str) -> Result<Vec<(String, PathBuf)>> {
    if group == "all" {
        return Ok(entries());
    }
    let groups = groups_path().map_or_else(BTreeMap::new, |path| read_groups(&path));
    entries_for(group, &groups)
}

fn entries_for(
    group: &str,
    groups: &BTreeMap<String, BTreeSet<String>>,
) -> Result<Vec<(String, PathBuf)>> {
    let paths = member_paths(group, groups)?;
    Ok(entries()
        .into_iter()
        .filter(|(_, path)| paths.contains(&path.to_string_lossy().to_string()))
        .collect())
}

fn member_paths<'a>(
    group: &str,
    groups: &'a BTreeMap<String, BTreeSet<String>>,
) -> Result<BTreeSet<&'a String>> {
    if !groups.contains_key(group)
        && !groups
            .keys()
            .any(|name| name.starts_with(&format!("{group}/")))
    {
        return group_missing(group);
    }
    Ok(groups
        .iter()
        .filter(|(name, _)| *name == group || name.starts_with(&format!("{group}/")))
        .flat_map(|(_, members)| members)
        .collect())
}

fn resolve_workspace(repository: &str) -> Result<PathBuf> {
    let exact = Path::new(repository).canonicalize().ok();
    let matches = entries()
        .into_iter()
        .filter(|(name, path)| name == repository || exact.as_ref() == Some(path))
        .map(|(_, path)| path)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [path] => Ok(path.clone()),
        [] => Err(Error::Protocol {
            context: "workspace not found",
            detail: repository.into(),
        }),
        _ => Err(Error::Protocol {
            context: "workspace name is ambiguous",
            detail: "pass its absolute path".into(),
        }),
    }
}

fn entries() -> Vec<(String, PathBuf)> {
    workspaces::live_entries()
        .into_iter()
        .filter_map(|entry| {
            Some((
                entry.get("name")?.as_str()?.to_string(),
                Path::new(entry.get("path")?.as_str()?).to_path_buf(),
            ))
        })
        .collect()
}

fn group_missing<T>(name: &str) -> Result<T> {
    Err(Error::Protocol {
        context: "group not found",
        detail: name.into(),
    })
}
#[allow(clippy::needless_pass_by_value)]
fn json_error(error: serde_json::Error) -> Error {
    Error::Protocol {
        context: "federation serialization failed",
        detail: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two repositories that talk to each other five different ways, each
    /// indexed on its own.
    fn linked_pair() -> (PathBuf, PathBuf) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!("aag-links-{}-{n}", std::process::id()));
        let producer = base.join("service");
        let consumer = base.join("web");
        for path in [&producer, &consumer] {
            let _ = std::fs::remove_dir_all(path);
            std::fs::create_dir_all(path).unwrap();
        }
        std::fs::write(
            producer.join("package.json"),
            "{\"name\": \"@shop/service\"}",
        )
        .unwrap();
        std::fs::write(
            producer.join("openapi.yaml"),
            "openapi: 3.0.0\ninfo:\n  title: Shop\n  version: '1'\npaths:\n  /orders/{id}:\n    get:\n      operationId: getOrder\n      responses:\n        '200':\n          description: one order\ncomponents:\n  schemas:\n    Order:\n      type: object\n      properties:\n        id:\n          type: integer\n",
        )
        .unwrap();
        std::fs::write(
            producer.join("index.js"),
            "function getOrder() {}\nfunction wire(app) { app.get('/orders/:id', getOrder); }\n             function placeOrder(bus) { bus.emit('order.created', {}); }\n             function wireTools(server) { server.tool('lookupOrder', getOrder); }\n             module.exports = { getOrder, placeOrder };\n",
        )
        .unwrap();
        std::fs::write(
            consumer.join("package.json"),
            "{\"name\": \"@shop/web\", \"dependencies\": {\"@shop/service\": \"1.0.0\"}}",
        )
        .unwrap();
        std::fs::write(
            consumer.join("app.js"),
            "import { getOrder } from '@shop/service';\n             class Order { constructor() { this.id = 0; } }\n             function loadOrder(client) { return client.get('/orders/42'); }\n             function onCreated(bus) { bus.subscribe('order.created', () => {}); }\n             function search(mcp) { return mcp.call_tool('lookupOrder'); }\n",
        )
        .unwrap();
        for path in [&producer, &consumer] {
            crate::bigbang::run(
                path,
                &crate::bigbang::Options {
                    no_viz: true,
                    no_install: true,
                    ..crate::bigbang::Options::default()
                },
            )
            .unwrap();
        }
        (producer, consumer)
    }

    fn link_kinds(payload: &str) -> BTreeSet<String> {
        serde_json::from_str::<Value>(payload).unwrap()["links"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|link| link["kind"].as_str().map(str::to_string))
            .collect()
    }

    #[test]
    fn five_kinds_of_cross_repository_link_are_found_without_merging_graphs() {
        let (producer, consumer) = linked_pair();
        let members = vec![
            ("service".to_string(), producer),
            ("web".to_string(), consumer),
        ];

        let payload = links_across("test", &members).unwrap();

        let kinds = link_kinds(&payload);
        for expected in ["api", "package", "event", "schema", "tool"] {
            assert!(
                kinds.contains(expected),
                "missing a {expected} link: {payload}"
            );
        }
    }

    #[test]
    fn an_api_link_names_the_caller_the_endpoint_and_its_evidence() {
        let (producer, consumer) = linked_pair();
        let members = vec![
            ("service".to_string(), producer),
            ("web".to_string(), consumer),
        ];

        let payload = links_across("test", &members).unwrap();
        let value: Value = serde_json::from_str(&payload).unwrap();
        let api = value["links"]
            .as_array()
            .unwrap()
            .iter()
            .find(|link| link["kind"] == "api")
            .unwrap_or_else(|| panic!("an api link: {payload}"));

        assert_eq!(api["from"]["repository"], "web");
        assert_eq!(api["from"]["name"], "loadOrder");
        assert_eq!(api["to"]["repository"], "service");
        assert!(
            api["to"]["name"].as_str().unwrap().contains("/orders/"),
            "{api}"
        );
        assert_eq!(
            api["evidence"],
            "the paths match once parameters are flattened"
        );
    }

    #[test]
    fn a_repository_is_not_linked_to_itself() {
        let (producer, _) = linked_pair();
        let members = vec![("service".to_string(), producer)];

        let payload = links_across("test", &members).unwrap();

        assert!(
            link_kinds(&payload).is_empty(),
            "a cross-repository link needs two repositories: {payload}"
        );
    }

    #[test]
    fn an_unreadable_member_is_reported_rather_than_failing_the_group() {
        let (producer, _) = linked_pair();
        let members = vec![
            ("service".to_string(), producer),
            (
                "never-indexed".to_string(),
                std::env::temp_dir().join("aag-missing-repo"),
            ),
        ];

        let payload = links_across("test", &members).unwrap();
        let value: Value = serde_json::from_str(&payload).unwrap();

        assert_eq!(value["unreadable"].as_array().unwrap().len(), 1);
        assert!(
            value["unreadable"][0]
                .as_str()
                .unwrap()
                .starts_with("never-indexed"),
            "{payload}"
        );
    }

    #[test]
    fn parent_group_includes_descendant_members() {
        let groups = BTreeMap::from([
            ("platform".into(), BTreeSet::from(["/repos/core".into()])),
            (
                "platform/backend".into(),
                BTreeSet::from(["/repos/api".into()]),
            ),
            ("sales".into(), BTreeSet::from(["/repos/crm".into()])),
        ]);
        let paths = member_paths("platform", &groups).unwrap();
        assert_eq!(paths.len(), 2);
        assert!(paths.iter().any(|path| path.as_str() == "/repos/core"));
        assert!(paths.iter().any(|path| path.as_str() == "/repos/api"));
    }

    #[test]
    fn group_names_reject_empty_segments() {
        assert!(validate_group("platform/backend").is_ok());
        assert!(validate_group("platform//backend").is_err());
        assert!(validate_group("all").is_err());
    }
}

// ---------------------------------------------------------------------------
// Cross-repository protocol links (P1.7)
// ---------------------------------------------------------------------------

/// One repository's half of every cross-repository link, read from its own
/// graph. Nothing is merged: each repository keeps its database, its ids, and
/// its ownership, and the links are computed over these summaries.
#[derive(Debug, Default, Clone)]
struct Facts {
    /// Endpoints this repository serves or declares, as `METHOD /path`.
    endpoints: BTreeSet<String>,
    /// `(calling symbol, METHOD /path)` for endpoints it consumes.
    consumes: Vec<(String, String)>,
    /// Package names this repository publishes, from its own manifests.
    packages: BTreeSet<String>,
    /// Symbols it declares, which is what another repository can import.
    exports: BTreeSet<String>,
    /// `(module source as written, importing file)`.
    imports: Vec<(String, String)>,
    /// `(publishing symbol, event name)`.
    emits: Vec<(String, String)>,
    /// `(listening symbol, event name)`.
    listens: Vec<(String, String)>,
    /// Schema names a contract declares here.
    schemas: BTreeSet<String>,
    /// Type names declared in code here, which a schema may correspond to.
    models: BTreeSet<String>,
    /// Tool names this repository defines.
    tools: BTreeSet<String>,
    /// `(calling symbol, tool name)` for tools it invokes.
    tool_calls: Vec<(String, String)>,
}

/// Reads one repository's facts from its own graph.
fn facts(path: &Path) -> Result<Facts> {
    let graph = crate::storage::Graph::open_existing(path)?;
    let mut facts = Facts {
        packages: manifest_packages(path),
        ..Facts::default()
    };
    for node in graph.all_nodes()? {
        match node.kind {
            crate::storage::NodeKind::Endpoint => {
                if let Some(name) = node.name.strip_prefix("TOOL ") {
                    facts.tools.insert(name.to_string());
                } else {
                    facts.endpoints.insert(node.name);
                }
            }
            crate::storage::NodeKind::Schema => {
                facts.schemas.insert(node.name);
            }
            crate::storage::NodeKind::Struct | crate::storage::NodeKind::Interface => {
                facts.models.insert(node.name.clone());
                facts.exports.insert(node.name);
            }
            crate::storage::NodeKind::File => {}
            _ => {
                facts.exports.insert(node.name);
            }
        }
    }
    for reference in graph.raw_references()? {
        let owner = reference.owner.clone();
        match reference.kind.as_str() {
            "consumer" if !owner.is_empty() => facts.consumes.push((owner, reference.target)),
            "event_emit" if !owner.is_empty() => facts.emits.push((owner, reference.target)),
            "event_listen" if !owner.is_empty() => facts.listens.push((owner, reference.target)),
            "tool_call" if !owner.is_empty() => facts.tool_calls.push((owner, reference.target)),
            "import" => {
                let source = serde_json::from_str::<Value>(&reference.target)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("source")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or(reference.target);
                facts.imports.push((source, reference.file_path));
            }
            _ => {}
        }
    }
    Ok(facts)
}

/// Package names a repository publishes, read from its own manifests. No
/// registry is consulted and nothing is fetched.
fn manifest_packages(path: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    if let Ok(text) = std::fs::read_to_string(path.join("package.json"))
        && let Ok(value) = serde_json::from_str::<Value>(&text)
        && let Some(name) = value.get("name").and_then(Value::as_str)
    {
        names.insert(name.to_string());
    }
    if let Ok(text) = std::fs::read_to_string(path.join("Cargo.toml")) {
        let mut in_package = false;
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                in_package = trimmed == "[package]";
                continue;
            }
            if in_package
                && let Some((key, value)) = trimmed.split_once('=')
                && key.trim() == "name"
            {
                names.insert(value.trim().trim_matches(['"', '\'']).to_string());
            }
        }
    }
    if let Ok(text) = std::fs::read_to_string(path.join("go.mod"))
        && let Some(line) = text.lines().find(|line| line.starts_with("module "))
    {
        names.insert(line.trim_start_matches("module ").trim().to_string());
    }
    names
}

/// The path shape two spellings of one endpoint share, so `/pets/{id}` in a
/// contract and `/pets/42` in a caller pair up.
fn shape(name: &str) -> String {
    let Some((method, path)) = name.split_once(' ') else {
        return name.to_string();
    };
    if !path.starts_with('/') {
        return name.to_string();
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

/// Cross-repository protocol links across a group, without merging graphs.
///
/// Five kinds, each a pair of halves that live in different repositories:
/// API producer to client, package export to import, event producer to consumer,
/// schema to model, and tool definition to invocation.
///
/// Every link is a *name* agreeing across an ownership boundary, so each one
/// carries the evidence that produced it and none is presented as certain. A
/// repository's own database is opened read-only and never written: federation
/// here is selection, not unification.
///
/// # Errors
/// Returns an error when the group is unknown, or when a member's graph cannot
/// be opened.
pub fn links_group(group: &str) -> Result<String> {
    links_across(group, &selected_entries(group)?)
}

/// The linking itself, over an explicit member list.
///
/// Split out from [`links_group`] so it can be exercised against scratch
/// repositories without touching the machine's workspace registry.
///
/// # Errors
/// Returns an error when the result cannot be serialized.
fn links_across(group: &str, members: &[(String, PathBuf)]) -> Result<String> {
    let mut gathered: Vec<(String, Facts)> = Vec::new();
    let mut unreadable = Vec::new();
    for (name, path) in members {
        match facts(path) {
            Ok(facts) => gathered.push((name.clone(), facts)),
            Err(error) => unreadable.push(format!("{name}: {error}")),
        }
    }
    let mut links: Vec<Value> = Vec::new();
    for (consumer_name, consumer) in &gathered {
        for (producer_name, producer) in &gathered {
            if producer_name == consumer_name {
                continue;
            }
            links.extend(pair_repositories(
                consumer_name,
                consumer,
                producer_name,
                producer,
            ));
        }
    }
    let payload = json!({
        "group": group,
        "repositories": gathered.iter().map(|(name, _)| name).collect::<Vec<_>>(),
        "links": links,
        "unreadable": unreadable,
        "note": "Each link is a name agreeing across an ownership boundary. Graphs are read \
                 separately and never merged.",
    });
    serde_json::to_string_pretty(&payload).map_err(json_error)
}

/// Links whose producing half is in `producer` and consuming half in `consumer`.
fn pair_repositories(
    consumer_name: &str,
    consumer: &Facts,
    producer_name: &str,
    producer: &Facts,
) -> Vec<Value> {
    let mut links = Vec::new();
    let served: BTreeMap<String, &String> = producer
        .endpoints
        .iter()
        .map(|endpoint| (shape(endpoint), endpoint))
        .collect();
    for (caller, target) in &consumer.consumes {
        if let Some(endpoint) = served.get(&shape(target)) {
            links.push(link(
                "api",
                consumer_name,
                caller,
                producer_name,
                endpoint,
                if *endpoint == target {
                    "the caller names the endpoint exactly"
                } else {
                    "the paths match once parameters are flattened"
                },
            ));
        }
    }
    for (source, file) in &consumer.imports {
        let Some(package) = producer
            .packages
            .iter()
            .find(|package| source == *package || source.starts_with(&format!("{package}/")))
        else {
            continue;
        };
        let symbol = source
            .rsplit('/')
            .next()
            .filter(|tail| producer.exports.contains(*tail))
            .unwrap_or(package);
        links.push(link(
            "package",
            consumer_name,
            file,
            producer_name,
            symbol,
            "the import names a package this repository publishes",
        ));
    }
    for (listener, event) in &consumer.listens {
        for (emitter, emitted) in &producer.emits {
            if emitted == event {
                links.push(link(
                    "event",
                    producer_name,
                    emitter,
                    consumer_name,
                    listener,
                    "publisher and listener name the same event",
                ));
            }
        }
    }
    for schema in &producer.schemas {
        if consumer.models.contains(schema) {
            links.push(link(
                "schema",
                producer_name,
                schema,
                consumer_name,
                schema,
                "a declared schema and a type share a name",
            ));
        }
    }
    for (caller, tool) in &consumer.tool_calls {
        if producer.tools.contains(tool) {
            links.push(link(
                "tool",
                consumer_name,
                caller,
                producer_name,
                &format!("TOOL {tool}"),
                "the call names a tool this repository defines",
            ));
        }
    }
    links
}

fn link(
    kind: &str,
    from_repository: &str,
    from: &str,
    to_repository: &str,
    to: &str,
    evidence: &str,
) -> Value {
    json!({
        "kind": kind,
        "from": {"repository": from_repository, "name": from},
        "to": {"repository": to_repository, "name": to},
        "evidence": evidence,
    })
}

/// Cross-repository links over every registered workspace.
///
/// # Errors
/// As [`links_group`].
pub fn links() -> Result<String> {
    links_group("all")
}
