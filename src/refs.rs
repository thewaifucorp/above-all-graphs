//! Branch-aware indexes and graph-state comparison — P1.17 of
//! `docs/capability-coverage.md`.
//!
//! `.aag/graph.db` describes one thing: the working tree, right now. That is
//! the right default and the wrong answer to "what did this branch change",
//! "what does merging this PR do to the graph", or "which symbol became a hub
//! last month".
//!
//! So a ref can be indexed on its own. `git worktree add --detach` checks the
//! ref out into a scratch directory, that tree is indexed into
//! `.aag/refs/<ref>.db`, and the worktree is removed — the user's working tree
//! is never touched, never stashed, and never checked out to something else.
//! Snapshots are cached by commit, so asking twice costs one index.
//!
//! Comparison is then plain set arithmetic over two graphs: symbols that
//! appeared, symbols that are gone, symbols that moved file, and the edges and
//! hub positions that changed with them.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};
use crate::storage::{Graph, Node, NodeKind};

/// Where snapshot databases live. Inside `.aag/`, so the existing gitignore
/// rule already covers them and `bigbang --force` clears them with the rest.
const SNAPSHOT_DIR: &str = "refs";

/// How many snapshot databases to keep. A snapshot is a full index — a few
/// megabytes each — and an unbounded cache would quietly grow forever in a
/// directory nobody looks at.
const KEEP_SNAPSHOTS: usize = 8;

/// A graph state to compare: the working tree, or some git ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// `.aag/graph.db` — the working tree as last indexed.
    Workspace,
    /// Any git ref: a branch, a tag, or a commit.
    Ref(String),
    /// A pull request's head, resolved through `gh`.
    PullRequest(u64),
}

impl State {
    /// Parses the CLI/MCP spelling: `workspace`, `pr/42`, or any git ref.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let trimmed = text.trim();
        if trimmed.eq_ignore_ascii_case("workspace") || trimmed.is_empty() {
            return Self::Workspace;
        }
        if let Some(number) = trimmed
            .strip_prefix("pr/")
            .or_else(|| trimmed.strip_prefix("PR/"))
            .or_else(|| trimmed.strip_prefix('#'))
            && let Ok(number) = number.parse()
        {
            return Self::PullRequest(number);
        }
        Self::Ref(trimmed.to_string())
    }

    /// How the state prints in a report.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Workspace => "workspace".to_string(),
            Self::Ref(reference) => reference.clone(),
            Self::PullRequest(number) => format!("pr/{number}"),
        }
    }
}

/// What changed between two graph states.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Delta {
    /// Symbols only the later state has.
    pub added: Vec<String>,
    /// Symbols only the earlier state has.
    pub removed: Vec<String>,
    /// Symbols in both, in a different file.
    pub moved: Vec<(String, String, String)>,
    /// Edges added, as `src -> dst (kind)`.
    pub edges_added: usize,
    /// Edges removed.
    pub edges_removed: usize,
    /// Symbols whose dependent count changed most, later count first.
    pub fan_in_shifts: Vec<(String, usize, usize)>,
    /// Files only the later state has.
    pub files_added: Vec<String>,
    /// Files only the earlier state has.
    pub files_removed: Vec<String>,
}

impl Delta {
    /// Whether the two states describe the same graph.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.moved.is_empty()
            && self.edges_added == 0
            && self.edges_removed == 0
            && self.fan_in_shifts.is_empty()
    }
}

