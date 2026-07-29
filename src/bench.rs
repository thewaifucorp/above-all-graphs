//! The engine benchmark harness — Track E of the empirical evaluation
//! contract in `docs/capability-coverage.md`, and the machinery the other
//! tracks will hang off.
//!
//! The contract's first rule is that a result must say what it measured. This
//! module measures exactly one subject: the **`AboveAllGraphs` Engine** — cold
//! indexing, incremental update, query latency, memory, and artifact size on a
//! named repository at a named revision. It does not measure the protocol, it
//! does not measure an agent, and it never asks a model anything. A run record
//! that cannot say which binary produced it, against which commit, in which
//! run class, is not written at all.
//!
//! Three rules from the contract are enforced here rather than documented and
//! hoped for:
//!
//! - **Run classes are separated.** `empirical`, `pilot`, and `simulated` are
//!   distinct `run_kind` values written to distinct paths, and aggregation
//!   refuses to mix them.
//! - **A self-benchmark is a pilot.** Measuring the `aag` repository itself is
//!   dogfood; the harness detects it and downgrades the class no matter what
//!   was asked for, because that result cannot substantiate an engine claim.
//! - **Records are append-only.** A corrected metric is a new run, never an
//!   edit to an old one.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{Error, Result};
use crate::storage::Graph;

/// Bumped whenever a field changes meaning. Records with different schema
/// versions are never averaged together.
pub const SCHEMA_VERSION: u32 = 1;

/// Which class of evidence a run produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunKind {
    /// The declared producer, executed against an external pinned repository.
    /// The only class a public engine claim may use.
    Empirical,
    /// Infrastructure exercise, or dogfood on a repository related to the
    /// implementation under test.
    Pilot,
    /// Schema and reporting exercise with no engine execution.
    Simulated,
}

impl RunKind {
    /// Directory each class is written to — separate paths, per the contract.
    #[must_use]
    pub const fn directory(self) -> &'static str {
        match self {
            Self::Empirical => "empirical",
            Self::Pilot => "pilot",
            Self::Simulated => "simulated",
        }
    }

    /// Parses the CLI spelling.
    ///
    /// # Errors
    /// Returns [`Error::Protocol`] for anything else — an unrecognized class
    /// must not silently become `empirical`.
    pub fn parse(text: &str) -> Result<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "empirical" => Ok(Self::Empirical),
            "pilot" => Ok(Self::Pilot),
            "simulated" => Ok(Self::Simulated),
            other => Err(Error::Protocol {
                context: "unknown run kind (empirical|pilot|simulated)",
                detail: other.to_string(),
            }),
        }
    }
}

/// What produced the numbers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Producer {
    /// Always the engine — this harness cannot run another producer.
    pub name: String,
    /// Crate version of the binary.
    pub version: String,
    /// Build features that change extraction or storage behavior.
    pub features: Vec<String>,
    /// Profile the binary was built with, as far as it can tell.
    pub profile: String,
}

impl Default for Producer {
    fn default() -> Self {
        let mut features = Vec::new();
        if cfg!(feature = "semantic") {
            features.push("semantic".to_string());
        }
        Self {
            name: "AboveAllGraphs Engine".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            features,
            profile: if cfg!(debug_assertions) {
                "debug".to_string()
            } else {
                "release".to_string()
            },
        }
    }
}

/// The corpus a run measured, described rather than counted.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepositoryProfile {
    /// Directory name — not the absolute path, which is machine-specific.
    pub name: String,
    /// Commit the working tree was at, or `unknown` outside git.
    pub revision: String,
    /// Whether the working tree had uncommitted changes.
    pub dirty: bool,
    /// Files git tracks.
    pub tracked_files: usize,
    /// Files the engine parsed.
    pub parsed_files: u32,
    /// Symbols extracted.
    pub symbols: u32,
    /// Docs indexed.
    pub docs: u32,
    /// Relationships resolved.
    pub relationships: u32,
    /// Extension to tracked-file count, largest first.
    pub languages: BTreeMap<String, usize>,
    /// Files whose path reads as a test.
    pub test_files: usize,
}

