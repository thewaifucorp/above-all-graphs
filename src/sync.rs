//! `aag sync` — refresh the index and export artifacts in place, without
//! deleting `.aag/` first (unlike `bigbang --force`). This is what the
//! `PostToolUse` hook runs after every agent edit, per `SPEC.md` section 8.
//!
//! Cross-file resolution recomputes from the whole repo's symbol table
//! (see `crate::resolve::index_repo`), so a sync is always one full pass —
//! the per-file win is the *short-circuit*: when `--file` points at a path
//! the index doesn't care about (`.aag/`, `target/`, an unrecognized
//! extension...), sync exits immediately without touching the graph.

use std::path::{Component, Path};

use crate::error::{Error, Result};
use crate::storage::Graph;
use crate::{export, resolve};

use crate::resolve::SKIP_DIRS;

/// Whether a change to `file` (repo-relative or absolute) can affect the
/// index at all. `None` means "no file hint given — always relevant".
#[must_use]
pub fn is_relevant(root: &Path, file: Option<&Path>) -> bool {
    let Some(file) = file else { return true };
    let relative = file.strip_prefix(root).unwrap_or(file);
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
    let summary = resolve::index_repo(&graph, root)?;
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
}
