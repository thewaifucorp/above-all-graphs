//! Outcome-backed work memory: what was asked, what was answered, which nodes
//! supported it, whether it held up, and what corrected it.
//!
//! P1.12 of `docs/capability-coverage.md`. The gate's constraint is the design:
//! **derive reviewable lessons without letting stale experience override current
//! source evidence.** Two rules follow from it, and neither is negotiable:
//!
//! - Every recalled entry is checked against the graph as it is *now*. An entry
//!   whose supporting symbols no longer exist is returned marked `stale`, never
//!   silently as fact.
//! - A lesson is a review candidate with its evidence attached, not an assertion.
//!   The output says how many entries it came from and how many of those are
//!   still supported, so a reader can dismiss it in one glance.
//!
//! Memory lives in `.aag/memory.db`, beside the graph and separate from it: an
//! index rebuild must not erase what a session learned, and a recorded answer
//! must never be mistaken for extracted evidence.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::storage::Graph;

/// How an answer turned out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// It held up.
    Worked,
    /// It did not, and `correction` says what replaced it.
    Wrong,
    /// Not yet known — recorded so an unverified answer is not counted as one
    /// that worked.
    Open,
}

impl Outcome {
    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Worked => "worked",
            Self::Wrong => "wrong",
            Self::Open => "open",
        }
    }

    /// Parses the stored or user-supplied form, defaulting to `open` — an
    /// unrecognized outcome must not read as a success.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "worked" | "ok" | "good" => Self::Worked,
            "wrong" | "bad" | "failed" => Self::Wrong,
            _ => Self::Open,
        }
    }
}

/// One remembered piece of work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Row id.
    pub id: i64,
    /// What was asked.
    pub question: String,
    /// What was answered.
    pub answer: String,
    /// Symbols the answer rested on.
    pub nodes: Vec<String>,
    /// How it turned out.
    pub outcome: Outcome,
    /// What replaced a wrong answer.
    pub correction: Option<String>,
    /// The commit the work landed in, when it landed in one.
    pub revision: Option<String>,
    /// Seconds since the epoch when it was recorded.
    pub recorded: i64,
}

/// One recalled entry, with what the current graph says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recalled {
    /// The stored entry.
    pub entry: Entry,
    /// Supporting symbols the graph still has.
    pub present: Vec<String>,
    /// Supporting symbols it no longer has.
    pub missing: Vec<String>,
}

impl Recalled {
    /// Whether the code this entry rested on has moved on.
    ///
    /// An entry with no supporting symbols is stale by this definition: nothing
    /// ties it to the repository, so it cannot be checked, and an unverifiable
    /// memory is exactly what must not override source evidence.
    #[must_use]
    pub fn stale(&self) -> bool {
        self.present.is_empty() || !self.missing.is_empty()
    }
}

/// Opens (and creates) the memory database beside the graph.
fn open(root: &Path) -> Result<Connection> {
    let directory = root.join(".aag");
    std::fs::create_dir_all(&directory).map_err(|source| Error::CreateDir {
        path: directory.clone(),
        source,
    })?;
    let connection =
        Connection::open(directory.join("memory.db")).map_err(|source| Error::Storage {
            context: "open work memory",
            source,
        })?;
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS entries (
                 id INTEGER PRIMARY KEY,
                 question TEXT NOT NULL,
                 answer TEXT NOT NULL,
                 nodes TEXT NOT NULL,
                 outcome TEXT NOT NULL,
                 correction TEXT,
                 revision TEXT,
                 recorded INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS entries_question ON entries(question);",
        )
        .map_err(|source| Error::Storage {
            context: "create work memory schema",
            source,
        })?;
    Ok(connection)
}

/// What to record.
#[derive(Debug, Clone, Default)]
pub struct Record {
    /// What was asked.
    pub question: String,
    /// What was answered.
    pub answer: String,
    /// Symbols the answer rested on.
    pub nodes: Vec<String>,
    /// How it turned out; `open` when not yet known.
    pub outcome: String,
    /// What replaced a wrong answer.
    pub correction: Option<String>,
    /// The commit the work landed in.
    pub revision: Option<String>,
}

/// Saves one entry and returns its id.
///
/// # Errors
/// Returns a storage error when the memory database cannot be written.
pub fn save(root: &Path, record: &Record) -> Result<i64> {
    let connection = open(root)?;
    let recorded = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(0));
    connection
        .execute(
            "INSERT INTO entries (question, answer, nodes, outcome, correction, revision, recorded)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            (
                &record.question,
                &record.answer,
                record.nodes.join(","),
                Outcome::parse(&record.outcome).as_str(),
                &record.correction,
                &record.revision,
                recorded,
            ),
        )
        .map_err(|source| Error::Storage {
            context: "save a work memory entry",
            source,
        })?;
    Ok(connection.last_insert_rowid())
}