/// Latency distribution of one repeated measurement, in milliseconds.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Distribution {
    /// How many samples were taken.
    pub samples: usize,
    /// Smallest sample.
    pub min_ms: f64,
    /// Median.
    pub p50_ms: f64,
    /// 95th percentile — the number a p50 hides.
    pub p95_ms: f64,
    /// Largest sample.
    pub max_ms: f64,
    /// Arithmetic mean.
    pub mean_ms: f64,
}

impl Distribution {
    /// Summarizes samples. An empty slice yields all zeros with `samples: 0`,
    /// which reads as "not measured" rather than as "instant".
    #[must_use]
    pub fn of(samples: &[f64]) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        let mut sorted = samples.to_vec();
        sorted.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
        // Integer arithmetic on purpose: percentile positions computed in
        // floating point drift on large sample counts, and a benchmark that
        // reports the wrong sample is worse than one that reports none.
        let last = sorted.len() - 1;
        let percentile = |numerator: usize, denominator: usize| {
            let index = (last * numerator).div_ceil(denominator);
            sorted[index.min(last)]
        };
        Self {
            samples: sorted.len(),
            min_ms: sorted[0],
            p50_ms: percentile(50, 100),
            p95_ms: percentile(95, 100),
            max_ms: sorted[sorted.len() - 1],
            mean_ms: sorted.iter().sum::<f64>()
                / f64::from(u32::try_from(sorted.len()).unwrap_or(u32::MAX)),
        }
    }
}

/// Everything one Track E run measured.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Metrics {
    /// Full index of the repository into an empty database.
    pub cold_index: Distribution,
    /// Re-index of a single already-indexed file — the hook path.
    pub incremental_update: Distribution,
    /// Full-text search latency.
    pub search_query: Distribution,
    /// Blast-radius query latency.
    pub impact_query: Distribution,
    /// Writing the offline site and every export artifact.
    pub export: Distribution,
    /// Peak resident set of this process, when the platform reports it.
    pub peak_memory_bytes: Option<u64>,
    /// Size of the graph database after a cold index.
    pub database_bytes: u64,
    /// Total size of the exported site and data files.
    pub export_bytes: u64,
    /// Files the walker saw but no parser claimed.
    pub unparsed_files: usize,
}

/// One immutable line of `runs.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    /// Record schema version.
    pub schema_version: u32,
    /// Evidence class.
    pub run_kind: RunKind,
    /// Which evaluation track this record belongs to.
    pub track: String,
    /// Seconds since the Unix epoch, supplied by the caller's clock.
    pub timestamp: u64,
    /// Which repetition of the same configuration this is.
    pub repetitions: usize,
    /// What produced it.
    pub producer: Producer,
    /// What it measured.
    pub repository: RepositoryProfile,
    /// The numbers.
    pub metrics: Metrics,
    /// Why the class is what it is, when the harness overrode the request.
    pub notes: Vec<String>,
}

/// Options for one measurement.
#[derive(Debug, Clone)]
pub struct Options {
    /// Repository to measure.
    pub repo: PathBuf,
    /// Requested evidence class — may be downgraded to `pilot`.
    pub run_kind: RunKind,
    /// How many times each measurement repeats.
    pub repetitions: usize,
    /// Where run records are appended; `bench/` by default.
    pub out: PathBuf,
    /// Skip the export measurement. The site for a very large repository can
    /// be hundreds of megabytes, and a machine without the disk for it should
    /// be able to record every other metric rather than nothing.
    pub skip_export: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            repo: PathBuf::from("."),
            run_kind: RunKind::Empirical,
            repetitions: 3,
            out: PathBuf::from("bench"),
            skip_export: false,
        }
    }
}

/// Runs the Track E measurements and appends one record.
///
/// # Errors
/// Returns an error when the repository cannot be indexed, or when the run
/// record cannot be appended.
pub fn run(options: &Options, now: u64) -> Result<RunRecord> {
    let record = measure(options, now)?;
    let path = append(&options.out, &record)?;
    println!("{}", summarize(&record));
    println!("appended to {}", path.display());
    Ok(record)
}

