//! Read-only GitHub pull-request intelligence enriched with local graph impact.

use std::{collections::BTreeSet, path::Path, process::Command};

use serde_json::json;

use crate::{
    error::{Error, Result},
    storage::Graph,
};

/// List open pull requests through the authenticated GitHub CLI.
///
/// # Errors
/// Returns an error when `gh` is unavailable, unauthenticated, or the request fails.
pub fn list(root: &Path, base: &str) -> Result<String> {
    let mut args = vec![
        "pr",
        "list",
        "--limit",
        "100",
        "--json",
        "number,title,headRefName,baseRefName,isDraft,reviewDecision,statusCheckRollup,updatedAt",
    ];
    if !base.trim().is_empty() {
        args.extend(["--base", base.trim()]);
    }
    gh(root, &args)
}

/// Return changed files, communities, symbols, and affected tests for one PR.
///
/// # Errors
/// Returns an error when GitHub or the local graph cannot be queried.
pub fn impact(root: &Path, number: &str) -> Result<String> {
    let output = gh(root, &["pr", "diff", number.trim(), "--name-only"])?;
    let files = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    impact_files(root, number, &files)
}

/// List non-draft PRs as the actionable triage set.
///
/// # Errors
/// Returns an error when GitHub cannot be queried or its response is malformed.
pub fn triage(root: &Path, base: &str) -> Result<String> {
    let raw = list(root, base)?;
    let mut prs: Vec<serde_json::Value> =
        serde_json::from_str(&raw).map_err(|error| Error::Protocol {
            context: "GitHub PR response parse failed",
            detail: error.to_string(),
        })?;
    prs.retain(|pr| {
        !pr.get("isDraft")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    });
    serde_json::to_string_pretty(&prs).map_err(|error| Error::Protocol {
        context: "GitHub PR response serialization failed",
        detail: error.to_string(),
    })
}

fn impact_files(root: &Path, number: &str, files: &[String]) -> Result<String> {
    let graph = Graph::open_existing(root)?;
    let nodes = graph.all_nodes()?;
    let edges = graph.all_edges()?;
    let changed: BTreeSet<&str> = files.iter().map(String::as_str).collect();
    let touched_ids: BTreeSet<i64> = nodes
        .iter()
        .filter(|node| changed.contains(node.file_path.as_str()))
        .filter_map(|node| node.id)
        .collect();
    let community_ids = crate::analysis::communities(&nodes, &edges)
        .into_iter()
        .filter(|community| {
            community
                .members
                .iter()
                .any(|member| touched_ids.contains(member))
        })
        .map(|community| community.id)
        .collect::<Vec<_>>();
    let affected_tests = crate::refactor::affected(root, files)?;
    serde_json::to_string_pretty(&json!({
        "pr": number,
        "changed_files": files,
        "touched_nodes": touched_ids.len(),
        "communities": community_ids,
        "affected_tests": affected_tests
    }))
    .map_err(|error| Error::Protocol {
        context: "PR impact serialization failed",
        detail: error.to_string(),
    })
}

fn gh(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("gh")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|error| Error::Protocol {
            context: "GitHub CLI invocation failed",
            detail: error.to_string(),
        })?;
    if !output.status.success() {
        return Err(Error::Protocol {
            context: "GitHub CLI request failed",
            detail: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_impact_from_files_reports_touched_nodes() {
        let root = std::env::temp_dir().join(format!("aag-pr-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("lib.rs"), "fn changed() {}").unwrap();
        crate::bigbang::run(
            &root,
            &crate::bigbang::Options {
                no_viz: true,
                no_install: true,
                ..Default::default()
            },
        )
        .unwrap();
        let report = impact_files(&root, "7", &["lib.rs".into()]).unwrap();
        assert!(report.contains("\"touched_nodes\": 2"));
    }
}
