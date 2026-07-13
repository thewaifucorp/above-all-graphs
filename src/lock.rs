//! Cross-process advisory lock serializing every writer that opens
//! `.aag/graph.db` for a full reindex pass — `bigbang`, `sync`, and the
//! `aag mcp` watcher's `reconcile`/`reindex` (see `crate::watch`) each do
//! an unsynchronized open + schema-migrate + `resolve::index_repo` today.
//! Without mutual exclusion, `bigbang --force`'s `remove_dir_all(.aag)`
//! racing a debounced watcher reindex can delete the directory out from
//! under an in-flight `SQLite` transaction, surfacing as
//! `disk I/O error: File being deleted does not exist`.
//!
//! The lock file lives beside `.aag/` (`<root>/.aag.lock`), never inside
//! it, so a forced rebuild's `remove_dir_all` never deletes the lock a
//! concurrent holder is waiting on.

use std::fs::File;
use std::path::Path;

use crate::error::{Error, Result};

/// Holds the exclusive lock for as long as it lives; dropping it closes
/// the file, which releases the OS-level lock.
pub struct Guard(#[allow(dead_code, reason = "held only for its Drop, never read")] File);

/// Blocks until the exclusive lock on `<root>/.aag.lock` is acquired.
///
/// # Errors
///
/// Returns [`Error::Lock`] if the lock file cannot be created/opened or
/// the lock cannot be acquired.
pub fn acquire(root: &Path) -> Result<Guard> {
    let path = root.join(".aag.lock");
    let file = File::create(&path).map_err(|source| Error::Lock {
        path: path.clone(),
        source,
    })?;
    file.lock().map_err(|source| Error::Lock { path, source })?;
    Ok(Guard(file))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn scratch_root() -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("aag-lock-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn second_exclusive_lock_blocks_until_first_drops() {
        let root = scratch_root();
        let first = acquire(&root).unwrap();

        // A non-blocking probe on the same file must see it held.
        let probe = File::create(root.join(".aag.lock")).unwrap();
        assert!(probe.try_lock().is_err());

        drop(first);
        // Once the guard drops, the lock is free again.
        assert!(probe.try_lock().is_ok());
    }
}