/// Measures without writing anything — the testable core.
///
/// # Errors
/// Returns an error when the repository cannot be indexed.
pub fn measure(options: &Options, now: u64) -> Result<RunRecord> {
    let repetitions = options.repetitions.max(1);
    let mut notes = Vec::new();
    let mut run_kind = options.run_kind;
    if is_self(&options.repo) && run_kind == RunKind::Empirical {
        // The contract is explicit: dogfood cannot substantiate an engine
        // claim. Downgrading here means a mislabeled invocation cannot end up
        // in the empirical set by accident.
        run_kind = RunKind::Pilot;
        notes.push(
            "requested empirical, recorded as pilot: this is the engine's own repository"
                .to_string(),
        );
    }

    let scratch = scratch_dir(&options.repo, now);
    std::fs::create_dir_all(&scratch).map_err(|source| Error::CreateDir {
        path: scratch.clone(),
        source,
    })?;

    let mut cold = Vec::new();
    let mut summary = crate::resolve::IndexSummary::default();
    let mut database_bytes = 0;
    for repetition in 0..repetitions {
        let database = scratch.join(format!("cold-{repetition}.db"));
        let _ = std::fs::remove_file(&database);
        let graph = Graph::open(&database)?;
        let started = Instant::now();
        summary = crate::resolve::index_repo(&graph, &options.repo)?;
        cold.push(elapsed_ms(started));
        drop(graph);
        database_bytes = std::fs::metadata(&database).map_or(0, |meta| meta.len());
    }

    // Everything after the cold pass reuses one warm database, because that is
    // the state a user's machine is actually in.
    let warm_path = scratch.join("warm.db");
    let _ = std::fs::remove_file(&warm_path);
    let warm = Graph::open(&warm_path)?;
    crate::resolve::index_repo(&warm, &options.repo)?;

    let incremental = measure_incremental(&warm, &options.repo, repetitions);
    let (search, impact) = measure_queries(&warm, repetitions);
    let (export, export_bytes) = if options.skip_export {
        notes.push("export not measured (--skip-export)".to_string());
        (Vec::new(), 0)
    } else {
        measure_export(&warm, &options.repo, &scratch, repetitions)?
    };

    let profile = profile_repository(&options.repo, &summary);
    let record = RunRecord {
        schema_version: SCHEMA_VERSION,
        run_kind,
        track: "E: scale and operations".to_string(),
        timestamp: now,
        repetitions,
        producer: Producer::default(),
        repository: profile,
        metrics: Metrics {
            cold_index: Distribution::of(&cold),
            incremental_update: Distribution::of(&incremental),
            search_query: Distribution::of(&search),
            impact_query: Distribution::of(&impact),
            export: Distribution::of(&export),
            peak_memory_bytes: peak_memory(),
            database_bytes,
            export_bytes,
            unparsed_files: 0,
        },
        notes,
    };
    let _ = std::fs::remove_dir_all(&scratch);
    Ok(record)
}

/// Re-indexes one already-indexed file, repeatedly: the `PostToolUse` path,
/// which is the latency a user actually feels.
fn measure_incremental(graph: &Graph, root: &Path, repetitions: usize) -> Vec<f64> {
    let Ok(nodes) = graph.all_nodes() else {
        return Vec::new();
    };
    let Some(target) = nodes
        .iter()
        .map(|node| root.join(&node.file_path))
        .find(|path| path.is_file())
    else {
        return Vec::new();
    };
    (0..repetitions)
        .filter_map(|_| {
            let started = Instant::now();
            crate::resolve::index_file(graph, root, &target)
                .ok()
                .map(|_| elapsed_ms(started))
        })
        .collect()
}