/// Records how an earlier answer turned out, and what corrected it.
///
/// # Errors
/// Returns a storage error, or [`Error::Protocol`] when no entry has that id.
pub fn correct(root: &Path, id: i64, outcome: &str, correction: Option<&str>) -> Result<()> {
    let connection = open(root)?;
    let changed = connection
        .execute(
            "UPDATE entries SET outcome = ?1, correction = COALESCE(?2, correction) WHERE id = ?3",
            (Outcome::parse(outcome).as_str(), correction, id),
        )
        .map_err(|source| Error::Storage {
            context: "correct a work memory entry",
            source,
        })?;
    if changed == 0 {
        return Err(Error::Protocol {
            context: "unknown memory entry",
            detail: format!("no entry with id {id}"),
        });
    }
    Ok(())
}

/// Every stored entry, newest first.
///
/// # Errors
/// Returns a storage error when the memory database cannot be read.
pub fn entries(root: &Path) -> Result<Vec<Entry>> {
    let connection = open(root)?;
    let mut statement = connection
        .prepare(
            "SELECT id, question, answer, nodes, outcome, correction, revision, recorded
             FROM entries ORDER BY recorded DESC, id DESC",
        )
        .map_err(|source| Error::Storage {
            context: "prepare work memory query",
            source,
        })?;
    let rows = statement
        .query_map([], |row| {
            let nodes: String = row.get(3)?;
            Ok(Entry {
                id: row.get(0)?,
                question: row.get(1)?,
                answer: row.get(2)?,
                nodes: nodes
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
                    .collect(),
                outcome: Outcome::parse(&row.get::<_, String>(4)?),
                correction: row.get(5)?,
                revision: row.get(6)?,
                recorded: row.get(7)?,
            })
        })
        .map_err(|source| Error::Storage {
            context: "run work memory query",
            source,
        })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|source| Error::Storage {
            context: "read a work memory row",
            source,
        })?);
    }
    Ok(out)
}

/// Entries relevant to `question`, each checked against the current graph.
///
/// Relevance is word overlap with the stored question, which is deliberately
/// dumb: memory is a hint, and a clever matcher would make it feel like an
/// answer. Every result carries which of its supporting symbols the graph still
/// has, so a stale entry is visibly stale.
///
/// # Errors
/// Returns a storage error when memory or the graph cannot be read.
pub fn recall(root: &Path, question: &str) -> Result<Vec<Recalled>> {
    let wanted: Vec<String> = words(question);
    let graph = Graph::open_existing(root).ok();
    let mut scored: Vec<(usize, Recalled)> = Vec::new();
    for entry in entries(root)? {
        let overlap = words(&entry.question)
            .iter()
            .filter(|word| wanted.contains(word))
            .count();
        if overlap == 0 && !wanted.is_empty() {
            continue;
        }
        let (mut present, mut missing) = (Vec::new(), Vec::new());
        for name in &entry.nodes {
            let known = graph
                .as_ref()
                .and_then(|graph| graph.find_by_name(name).ok().flatten())
                .is_some();
            if known {
                present.push(name.clone());
            } else {
                missing.push(name.clone());
            }
        }
        scored.push((
            overlap,
            Recalled {
                entry,
                present,
                missing,
            },
        ));
    }
    // Most relevant first; a wrong answer outranks an open one at equal
    // relevance, because knowing what failed is the more useful memory.
    scored.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| rank(right.1.entry.outcome).cmp(&rank(left.1.entry.outcome)))
            .then_with(|| right.1.entry.recorded.cmp(&left.1.entry.recorded))
    });
    Ok(scored.into_iter().map(|(_, recalled)| recalled).collect())
}

const fn rank(outcome: Outcome) -> u8 {
    match outcome {
        Outcome::Wrong => 2,
        Outcome::Worked => 1,
        Outcome::Open => 0,
    }
}

