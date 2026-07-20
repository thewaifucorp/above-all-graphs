//! Optional local semantic embeddings and vector retrieval.

#[cfg(feature = "semantic")]
use std::cmp::Ordering;
use std::path::Path;

use crate::error::{Error, Result};
use crate::storage::{Graph, Node};

/// Stable model id persisted alongside vectors.
pub const MODEL: &str = "fastembed/all-MiniLM-L6-v2";

/// Generates embeddings for every indexed node using local ONNX inference.
///
/// # Errors
/// Returns an error when the model cannot load or vectors cannot be stored.
#[cfg(feature = "semantic")]
pub fn build(root: &Path) -> Result<usize> {
    use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
    let graph = Graph::open_existing(root)?;
    let nodes = graph.all_nodes()?;
    let texts = nodes.iter().map(node_text).collect::<Vec<_>>();
    let mut model = TextEmbedding::try_new(
        TextInitOptions::new(EmbeddingModel::AllMiniLML6V2)
            .with_show_download_progress(true)
            .with_intra_threads(4),
    )
    .map_err(|error| Error::Protocol {
        context: "embedding model initialization failed",
        detail: error.to_string(),
    })?;
    let vectors = model
        .embed(&texts, Some(128))
        .map_err(|error| Error::Protocol {
            context: "embedding generation failed",
            detail: error.to_string(),
        })?;
    graph.transaction(|| {
        for (node, vector) in nodes.iter().zip(vectors.iter()) {
            if let Some(id) = node.id {
                graph.set_embedding(id, MODEL, vector)?;
            }
        }
        Ok(())
    })?;
    Ok(vectors.len())
}

/// Explains how to enable embeddings when this binary was built light.
///
/// # Errors
/// Always returns an error explaining how to build semantic support.
#[cfg(not(feature = "semantic"))]
pub fn build(_root: &Path) -> Result<usize> {
    Err(Error::Protocol {
        context: "semantic embeddings unavailable",
        detail: "rebuild with `cargo build --release --features semantic`".into(),
    })
}

/// Returns semantic candidates ordered by cosine similarity. An empty vector
/// means the repository has not generated embeddings yet.
///
/// # Errors
/// Returns an error when the model or stored vectors cannot be read.
#[cfg(feature = "semantic")]
pub fn search(graph: &Graph, query: &str, limit: usize) -> Result<Vec<(Node, f32)>> {
    use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
    let stored = graph.embeddings(MODEL)?;
    if stored.is_empty() {
        return Ok(Vec::new());
    }
    let mut model = TextEmbedding::try_new(
        TextInitOptions::new(EmbeddingModel::AllMiniLML6V2)
            .with_show_download_progress(false)
            .with_intra_threads(4),
    )
    .map_err(|error| Error::Protocol {
        context: "embedding model initialization failed",
        detail: error.to_string(),
    })?;
    let query_vector = model
        .embed([format!("query: {query}")], Some(1))
        .map_err(|error| Error::Protocol {
            context: "query embedding failed",
            detail: error.to_string(),
        })?
        .pop()
        .unwrap_or_default();
    let by_id = graph
        .all_nodes()?
        .into_iter()
        .filter_map(|node| node.id.map(|id| (id, node)))
        .collect::<std::collections::HashMap<_, _>>();
    let mut ranked = stored
        .into_iter()
        .filter_map(|(id, vector)| {
            by_id
                .get(&id)
                .cloned()
                .map(|node| (node, cosine(&query_vector, &vector)))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.1.partial_cmp(&left.1).unwrap_or(Ordering::Equal));
    ranked.truncate(limit);
    Ok(ranked)
}

#[cfg(not(feature = "semantic"))]
/// Returns no semantic candidates in a lightweight build.
///
/// # Errors
/// This lightweight implementation currently cannot fail.
pub fn search(_graph: &Graph, _query: &str, _limit: usize) -> Result<Vec<(Node, f32)>> {
    Ok(Vec::new())
}

#[cfg(feature = "semantic")]
fn node_text(node: &Node) -> String {
    format!(
        "passage: {} {} {}",
        node.kind.as_str(),
        node.name,
        node.description.as_deref().unwrap_or_default()
    )
}

#[cfg(any(feature = "semantic", test))]
fn cosine(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    let dot = left.iter().zip(right).map(|(a, b)| a * b).sum::<f32>();
    let left_norm = left.iter().map(|value| value * value).sum::<f32>().sqrt();
    let right_norm = right.iter().map(|value| value * value).sum::<f32>().sqrt();
    if left_norm == 0.0 || right_norm == 0.0 {
        0.0
    } else {
        dot / (left_norm * right_norm)
    }
}

#[cfg(test)]
mod tests {
    use super::cosine;

    #[test]
    fn cosine_orders_equal_above_orthogonal() {
        assert!(cosine(&[1.0, 0.0], &[1.0, 0.0]) > cosine(&[1.0, 0.0], &[0.0, 1.0]));
    }
}
