//! Deterministic graph-level architecture analysis: communities and execution processes.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use crate::storage::{Edge, EdgeKind, Node, NodeKind};
use crate::{error::Result, storage::Graph};

/// A densely-related group discovered without source-language assumptions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Community {
    /// Stable representative node id.
    pub id: i64,
    /// Member node ids, sorted.
    pub members: Vec<i64>,
}

/// An entrypoint and the observed call graph reachable from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Process {
    /// Entrypoint node id.
    pub entrypoint: i64,
    /// Reachable node ids in breadth-first order.
    pub steps: Vec<i64>,
}

/// Deterministic label-propagation communities over the undirected graph.
#[must_use]
pub fn communities(nodes: &[Node], edges: &[Edge]) -> Vec<Community> {
    let ids: BTreeSet<i64> = nodes.iter().filter_map(|node| node.id).collect();
    let mut neighbors: HashMap<i64, Vec<i64>> = HashMap::new();
    for edge in edges {
        neighbors.entry(edge.src).or_default().push(edge.dst);
        neighbors.entry(edge.dst).or_default().push(edge.src);
    }
    let mut labels: HashMap<i64, i64> = ids.iter().map(|id| (*id, *id)).collect();
    for _ in 0..12 {
        let mut changed = false;
        for id in &ids {
            let mut votes: BTreeMap<i64, usize> = BTreeMap::new();
            for neighbor in neighbors.get(id).into_iter().flatten() {
                *votes
                    .entry(*labels.get(neighbor).unwrap_or(neighbor))
                    .or_default() += 1;
            }
            let selected = votes
                .into_iter()
                .max_by(|(left_label, left_count), (right_label, right_count)| {
                    left_count
                        .cmp(right_count)
                        .then_with(|| right_label.cmp(left_label))
                })
                .map_or(*id, |(label, _)| label);
            if labels.insert(*id, selected) != Some(selected) {
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut grouped: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for (id, label) in labels {
        grouped.entry(label).or_default().push(id);
    }
    grouped
        .into_iter()
        .filter_map(|(id, mut members)| {
            members.sort_unstable();
            (members.len() > 1).then_some(Community { id, members })
        })
        .collect()
}

/// Detect call-graph roots and their reachable execution processes.
#[must_use]
pub fn processes(nodes: &[Node], edges: &[Edge]) -> Vec<Process> {
    let callable: BTreeSet<i64> = nodes
        .iter()
        .filter(|node| matches!(node.kind, NodeKind::Function | NodeKind::Method))
        .filter_map(|node| node.id)
        .collect();
    let mut incoming = HashSet::new();
    let mut outgoing: HashMap<i64, Vec<i64>> = HashMap::new();
    for edge in edges.iter().filter(|edge| edge.kind == EdgeKind::Calls) {
        if callable.contains(&edge.src) && callable.contains(&edge.dst) {
            incoming.insert(edge.dst);
            outgoing.entry(edge.src).or_default().push(edge.dst);
        }
    }
    let mut found = Vec::new();
    for root in callable.iter().filter(|id| !incoming.contains(id)) {
        let mut queue = VecDeque::from([*root]);
        let mut visited = HashSet::new();
        let mut steps = Vec::new();
        while let Some(id) = queue.pop_front() {
            if !visited.insert(id) || steps.len() >= 100 {
                continue;
            }
            steps.push(id);
            let mut next = outgoing.get(&id).cloned().unwrap_or_default();
            next.sort_unstable();
            queue.extend(next);
        }
        if steps.len() > 1 {
            found.push(Process {
                entrypoint: *root,
                steps,
            });
        }
    }
    found
}

/// Format communities for CLI and agent consumption.
///
/// # Errors
/// Returns a storage error when the index cannot be read.
pub fn communities_format(root: &std::path::Path, query: &str) -> Result<String> {
    let graph = Graph::open_existing(root)?;
    let nodes = graph.all_nodes()?;
    let edges = graph.all_edges()?;
    let by_id: HashMap<i64, &Node> = nodes
        .iter()
        .filter_map(|node| node.id.map(|id| (id, node)))
        .collect();
    let mut lines = Vec::new();
    for community in communities(&nodes, &edges) {
        let names = community
            .members
            .iter()
            .filter_map(|id| by_id.get(id).map(|node| node.name.as_str()))
            .collect::<Vec<_>>();
        if query.is_empty() || names.iter().any(|name| name.contains(query)) {
            lines.push(format!(
                "community {} ({} nodes): {}",
                community.id,
                names.len(),
                names.join(", ")
            ));
        }
    }
    Ok(if lines.is_empty() {
        "no communities found".into()
    } else {
        lines.join("\n")
    })
}

/// Format detected processes for CLI and agent consumption.
///
/// # Errors
/// Returns a storage error when the index cannot be read.
pub fn processes_format(root: &std::path::Path, query: &str) -> Result<String> {
    let graph = Graph::open_existing(root)?;
    let nodes = graph.all_nodes()?;
    let edges = graph.all_edges()?;
    let by_id: HashMap<i64, &Node> = nodes
        .iter()
        .filter_map(|node| node.id.map(|id| (id, node)))
        .collect();
    let mut lines = Vec::new();
    for process in processes(&nodes, &edges) {
        let Some(entrypoint) = by_id.get(&process.entrypoint) else {
            continue;
        };
        if !query.is_empty() && !entrypoint.name.contains(query) {
            continue;
        }
        let names = process
            .steps
            .iter()
            .filter_map(|id| by_id.get(id).map(|node| node.name.as_str()))
            .collect::<Vec<_>>();
        lines.push(format!("{}: {}", entrypoint.name, names.join(" -> ")));
    }
    Ok(if lines.is_empty() {
        "no processes found".into()
    } else {
        lines.join("\n")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{Confidence, EdgeKind};

    fn node(id: i64, name: &str) -> Node {
        Node {
            id: Some(id),
            kind: NodeKind::Function,
            name: name.into(),
            file_path: "a.rs".into(),
            start_line: 1,
            end_line: 1,
            description: None,
        }
    }

    #[test]
    fn detects_community_and_rooted_process() {
        let nodes = vec![node(1, "main"), node(2, "service"), node(3, "save")];
        let edges = vec![
            Edge {
                src: 1,
                dst: 2,
                kind: EdgeKind::Calls,
                confidence: Confidence::Inferred,
            },
            Edge {
                src: 2,
                dst: 3,
                kind: EdgeKind::Calls,
                confidence: Confidence::Inferred,
            },
        ];
        assert_eq!(
            communities(&nodes, &edges)
                .iter()
                .map(|item| item.members.len())
                .sum::<usize>(),
            3
        );
        assert_eq!(
            processes(&nodes, &edges),
            vec![Process {
                entrypoint: 1,
                steps: vec![1, 2, 3]
            }]
        );
    }
}
