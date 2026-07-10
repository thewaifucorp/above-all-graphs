//! Native filesystem watcher for incremental freshness, backed by `notify`
//! (FSEvents/inotify/`ReadDirectoryChangesW`, whichever the OS provides) via
//! `notify-debouncer-mini`. Per `SPEC.md` section 2: bursts of edits
//! collapse into one reindex after a debounce window, and there is no
//! manual reindex step in the normal flow — this is the only path that
//! keeps the index fresh once `aag mcp` is running.
//!
//! Reindexing is a full `resolve::index_repo` pass rather than a per-file
//! patch: the cross-file name resolution in `crate::resolve` recomputes
//! from the whole repo's symbol table anyway, so patching just one file's
//! nodes/edges would still need that same whole-repo pass to stay correct.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use notify::RecursiveMode;
use notify_debouncer_mini::{DebouncedEvent, new_debouncer};

use crate::error::{Error, Result};
use crate::resolve;
use crate::storage::Graph;

/// Debounce window: per `SPEC.md` section 2 ("Debounce configurável
/// (default ~2s)").
const DEBOUNCE: Duration = Duration::from_secs(2);

/// Directories whose changes never trigger a reindex — `.aag` in
/// particular, since writing `graph.db` during a reindex would otherwise
/// immediately trigger the next one. Same list the indexer skips.
use crate::resolve::SKIP_DIRS;

/// Runs one reindex pass immediately (reconciliation on connect — absorbs
/// edits made while no watcher was running: `git pull`, another editor, a
/// previous session), creating `.aag/` if it doesn't exist yet.
///
/// # Errors
///
/// Returns an error if `.aag/` cannot be created or the reindex fails.
pub fn reconcile(root: &Path) -> Result<()> {
    let aag_dir = root.join(".aag");
    std::fs::create_dir_all(&aag_dir).map_err(|source| Error::CreateDir {
        path: aag_dir.clone(),
        source,
    })?;
    let graph = Graph::open(&aag_dir.join("graph.db"))?;
    let summary = resolve::index_repo(&graph, root)?;
    tracing::info!(
        files = summary.files,
        docs = summary.docs,
        nodes = summary.nodes,
        edges = summary.edges,
        "reconciled index on connect"
    );
    Ok(())
}

/// Spawns the background watcher thread and returns immediately. The
/// watcher runs until the process exits; errors watching or reindexing are
/// logged rather than propagated — one bad reindex shouldn't take freshness
/// down for the rest of the session.
pub fn spawn(root: PathBuf) {
    thread::spawn(move || {
        if let Err(error) = watch_loop(&root) {
            tracing::warn!(%error, "watcher stopped");
        }
    });
}

fn watch_loop(root: &Path) -> notify::Result<()> {
    let (tx, rx) = mpsc::channel();
    let mut debouncer = new_debouncer(DEBOUNCE, tx)?;
    debouncer.watcher().watch(root, RecursiveMode::Recursive)?;

    for result in rx {
        match result {
            Ok(events) if events.iter().any(|event| is_relevant(root, event)) => {
                reindex(root, events.len());
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "watch error"),
        }
    }
    Ok(())
}

fn is_relevant(root: &Path, event: &DebouncedEvent) -> bool {
    let relative = event.path.strip_prefix(root).unwrap_or(&event.path);
    !relative.components().any(|component| {
        matches!(
            component,
            std::path::Component::Normal(name)
                if SKIP_DIRS.contains(&name.to_string_lossy().as_ref())
        )
    })
}

fn reindex(root: &Path, changed_paths: usize) {
    let graph = match Graph::open(&root.join(".aag").join("graph.db")) {
        Ok(graph) => graph,
        Err(error) => {
            tracing::warn!(%error, "reindex: could not open graph");
            return;
        }
    };
    match resolve::index_repo(&graph, root) {
        Ok(summary) => tracing::info!(
            changed_paths,
            files = summary.files,
            docs = summary.docs,
            nodes = summary.nodes,
            edges = summary.edges,
            "reindexed after file change"
        ),
        Err(error) => tracing::warn!(%error, "reindex failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(path: &str) -> DebouncedEvent {
        DebouncedEvent {
            path: PathBuf::from(path),
            kind: notify_debouncer_mini::DebouncedEventKind::Any,
        }
    }

    #[test]
    fn source_file_change_is_relevant() {
        let root = Path::new("/repo");
        assert!(is_relevant(root, &event("/repo/src/lib.rs")));
    }

    #[test]
    fn aag_index_write_is_not_relevant() {
        let root = Path::new("/repo");
        assert!(!is_relevant(root, &event("/repo/.aag/graph.db")));
    }

    #[test]
    fn git_internal_change_is_not_relevant() {
        let root = Path::new("/repo");
        assert!(!is_relevant(root, &event("/repo/.git/index")));
    }
}