/// Words worth matching on: lowercase, deduplicated, and without the words that
/// appear in every question.
fn words(text: &str) -> Vec<String> {
    const NOISE: &[&str] = &[
        "the", "a", "an", "is", "are", "was", "were", "how", "what", "why", "does", "do", "did",
        "in", "of", "to", "and", "or", "for", "this", "that", "it", "on", "with",
    ];
    let mut out: Vec<String> = text
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|word| word.len() > 2)
        .map(str::to_ascii_lowercase)
        .filter(|word| !NOISE.contains(&word.as_str()))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// One reviewable lesson, derived from repeated outcomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lesson {
    /// The symbol the lesson is about.
    pub subject: String,
    /// What the entries say happened.
    pub claim: String,
    /// How many entries it was derived from.
    pub entries: usize,
    /// How many of those are still supported by the current graph.
    pub supported: usize,
    /// Ids of the entries behind it, so it can be reviewed rather than trusted.
    pub evidence: Vec<i64>,
}

/// Derives lessons from entries that agree, without asserting any of them.
///
/// A lesson needs at least two entries about one symbol, because one outcome is
/// an anecdote. It reports how many of its entries the graph still supports:
/// a lesson at `0 supported` is about code that no longer exists, and the
/// formatter says so instead of repeating it as advice.
///
/// # Errors
/// Returns a storage error when memory or the graph cannot be read.
pub fn lessons(root: &Path) -> Result<Vec<Lesson>> {
    let graph = Graph::open_existing(root).ok();
    let mut by_subject: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    for entry in entries(root)? {
        for name in &entry.nodes {
            by_subject
                .entry(name.clone())
                .or_default()
                .push(entry.clone());
        }
    }
    let mut out = Vec::new();
    for (subject, group) in by_subject {
        if group.len() < 2 {
            continue;
        }
        let wrong = group
            .iter()
            .filter(|entry| entry.outcome == Outcome::Wrong)
            .count();
        let worked = group
            .iter()
            .filter(|entry| entry.outcome == Outcome::Worked)
            .count();
        if wrong == 0 && worked == 0 {
            continue;
        }
        let present = graph
            .as_ref()
            .and_then(|graph| graph.find_by_name(&subject).ok().flatten())
            .is_some();
        let claim = if wrong > worked {
            let corrections: Vec<&str> = group
                .iter()
                .filter_map(|entry| entry.correction.as_deref())
                .collect();
            format!(
                "answers about `{subject}` were wrong {wrong} of {} times{}",
                group.len(),
                if corrections.is_empty() {
                    String::new()
                } else {
                    format!("; corrected to: {}", corrections.join(" | "))
                }
            )
        } else {
            format!(
                "answers about `{subject}` held up {worked} of {} times",
                group.len()
            )
        };
        out.push(Lesson {
            subject,
            claim,
            entries: group.len(),
            supported: if present { group.len() } else { 0 },
            evidence: group.iter().map(|entry| entry.id).collect(),
        });
    }
    // Most-evidenced first, and a lesson the graph still supports before one it
    // does not.
    out.sort_by(|left, right| {
        right
            .supported
            .cmp(&left.supported)
            .then_with(|| right.entries.cmp(&left.entries))
            .then_with(|| left.subject.cmp(&right.subject))
    });
    Ok(out)
}

/// Renders a recall for an agent or a terminal.
///
/// # Errors
/// As [`recall`].
pub fn format_recall(root: &Path, question: &str) -> Result<String> {
    let recalled = recall(root, question)?;
    if recalled.is_empty() {
        return Ok(format!(
            "nothing remembered about `{question}`. Memory is a hint, and its absence says \
             nothing about the code — ask the graph."
        ));
    }
    let mut out = vec![
        "Remembered work. This is recorded experience, not extracted evidence: where it \
         disagrees with the graph, the graph is right."
            .to_string(),
        String::new(),
    ];
    for item in &recalled {
        let mut line = format!(
            "#{} [{}]{} {}",
            item.entry.id,
            item.entry.outcome.as_str(),
            if item.stale() { " [stale]" } else { "" },
            item.entry.question
        );
        let _ = write!(line, "\n     answered: {}", item.entry.answer);
        if let Some(correction) = &item.entry.correction {
            let _ = write!(line, "\n     corrected to: {correction}");
        }
        if !item.present.is_empty() {
            let _ = write!(
                line,
                "\n     still in the graph: {}",
                item.present.join(", ")
            );
        }
        if !item.missing.is_empty() {
            let _ = write!(
                line,
                "\n     gone from the graph: {} — verify before reusing this",
                item.missing.join(", ")
            );
        }
        if let Some(revision) = &item.entry.revision {
            let _ = write!(line, "\n     landed in: {revision}");
        }
        out.push(line);
    }
    Ok(out.join("\n"))
}

