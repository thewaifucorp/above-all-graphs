//! Workspace registry — `aag`'s answer to multi-repo without imitating
//! `GitNexus`'s unified enterprise graph: every repo keeps its own local
//! `.aag/` graph (zero coupling between repos), and a lightweight global
//! registry at `~/.config/aag/workspaces.json` records every workspace
//! this machine has indexed. `aag workspaces` lists them from the CLI;
//! `aag hub` (`crate::hub`) serves them as one real app over HTTP — this
//! module owns the registry, `hub` owns the server.
//!
//! Registration happens as a side effect of every `bigbang`/`sync`, so
//! the registry is maintenance-free. Entries whose `.aag/` vanished are
//! pruned on read, never on write — a broken registry must not break
//! indexing.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::resolve::IndexSummary;

/// Where the registry lives: `$XDG_CONFIG_HOME/aag/workspaces.json`,
/// falling back to `~/.config/aag/workspaces.json`.
fn registry_path() -> Option<PathBuf> {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|home| PathBuf::from(home).join(".config"))
        })?;
    Some(config_home.join("aag").join("workspaces.json"))
}

/// Records `root` (canonicalized) and its index stats in the registry.
/// Infallible by design: a registry hiccup must never fail an index pass —
/// errors are logged and swallowed.
pub fn register(root: &Path, summary: &IndexSummary) {
    let Some(path) = registry_path() else { return };
    if let Err(error) = register_at(&path, root, summary) {
        tracing::debug!(%error, "workspace registry update failed");
    }
}

/// The testable core of [`register`].
///
/// # Errors
///
/// Returns a write/create error if the registry cannot be written.
///
/// # Panics
///
/// Never in practice: the `workspaces` entry is inserted as an object one
/// line before it is read back as one.
pub fn register_at(registry: &Path, root: &Path, summary: &IndexSummary) -> Result<()> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let key = root.to_string_lossy().to_string();
    let name = root
        .file_name()
        .map_or_else(|| key.clone(), |name| name.to_string_lossy().to_string());

    let mut doc = read_registry(registry);
    let workspaces = doc
        .entry("workspaces")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .expect("just inserted an object");
    workspaces.insert(
        key,
        json!({
            "name": name,
            "files": summary.files,
            "symbols": summary.nodes,
            "docs": summary.docs,
            "edges": summary.edges,
            "indexed_at": SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        }),
    );

    if let Some(parent) = registry.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let pretty =
        serde_json::to_string_pretty(&Value::Object(doc)).unwrap_or_else(|_| String::from("{}"));
    std::fs::write(registry, pretty + "\n").map_err(|source| Error::Write {
        path: registry.to_path_buf(),
        source,
    })
}

/// Prints every registered workspace, pruning entries whose `.aag/` no
/// longer exists (repo deleted or index removed).
///
/// # Errors
///
/// Returns a write error only if pruning cannot rewrite the registry.
pub fn list() -> Result<()> {
    let Some(path) = registry_path() else {
        println!("no home directory — no workspace registry");
        return Ok(());
    };
    for line in list_at(&path)? {
        println!("{line}");
    }
    if !live_entries().is_empty() {
        println!("tip: run `aag ui` to browse all of these as one app");
    }
    Ok(())
}

/// The testable core of [`list`]: returns the display lines.
///
/// # Errors
///
/// Returns a write error only if pruning cannot rewrite the registry.
pub fn list_at(registry: &Path) -> Result<Vec<String>> {
    let mut doc = read_registry(registry);
    let Some(workspaces) = doc.get_mut("workspaces").and_then(Value::as_object_mut) else {
        return Ok(vec![String::from(
            "no workspaces indexed yet — run `aag bigbang` in a repo",
        )]);
    };

    let before = workspaces.len();
    workspaces.retain(|path, _| Path::new(path).join(".aag").is_dir());
    let pruned = before - workspaces.len();

    let mut lines: Vec<String> = workspaces
        .iter()
        .map(|(path, info)| {
            format!(
                "{}  ({} files, {} symbols, {} edges)  {}",
                info.get("name").and_then(Value::as_str).unwrap_or("?"),
                info.get("files").and_then(Value::as_u64).unwrap_or(0),
                info.get("symbols").and_then(Value::as_u64).unwrap_or(0),
                info.get("edges").and_then(Value::as_u64).unwrap_or(0),
                path,
            )
        })
        .collect();
    lines.sort();

    if lines.is_empty() {
        lines.push(String::from(
            "no workspaces indexed yet — run `aag bigbang` in a repo",
        ));
    }
    if pruned > 0 {
        let pretty = serde_json::to_string_pretty(&Value::Object(doc))
            .unwrap_or_else(|_| String::from("{}"));
        std::fs::write(registry, pretty + "\n").map_err(|source| Error::Write {
            path: registry.to_path_buf(),
            source,
        })?;
        lines.push(format!("(pruned {pruned} stale entr(y/ies))"));
    }
    Ok(lines)
}

/// Live snapshot of every registered workspace — see [`live_entries_at`].
#[must_use]
pub fn live_entries() -> Vec<Value> {
    registry_path()
        .map(|path| live_entries_at(&path))
        .unwrap_or_default()
}