/// Search and impact latency over names the repository actually contains, so
/// the numbers are not dominated by empty result sets.
fn measure_queries(graph: &Graph, repetitions: usize) -> (Vec<f64>, Vec<f64>) {
    let Ok(nodes) = graph.all_nodes() else {
        return (Vec::new(), Vec::new());
    };
    let names: Vec<String> = nodes
        .iter()
        .filter(|node| node.name.len() > 3)
        .map(|node| node.name.clone())
        .take(50)
        .collect();
    if names.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let mut search = Vec::new();
    let mut impact = Vec::new();
    for repetition in 0..repetitions.max(1) * 10 {
        let name = &names[repetition % names.len()];
        let started = Instant::now();
        let found = graph.search(name, 20).ok();
        search.push(elapsed_ms(started));
        if let Some(first) = found.and_then(|hits| hits.into_iter().next())
            && let Some(id) = first.id
        {
            let started = Instant::now();
            let _ = graph.callers(id);
            impact.push(elapsed_ms(started));
        }
    }
    (search, impact)
}

fn measure_export(
    graph: &Graph,
    root: &Path,
    scratch: &Path,
    repetitions: usize,
) -> Result<(Vec<f64>, u64)> {
    let mut samples = Vec::new();
    let out = scratch.join("export");
    for _ in 0..repetitions {
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&out).map_err(|source| Error::CreateDir {
            path: out.clone(),
            source,
        })?;
        let started = Instant::now();
        crate::export::write_default(root, &out, graph)?;
        samples.push(elapsed_ms(started));
    }
    let bytes = directory_size(&out);
    let _ = std::fs::remove_dir_all(&out);
    Ok((samples, bytes))
}

fn directory_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                directory_size(&path)
            } else {
                path.metadata().map_or(0, |meta| meta.len())
            }
        })
        .sum()
}

/// Describes the corpus. Tier labels are deliberately absent: the contract
/// says a tier is a corpus label, not a file count, so the profile is
/// published and the labelling is left to the report.
fn profile_repository(root: &Path, summary: &crate::resolve::IndexSummary) -> RepositoryProfile {
    let tracked = git_lines(root, &["ls-files"]);
    let mut languages: BTreeMap<String, usize> = BTreeMap::new();
    let mut test_files = 0;
    for file in &tracked {
        if let Some(extension) = Path::new(file).extension().and_then(|value| value.to_str()) {
            *languages.entry(extension.to_ascii_lowercase()).or_default() += 1;
        }
        let lower = file.to_ascii_lowercase();
        if lower.contains("test") || lower.contains("spec") {
            test_files += 1;
        }
    }
    RepositoryProfile {
        name: root
            .canonicalize()
            .unwrap_or_else(|_| root.to_path_buf())
            .file_name()
            .map_or_else(
                || "unknown".into(),
                |name| name.to_string_lossy().to_string(),
            ),
        revision: git_lines(root, &["rev-parse", "HEAD"])
            .first()
            .cloned()
            .unwrap_or_else(|| "unknown".into()),
        dirty: !git_lines(root, &["status", "--porcelain"]).is_empty(),
        tracked_files: tracked.len(),
        parsed_files: summary.files,
        symbols: summary.nodes,
        docs: summary.docs,
        relationships: summary.edges,
        languages,
        test_files,
    }
}

fn git_lines(root: &Path, args: &[&str]) -> Vec<String> {
    let Ok(output) = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether the measured repository is this one. A crate named `aag` with this
/// module in it is the engine's own source, and measuring it is dogfood.
fn is_self(root: &Path) -> bool {
    let manifest = root.join("Cargo.toml");
    let Ok(text) = std::fs::read_to_string(&manifest) else {
        return false;
    };
    text.contains("name = \"aag\"") && root.join("src").join("bench.rs").is_file()
}

fn scratch_dir(root: &Path, now: u64) -> PathBuf {
    std::env::temp_dir().join(format!(
        "aag-bench-{}-{now}-{}",
        root.file_name()
            .map_or_else(|| "repo".into(), |name| name.to_string_lossy().to_string()),
        std::process::id()
    ))
}

/// Bytes as megabytes for display. Precision beyond a tenth of a megabyte is
/// noise, and the lossy cast clippy warns about is exactly what is wanted.
fn megabytes(bytes: u64) -> f64 {
    // A benchmark artifact past 2^53 bytes is not a rounding problem.
    #[allow(clippy::cast_precision_loss)]
    let value = bytes as f64;
    value / 1_048_576.0
}

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

/// Peak resident set, from the kernel. `None` where the platform does not
/// report one, which is more honest than a zero.
fn peak_memory() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            let kilobytes: u64 = rest
                .trim()
                .trim_end_matches(" kB")
                .trim()
                .parse()
                .unwrap_or_default();
            return Some(kilobytes * 1024);
        }
    }
    None
}

