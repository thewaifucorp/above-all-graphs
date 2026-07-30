//! `aag hook <event>` — the Claude Code hook entry points that `aag
//! install` registers in `.claude/settings.json`, per `SPEC.md` section 8.
//!
//! Contract with the agent harness: read one JSON payload from stdin,
//! optionally print a `hookSpecificOutput` JSON to stdout, and **always
//! exit 0** — a knowledge-graph hiccup must never block or fail the
//! agent's edit. Every fallible step here degrades to "do nothing"
//! rather than erroring.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::storage::{Graph, NodeKind};

/// Callers at-or-above this count get a `PreToolUse` blast-radius warning.
const WARN_CALLERS: usize = 5;

/// At most this many symbols are named in one warning line.
const WARN_TOP: usize = 3;

/// Which hook event `aag hook` was invoked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// `PreToolUse` on Edit|Write — inject a blast-radius warning.
    PreEdit,
    /// `PostToolUse` on Write|Edit — kick off a background `aag sync`.
    PostEdit,
    /// `SessionStart` — reconcile the index and inject a graph digest.
    SessionStart,
    /// `PreToolUse` on Grep|Glob — point a code search at the graph instead.
    ///
    /// Without this the graph is installed but never reaches the agent at the
    /// one moment it would replace the tool being reached for: the search
    /// itself. `pre-edit` fires after navigation already happened by grep,
    /// which is too late to change how the code was found.
    PreSearch,
}

/// Runs one hook event against the index under `root`, reading the hook
/// payload from `input`. Infallible by design: failures are logged at
/// debug level and swallowed.
pub fn run(root: &Path, event: Event, input: &mut dyn Read) {
    // The harness invokes hooks with `--path .` but sends absolute file
    // paths in the payload — canonicalize so strip_prefix lines up.
    let root = &root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let payload = read_payload(input);
    match event {
        Event::PreEdit => pre_edit(root, &payload),
        Event::PostEdit => post_edit(root, &payload),
        Event::SessionStart => session_start(root),
        Event::PreSearch => pre_search(root, &payload),
    }
}

/// The search term from a `Grep`/`Glob` payload. Claude Code sends `pattern`;
/// a `Bash` grep arrives as `command`, which we deliberately ignore — parsing a
/// shell line is guesswork, and a false nudge trains the agent to skip nudges.
fn search_pattern(payload: &Value) -> Option<String> {
    let input = payload.get("tool_input")?;
    let pattern = input.get("pattern")?.as_str()?.trim();
    (!pattern.is_empty()).then(|| pattern.to_string())
}

/// `PreToolUse` on Grep|Glob: when the pattern names something the graph
/// already knows, say so and point at the tool that answers better.
///
/// Silent unless the graph can actually beat the search — a pattern that
/// matches no indexed symbol gets no output, because the honest answer there is
/// "grep is the right tool".
fn pre_search(root: &Path, payload: &Value) {
    let Some(pattern) = search_pattern(payload) else {
        return;
    };
    let Some(hint) = search_hint(root, &pattern) else {
        return;
    };
    print_context("PreToolUse", &hint);
}

/// The nudge for `pattern`, or `None` when the index is missing or holds no
/// symbol by that name.
fn search_hint(root: &Path, pattern: &str) -> Option<String> {
    let graph = Graph::open_existing(root).ok()?;
    let nodes = graph.all_nodes().ok()?;

    // Bare identifier-ish patterns only: a regex with metacharacters is a text
    // search, not a symbol lookup, and the graph has nothing better to offer.
    if !pattern
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == ':')
    {
        return None;
    }

    let needle = pattern.rsplit(':').next().unwrap_or(pattern);
    let matches: Vec<&str> = nodes
        .iter()
        .filter(|node| node.kind != NodeKind::File && node.name == needle)
        .map(|node| node.file_path.as_str())
        .collect();
    if matches.is_empty() {
        return None;
    }

    let mut files: Vec<&str> = matches.clone();
    files.sort_unstable();
    files.dedup();
    let shown: Vec<&str> = files.iter().take(WARN_TOP).copied().collect();
    let more = files.len().saturating_sub(shown.len());
    let tail = if more > 0 {
        format!(" (+{more} more)")
    } else {
        String::new()
    };

    Some(format!(
        "aag: `{needle}` is already in the graph — {} definition(s) in {}{tail}. \
         `aag explore {needle}` returns the source plus call paths, and \
         `aag impact {needle}` the blast radius, across every indexed repo — \
         which a grep of this repo cannot see.",
        matches.len(),
        shown.join(", "),
    ))
}

fn read_payload(input: &mut dyn Read) -> Value {
    let mut raw = String::new();
    if input.read_to_string(&mut raw).is_err() {
        return Value::Null;
    }
    serde_json::from_str(&raw).unwrap_or(Value::Null)
}