/// The testable core of [`live_entries`]: every registered workspace,
/// sorted by path, as JSON objects carrying `path` alongside the stored
/// `name`/`files`/`symbols`/`docs`/`edges` fields — the shape
/// `crate::hub`'s `/api/workspaces` route serves verbatim. Read fresh
/// from disk every call: the hub has no cache, so it always reflects the
/// latest `bigbang`/`sync`. Does not prune (unlike `list_at`) — a
/// display-only read must not race a concurrent writer over the
/// registry file.
#[must_use]
pub fn live_entries_at(registry: &Path) -> Vec<Value> {
    let doc = read_registry(registry);
    let Some(workspaces) = doc.get("workspaces").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut entries: Vec<(&String, &Value)> = workspaces.iter().collect();
    entries.sort_by_key(|(path, _)| path.to_lowercase());
    entries
        .into_iter()
        .map(|(path, info)| {
            let mut info = info.clone();
            if let Some(obj) = info.as_object_mut() {
                obj.insert("path".into(), json!(path));
            }
            info
        })
        .collect()
}

/// Whether `root` is registered — see [`is_registered_at`].
#[must_use]
pub fn is_registered(root: &Path) -> bool {
    registry_path().is_some_and(|path| is_registered_at(&path, root))
}

/// The testable core of [`is_registered`]: whether `root` is an exact key
/// in the registry right now. `crate::hub` checks this before serving any
/// file, so the local server can only ever read inside directories `aag`
/// itself already indexed — not an arbitrary path a request happens to name.
#[must_use]
pub fn is_registered_at(registry: &Path, root: &Path) -> bool {
    let doc = read_registry(registry);
    doc.get("workspaces")
        .and_then(Value::as_object)
        .is_some_and(|workspaces| workspaces.contains_key(&root.to_string_lossy().to_string()))
}

/// The registry as a JSON object; missing or corrupt file = empty object
/// (the registry is disposable state, always safe to rebuild).
fn read_registry(path: &Path) -> serde_json::Map<String, Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|value| match value {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn scratch() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("aag-ws-test-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn summary() -> IndexSummary {
        IndexSummary {
            files: 2,
            nodes: 5,
            docs: 1,
            edges: 4,
        }
    }

    #[test]
    fn register_then_list_round_trips() {
        let base = scratch();
        let registry = base.join("workspaces.json");
        let repo = base.join("myrepo");
        fs::create_dir_all(repo.join(".aag")).unwrap();

        register_at(&registry, &repo, &summary()).unwrap();
        let lines = list_at(&registry).unwrap();

        assert_eq!(lines.len(), 1, "lines: {lines:?}");
        assert!(lines[0].starts_with("myrepo"), "line: {}", lines[0]);
        assert!(lines[0].contains("5 symbols"));
    }

    #[test]
    fn reregister_updates_not_duplicates() {
        let base = scratch();
        let registry = base.join("workspaces.json");
        let repo = base.join("myrepo");
        fs::create_dir_all(repo.join(".aag")).unwrap();

        register_at(&registry, &repo, &summary()).unwrap();
        let updated = IndexSummary {
            nodes: 99,
            ..summary()
        };
        register_at(&registry, &repo, &updated).unwrap();

        let lines = list_at(&registry).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("99 symbols"), "line: {}", lines[0]);
    }

    #[test]
    fn stale_workspace_is_pruned_on_list() {
        let base = scratch();
        let registry = base.join("workspaces.json");
        let gone = base.join("deleted-repo");
        fs::create_dir_all(gone.join(".aag")).unwrap();
        register_at(&registry, &gone, &summary()).unwrap();
        fs::remove_dir_all(&gone).unwrap();

        let lines = list_at(&registry).unwrap();
        assert!(
            lines.iter().any(|line| line.contains("pruned 1")),
            "lines: {lines:?}"
        );
        // Second list: registry already clean.
        let lines = list_at(&registry).unwrap();
        assert!(lines[0].contains("no workspaces"), "lines: {lines:?}");
    }

    #[test]
    fn live_entries_carries_path_and_stats() {
        let base = scratch();
        let registry = base.join("workspaces.json");
        let repo = base.join("myrepo");
        fs::create_dir_all(repo.join(".aag")).unwrap();
        register_at(&registry, &repo, &summary()).unwrap();

        let entries = live_entries_at(&registry);
        assert_eq!(entries.len(), 1);
        let canonical = repo.canonicalize().unwrap();
        assert_eq!(
            entries[0]["path"],
            json!(canonical.to_string_lossy().to_string())
        );
        assert_eq!(entries[0]["symbols"], json!(5));
    }

    #[test]
    fn live_entries_does_not_prune() {
        let base = scratch();
        let registry = base.join("workspaces.json");
        let gone = base.join("deleted-repo");
        fs::create_dir_all(gone.join(".aag")).unwrap();
        register_at(&registry, &gone, &summary()).unwrap();
        fs::remove_dir_all(&gone).unwrap();

        // Unlike list_at, a live read must not mutate the registry file —
        // the entry (now stale) is still reported, only the file server
        // will find no `.aag/` behind it and 404.
        assert_eq!(live_entries_at(&registry).len(), 1);
    }

    #[test]
    fn is_registered_matches_exact_canonical_key_only() {
        let base = scratch();
        let registry = base.join("workspaces.json");
        let repo = base.join("myrepo");
        fs::create_dir_all(repo.join(".aag")).unwrap();
        register_at(&registry, &repo, &summary()).unwrap();

        let canonical = repo.canonicalize().unwrap();
        assert!(is_registered_at(&registry, &canonical));
        assert!(!is_registered_at(&registry, &base.join("some-other-repo")));
    }

    #[test]
    fn corrupt_registry_is_rebuilt_not_fatal() {
        let base = scratch();
        let registry = base.join("workspaces.json");
        fs::write(&registry, "not json").unwrap();
        let repo = base.join("myrepo");
        fs::create_dir_all(repo.join(".aag")).unwrap();

        register_at(&registry, &repo, &summary()).unwrap();
        assert_eq!(list_at(&registry).unwrap().len(), 1);
    }
}
