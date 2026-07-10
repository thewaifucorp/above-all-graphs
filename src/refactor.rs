//! Refactor tooling — per `SPEC.md` section 7: coordinated multi-file
//! rename, and `affected` for CI/pre-commit hooks. Impact analysis (blast
//! radius before editing) lives in `crate::impact`.
//!
//! Rename is the one `aag` feature that writes to the user's source files
//! rather than just reading the graph, so [`rename_run`] previews by
//! default and only mutates files when `write` is `true` — [`rename_plan`]
//! stays pure so the CLI/MCP surfaces can show the same preview either way.
//!
//! Both rename and `affected` are whole-word, name-based heuristics, same
//! as the rest of the graph's confidence-tagged resolution: rename skips
//! files reached only through an `AMBIGUOUS` edge, and never touches files
//! reached only through an `Explains` edge (docs are prose, not code —
//! renaming an identifier inside them is a different, riskier operation
//! than `crate::docs::describe` is meant for).

use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs;
use std::io::BufRead;
use std::path::Path;

use crate::error::{Error, Result};
use crate::storage::{Confidence, EdgeKind, Graph};

/// Caps BFS depth for `affected`'s dependency walk — same rationale as
/// `crate::impact::MAX_DEPTH`.
const MAX_DEPTH: u32 = 20;

/// One file rename would touch, and how many whole-word matches it has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameChange {
    /// Path relative to the indexed root.
    pub file_path: String,
    /// Whole-word occurrences of the old name in this file.
    pub occurrences: usize,
}

/// Runs `aag rename <old> <new>`: previews the plan, or applies it and
/// reindexes when `write` is `true`.
///
/// # Errors
///
/// See [`rename_plan`] and [`rename_apply`].
pub fn rename_run(root: &Path, old_name: &str, new_name: &str, write: bool) -> Result<()> {
    let changes = rename_plan(root, old_name, new_name)?;
    if write {
        rename_apply(root, &changes, old_name, new_name)?;
        println!(
            "renamed `{old_name}` to `{new_name}` in {} file(s); reindexed",
            changes.len()
        );
    } else {
        println!("{}", format_plan(&changes, old_name, new_name));
    }
    Ok(())
}

fn format_plan(changes: &[RenameChange], old_name: &str, new_name: &str) -> String {
    if changes.is_empty() {
        return format!("no occurrences of `{old_name}` found to rename");
    }
    let mut out = format!("rename `{old_name}` -> `{new_name}`:\n");
    for change in changes {
        let _ = writeln!(
            out,
            "  - {} ({} occurrence(s))",
            change.file_path, change.occurrences
        );
    }
    let _ = write!(out, "\npass --write to apply");
    out
}

/// Finds every file that would need a whole-word `old_name` -> `new_name`
/// replacement: the symbol's own declaration file, plus every file that
/// reaches it through a non-`AMBIGUOUS`, non-`Explains` edge.
///
/// # Errors
///
/// Returns [`Error::IndexMissing`] if `root` has no index, `Error::SymbolNotFound`
/// if `old_name` isn't in the graph, or [`Error::AmbiguousRename`] if more
/// than one symbol shares that name.
///
/// # Panics
///
/// Never in practice: a [`Node`](crate::storage::Node) read back from
/// storage always has `id: Some(_)`.
pub fn rename_plan(root: &Path, old_name: &str, new_name: &str) -> Result<Vec<RenameChange>> {
    let graph = Graph::open_existing(root)?;
    let all_nodes = graph.all_nodes()?;
    let matches: Vec<_> = all_nodes.iter().filter(|n| n.name == old_name).collect();

    let target = match matches.as_slice() {
        [] => {
            return Err(Error::SymbolNotFound {
                name: old_name.to_string(),
            });
        }
        [single] => single,
        multiple => {
            return Err(Error::AmbiguousRename {
                name: old_name.to_string(),
                count: multiple.len(),
            });
        }
    };
    let target_id = target
        .id
        .expect("node loaded from storage always has an id");

    let mut files: HashSet<String> = HashSet::new();
    files.insert(target.file_path.clone());
    for (caller, kind, confidence) in graph.callers(target_id)? {
        if confidence == Confidence::Ambiguous || kind == EdgeKind::Explains {
            continue;
        }
        files.insert(caller.file_path);
    }

    let mut changes = Vec::new();
    for file_path in files {
        let Ok(content) = fs::read_to_string(root.join(&file_path)) else {
            continue;
        };
        let occurrences = whole_word_count(&content, old_name);
        if occurrences > 0 {
            changes.push(RenameChange {
                file_path,
                occurrences,
            });
        }
    }
    changes.sort_by(|a, b| a.file_path.cmp(&b.file_path));
    let _ = new_name; // reserved for a future dry-run diff preview
    Ok(changes)
}