/// The edited file from a hook payload. Claude Code nests it under
/// `tool_input.file_path`; Cursor's `afterFileEdit` sends a top-level
/// `file_path`.
fn edited_file(payload: &Value) -> Option<PathBuf> {
    payload
        .get("tool_input")
        .and_then(|input| input.get("file_path"))
        .or_else(|| payload.get("file_path"))
        .and_then(Value::as_str)
        .map(PathBuf::from)
}

/// `PreToolUse`: if the file about to be edited contains symbols with a wide
/// blast radius, tell the agent BEFORE the edit lands.
fn pre_edit(root: &Path, payload: &Value) {
    let Some(file) = edited_file(payload) else {
        return;
    };
    if !crate::sync::is_relevant(root, Some(&file)) {
        return;
    }
    let Some(warning) = blast_radius_warning(root, &file) else {
        return;
    };
    print_context("PreToolUse", &warning);
}

/// The one-line warning for `file`, or `None` when the index is missing,
/// the file isn't indexed, or nothing in it is widely used.
fn blast_radius_warning(root: &Path, file: &Path) -> Option<String> {
    let graph = Graph::open_existing(root).ok()?;
    let relative = file
        .strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/");

    let nodes = graph.all_nodes().ok()?;
    let in_degree = in_degree_by_node(&graph);

    let mut hot: Vec<(usize, String)> = nodes
        .iter()
        .filter(|node| node.file_path == relative && node.kind != NodeKind::File)
        .filter_map(|node| {
            let count = node.id.map_or(0, |id| *in_degree.get(&id).unwrap_or(&0));
            (count >= WARN_CALLERS).then(|| (count, node.name.clone()))
        })
        .collect();
    if hot.is_empty() {
        return None;
    }
    hot.sort_by_key(|entry| std::cmp::Reverse(entry.0));

    let listed = hot
        .iter()
        .take(WARN_TOP)
        .map(|(count, name)| format!("`{name}` ({count} callers)"))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "aag: {relative} contains widely-used symbols: {listed}. \
         Run `aag impact <symbol>` before changing signatures or behavior."
    ))
}

/// In-degree (callers/importers/mentions) per node id, from one
/// `all_edges` scan — cheaper than a `callers()` query per node.
fn in_degree_by_node(graph: &Graph) -> HashMap<i64, usize> {
    let mut in_degree: HashMap<i64, usize> = HashMap::new();
    if let Ok(edges) = graph.all_edges() {
        for edge in edges {
            *in_degree.entry(edge.dst).or_insert(0) += 1;
        }
    }
    in_degree
}

