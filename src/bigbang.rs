//! The `bigbang` bootstrap: creates the `.aag/` index directory and runs the
//! first index from scratch.
//!
//! Later phases (watcher, MCP server) hang off this same `.aag/` root; this
//! module owns its lifecycle (create / rebuild / skip) plus kicking off the
//! initial `resolve::index_repo` pass and the default export artifacts
//! (`crate::export`), unless `--no-viz` was passed.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::export;
use crate::resolve::{self, IndexSummary};
use crate::storage::Graph;

/// Flags controlling one `aag bigbang` run.
#[derive(Debug, Default, Clone)]
pub struct Options {
    /// Discard any existing index and rebuild from scratch.
    pub force: bool,
    /// Skip writing `graph.json`/`graph.html`/report/wiki/etc.
    pub no_viz: bool,
    /// Also write an Obsidian-vault-compatible export under
    /// `<dir>/aag/` (never the vault root — see `crate::export::write_obsidian`).
    pub obsidian_dir: Option<PathBuf>,
    /// Skip agent integration (MCP config, hooks, skill pack).
    pub no_install: bool,
}

/// Runs `aag bigbang` against `root`.
///
/// - If no index exists yet, creates `.aag/`, indexes `root`, and returns.
/// - If an index exists and `options.force` is `false`, skips (no-op) — a
///   repeat `bigbang` on an already-indexed repo is not an error.
/// - If an index exists and `options.force` is `true`, removes it first,
///   then recreates it and re-indexes.
///
/// # Errors
///
/// Returns [`Error::RemoveDir`] if a forced rebuild cannot delete the
/// existing `.aag/` directory, [`Error::CreateDir`] if the directory cannot
/// be (re)created, or a storage/parse/IO error if indexing or exporting fails.
pub fn run(root: &Path, options: &Options) -> Result<()> {
    // Agent integration runs on every bigbang — even a skipped reindex —
    // so a newly installed agent gets wired up. Idempotent, and a failure
    // here must never take down indexing (install-and-forget, not
    // install-or-crash).
    if !options.no_install
        && let Err(error) = crate::install::run(root, options.force)
    {
        tracing::warn!(%error, "agent integration failed — index still usable");
    }

    let aag_dir = root.join(".aag");
    let already_indexed = aag_dir.is_dir();

    if already_indexed && !options.force {
        tracing::info!(
            path = %aag_dir.display(),
            "index already exists, skipping (pass --force to rebuild)"
        );
        return Ok(());
    }

    // Excludes the `aag mcp` watcher's reconcile/reindex and `aag sync`
    // (see `crate::lock`) for the rest of this run — otherwise a debounced
    // watcher reindex racing this rebuild can delete `.aag/` out from
    // under its in-flight `SQLite` transaction.
    let _lock = crate::lock::acquire(root)?;

    if already_indexed {
        tracing::info!(path = %aag_dir.display(), "removing existing index for rebuild");
        fs::remove_dir_all(&aag_dir).map_err(|source| Error::RemoveDir {
            path: aag_dir.clone(),
            source,
        })?;
    }

    fs::create_dir_all(&aag_dir).map_err(|source| Error::CreateDir {
        path: aag_dir.clone(),
        source,
    })?;
    tracing::info!(path = %aag_dir.display(), "index directory created");

    let graph = index(root, &aag_dir)?;

    if options.no_viz {
        tracing::info!("--no-viz: skipping export artifacts");
    } else {
        export::write_default(root, &aag_dir, &graph)?;
        tracing::info!(path = %aag_dir.display(), "wrote index.html/graph.html/report.html/wiki/graph.json/graph.graphml/cypher.txt");
    }

    if let Some(vault_dir) = &options.obsidian_dir {
        export::write_obsidian(vault_dir, &graph)?;
        tracing::info!(path = %vault_dir.join("aag").display(), "wrote obsidian export");
    }

    println!("indexed — run `aag ui` to explore");
    Ok(())
}

fn index(root: &Path, aag_dir: &Path) -> Result<Graph> {
    let graph = Graph::open(&aag_dir.join("graph.db"))?;
    let summary: IndexSummary = resolve::index_repo(&graph, root)?;
    tracing::info!(
        files = summary.files,
        nodes = summary.nodes,
        docs = summary.docs,
        edges = summary.edges,
        "first index complete"
    );
    // Unit tests index throwaway temp repos — keep those out of the
    // user's real workspace registry.
    #[cfg(not(test))]
    crate::workspaces::register(root, &summary);
    Ok(graph)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Fresh, unique scratch dir per test — parallel `cargo test` safe.
    fn scratch_root() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("aag-bigbang-test-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// `no_install: true` always — agent detection reads the real home
    /// directory, and tests must never touch the machine's agent configs.
    fn opts() -> Options {
        Options {
            no_install: true,
            ..Options::default()
        }
    }

    #[test]
    fn creates_index_when_missing() {
        let root = scratch_root();

        run(&root, &opts()).unwrap();

        assert!(root.join(".aag").is_dir());
    }

    #[test]
    fn indexes_source_files_on_first_run() {
        let root = scratch_root();
        fs::write(root.join("lib.rs"), "fn run() {}").unwrap();

        run(&root, &opts()).unwrap();

        let graph = crate::storage::Graph::open(&root.join(".aag").join("graph.db")).unwrap();
        assert!(graph.find_by_name("run").unwrap().is_some());
    }

    #[test]
    fn writes_default_export_artifacts() {
        let root = scratch_root();
        fs::write(root.join("lib.rs"), "fn run() {}").unwrap();

        run(&root, &opts()).unwrap();

        assert!(root.join(".aag").join("graph.json").is_file());
        assert!(root.join(".aag").join("graph.html").is_file());
        assert!(root.join(".aag").join("index.html").is_file());
        assert!(root.join(".aag").join("report.html").is_file());
        assert!(root.join(".aag").join("wiki").join("index.html").is_file());
    }

    #[test]
    fn no_viz_skips_export_artifacts() {
        let root = scratch_root();
        fs::write(root.join("lib.rs"), "fn run() {}").unwrap();

        run(
            &root,
            &Options {
                no_viz: true,
                ..opts()
            },
        )
        .unwrap();

        assert!(!root.join(".aag").join("graph.json").exists());
    }

    #[test]
    fn skips_existing_index_without_force() {
        let root = scratch_root();
        let marker = root.join(".aag").join("marker");
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::write(&marker, b"keep me").unwrap();

        run(&root, &opts()).unwrap();

        assert!(marker.is_file(), "skip must not touch the existing index");
    }

    #[test]
    fn rebuilds_existing_index_when_forced() {
        let root = scratch_root();
        let marker = root.join(".aag").join("marker");
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::write(&marker, b"discard me").unwrap();

        run(
            &root,
            &Options {
                force: true,
                ..opts()
            },
        )
        .unwrap();

        assert!(root.join(".aag").is_dir(), "index must exist after rebuild");
        assert!(!marker.exists(), "force must discard the previous index");
    }
}