/// Applies a [`rename_plan`], then reindexes so the graph reflects the new
/// name immediately (the same full-rebuild `resolve::index_repo` the
/// watcher relies on — see its docs for why that's safe to call here too).
///
/// # Errors
///
/// Returns a storage/IO error if a file can't be written or the reindex fails.
pub fn rename_apply(
    root: &Path,
    changes: &[RenameChange],
    old_name: &str,
    new_name: &str,
) -> Result<()> {
    for change in changes {
        let path = root.join(&change.file_path);
        let content = fs::read_to_string(&path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        let replaced = whole_word_replace(&content, old_name, new_name);
        fs::write(&path, replaced).map_err(|source| Error::Write {
            path: path.clone(),
            source,
        })?;
    }

    let graph = Graph::open_existing(root)?;
    crate::resolve::index_repo(&graph, root)?;
    Ok(())
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_word_boundary_match(content: &str, start: usize, word: &str) -> bool {
    let before_ok = content[..start]
        .chars()
        .next_back()
        .is_none_or(|c| !is_ident_char(c));
    let after_ok = content[start + word.len()..]
        .chars()
        .next()
        .is_none_or(|c| !is_ident_char(c));
    before_ok && after_ok
}

fn whole_word_count(content: &str, word: &str) -> usize {
    let mut count = 0;
    let mut start = 0;
    while let Some(offset) = content[start..].find(word) {
        let at = start + offset;
        if is_word_boundary_match(content, at, word) {
            count += 1;
        }
        start = at + word.len();
    }
    count
}

fn whole_word_replace(content: &str, old: &str, new: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut start = 0;
    while let Some(offset) = content[start..].find(old) {
        let at = start + offset;
        out.push_str(&content[start..at]);
        if is_word_boundary_match(content, at, old) {
            out.push_str(new);
        } else {
            out.push_str(old);
        }
        start = at + old.len();
    }
    out.push_str(&content[start..]);
    out
}

/// Runs `aag affected --stdin`: reads changed file paths (one per line,
/// e.g. `git diff --name-only`) and prints every test-looking file whose
/// symbols transitively depend on something in a changed file.
///
/// # Errors
///
/// Returns [`Error::IndexMissing`] if `root` has no index, or a storage error.
pub fn affected_run(root: &Path, stdin: impl BufRead) -> Result<()> {
    let changed: Vec<String> = stdin
        .lines()
        .map_while(std::result::Result::ok)
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();

    let files = affected(root, &changed)?;
    if files.is_empty() {
        println!("no affected test files found");
    } else {
        for file in files {
            println!("{file}");
        }
    }
    Ok(())
}

/// Files reached transitively from any symbol declared in `changed_files`,
/// filtered to ones that look like test files, excluding `changed_files`
/// themselves (those are already known-affected — this tool's value is
/// surfacing *downstream* impact).
///
/// # Errors
///
/// Returns [`Error::IndexMissing`] if `root` has no index, or a storage error.
pub fn affected(root: &Path, changed_files: &[String]) -> Result<Vec<String>> {
    let graph = Graph::open_existing(root)?;
    let all_nodes = graph.all_nodes()?;
    let changed_set: HashSet<&str> = changed_files.iter().map(String::as_str).collect();

    let mut frontier: Vec<i64> = all_nodes
        .iter()
        .filter(|node| changed_set.contains(node.file_path.as_str()))
        .filter_map(|node| node.id)
        .collect();

    let mut visited_nodes: HashSet<i64> = frontier.iter().copied().collect();
    let mut visited_files: HashSet<String> = HashSet::new();
    let mut depth = 0;
    while !frontier.is_empty() && depth < MAX_DEPTH {
        depth += 1;
        let mut next = Vec::new();
        for id in frontier {
            for (caller, _, _) in graph.callers(id)? {
                visited_files.insert(caller.file_path.clone());
                let Some(caller_id) = caller.id else { continue };
                if visited_nodes.insert(caller_id) {
                    next.push(caller_id);
                }
            }
        }
        frontier = next;
    }

    let mut affected: Vec<String> = visited_files
        .into_iter()
        .filter(|f| !changed_set.contains(f.as_str()))
        .filter(|f| looks_like_test_file(f))
        .collect();
    affected.sort();
    Ok(affected)
}

fn looks_like_test_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.contains("/tests/")
        || lower.starts_with("tests/")
        || lower.contains("test_")
        || lower.contains("_test.")
        || lower.contains(".test.")
        || lower.contains(".spec.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn scratch_root() -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("aag-refactor-test-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn indexed(root: &Path) {
        crate::bigbang::run(
            root,
            &crate::bigbang::Options {
                no_viz: true,
                ..crate::bigbang::Options::default()
            },
        )
        .unwrap();
    }

    #[test]
    fn whole_word_replace_skips_substring_matches() {
        assert_eq!(
            whole_word_replace("Foo and FooBar", "Foo", "Baz"),
            "Baz and FooBar"
        );
    }

    #[test]
    fn whole_word_count_matches_spec() {
        assert_eq!(whole_word_count("Widget widget Widget", "Widget"), 2);
    }

    #[test]
    fn rename_plan_covers_declaration_and_callers() {
        let root = scratch_root();
        fs::write(root.join("a.rs"), "struct Widget;").unwrap();
        fs::write(
            root.join("b.rs"),
            "use crate::a::Widget; fn make() -> Widget { Widget }",
        )
        .unwrap();
        indexed(&root);

        let changes = rename_plan(&root, "Widget", "Gadget").unwrap();

        let files: Vec<&str> = changes.iter().map(|c| c.file_path.as_str()).collect();
        assert!(files.contains(&"a.rs"));
        assert!(files.contains(&"b.rs"));
    }

    #[test]
    fn rename_plan_rejects_ambiguous_target() {
        let root = scratch_root();
        fs::write(root.join("a.rs"), "fn run() {}").unwrap();
        fs::write(root.join("b.rs"), "fn run() {}").unwrap();
        indexed(&root);

        let result = rename_plan(&root, "run", "execute");

        assert!(matches!(
            result,
            Err(Error::AmbiguousRename { count: 2, .. })
        ));
    }

    #[test]
    fn rename_apply_writes_new_name_and_reindexes() {
        // `b.rs` needs a real edge to `Widget` (here, a `use` import) for
        // rename's file discovery to include it — a bare type reference
        // with no call/import syntax isn't captured as an edge yet (see
        // module docs on rename's name-based, edge-scoped heuristic).
        let root = scratch_root();
        fs::write(root.join("a.rs"), "struct Widget;").unwrap();
        fs::write(
            root.join("b.rs"),
            "use crate::a::Widget; fn make() -> Widget { Widget }",
        )
        .unwrap();
        indexed(&root);

        let changes = rename_plan(&root, "Widget", "Gadget").unwrap();
        rename_apply(&root, &changes, "Widget", "Gadget").unwrap();

        let b_content = fs::read_to_string(root.join("b.rs")).unwrap();
        assert_eq!(
            b_content,
            "use crate::a::Gadget; fn make() -> Gadget { Gadget }"
        );

        let graph = Graph::open_existing(&root).unwrap();
        assert!(graph.find_by_name("Gadget").unwrap().is_some());
        assert!(graph.find_by_name("Widget").unwrap().is_none());
    }

    #[test]
    fn affected_finds_downstream_test_file_not_the_changed_file_itself() {
        let root = scratch_root();
        fs::write(root.join("lib.rs"), "pub fn core() {}").unwrap();
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::write(
            root.join("tests").join("core_test.rs"),
            "fn it_works() { core(); }",
        )
        .unwrap();
        indexed(&root);

        let files = affected(&root, &["lib.rs".to_string()]).unwrap();

        assert_eq!(files, vec!["tests/core_test.rs".to_string()]);
    }

    #[test]
    fn affected_returns_empty_for_unrelated_change() {
        let root = scratch_root();
        fs::write(root.join("lib.rs"), "pub fn core() {}").unwrap();
        indexed(&root);

        let files = affected(&root, &["unrelated.rs".to_string()]).unwrap();

        assert!(files.is_empty());
    }
}