/// Compares two graphs, earlier first.
///
/// # Errors
/// Returns a storage error when either graph cannot be read.
pub fn compare(before: &Graph, after: &Graph) -> Result<Delta> {
    let (before_symbols, before_fan_in, before_files) = shape(before)?;
    let (after_symbols, after_fan_in, after_files) = shape(after)?;

    let before_names: BTreeSet<&String> = before_symbols.keys().collect();
    let after_names: BTreeSet<&String> = after_symbols.keys().collect();

    let added: Vec<String> = after_names
        .difference(&before_names)
        .map(|name| (*name).clone())
        .collect();
    let removed: Vec<String> = before_names
        .difference(&after_names)
        .map(|name| (*name).clone())
        .collect();
    let moved: Vec<(String, String, String)> = before_names
        .intersection(&after_names)
        .filter_map(|name| {
            let from = before_symbols.get(*name)?;
            let to = after_symbols.get(*name)?;
            (from != to).then(|| ((*name).clone(), from.clone(), to.clone()))
        })
        .collect();

    let before_edges = edge_set(before)?;
    let after_edges = edge_set(after)?;

    let mut fan_in_shifts: Vec<(String, usize, usize)> = before_fan_in
        .keys()
        .chain(after_fan_in.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|name| {
            let was = before_fan_in.get(name).copied().unwrap_or_default();
            let now = after_fan_in.get(name).copied().unwrap_or_default();
            (was != now).then(|| (name.clone(), was, now))
        })
        .collect();
    fan_in_shifts.sort_by_key(|(name, was, now)| {
        (
            std::cmp::Reverse(now.abs_diff(*was)),
            std::cmp::Reverse(*now),
            name.clone(),
        )
    });

    Ok(Delta {
        added,
        removed,
        moved,
        edges_added: after_edges.difference(&before_edges).count(),
        edges_removed: before_edges.difference(&after_edges).count(),
        fan_in_shifts,
        files_added: after_files.difference(&before_files).cloned().collect(),
        files_removed: before_files.difference(&after_files).cloned().collect(),
    })
}

type Shape = (
    BTreeMap<String, String>,
    BTreeMap<String, usize>,
    BTreeSet<String>,
);

/// A graph reduced to what can be compared across two databases: node ids are
/// per-database, so everything is keyed by qualified name instead.
fn shape(graph: &Graph) -> Result<Shape> {
    let nodes = graph.all_nodes()?;
    let by_id: BTreeMap<i64, &Node> = nodes
        .iter()
        .filter_map(|node| node.id.map(|id| (id, node)))
        .collect();
    let mut symbols = BTreeMap::new();
    let mut files = BTreeSet::new();
    for node in &nodes {
        if matches!(node.kind, NodeKind::File) {
            files.insert(node.file_path.clone());
            continue;
        }
        symbols.insert(qualify(node), node.file_path.clone());
    }
    let mut fan_in: BTreeMap<String, usize> = BTreeMap::new();
    for edge in graph.all_edges()? {
        if let Some(node) = by_id.get(&edge.dst)
            && !matches!(node.kind, NodeKind::File)
        {
            *fan_in.entry(qualify(node)).or_default() += 1;
        }
    }
    Ok((symbols, fan_in, files))
}

/// `kind name` — file-independent on purpose, so a symbol that moved reads as
/// moved rather than as one deletion plus one addition.
fn qualify(node: &Node) -> String {
    format!("{} {}", node.kind.as_str(), node.name)
}

fn edge_set(graph: &Graph) -> Result<BTreeSet<(String, String, String)>> {
    let nodes = graph.all_nodes()?;
    let by_id: BTreeMap<i64, &Node> = nodes
        .iter()
        .filter_map(|node| node.id.map(|id| (id, node)))
        .collect();
    let mut out = BTreeSet::new();
    for edge in graph.all_edges()? {
        let (Some(src), Some(dst)) = (by_id.get(&edge.src), by_id.get(&edge.dst)) else {
            continue;
        };
        out.insert((qualify(src), qualify(dst), edge.kind.as_str().to_string()));
    }
    Ok(out)
}

/// Opens the graph for a state, indexing a snapshot first when the state is a
/// ref that has not been snapshotted at that commit yet.
///
/// # Errors
/// Returns an error when git or `gh` fails, when the ref does not exist, or
/// when indexing the snapshot fails.
pub fn open(root: &Path, state: &State) -> Result<Graph> {
    match state {
        State::Workspace => Graph::open_existing(root),
        State::Ref(reference) => Graph::open(&snapshot(root, reference)?),
        State::PullRequest(number) => {
            let head = pull_request_head(root, *number)?;
            Graph::open(&snapshot(root, &head)?)
        }
    }
}

