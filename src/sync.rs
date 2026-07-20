//! `aag sync` — refresh the index and export artifacts in place, without
//! deleting `.aag/` first (unlike `bigbang --force`). This is what the
//! `PostToolUse` hook runs after every agent edit, per `SPEC.md` section 8.
//!
//! With `--file`, only that file is read and parsed. Cross-file edges are
//! then re-resolved from raw references persisted during the initial index.

use std::path::{Component, Path};

use crate::error::{Error, Result};
use crate::storage::Graph;
use crate::{export, resolve};

use crate::resolve::{SKIP_DIRS, SKIP_FILES};

/// Whether a change to `file` (repo-relative or absolute) can affect the
/// index at all. `None` means "no file hint given — always relevant".
#[must_use]
pub fn is_relevant(root: &Path, file: Option<&Path>) -> bool {
    let Some(file) = file else { return true };
    let relative = file.strip_prefix(root).unwrap_or(file);
    if relative
        .file_name()
        .is_some_and(|name| SKIP_FILES.contains(&name.to_string_lossy().as_ref()))
    {
        return false;
    }
    if !resolve::is_indexable_path(relative) {
        return false;
    }
    !relative.components().any(|component| {
        matches!(
            component,
            Component::Normal(name)
                if SKIP_DIRS.contains(&name.to_string_lossy().as_ref())
        )
    })
}

/// Runs `aag sync` against `root`: one `resolve::index_repo` pass plus the
/// default export artifacts, reusing the existing `.aag/graph.db`.
///
/// With `file: Some(path)` pointing at an irrelevant location, returns
/// `Ok(())` immediately without opening the graph — this keeps the
/// `PostToolUse` hook free even when the agent edits `.aag/` outputs or
/// build artifacts.
///
/// # Errors
///
/// Returns [`Error::IndexMissing`] if `root` has no `.aag/` yet (sync
/// refreshes an index, it doesn't bootstrap one — that's `aag bigbang`),
/// or a storage/write error if the reindex or export fails.
pub fn run(root: &Path, file: Option<&Path>, no_viz: bool) -> Result<()> {
    // Hooks pass `--path .` while the payload's file_path is absolute —
    // canonicalize so strip_prefix works and relevance checks see the
    // repo-relative path.
    let root = &root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if !is_relevant(root, file) {
        tracing::debug!(file = ?file, "sync: irrelevant path, skipping");
        return Ok(());
    }

    let aag_dir = root.join(".aag");
    if !aag_dir.is_dir() {
        return Err(Error::IndexMissing { path: root.clone() });
    }

    // See `crate::lock` — excludes `bigbang --force` and the `aag mcp`
    // watcher's reconcile/reindex.
    let _lock = crate::lock::acquire(root)?;
    let graph = Graph::open(&aag_dir.join("graph.db"))?;
    let summary = if let Some(file) = file.filter(|_| graph.incremental_ready().unwrap_or(false)) {
        let absolute = if file.is_absolute() {
            file.to_path_buf()
        } else {
            root.join(file)
        };
        resolve::index_file(&graph, root, &absolute)?
    } else {
        resolve::index_repo(&graph, root)?
    };
    tracing::info!(
        files = summary.files,
        docs = summary.docs,
        nodes = summary.nodes,
        edges = summary.edges,
        "synced index"
    );
    // Unit tests sync throwaway temp repos — keep those out of the
    // user's real workspace registry.
    #[cfg(not(test))]
    crate::workspaces::register(root, &summary);

    if !no_viz {
        export::write_default(root, &aag_dir, &graph)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn scratch_root() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("aag-sync-test-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn no_file_hint_is_always_relevant() {
        assert!(is_relevant(Path::new("/repo"), None));
    }

    #[test]
    fn source_file_is_relevant() {
        assert!(is_relevant(
            Path::new("/repo"),
            Some(Path::new("/repo/src/lib.rs"))
        ));
    }

    #[test]
    fn aag_output_is_not_relevant() {
        assert!(!is_relevant(
            Path::new("/repo"),
            Some(Path::new("/repo/.aag/graph.html"))
        ));
    }

    #[test]
    fn index_lock_is_not_relevant() {
        assert!(!is_relevant(
            Path::new("/repo"),
            Some(Path::new("/repo/.aag.lock"))
        ));
    }

    #[test]
    fn relative_path_in_skip_dir_is_not_relevant() {
        assert!(!is_relevant(
            Path::new("/repo"),
            Some(Path::new("target/debug/build.rs"))
        ));
    }

    #[test]
    fn sync_without_index_errors() {
        let root = scratch_root();
        let error = run(&root, None, true).unwrap_err();
        assert!(matches!(error, Error::IndexMissing { .. }));
    }

    #[test]
    fn sync_refreshes_existing_index() {
        let root = scratch_root();
        crate::bigbang::run(
            &root,
            &crate::bigbang::Options {
                no_install: true,
                ..Default::default()
            },
        )
        .unwrap();

        fs::write(root.join("lib.rs"), "fn fresh_symbol() {}").unwrap();
        run(&root, Some(&root.join("lib.rs")), true).unwrap();

        let graph = Graph::open(&root.join(".aag").join("graph.db")).unwrap();
        assert!(graph.find_by_name("fresh_symbol").unwrap().is_some());
    }

    #[test]
    fn sync_short_circuits_on_irrelevant_file() {
        let root = scratch_root();
        // No index exists — an irrelevant file must still return Ok without
        // hitting the IndexMissing check.
        run(&root, Some(Path::new(".aag/graph.db")), true).unwrap();
    }

    #[test]
    fn file_sync_preserves_unchanged_node_ids_and_handles_deletion() {
        let root = scratch_root();
        fs::write(root.join("stable.rs"), "fn stable() {}").unwrap();
        fs::write(root.join("changing.rs"), "fn old_name() { stable(); }").unwrap();
        crate::bigbang::run(
            &root,
            &crate::bigbang::Options {
                no_install: true,
                ..Default::default()
            },
        )
        .unwrap();
        let graph = Graph::open(&root.join(".aag/graph.db")).unwrap();
        let stable_id = graph.find_by_name("stable").unwrap().unwrap().id;
        drop(graph);

        fs::write(root.join("changing.rs"), "fn new_name() { stable(); }").unwrap();
        run(&root, Some(&root.join("changing.rs")), true).unwrap();
        let graph = Graph::open(&root.join(".aag/graph.db")).unwrap();
        assert_eq!(graph.find_by_name("stable").unwrap().unwrap().id, stable_id);
        assert!(graph.find_by_name("old_name").unwrap().is_none());
        assert!(graph.find_by_name("new_name").unwrap().is_some());
        drop(graph);

        fs::remove_file(root.join("changing.rs")).unwrap();
        run(&root, Some(&root.join("changing.rs")), true).unwrap();
        let graph = Graph::open(&root.join(".aag/graph.db")).unwrap();
        assert!(graph.find_by_name("new_name").unwrap().is_none());
        assert_eq!(graph.find_by_name("stable").unwrap().unwrap().id, stable_id);
    }
}
