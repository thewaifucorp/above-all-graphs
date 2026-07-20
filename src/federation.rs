//! Federated read/query surface across every locally registered AAG workspace.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::{
    error::{Error, Result},
    workspaces,
};

/// List registered repositories participating in the default federation.
#[must_use]
pub fn list() -> String {
    serde_json::to_string_pretty(&workspaces::live_entries()).unwrap_or_else(|_| "[]".into())
}

/// Query every live repository and group results by workspace.
///
/// # Errors
/// Returns an error only if all workspace queries fail.
pub fn query(question: &str) -> Result<String> {
    let mut results = Vec::new();
    let mut errors = Vec::new();
    for (name, path) in entries() {
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
    serde_json::to_string_pretty(&results).map_err(|error| Error::Protocol {
        context: "federated query serialization failed",
        detail: error.to_string(),
    })
}

/// Validate freshness and protocol manifests across registered workspaces.
#[must_use]
pub fn status() -> String {
    let rows = entries().into_iter().map(|(name, path)| {
        let manifest = path.join(".aag/context.yaml");
        let valid = manifest.is_file() && crate::protocol::run_validate(&manifest).is_ok();
        json!({"repository": name, "path": path, "indexed": path.join(".aag/graph.db").is_file(), "manifest_valid": valid})
    }).collect::<Vec<_>>();
    serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".into())
}

/// Collect declared API/database/infrastructure contracts from all manifests.
///
/// # Errors
/// Returns an error if a present manifest is malformed.
pub fn contracts() -> Result<String> {
    let mut rows = Vec::new();
    for (name, path) in entries() {
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
        rows.push(json!({
            "repository": name,
            "path": path,
            "api": value.pointer("/extensions/x-aag-declared-contracts").cloned(),
            "artifacts": value.pointer("/extensions/x-aag-declared-artifacts").cloned()
        }));
    }
    serde_json::to_string_pretty(&rows).map_err(|error| Error::Protocol {
        context: "federated contracts serialization failed",
        detail: error.to_string(),
    })
}

/// Synchronize every registered workspace and regenerate its artifacts.
///
/// # Errors
/// Returns a combined error if any workspace fails to synchronize.
pub fn sync() -> Result<String> {
    let mut synced = Vec::new();
    let mut errors = Vec::new();
    for (name, path) in entries() {
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

fn entries() -> Vec<(String, PathBuf)> {
    workspaces::live_entries()
        .into_iter()
        .filter_map(|entry| {
            let name = entry.get("name")?.as_str()?.to_string();
            let path = Path::new(entry.get("path")?.as_str()?).to_path_buf();
            Some((name, path))
        })
        .collect()
}