/// Indexes `reference` into `.aag/refs/<commit>.db`, reusing the file when it
/// is already there. Returns the database path.
///
/// # Errors
/// Returns an error when git cannot resolve or check out the ref, or when
/// indexing fails.
pub fn snapshot(root: &Path, reference: &str) -> Result<PathBuf> {
    let commit = resolve(root, reference)?;
    let directory = root.join(".aag").join(SNAPSHOT_DIR);
    std::fs::create_dir_all(&directory).map_err(|source| Error::CreateDir {
        path: directory.clone(),
        source,
    })?;
    let database = directory.join(format!("{commit}.db"));
    if database.is_file() {
        tracing::debug!(%commit, "reusing snapshot");
        return Ok(database);
    }

    // A detached worktree, not a checkout: the user's working tree, index, and
    // current branch are untouched, and an uncommitted change is never at risk.
    let worktree = directory.join(format!("tree-{commit}"));
    let _ = git(
        root,
        &["worktree", "remove", "--force", &worktree.to_string_lossy()],
    );
    git(
        root,
        &[
            "worktree",
            "add",
            "--detach",
            "--quiet",
            &worktree.to_string_lossy(),
            &commit,
        ],
    )?;

    let result = index_snapshot(&worktree, &database);
    // Remove the worktree whether or not indexing worked, so a failure does
    // not leave a stray checkout behind.
    let _ = git(
        root,
        &["worktree", "remove", "--force", &worktree.to_string_lossy()],
    );
    result?;
    tracing::info!(%commit, path = %database.display(), "indexed snapshot");
    prune_snapshots(&directory, &database);
    Ok(database)
}

/// Drops the least recently used snapshots, never the one just written.
/// Failure here is not worth an error: a stale cache file costs disk, and the
/// answer the caller asked for is already computed.
fn prune_snapshots(directory: &Path, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut databases: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "db"))
        .filter(|path| path != keep)
        .filter_map(|path| {
            let modified = path.metadata().and_then(|meta| meta.modified()).ok()?;
            Some((modified, path))
        })
        .collect();
    if databases.len() < KEEP_SNAPSHOTS {
        return;
    }
    databases.sort();
    let excess = databases.len() + 1 - KEEP_SNAPSHOTS;
    for (_, path) in databases.into_iter().take(excess) {
        if std::fs::remove_file(&path).is_ok() {
            tracing::debug!(path = %path.display(), "pruned snapshot");
        }
    }
}

fn index_snapshot(worktree: &Path, database: &Path) -> Result<()> {
    let graph = Graph::open(database)?;
    crate::resolve::index_repo(&graph, worktree)?;
    Ok(())
}

/// Resolves any ref spelling to a commit id — the cache key, so `main` picks
/// up new commits instead of serving a stale snapshot forever.
fn resolve(root: &Path, reference: &str) -> Result<String> {
    let out = git(
        root,
        &["rev-parse", "--verify", &format!("{reference}^{{commit}}")],
    )?;
    let commit = out.trim().to_string();
    if commit.is_empty() {
        return Err(Error::Protocol {
            context: "git ref did not resolve to a commit",
            detail: reference.to_string(),
        });
    }
    Ok(commit)
}

fn pull_request_head(root: &Path, number: u64) -> Result<String> {
    let out = run(
        root,
        "gh",
        &[
            "pr",
            "view",
            &number.to_string(),
            "--json",
            "headRefOid",
            "-q",
            ".headRefOid",
        ],
        "GitHub CLI",
    )?;
    let head = out.trim().to_string();
    if head.is_empty() {
        return Err(Error::Protocol {
            context: "pull request has no head commit",
            detail: format!("pr/{number}"),
        });
    }
    Ok(head)
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    run(root, "git", args, "git")
}

