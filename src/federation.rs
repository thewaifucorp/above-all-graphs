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