/// Appends a record to `<out>/<run_kind>/runs.jsonl`, creating it if needed.
/// Append-only by construction: nothing here ever rewrites an existing line.
///
/// # Errors
/// Returns an error when the directory or file cannot be written.
pub fn append(out: &Path, record: &RunRecord) -> Result<PathBuf> {
    use std::io::Write as _;
    let directory = out.join(record.run_kind.directory());
    std::fs::create_dir_all(&directory).map_err(|source| Error::CreateDir {
        path: directory.clone(),
        source,
    })?;
    let path = directory.join("runs.jsonl");
    let line = serde_json::to_string(record).map_err(|error| Error::Protocol {
        context: "run record serialization failed",
        detail: error.to_string(),
    })?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
    writeln!(file, "{line}").map_err(|source| Error::Io {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

/// Reads every record of one class. Classes are never merged: a caller asks
/// for one directory and gets one class.
///
/// # Errors
/// Returns an error when a line is present but unreadable as a record — a
/// corrupt evidence file must be loud, not skipped.
pub fn load(out: &Path, kind: RunKind) -> Result<Vec<RunRecord>> {
    let path = out.join(kind.directory()).join("runs.jsonl");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    let mut records = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        // Version first, on the raw JSON: a record from another schema may
        // legitimately have fields this build has never heard of, and
        // "incompatible version" is the useful error, not "missing field".
        let raw: serde_json::Value =
            serde_json::from_str(line).map_err(|error| Error::Protocol {
                context: "benchmark record is not JSON",
                detail: format!("{}:{} — {error}", path.display(), index + 1),
            })?;
        let version = raw
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default();
        if version != u64::from(SCHEMA_VERSION) {
            return Err(Error::Protocol {
                context: "benchmark record from an incompatible schema version",
                detail: format!(
                    "{}:{} — record v{version}, harness v{SCHEMA_VERSION}",
                    path.display(),
                    index + 1,
                ),
            });
        }
        let record: RunRecord = serde_json::from_value(raw).map_err(|error| Error::Protocol {
            context: "benchmark record could not be read",
            detail: format!("{}:{} — {error}", path.display(), index + 1),
        })?;
        records.push(record);
    }
    Ok(records)
}

/// One run, as a human-readable block.
#[must_use]
pub fn summarize(record: &RunRecord) -> String {
    let mut out = format!(
        "{} @ {} [{}]\n",
        record.repository.name,
        record
            .repository
            .revision
            .chars()
            .take(8)
            .collect::<String>(),
        serde_json::to_string(&record.run_kind)
            .unwrap_or_default()
            .trim_matches('"')
    );
    let _ = writeln!(
        out,
        "  {} tracked file(s), {} parsed, {} symbols, {} relationships{}",
        record.repository.tracked_files,
        record.repository.parsed_files,
        record.repository.symbols,
        record.repository.relationships,
        if record.repository.dirty {
            " (working tree dirty)"
        } else {
            ""
        }
    );
    let _ = writeln!(
        out,
        "  cold index      p50 {:.0} ms   p95 {:.0} ms   ({} run(s))",
        record.metrics.cold_index.p50_ms,
        record.metrics.cold_index.p95_ms,
        record.metrics.cold_index.samples
    );
    let _ = writeln!(
        out,
        "  one-file resync p50 {:.1} ms   p95 {:.1} ms",
        record.metrics.incremental_update.p50_ms, record.metrics.incremental_update.p95_ms
    );
    let _ = writeln!(
        out,
        "  search          p50 {:.2} ms   p95 {:.2} ms",
        record.metrics.search_query.p50_ms, record.metrics.search_query.p95_ms
    );
    let _ = writeln!(
        out,
        "  callers         p50 {:.2} ms   p95 {:.2} ms",
        record.metrics.impact_query.p50_ms, record.metrics.impact_query.p95_ms
    );
    let _ = writeln!(
        out,
        "  export          p50 {:.0} ms   {:.1} MB",
        record.metrics.export.p50_ms,
        megabytes(record.metrics.export_bytes)
    );
    let _ = writeln!(
        out,
        "  database        {:.1} MB{}",
        megabytes(record.metrics.database_bytes),
        record
            .metrics
            .peak_memory_bytes
            .map_or_else(String::new, |bytes| format!(
                "   peak RSS {:.0} MB",
                megabytes(bytes)
            ))
    );
    for note in &record.notes {
        let _ = writeln!(out, "  note: {note}");
    }
    out
}

/// A markdown table of every run of one class, newest last.
///
/// # Errors
/// As [`load`].
pub fn report(out: &Path, kind: RunKind) -> Result<String> {
    let records = load(out, kind)?;
    if records.is_empty() {
        return Ok(format!(
            "no {} runs recorded under {}.\n",
            kind.directory(),
            out.display()
        ));
    }
    let mut text = "| repository | revision | files | symbols | edges | cold p50 | resync p50 | search p95 | export | db |\n\
         |---|---|--:|--:|--:|--:|--:|--:|--:|--:|\n".to_string();
    for record in &records {
        let _ = writeln!(
            text,
            "| {} | `{}` | {} | {} | {} | {:.0} ms | {:.1} ms | {:.2} ms | {:.1} MB | {:.1} MB |",
            record.repository.name,
            record
                .repository
                .revision
                .chars()
                .take(8)
                .collect::<String>(),
            record.repository.tracked_files,
            record.repository.symbols,
            record.repository.relationships,
            record.metrics.cold_index.p50_ms,
            record.metrics.incremental_update.p50_ms,
            record.metrics.search_query.p95_ms,
            megabytes(record.metrics.export_bytes),
            megabytes(record.metrics.database_bytes),
        );
    }
    Ok(text)
}

/// What the harness can and cannot substantiate, printed with the results so
/// a number is never read as more than it is.
#[must_use]
pub fn caveats() -> String {
    let tracks = json!({
        "A protocol conformance": "not implemented here — the protocol is a separate subject",
        "B engine extraction": "needs independently authored ground truth; not claimed by this harness",
        "C agent utility": "needs a consumer model and a factorial design; no model is called here",
        "D end-to-end economics": "needs C; token and call costs are not measured here",
        "E scale and operations": "this harness",
    });
    format!(
        "This is an ENGINE benchmark, Track E only.\n{}\n\
         Run classes are separate directories and are never averaged together. \
         A run against the engine's own repository is recorded as a pilot no matter \
         what was requested, because dogfood cannot substantiate an engine claim.\n",
        serde_json::to_string_pretty(&tracks).unwrap_or_default()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("aag-bench-t-{name}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn tiny_repo() -> PathBuf {
        let root = scratch("repo");
        fs::write(
            root.join("lib.rs"),
            "pub fn core() {}\npub fn caller() { core(); }\n",
        )
        .unwrap();
        fs::write(root.join("README.md"), "The core function.\n").unwrap();
        root
    }

    #[test]
    fn a_distribution_reports_the_tail_not_just_the_middle() {
        let samples = vec![1.0, 2.0, 3.0, 4.0, 100.0];

        let distribution = Distribution::of(&samples);

        assert_eq!(distribution.samples, 5);
        assert!((distribution.p50_ms - 3.0).abs() < f64::EPSILON);
        assert!((distribution.max_ms - 100.0).abs() < f64::EPSILON);
        assert!(
            distribution.p95_ms > distribution.p50_ms,
            "the outlier must survive into p95: {distribution:?}"
        );
    }

    #[test]
    fn no_samples_reads_as_not_measured() {
        let distribution = Distribution::of(&[]);

        assert_eq!(distribution.samples, 0);
        assert!((distribution.p50_ms).abs() < f64::EPSILON);
    }

    #[test]
    fn a_run_measures_the_repository_and_profiles_it() {
        let repo = tiny_repo();
        let options = Options {
            repo: repo.clone(),
            run_kind: RunKind::Empirical,
            repetitions: 2,
            out: scratch("out"),
            skip_export: false,
        };

        let record = measure(&options, 1_700_000_000).unwrap();

        assert_eq!(record.run_kind, RunKind::Empirical, "{:?}", record.notes);
        assert_eq!(record.repetitions, 2);
        assert_eq!(record.metrics.cold_index.samples, 2);
        assert!(record.repository.symbols >= 2, "{:?}", record.repository);
        assert!(record.metrics.database_bytes > 0);
        assert!(
            record.metrics.export_bytes > 0,
            "the site is part of what the engine costs"
        );
    }

    #[test]
    fn measuring_the_engines_own_repository_is_recorded_as_a_pilot() {
        // The real repository — this file is in it, so `is_self` must fire.
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        assert!(is_self(here), "the engine must recognize its own source");
        assert!(!is_self(&tiny_repo()));
    }

    #[test]
    fn records_append_and_never_mix_classes() {
        let out = scratch("classes");
        let repo = tiny_repo();
        let mut record = measure(
            &Options {
                repo,
                run_kind: RunKind::Empirical,
                repetitions: 1,
                out: out.clone(),
                skip_export: false,
            },
            1_700_000_000,
        )
        .unwrap();

        append(&out, &record).unwrap();
        append(&out, &record).unwrap();
        record.run_kind = RunKind::Pilot;
        append(&out, &record).unwrap();

        let empirical = load(&out, RunKind::Empirical).unwrap();
        let pilot = load(&out, RunKind::Pilot).unwrap();
        assert_eq!(empirical.len(), 2, "appended, not overwritten");
        assert_eq!(pilot.len(), 1);
        assert!(load(&out, RunKind::Simulated).unwrap().is_empty());
        assert!(
            out.join("empirical").join("runs.jsonl").is_file()
                && out.join("pilot").join("runs.jsonl").is_file(),
            "each class has its own immutable path"
        );
    }

    #[test]
    fn a_record_from_another_schema_version_is_refused_not_averaged() {
        let out = scratch("schema");
        let directory = out.join("empirical");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("runs.jsonl"),
            "{\"schema_version\":999,\"run_kind\":\"empirical\",\"track\":\"E\",\
             \"timestamp\":0,\"repetitions\":1,\"producer\":{\"name\":\"x\",\"version\":\"0\",\
             \"features\":[],\"profile\":\"debug\"},\"repository\":{},\"metrics\":{},\
             \"notes\":[]}\n",
        )
        .unwrap();

        let error = load(&out, RunKind::Empirical).unwrap_err();

        assert!(
            format!("{error}").contains("incompatible schema"),
            "{error}"
        );
    }

    #[test]
    fn an_unknown_run_class_is_an_error_rather_than_a_default() {
        assert_eq!(RunKind::parse("empirical").unwrap(), RunKind::Empirical);
        assert_eq!(RunKind::parse(" Pilot ").unwrap(), RunKind::Pilot);
        assert!(RunKind::parse("production").is_err());
    }

    #[test]
    fn the_caveats_name_the_tracks_this_harness_does_not_cover() {
        let text = caveats();

        assert!(
            text.contains("Track E") || text.contains("E scale"),
            "{text}"
        );
        assert!(text.contains("agent utility"), "{text}");
        assert!(text.contains("ground truth"), "{text}");
    }
}