/// `PostToolUse`: spawn a detached `aag sync --file <path>` and return
/// immediately — the harness gets its exit 0 without waiting on the
/// reindex. The relevance short-circuit runs here too, so edits to `.aag/`
/// outputs don't even cost a process spawn.
fn post_edit(root: &Path, payload: &Value) {
    let Some(file) = edited_file(payload) else {
        return;
    };
    if !crate::sync::is_relevant(root, Some(&file)) {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let spawned = std::process::Command::new(exe)
        .arg("sync")
        .arg("--path")
        .arg(root)
        .arg("--file")
        .arg(&file)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Err(error) = spawned {
        tracing::debug!(%error, "post-edit: could not spawn sync");
    }
}

/// `SessionStart`: absorb edits made while no watcher was running, then hand
/// the agent a map of the repo so it starts the session oriented.
fn session_start(root: &Path) {
    if let Err(error) = crate::watch::reconcile(root) {
        tracing::debug!(%error, "session-start: reconcile failed");
        return;
    }
    if let Some(digest) = graph_digest(root) {
        print_context("SessionStart", &digest);
    }
}

/// Short orientation blurb: index size, top hubs by in-degree, and where
/// the generated site lives.
fn graph_digest(root: &Path) -> Option<String> {
    let graph = Graph::open_existing(root).ok()?;
    let nodes = graph.all_nodes().ok()?;
    let edge_count = graph.all_edges().ok()?.len();
    let in_degree = in_degree_by_node(&graph);

    let files = nodes
        .iter()
        .filter(|node| node.kind == NodeKind::File)
        .count();
    let symbols = nodes
        .iter()
        .filter(|node| !matches!(node.kind, NodeKind::File | NodeKind::Doc))
        .count();

    let mut hubs: Vec<(usize, String)> = nodes
        .iter()
        .filter(|node| !matches!(node.kind, NodeKind::File | NodeKind::Doc))
        .filter_map(|node| {
            let count = *in_degree.get(&node.id?).unwrap_or(&0);
            // Qualify with the file: same-named symbols (e.g. one `run`
            // per module) are distinct hubs and unreadable unqualified.
            (count > 0).then(|| (count, format!("{}:{}", node.file_path, node.name)))
        })
        .collect();
    hubs.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let hubs = hubs
        .iter()
        .take(5)
        .map(|(count, name)| format!("`{name}` ({count})"))
        .collect::<Vec<_>>()
        .join(", ");

    let mut digest = format!(
        "aag knowledge graph is fresh: {files} files, {symbols} symbols, {edge_count} edges."
    );
    if !hubs.is_empty() {
        let _ = write!(digest, " Most-connected symbols: {hubs}.");
    }
    digest.push_str(
        " Query it with the `explore` MCP tool or `aag explore <query>`; \
         blast radius via `aag impact <symbol>`. Site: .aag/index.html",
    );
    Some(digest)
}

/// Prints the `hookSpecificOutput` envelope the Claude Code hook protocol
/// expects on stdout.
fn print_context(event_name: &str, context: &str) {
    let output = json!({
        "hookSpecificOutput": {
            "hookEventName": event_name,
            "additionalContext": context,
        }
    });
    println!("{output}");
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
        let dir = std::env::temp_dir().join(format!("aag-hook-test-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn edited_file_reads_tool_input() {
        let payload = json!({"tool_input": {"file_path": "/repo/src/lib.rs"}});
        assert_eq!(
            edited_file(&payload),
            Some(PathBuf::from("/repo/src/lib.rs"))
        );
    }

    #[test]
    fn edited_file_absent_returns_none() {
        assert_eq!(edited_file(&Value::Null), None);
        assert_eq!(edited_file(&json!({"tool_input": {}})), None);
    }

    #[test]
    fn search_pattern_reads_grep_payload_and_ignores_the_rest() {
        assert_eq!(
            search_pattern(&json!({"tool_input": {"pattern": "SecretResolver"}})).as_deref(),
            Some("SecretResolver")
        );
        // Blank, absent, and a Bash command line all yield nothing: a false
        // nudge trains the agent to ignore nudges.
        assert_eq!(search_pattern(&json!({"tool_input": {"pattern": "  "}})), None);
        assert_eq!(search_pattern(&json!({"tool_input": {}})), None);
        assert_eq!(
            search_pattern(&json!({"tool_input": {"command": "grep -rn foo src/"}})),
            None
        );
        assert_eq!(search_pattern(&Value::Null), None);
    }

    /// No index, no output — the hook must stay silent rather than nudge toward
    /// a graph that cannot answer.
    #[test]
    fn search_hint_without_an_index_is_none() {
        let root = scratch_root();
        assert_eq!(search_hint(&root, "AnySymbol"), None);
    }

    #[test]
    fn pre_search_on_malformed_stdin_is_swallowed() {
        let root = scratch_root();
        let mut input = std::io::Cursor::new(b"{not json".to_vec());
        run(&root, Event::PreSearch, &mut input);
    }

    #[test]
    fn malformed_stdin_is_swallowed() {
        let root = scratch_root();
        let mut input = std::io::Cursor::new(b"not json at all".to_vec());
        // Must not panic or error — hooks are infallible.
        run(&root, Event::PreEdit, &mut input);
    }

    #[test]
    fn warning_fires_only_above_threshold() {
        let root = scratch_root();
        let mut callers = String::new();
        for i in 0..WARN_CALLERS {
            let _ = writeln!(callers, "fn caller_{i}() {{ hot(); }}");
        }
        fs::write(root.join("callers.rs"), callers).unwrap();
        fs::write(root.join("hot.rs"), "fn hot() {}\nfn cold() {}").unwrap();
        crate::bigbang::run(
            &root,
            &crate::bigbang::Options {
                no_viz: true,
                no_install: true,
                ..Default::default()
            },
        )
        .unwrap();

        let warning = blast_radius_warning(&root, &root.join("hot.rs")).unwrap();
        assert!(warning.contains("`hot`"), "warning was: {warning}");
        assert!(!warning.contains("`cold`"), "warning was: {warning}");

        assert!(blast_radius_warning(&root, &root.join("callers.rs")).is_none());
    }

    #[test]
    fn digest_reports_counts_and_hubs() {
        let root = scratch_root();
        fs::write(root.join("a.rs"), "fn caller() { helper(); }").unwrap();
        fs::write(root.join("b.rs"), "fn helper() {}").unwrap();
        crate::bigbang::run(
            &root,
            &crate::bigbang::Options {
                no_viz: true,
                no_install: true,
                ..Default::default()
            },
        )
        .unwrap();

        let digest = graph_digest(&root).unwrap();
        assert!(digest.contains("2 files"), "digest was: {digest}");
        assert!(digest.contains("`b.rs:helper`"), "digest was: {digest}");
    }
}
