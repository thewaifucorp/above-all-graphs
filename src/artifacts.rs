//! Language-neutral database and infrastructure contract ingestion.

use std::{collections::HashMap, fs, path::Path};

use crate::{
    error::Result,
    storage::{
        Confidence, Edge, EdgeKind, EvidenceKind, Graph, Node, NodeKind, Perspective, Provenance,
    },
};

/// Number of declared artifact nodes indexed from one file, or `None` when unsupported.
pub(crate) fn index_artifact(graph: &Graph, relative: &str, path: &Path) -> Result<Option<u32>> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let Ok(source) = fs::read_to_string(path) else {
        return Ok(None);
    };
    match extension {
        "sql" => index_sql(graph, relative, &source).map(Some),
        "tf" | "hcl" => index_terraform(graph, relative, &source).map(Some),
        _ => Ok(None),
    }
}

fn index_sql(graph: &Graph, relative: &str, source: &str) -> Result<u32> {
    let provenance = provenance(relative, EvidenceKind::SqlSchema);
    let mut tables = HashMap::new();
    for statement in source.split(';') {
        let words = statement.split_whitespace().collect::<Vec<_>>();
        let Some(position) = words.windows(2).position(|pair| {
            pair[0].eq_ignore_ascii_case("create") && pair[1].eq_ignore_ascii_case("table")
        }) else {
            continue;
        };
        let Some(raw_name) = words.get(position + 2) else {
            continue;
        };
        let name = clean_identifier(raw_name);
        if name.is_empty() {
            continue;
        }
        let id = graph.insert_node_with_provenance(
            &Node {
                id: None,
                kind: NodeKind::DatabaseTable,
                name: name.to_string(),
                file_path: relative.to_string(),
                start_line: line_of(source, statement),
                end_line: line_of(source, statement)
                    + u32::try_from(statement.lines().count()).unwrap_or(u32::MAX),
                description: Some(statement.trim().to_string()),
            },
            &provenance,
        )?;
        tables.insert(name.to_ascii_lowercase(), id);
    }
    for statement in source.split(';') {
        let lower = statement.to_ascii_lowercase();
        let Some(create) = lower.find("create table") else {
            continue;
        };
        let source_name = clean_identifier(
            statement
                .get(create + 12..)
                .unwrap_or_default()
                .split_whitespace()
                .next()
                .unwrap_or_default(),
        )
        .to_ascii_lowercase();
        let Some(&source_id) = tables.get(&source_name) else {
            continue;
        };
        for reference in reference_names(statement) {
            if let Some(&target_id) = tables.get(&reference.to_ascii_lowercase()) {
                graph.insert_edge_with_provenance(
                    &Edge {
                        src: source_id,
                        dst: target_id,
                        kind: EdgeKind::References,
                        confidence: Confidence::Extracted,
                    },
                    &provenance,
                )?;
            }
        }
    }
    Ok(u32::try_from(tables.len()).unwrap_or(u32::MAX))
}

fn index_terraform(graph: &Graph, relative: &str, source: &str) -> Result<u32> {
    let provenance = provenance(relative, EvidenceKind::Infrastructure);
    let mut count = 0_u32;
    for (index, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        let kind = ["resource", "data", "module"]
            .into_iter()
            .find(|keyword| trimmed.starts_with(&format!("{keyword} \"")));
        let Some(kind) = kind else { continue };
        let quoted = trimmed.split('"').skip(1).step_by(2).collect::<Vec<_>>();
        let name = if kind == "module" {
            quoted.first().copied().unwrap_or_default().to_string()
        } else {
            format!(
                "{}.{}",
                quoted.first().copied().unwrap_or_default(),
                quoted.get(1).copied().unwrap_or_default()
            )
        };
        if name.trim_matches('.').is_empty() {
            continue;
        }
        graph.insert_node_with_provenance(
            &Node {
                id: None,
                kind: NodeKind::InfraResource,
                name,
                file_path: relative.to_string(),
                start_line: u32::try_from(index + 1).unwrap_or(u32::MAX),
                end_line: u32::try_from(index + 1).unwrap_or(u32::MAX),
                description: Some(format!("{kind} declaration")),
            },
            &provenance,
        )?;
        count = count.saturating_add(1);
    }
    Ok(count)
}

fn provenance(relative: &str, evidence_kind: EvidenceKind) -> Provenance {
    Provenance {
        perspective: Perspective::Declared,
        evidence_kind,
        evidence_source: Some(relative.to_string()),
    }
}

fn clean_identifier(value: &str) -> &str {
    value.trim_matches(|character: char| {
        matches!(character, '`' | '"' | '[' | ']' | '(' | ')' | ',')
    })
}

fn reference_names(statement: &str) -> Vec<&str> {
    let words = statement.split_whitespace().collect::<Vec<_>>();
    words
        .windows(2)
        .filter(|pair| pair[0].eq_ignore_ascii_case("references"))
        .map(|pair| clean_identifier(pair[1].split('(').next().unwrap_or(pair[1])))
        .collect()
}

fn line_of(source: &str, fragment: &str) -> u32 {
    source.find(fragment).map_or(1, |offset| {
        u32::try_from(source[..offset].lines().count() + 1).unwrap_or(u32::MAX)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_sql_tables_foreign_keys_and_terraform_resources() {
        let graph = Graph::open_in_memory().unwrap();
        assert_eq!(index_sql(&graph, "schema.sql", "CREATE TABLE users (id INT); CREATE TABLE posts (user_id INT REFERENCES users(id));").unwrap(), 2);
        let users = graph.find_by_name("users").unwrap().unwrap();
        assert!(
            graph
                .callers(users.id.unwrap())
                .unwrap()
                .iter()
                .any(|(node, kind, _)| node.name == "posts" && *kind == EdgeKind::References)
        );
        assert_eq!(
            index_terraform(
                &graph,
                "main.tf",
                "resource \"aws_s3_bucket\" \"assets\" {\n}\nmodule \"network\" {\n}"
            )
            .unwrap(),
            2
        );
        assert!(
            graph
                .find_by_name("aws_s3_bucket.assets")
                .unwrap()
                .is_some()
        );
    }
}