fn run(root: &Path, program: &str, args: &[&str], label: &'static str) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|error| Error::Protocol {
            context: "external command invocation failed",
            detail: format!("{label}: {error}"),
        })?;
    if !output.status.success() {
        return Err(Error::Protocol {
            context: "external command failed",
            detail: format!(
                "{label}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// How many entries each list prints before it is summarized.
const SHOWN: usize = 20;

/// `aag graph-diff <before> <after>` — the report.
///
/// # Errors
/// As [`open`], plus a storage error when either graph cannot be read.
pub fn format(root: &Path, before: &State, after: &State) -> Result<String> {
    let before_graph = open(root, before)?;
    let after_graph = open(root, after)?;
    let delta = compare(&before_graph, &after_graph)?;

    let mut out = format!("{} → {}\n\n", before.label(), after.label());
    if delta.is_empty() {
        out.push_str("the two states describe the same graph.\n");
        return Ok(out);
    }

    let _ = writeln!(
        out,
        "{} symbol(s) added, {} removed, {} moved; {} edge(s) added, {} removed",
        delta.added.len(),
        delta.removed.len(),
        delta.moved.len(),
        delta.edges_added,
        delta.edges_removed
    );
    if !delta.files_added.is_empty() || !delta.files_removed.is_empty() {
        let _ = writeln!(
            out,
            "{} file(s) added, {} removed",
            delta.files_added.len(),
            delta.files_removed.len()
        );
    }
    out.push('\n');

    section(&mut out, "added", &delta.added);
    section(&mut out, "removed", &delta.removed);

    if !delta.moved.is_empty() {
        out.push_str("moved:\n");
        for (name, from, to) in delta.moved.iter().take(SHOWN) {
            let _ = writeln!(out, "  {name}: {from} → {to}");
        }
        remainder(&mut out, delta.moved.len());
    }

    if !delta.fan_in_shifts.is_empty() {
        out.push_str("fan-in changed:\n");
        for (name, was, now) in delta.fan_in_shifts.iter().take(SHOWN) {
            let arrow = if now > was { "↑" } else { "↓" };
            let _ = writeln!(out, "  {name}: {was} → {now} {arrow}");
        }
        remainder(&mut out, delta.fan_in_shifts.len());
        out.push_str(
            "\nA symbol whose fan-in jumped is one the rest of the code just started \
             depending on — check it with `aag impact` before the next change.\n",
        );
    }
    Ok(out)
}

fn section(out: &mut String, title: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    let _ = writeln!(out, "{title}:");
    for item in items.iter().take(SHOWN) {
        let _ = writeln!(out, "  {item}");
    }
    remainder(out, items.len());
}

fn remainder(out: &mut String, total: usize) {
    if total > SHOWN {
        let _ = writeln!(out, "  …and {} more", total - SHOWN);
    }
    out.push('\n');
}

/// `aag graph-diff` — prints [`format`].
///
/// # Errors
/// As [`format`].
pub fn run_diff(root: &Path, before: &str, after: &str) -> Result<()> {
    println!(
        "{}",
        format(root, &State::parse(before), &State::parse(after))?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("aag-refs-{name}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn indexed(root: &Path) -> Graph {
        let graph = Graph::open(&root.join("graph.db")).unwrap();
        crate::resolve::index_repo(&graph, root).unwrap();
        graph
    }

    #[test]
    fn a_state_is_parsed_from_how_a_human_writes_it() {
        assert_eq!(State::parse("workspace"), State::Workspace);
        assert_eq!(State::parse(""), State::Workspace);
        assert_eq!(State::parse("main"), State::Ref("main".into()));
        assert_eq!(State::parse("pr/42"), State::PullRequest(42));
        assert_eq!(State::parse("#42"), State::PullRequest(42));
        assert_eq!(
            State::parse("feat/pr/thing"),
            State::Ref("feat/pr/thing".into()),
            "a branch that merely contains `pr/` is a branch"
        );
    }

    #[test]
    fn an_added_symbol_and_a_removed_one_are_told_apart() {
        let before_root = scratch("before");
        fs::write(
            before_root.join("lib.rs"),
            "pub fn kept() {}\npub fn gone() {}\n",
        )
        .unwrap();
        let before = indexed(&before_root);
        let after_root = scratch("after");
        fs::write(
            after_root.join("lib.rs"),
            "pub fn kept() {}\npub fn fresh() {}\n",
        )
        .unwrap();
        let after = indexed(&after_root);

        let delta = compare(&before, &after).unwrap();

        assert!(
            delta.added.iter().any(|name| name.ends_with("fresh")),
            "{delta:?}"
        );
        assert!(
            delta.removed.iter().any(|name| name.ends_with("gone")),
            "{delta:?}"
        );
        assert!(
            !delta.added.iter().any(|name| name.ends_with("kept")),
            "an unchanged symbol is not news: {delta:?}"
        );
    }

    #[test]
    fn a_symbol_that_changed_file_reads_as_moved_not_as_two_events() {
        let before_root = scratch("move-before");
        fs::write(before_root.join("old.rs"), "pub fn travels() {}\n").unwrap();
        let before = indexed(&before_root);
        let after_root = scratch("move-after");
        fs::write(after_root.join("new.rs"), "pub fn travels() {}\n").unwrap();
        let after = indexed(&after_root);

        let delta = compare(&before, &after).unwrap();

        assert!(delta.added.is_empty(), "{delta:?}");
        assert!(delta.removed.is_empty(), "{delta:?}");
        let (name, from, to) = delta.moved.first().expect("a move");
        assert!(name.ends_with("travels"));
        assert_eq!((from.as_str(), to.as_str()), ("old.rs", "new.rs"));
    }

    #[test]
    fn a_symbol_the_code_starts_leaning_on_shows_up_as_a_fan_in_shift() {
        let before_root = scratch("fan-before");
        fs::write(
            before_root.join("lib.rs"),
            "pub fn core() {}\npub fn a() {}\n",
        )
        .unwrap();
        let before = indexed(&before_root);
        let after_root = scratch("fan-after");
        fs::write(
            after_root.join("lib.rs"),
            "pub fn core() {}\npub fn a() { core(); }\npub fn b() { core(); }\n",
        )
        .unwrap();
        let after = indexed(&after_root);

        let delta = compare(&before, &after).unwrap();

        let shift = delta
            .fan_in_shifts
            .iter()
            .find(|(name, _, _)| name.ends_with("core"))
            .expect("core gained dependents");
        assert!(shift.2 > shift.1, "{shift:?}");
        assert!(delta.edges_added >= 2, "{delta:?}");
    }

    #[test]
    fn two_indexes_of_the_same_tree_compare_as_identical() {
        let root = scratch("same");
        fs::write(
            root.join("lib.rs"),
            "pub fn one() {}\npub fn two() { one(); }\n",
        )
        .unwrap();
        let left = indexed(&root);
        let right = Graph::open(&root.join("other.db")).unwrap();
        crate::resolve::index_repo(&right, &root).unwrap();

        let delta = compare(&left, &right).unwrap();

        assert!(delta.is_empty(), "{delta:?}");
    }

    #[test]
    fn the_snapshot_cache_stays_bounded_and_keeps_the_newest() {
        let directory = scratch("prune");
        let mut written = Vec::new();
        for index in 0..12u64 {
            let path = directory.join(format!("commit{index:02}.db"));
            fs::write(&path, b"snapshot").unwrap();
            // Pruning orders by modification time, and a loop this tight can
            // write twelve files inside one filesystem tick — so each one is
            // aged explicitly.
            let handle = fs::File::open(&path).unwrap();
            handle
                .set_times(fs::FileTimes::new().set_modified(
                    std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000 + index),
                ))
                .unwrap();
            written.push(path);
        }
        let newest = written.last().unwrap().clone();

        prune_snapshots(&directory, &newest);

        let left: Vec<String> = fs::read_dir(&directory)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(left.len(), KEEP_SNAPSHOTS, "{left:?}");
        assert!(left.contains(&"commit11.db".to_string()), "{left:?}");
        assert!(
            !left.contains(&"commit00.db".to_string()),
            "the oldest goes first: {left:?}"
        );
    }

    /// The end-to-end path: a real repository, a real commit, indexed through
    /// a detached worktree that leaves the working tree alone.
    #[test]
    fn a_commit_is_indexed_without_touching_the_working_tree() {
        let root = scratch("git");
        let git_ok = Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&root)
            .status()
            .is_ok_and(|status| status.success());
        if !git_ok {
            return;
        }
        for (key, value) in [("user.email", "t@example.com"), ("user.name", "t")] {
            let _ = Command::new("git")
                .args(["config", key, value])
                .current_dir(&root)
                .status();
        }
        fs::write(root.join("lib.rs"), "pub fn first() {}\n").unwrap();
        let _ = Command::new("git")
            .args(["add", "-A"])
            .current_dir(&root)
            .status();
        let _ = Command::new("git")
            .args(["commit", "-qm", "first"])
            .current_dir(&root)
            .status();
        // The working tree moves on, uncommitted.
        fs::write(root.join("lib.rs"), "pub fn second() {}\n").unwrap();

        let database = snapshot(&root, "HEAD").expect("snapshot of HEAD");
        let snapshotted = Graph::open(&database).unwrap();

        assert!(
            snapshotted
                .all_nodes()
                .unwrap()
                .iter()
                .any(|node| node.name == "first"),
            "the commit's content is what was indexed"
        );
        assert_eq!(
            fs::read_to_string(root.join("lib.rs")).unwrap(),
            "pub fn second() {}\n",
            "the working tree is exactly as the user left it"
        );
        assert!(
            !root
                .join(".aag")
                .join(SNAPSHOT_DIR)
                .join("tree-HEAD")
                .exists(),
            "the scratch worktree is cleaned up"
        );
    }
}