/// Renders derived lessons.
///
/// # Errors
/// As [`lessons`].
pub fn format_lessons(root: &Path, subject: &str) -> Result<String> {
    let lessons: Vec<Lesson> = lessons(root)?
        .into_iter()
        .filter(|lesson| subject.is_empty() || lesson.subject.contains(subject))
        .collect();
    if lessons.is_empty() {
        return Ok(
            "no lessons yet. A lesson needs at least two recorded outcomes about one \
                   symbol — one outcome is an anecdote."
                .to_string(),
        );
    }
    let mut out = vec![
        "Review candidates derived from recorded outcomes. Each is a pattern in what was \
         recorded, not a fact about the code."
            .to_string(),
        String::new(),
    ];
    for lesson in &lessons {
        out.push(format!(
            "{} (from {} entr{}, {} still supported by the graph; ids {})",
            lesson.claim,
            lesson.entries,
            if lesson.entries == 1 { "y" } else { "ies" },
            lesson.supported,
            lesson
                .evidence
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
        if lesson.supported == 0 {
            out.push(
                "     the symbol this is about is not in the current graph — treat it as history"
                    .to_string(),
            );
        }
    }
    Ok(out.join("\n"))
}

/// The JSON form, for the MCP surface.
///
/// # Errors
/// As [`recall`].
pub fn recall_json(root: &Path, question: &str) -> Result<String> {
    let rows: Vec<Value> = recall(root, question)?
        .into_iter()
        .map(|item| {
            json!({
                "id": item.entry.id,
                "question": item.entry.question,
                "answer": item.entry.answer,
                "outcome": item.entry.outcome.as_str(),
                "correction": item.entry.correction,
                "revision": item.entry.revision,
                "stale": item.stale(),
                "present": item.present,
                "missing": item.missing,
            })
        })
        .collect();
    let payload = json!({
        "recalled": rows,
        "note": "Recorded experience, not extracted evidence. Where it disagrees with the \
                 graph, the graph is right.",
    });
    serde_json::to_string_pretty(&payload).map_err(|error| Error::Protocol {
        context: "serialize recalled memory",
        detail: error.to_string(),
    })
}

/// Where memory is stored, for anyone who needs to back it up or delete it.
#[must_use]
pub fn location(root: &Path) -> PathBuf {
    root.join(".aag").join("memory.db")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// A scratch repository with a graph, so recall can check memory against it.
    fn indexed_root() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("aag-memory-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("lib.rs"),
            "fn resolve_call() {}\nfn caller() { resolve_call(); }\n",
        )
        .unwrap();
        crate::bigbang::run(
            &root,
            &crate::bigbang::Options {
                no_viz: true,
                no_install: true,
                ..crate::bigbang::Options::default()
            },
        )
        .unwrap();
        root
    }

    fn record(question: &str, answer: &str, nodes: &[&str], outcome: &str) -> Record {
        Record {
            question: question.to_string(),
            answer: answer.to_string(),
            nodes: nodes.iter().map(|name| (*name).to_string()).collect(),
            outcome: outcome.to_string(),
            correction: None,
            revision: None,
        }
    }

    #[test]
    fn a_saved_entry_comes_back_with_its_outcome() {
        let root = indexed_root();

        let id = save(
            &root,
            &record(
                "how does call resolution pick a candidate",
                "through the narrowing ladder in resolve_call",
                &["resolve_call"],
                "worked",
            ),
        )
        .unwrap();

        let recalled = recall(&root, "how does call resolution work").unwrap();
        assert_eq!(recalled.len(), 1, "{recalled:?}");
        assert_eq!(recalled[0].entry.id, id);
        assert_eq!(recalled[0].entry.outcome, Outcome::Worked);
        assert_eq!(recalled[0].present, vec!["resolve_call".to_string()]);
        assert!(!recalled[0].stale());
    }

    #[test]
    fn an_entry_whose_symbol_is_gone_comes_back_marked_stale() {
        let root = indexed_root();
        save(
            &root,
            &record(
                "where is the legacy resolver",
                "in old_resolver",
                &["old_resolver"],
                "worked",
            ),
        )
        .unwrap();

        let recalled = recall(&root, "legacy resolver").unwrap();

        assert_eq!(recalled.len(), 1);
        assert!(
            recalled[0].stale(),
            "the symbol it rested on is not in the graph: {recalled:?}"
        );
        assert_eq!(recalled[0].missing, vec!["old_resolver".to_string()]);
        let text = format_recall(&root, "legacy resolver").unwrap();
        assert!(text.contains("[stale]"), "{text}");
        assert!(text.contains("verify before reusing this"), "{text}");
        assert!(
            text.contains("where it disagrees with the graph, the graph is right"),
            "the header has to say what memory is: {text}"
        );
    }

    #[test]
    fn an_unverified_answer_is_open_not_a_success() {
        let root = indexed_root();
        let id = save(
            &root,
            &record("does sync short-circuit", "yes", &["resolve_call"], "maybe"),
        )
        .unwrap();

        let before = entries(&root).unwrap();
        assert_eq!(
            before[0].outcome,
            Outcome::Open,
            "an unknown word is not a success"
        );

        correct(&root, id, "wrong", Some("no, only for irrelevant paths")).unwrap();

        let after = entries(&root).unwrap();
        assert_eq!(after[0].outcome, Outcome::Wrong);
        assert_eq!(
            after[0].correction.as_deref(),
            Some("no, only for irrelevant paths")
        );
    }

    #[test]
    fn correcting_an_unknown_entry_is_an_error() {
        let root = indexed_root();
        let error = correct(&root, 999, "wrong", None).unwrap_err();
        assert!(error.to_string().contains("no entry with id 999"));
    }

    #[test]
    fn a_lesson_needs_two_outcomes_and_carries_its_evidence() {
        let root = indexed_root();
        let first = save(
            &root,
            &record(
                "who calls resolve_call",
                "nobody",
                &["resolve_call"],
                "wrong",
            ),
        )
        .unwrap();
        assert!(
            lessons(&root).unwrap().is_empty(),
            "one outcome is an anecdote"
        );

        let second = save(
            &root,
            &record(
                "is resolve_call unused",
                "no, caller calls it",
                &["resolve_call"],
                "wrong",
            ),
        )
        .unwrap();

        let lessons = lessons(&root).unwrap();
        assert_eq!(lessons.len(), 1, "{lessons:?}");
        assert_eq!(lessons[0].subject, "resolve_call");
        assert!(
            lessons[0].claim.contains("wrong 2 of 2"),
            "{:?}",
            lessons[0]
        );
        assert_eq!(lessons[0].supported, 2, "the symbol is still in the graph");
        assert!(lessons[0].evidence.contains(&first) && lessons[0].evidence.contains(&second));
        let text = format_lessons(&root, "").unwrap();
        assert!(text.contains("not a fact about the code"), "{text}");
    }

    #[test]
    fn a_lesson_about_deleted_code_is_labelled_history() {
        let root = indexed_root();
        for question in ["is gone_fn safe to change", "what calls gone_fn"] {
            save(&root, &record(question, "unclear", &["gone_fn"], "wrong")).unwrap();
        }

        let lessons = lessons(&root).unwrap();

        assert_eq!(lessons[0].supported, 0);
        let text = format_lessons(&root, "").unwrap();
        assert!(text.contains("treat it as history"), "{text}");
    }

    #[test]
    fn recall_prefers_a_wrong_answer_over_an_open_one() {
        let root = indexed_root();
        save(
            &root,
            &record("about resolve_call", "guess", &["resolve_call"], "open"),
        )
        .unwrap();
        save(
            &root,
            &record(
                "about resolve_call",
                "wrong guess",
                &["resolve_call"],
                "wrong",
            ),
        )
        .unwrap();

        let recalled = recall(&root, "about resolve_call").unwrap();

        assert_eq!(
            recalled[0].entry.outcome,
            Outcome::Wrong,
            "knowing what failed is the more useful memory"
        );
    }

    #[test]
    fn nothing_remembered_says_so_without_implying_anything_about_the_code() {
        let root = indexed_root();

        let text = format_recall(&root, "anything at all").unwrap();

        assert!(text.contains("nothing remembered"), "{text}");
        assert!(text.contains("says nothing about the code"), "{text}");
    }

    #[test]
    fn memory_survives_an_index_rebuild() {
        let root = indexed_root();
        let id = save(&root, &record("kept", "yes", &["resolve_call"], "worked")).unwrap();

        crate::bigbang::run(
            &root,
            &crate::bigbang::Options {
                force: true,
                no_viz: true,
                no_install: true,
                ..crate::bigbang::Options::default()
            },
        )
        .unwrap();

        let kept = entries(&root).unwrap();
        assert!(
            kept.iter().any(|entry| entry.id == id),
            "a rebuilt graph must not erase what a session learned"
        );
    }
}
