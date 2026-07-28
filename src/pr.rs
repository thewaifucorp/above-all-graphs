//! Graph-backed pull-request workflow — P1.11 of `docs/capability-coverage.md`.
//!
//! GitHub already says what a pull request *is*: title, branch, checks, review
//! state. What it cannot say is what the change reaches — which symbols other
//! code depends on, which tests must run, and which other open PR is about to
//! collide with this one. That is the graph's half, and it is the whole reason
//! this module exists.
//!
//! Everything that talks to GitHub is a thin `gh` call at the edges. The
//! analysis is pure functions over `(graph, changed files)`, which is what
//! makes it testable without a network and what keeps the risk model honest:
//! every number below comes from a rule stated in [`RISK_RULES`], not from a
//! feeling about a diff.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use serde_json::json;

use crate::{
    error::{Error, Result},
    storage::{Graph, Node, NodeKind},
};

/// In-degree at which a symbol counts as a hub: enough dependents that
/// changing it is a repository-wide event rather than a local one.
const HUB_DEGREE: usize = 10;

/// The rules that produce a risk score, stated so a number can be argued with.
pub const RISK_RULES: &[(&str, u32)] = &[
    ("each touched hub symbol (10+ dependents)", 3),
    ("every 25 symbols in the blast radius", 1),
    ("affected tests exist but the PR changes none", 4),
    ("required checks are failing", 3),
    ("the PR shares a community with another open PR", 2),
];

/// One pull request, as GitHub describes it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PullRequest {
    /// PR number.
    pub number: u64,
    /// PR title.
    pub title: String,
    /// Head branch.
    pub head: String,
    /// Base branch.
    pub base: String,
    /// Whether it is a draft.
    pub draft: bool,
    /// `APPROVED`, `CHANGES_REQUESTED`, `REVIEW_REQUIRED`, or empty.
    pub review: String,
    /// Rollup state of the checks: `SUCCESS`, `FAILURE`, `PENDING`, or empty.
    pub checks: String,
    /// Repository-relative paths the PR changes.
    pub files: Vec<String>,
}

/// One reason a pull request scored what it scored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Points this contributed.
    pub weight: u32,
    /// What it is, in one line.
    pub detail: String,
}

/// What the graph knows about one pull request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Insight {
    /// PR number.
    pub number: u64,
    /// PR title.
    pub title: String,
    /// Symbols declared in the changed files.
    pub touched: Vec<String>,
    /// Touched symbols with [`HUB_DEGREE`] or more dependents.
    pub hubs: Vec<String>,
    /// Communities the change lands in, by representative node id.
    pub communities: Vec<i64>,
    /// Symbols that transitively depend on something the PR changes.
    pub blast_radius: usize,
    /// Test-looking files the change reaches.
    pub affected_tests: Vec<String>,
    /// Total risk score.
    pub risk: u32,
    /// Why, highest-weight first.
    pub findings: Vec<Finding>,
}

impl Insight {
    /// The one word a queue is sorted by.
    #[must_use]
    pub const fn band(&self) -> &'static str {
        match self.risk {
            0..=3 => "low",
            4..=9 => "medium",
            _ => "high",
        }
    }
}

/// Two open pull requests that are about to meet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overlap {
    /// The lower PR number.
    pub left: u64,
    /// The higher PR number.
    pub right: u64,
    /// Files both change — a textual conflict waiting to happen.
    pub shared_files: Vec<String>,
    /// Symbols both reach through the graph, without sharing a file.
    pub shared_symbols: Vec<String>,
    /// Communities both land in.
    pub shared_communities: Vec<i64>,
}

impl Overlap {
    /// How likely these two are to fight, from what they share.
    #[must_use]
    pub const fn severity(&self) -> &'static str {
        if !self.shared_files.is_empty() {
            "conflict"
        } else if !self.shared_symbols.is_empty() {
            "semantic"
        } else {
            "adjacent"
        }
    }
}

/// List open pull requests through the authenticated GitHub CLI.
///
/// # Errors
/// Returns an error when `gh` is unavailable, unauthenticated, or the request fails.
pub fn list(root: &Path, base: &str) -> Result<String> {
    let mut args = vec![
        "pr",
        "list",
        "--limit",
        "100",
        "--json",
        "number,title,headRefName,baseRefName,isDraft,reviewDecision,statusCheckRollup,updatedAt",
    ];
    if !base.trim().is_empty() {
        args.extend(["--base", base.trim()]);
    }
    gh(root, &args)
}

/// Return changed files, communities, symbols, and affected tests for one PR.
///
/// # Errors
/// Returns an error when GitHub or the local graph cannot be queried.
pub fn impact(root: &Path, number: &str) -> Result<String> {
    let files = changed_files(root, number.trim())?;
    impact_files(root, number, &files)
}

/// List non-draft PRs as the actionable triage set.
///
/// # Errors
/// Returns an error when GitHub cannot be queried or its response is malformed.
pub fn triage(root: &Path, base: &str) -> Result<String> {
    let raw = list(root, base)?;
    let mut prs: Vec<serde_json::Value> =
        serde_json::from_str(&raw).map_err(|error| Error::Protocol {
            context: "GitHub PR response parse failed",
            detail: error.to_string(),
        })?;
    prs.retain(|pr| {
        !pr.get("isDraft")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    });
    serde_json::to_string_pretty(&prs).map_err(|error| Error::Protocol {
        context: "GitHub PR response serialization failed",
        detail: error.to_string(),
    })
}

/// Every open pull request with its changed files, ready for analysis.
///
/// # Errors
/// Returns an error when GitHub cannot be queried or answers with something
/// other than the requested JSON.
pub fn open_pull_requests(root: &Path, base: &str) -> Result<Vec<PullRequest>> {
    let raw = list(root, base)?;
    let listed: Vec<serde_json::Value> =
        serde_json::from_str(&raw).map_err(|error| Error::Protocol {
            context: "GitHub PR response parse failed",
            detail: error.to_string(),
        })?;
    let mut out = Vec::new();
    for entry in listed {
        let number = entry
            .get("number")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default();
        let files = changed_files(root, &number.to_string()).unwrap_or_default();
        out.push(PullRequest {
            number,
            title: string_field(&entry, "title"),
            head: string_field(&entry, "headRefName"),
            base: string_field(&entry, "baseRefName"),
            draft: entry
                .get("isDraft")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            review: string_field(&entry, "reviewDecision"),
            checks: rollup_state(&entry),
            files,
        });
    }
    Ok(out)
}

fn string_field(entry: &serde_json::Value, key: &str) -> String {
    entry
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The worst state among the PR's checks, because one failure is the answer
/// even when everything else is green.
fn rollup_state(entry: &serde_json::Value) -> String {
    let Some(checks) = entry
        .get("statusCheckRollup")
        .and_then(serde_json::Value::as_array)
    else {
        return String::new();
    };
    let mut state = String::new();
    for check in checks {
        let value = check
            .get("conclusion")
            .or_else(|| check.get("state"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_ascii_uppercase();
        if value == "FAILURE" || value == "TIMED_OUT" || value == "CANCELLED" {
            return "FAILURE".to_string();
        }
        if value == "PENDING" || value.is_empty() {
            state = "PENDING".to_string();
        } else if state.is_empty() {
            state = "SUCCESS".to_string();
        }
    }
    state
}

fn changed_files(root: &Path, number: &str) -> Result<Vec<String>> {
    let output = gh(root, &["pr", "diff", number, "--name-only"])?;
    Ok(output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

/// The graph facts one repository holds, loaded once for a whole dashboard.
struct Repository {
    nodes: Vec<Node>,
    /// Node id to the ids that depend on it.
    dependents: BTreeMap<i64, Vec<i64>>,
    /// Node id to community id.
    community_of: BTreeMap<i64, i64>,
}

impl Repository {
    fn load(root: &Path) -> Result<Self> {
        let graph = Graph::open_existing(root)?;
        let nodes = graph.all_nodes()?;
        let edges = graph.all_edges()?;
        let mut dependents: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
        for edge in &edges {
            dependents.entry(edge.dst).or_default().push(edge.src);
        }
        let mut community_of = BTreeMap::new();
        for community in crate::analysis::communities(&nodes, &edges) {
            for member in community.members {
                community_of.insert(member, community.id);
            }
        }
        Ok(Self {
            nodes,
            dependents,
            community_of,
        })
    }

    /// Symbol nodes declared in any of `files`.
    fn symbols_in(&self, files: &BTreeSet<&str>) -> Vec<&Node> {
        self.nodes
            .iter()
            .filter(|node| !matches!(node.kind, NodeKind::File | NodeKind::Doc))
            .filter(|node| files.contains(node.file_path.as_str()))
            .collect()
    }

    /// Everything that transitively depends on `seeds`.
    fn reachable_dependents(&self, seeds: &[i64]) -> BTreeSet<i64> {
        let mut seen: BTreeSet<i64> = BTreeSet::new();
        let mut stack: Vec<i64> = seeds.to_vec();
        while let Some(current) = stack.pop() {
            for &dependent in self.dependents.get(&current).into_iter().flatten() {
                if seen.insert(dependent) {
                    stack.push(dependent);
                }
            }
        }
        for seed in seeds {
            seen.remove(seed);
        }
        seen
    }
}

/// What the graph knows about one pull request's changed files.
fn analyze(repository: &Repository, root: &Path, request: &PullRequest) -> Result<Insight> {
    let files: BTreeSet<&str> = request.files.iter().map(String::as_str).collect();
    let touched_nodes = repository.symbols_in(&files);
    let ids: Vec<i64> = touched_nodes.iter().filter_map(|node| node.id).collect();
    let hubs: Vec<String> = touched_nodes
        .iter()
        .filter(|node| {
            node.id.is_some_and(|id| {
                repository.dependents.get(&id).map_or(0, std::vec::Vec::len) >= HUB_DEGREE
            })
        })
        .map(|node| format!("{}:{}", node.file_path, node.name))
        .collect();
    let communities: Vec<i64> = ids
        .iter()
        .filter_map(|id| repository.community_of.get(id).copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let blast_radius = repository.reachable_dependents(&ids).len();
    let affected_tests = crate::refactor::affected(root, &request.files)?;

    let mut findings = Vec::new();
    if !hubs.is_empty() {
        findings.push(Finding {
            weight: 3_u32.saturating_mul(u32::try_from(hubs.len()).unwrap_or(u32::MAX)),
            detail: format!(
                "touches {} hub symbol(s) with {HUB_DEGREE}+ dependents: {}",
                hubs.len(),
                hubs.join(", ")
            ),
        });
    }
    let radius_points = u32::try_from(blast_radius / 25).unwrap_or(u32::MAX);
    if radius_points > 0 {
        findings.push(Finding {
            weight: radius_points,
            detail: format!("{blast_radius} symbols transitively depend on this change"),
        });
    }
    let changes_a_test = request
        .files
        .iter()
        .any(|file| crate::refactor::looks_like_test_file(file));
    if !affected_tests.is_empty() && !changes_a_test {
        findings.push(Finding {
            weight: 4,
            detail: format!(
                "{} affected test file(s) and none of them changed: {}",
                affected_tests.len(),
                affected_tests.join(", ")
            ),
        });
    }
    if request.checks == "FAILURE" {
        findings.push(Finding {
            weight: 3,
            detail: "required checks are failing".to_string(),
        });
    }
    findings.sort_by_key(|finding| std::cmp::Reverse(finding.weight));
    let risk = findings.iter().map(|finding| finding.weight).sum();

    Ok(Insight {
        number: request.number,
        title: request.title.clone(),
        touched: touched_nodes
            .iter()
            .map(|node| format!("{}:{}", node.file_path, node.name))
            .collect(),
        hubs,
        communities,
        blast_radius,
        affected_tests,
        risk,
        findings,
    })
}

/// Pairs of pull requests that share files, symbols, or a community.
///
/// Sharing a file is a textual conflict on the way. Sharing a *symbol* without
/// sharing a file is the one a diff cannot show you: two branches editing
/// different files that both call the same function will merge cleanly and
/// still break. Sharing only a community is neither — it is a note that two
/// people are working in one area.
#[must_use]
pub fn overlaps(insights: &[(PullRequest, Insight)]) -> Vec<Overlap> {
    let mut out = Vec::new();
    for (position, (left_pr, left)) in insights.iter().enumerate() {
        for (right_pr, right) in insights.iter().skip(position + 1) {
            let left_files: BTreeSet<&str> = left_pr.files.iter().map(String::as_str).collect();
            let right_files: BTreeSet<&str> = right_pr.files.iter().map(String::as_str).collect();
            let shared_files: Vec<String> = left_files
                .intersection(&right_files)
                .map(|file| (*file).to_string())
                .collect();
            let left_symbols: BTreeSet<&str> = left.touched.iter().map(String::as_str).collect();
            let right_symbols: BTreeSet<&str> = right.touched.iter().map(String::as_str).collect();
            let shared_symbols: Vec<String> = left_symbols
                .intersection(&right_symbols)
                .map(|symbol| (*symbol).to_string())
                .filter(|symbol| {
                    // A symbol inside a shared file is already reported as a
                    // file conflict; repeating it says nothing new.
                    symbol
                        .split_once(':')
                        .is_none_or(|(file, _)| !shared_files.iter().any(|shared| shared == file))
                })
                .collect();
            let left_communities: BTreeSet<i64> = left.communities.iter().copied().collect();
            let right_communities: BTreeSet<i64> = right.communities.iter().copied().collect();
            let shared_communities: Vec<i64> = left_communities
                .intersection(&right_communities)
                .copied()
                .collect();
            if shared_files.is_empty() && shared_symbols.is_empty() && shared_communities.is_empty()
            {
                continue;
            }
            out.push(Overlap {
                left: left_pr.number.min(right_pr.number),
                right: left_pr.number.max(right_pr.number),
                shared_files,
                shared_symbols,
                shared_communities,
            });
        }
    }
    out.sort_by(|left, right| {
        severity_rank(right)
            .cmp(&severity_rank(left))
            .then(left.left.cmp(&right.left))
    });
    out
}

const fn severity_rank(overlap: &Overlap) -> u8 {
    match overlap.severity().as_bytes() {
        b"conflict" => 2,
        b"semantic" => 1,
        _ => 0,
    }
}

/// Every open pull request with what the graph says about it, plus the
/// overlaps between them.
pub type Queue = (Vec<(PullRequest, Insight)>, Vec<Overlap>);

/// Analyzes every open pull request, adding the community-overlap penalty that
/// only exists once the others are known.
///
/// # Errors
/// As [`open_pull_requests`], plus a storage error when the graph cannot be
/// read.
pub fn review_queue(root: &Path, base: &str) -> Result<Queue> {
    let repository = Repository::load(root)?;
    let requests = open_pull_requests(root, base)?;
    let mut analyzed = Vec::new();
    for request in requests {
        let insight = analyze(&repository, root, &request)?;
        analyzed.push((request, insight));
    }
    let overlaps = overlaps(&analyzed);
    for (request, insight) in &mut analyzed {
        let shares = overlaps
            .iter()
            .filter(|overlap| overlap.left == request.number || overlap.right == request.number)
            .count();
        if shares > 0 {
            insight.findings.push(Finding {
                weight: 2,
                detail: format!("overlaps {shares} other open pull request(s)"),
            });
            insight.risk = insight.risk.saturating_add(2);
            insight
                .findings
                .sort_by_key(|finding| std::cmp::Reverse(finding.weight));
        }
    }
    // Highest risk first; a draft sinks below ready work at equal risk.
    analyzed.sort_by(|left, right| {
        left.0
            .draft
            .cmp(&right.0.draft)
            .then(right.1.risk.cmp(&left.1.risk))
            .then(left.0.number.cmp(&right.0.number))
    });
    Ok((analyzed, overlaps))
}

/// The dashboard: every open pull request, ranked, with what the graph says.
///
/// # Errors
/// As [`review_queue`].
pub fn dashboard(root: &Path, base: &str) -> Result<String> {
    let (analyzed, overlaps) = review_queue(root, base)?;
    if analyzed.is_empty() {
        return Ok("no open pull requests.".to_string());
    }
    let mut out = format!(
        "{} open pull request(s), highest risk first. Risk is a stated rule set, \
         not a judgement:\n",
        analyzed.len()
    );
    for (rule, points) in RISK_RULES {
        let _ = write!(out, "\n  +{points}  {rule}");
    }
    out.push_str("\n\n");
    for (request, insight) in &analyzed {
        let _ = write!(
            out,
            "#{} [{}, risk {}] {}\n     {} → {}{}{}\n",
            request.number,
            insight.band(),
            insight.risk,
            request.title,
            request.head,
            request.base,
            if request.draft { " (draft)" } else { "" },
            match request.checks.as_str() {
                "FAILURE" => " · checks failing",
                "PENDING" => " · checks pending",
                _ => "",
            }
        );
        let _ = writeln!(
            out,
            "     {} file(s), {} symbol(s), blast radius {}",
            request.files.len(),
            insight.touched.len(),
            insight.blast_radius
        );
        for finding in &insight.findings {
            let _ = writeln!(out, "     +{}  {}", finding.weight, finding.detail);
        }
        out.push('\n');
    }
    if overlaps.is_empty() {
        out.push_str("no two open pull requests share a file, a symbol, or a community.\n");
    } else {
        out.push_str("overlaps:\n");
        for overlap in &overlaps {
            let _ = write!(out, "  #{} × #{}", overlap.left, overlap.right);
            let _ = write!(out, " [{}]", overlap.severity());
            if !overlap.shared_files.is_empty() {
                let _ = write!(out, " files: {}", overlap.shared_files.join(", "));
            }
            if !overlap.shared_symbols.is_empty() {
                let _ = write!(
                    out,
                    " symbols both reach: {}",
                    overlap.shared_symbols.join(", ")
                );
            }
            if overlap.shared_files.is_empty() && overlap.shared_symbols.is_empty() {
                let _ = write!(
                    out,
                    " same area ({} community)",
                    overlap.shared_communities.len()
                );
            }
            out.push('\n');
        }
    }
    Ok(out)
}

/// Conflicts only, for a pre-merge check.
///
/// # Errors
/// As [`review_queue`].
pub fn conflicts(root: &Path, base: &str) -> Result<String> {
    let (_, overlaps) = review_queue(root, base)?;
    let real: Vec<&Overlap> = overlaps
        .iter()
        .filter(|overlap| overlap.severity() != "adjacent")
        .collect();
    if real.is_empty() {
        return Ok(
            "no open pull requests share a file or a symbol. Sharing a community \
                   is proximity, not a conflict, and is listed by `aag pr dashboard`."
                .to_string(),
        );
    }
    let mut out = String::new();
    for overlap in real {
        let _ = writeln!(
            out,
            "#{} × #{} [{}]",
            overlap.left,
            overlap.right,
            overlap.severity()
        );
        for file in &overlap.shared_files {
            let _ = writeln!(out, "     same file: {file}");
        }
        for symbol in &overlap.shared_symbols {
            let _ = writeln!(out, "     both reach: {symbol}");
        }
    }
    out.push_str(
        "\nA shared file is a merge conflict on the way. A shared symbol with no shared \
         file is the one a diff cannot show you: both branches merge cleanly and still \
         disagree.",
    );
    Ok(out)
}

/// Local git worktrees, mapped to the pull request on each branch.
///
/// # Errors
/// Returns an error when `git` cannot be run. A worktree with no open PR is
/// reported as such rather than omitted.
pub fn worktrees(root: &Path, base: &str) -> Result<String> {
    let listed = git(root, &["worktree", "list", "--porcelain"])?;
    let mut trees: Vec<(String, String)> = Vec::new();
    let mut path = String::new();
    for line in listed.lines() {
        if let Some(value) = line.strip_prefix("worktree ") {
            path = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("branch ") {
            // `refs/heads/feat/x` is the branch `feat/x`: strip the ref
            // namespace, not the first slash — `gh` reports the full name.
            let branch = value
                .trim()
                .strip_prefix("refs/heads/")
                .unwrap_or(value.trim())
                .to_string();
            trees.push((path.clone(), branch));
        } else if line.trim() == "detached" {
            trees.push((path.clone(), "(detached)".to_string()));
        }
    }
    if trees.is_empty() {
        return Ok("no git worktrees.".to_string());
    }
    let requests = open_pull_requests(root, base).unwrap_or_default();
    let mut out = format!("{} worktree(s):\n", trees.len());
    for (path, branch) in &trees {
        let matched = requests.iter().find(|request| &request.head == branch);
        let _ = match matched {
            Some(request) => write!(
                out,
                "  {path}\n     {branch} → #{} {}\n",
                request.number, request.title
            ),
            None => write!(out, "  {path}\n     {branch} → no open pull request\n"),
        };
    }
    Ok(out)
}

fn impact_files(root: &Path, number: &str, files: &[String]) -> Result<String> {
    let repository = Repository::load(root)?;
    let request = PullRequest {
        number: number.trim().parse().unwrap_or_default(),
        files: files.to_vec(),
        ..PullRequest::default()
    };
    let insight = analyze(&repository, root, &request)?;
    serde_json::to_string_pretty(&json!({
        "pr": number,
        "changed_files": files,
        "touched_nodes": insight.touched.len(),
        "communities": insight.communities,
        "blast_radius": insight.blast_radius,
        "hubs": insight.hubs,
        "risk": insight.risk,
        "band": insight.band(),
        "findings": insight.findings.iter().map(|finding| json!({
            "weight": finding.weight,
            "detail": finding.detail,
        })).collect::<Vec<_>>(),
        "affected_tests": insight.affected_tests
    }))
    .map_err(|error| Error::Protocol {
        context: "PR impact serialization failed",
        detail: error.to_string(),
    })
}

fn gh(root: &Path, args: &[&str]) -> Result<String> {
    run(root, "gh", args, "GitHub CLI")
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A repository where `core` is a hub, `edge` is not, and a test file
    /// covers the hub's file.
    fn indexed_root() -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("aag-pr-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("core.rs"),
            "pub fn core() {}\npub fn helper() { core(); }\n",
        )
        .unwrap();
        let mut callers = String::new();
        for index in 0..12 {
            let _ = write!(
                callers,
                "use crate::core;\npub fn caller{index}() {{ core(); }}\n"
            );
        }
        std::fs::write(root.join("callers.rs"), callers).unwrap();
        std::fs::write(root.join("edge.rs"), "pub fn edge() {}\n").unwrap();
        std::fs::write(
            root.join("core_test.rs"),
            "use crate::core;\n#[test]\nfn covers() { core(); }\n",
        )
        .unwrap();
        crate::bigbang::run(
            &root,
            &crate::bigbang::Options {
                no_viz: true,
                no_install: true,
                ..Default::default()
            },
        )
        .unwrap();
        root
    }

    fn request(number: u64, files: &[&str]) -> PullRequest {
        PullRequest {
            number,
            title: format!("pull request {number}"),
            head: format!("feat/{number}"),
            base: "main".to_string(),
            files: files.iter().map(|file| (*file).to_string()).collect(),
            ..PullRequest::default()
        }
    }

    #[test]
    fn a_hub_and_a_leaf_do_not_score_the_same() {
        let root = indexed_root();
        let repository = Repository::load(&root).unwrap();

        let hub = analyze(&repository, &root, &request(1, &["core.rs"])).unwrap();
        let leaf = analyze(&repository, &root, &request(2, &["edge.rs"])).unwrap();

        assert!(
            hub.hubs.iter().any(|name| name.ends_with(":core")),
            "`core` has 12 dependents: {:?}",
            hub.hubs
        );
        assert!(hub.blast_radius > leaf.blast_radius, "{hub:?} vs {leaf:?}");
        assert!(
            hub.risk > leaf.risk,
            "risk follows the graph, not the diff size: {} vs {}",
            hub.risk,
            leaf.risk
        );
        assert!(
            hub.findings
                .iter()
                .any(|finding| finding.detail.contains("hub symbol")),
            "and it says why: {:?}",
            hub.findings
        );
    }

    #[test]
    fn a_change_with_uncovered_tests_is_flagged_and_one_that_touches_them_is_not() {
        let root = indexed_root();
        let repository = Repository::load(&root).unwrap();

        let untested = analyze(&repository, &root, &request(1, &["core.rs"])).unwrap();
        let tested = analyze(
            &repository,
            &root,
            &request(2, &["core.rs", "core_test.rs"]),
        )
        .unwrap();

        assert!(
            untested
                .findings
                .iter()
                .any(|finding| finding.detail.contains("none of them changed")),
            "{:?}",
            untested.findings
        );
        assert!(
            !tested
                .findings
                .iter()
                .any(|finding| finding.detail.contains("none of them changed")),
            "the PR changed the test that covers it: {:?}",
            tested.findings
        );
    }

    #[test]
    fn two_pull_requests_that_never_share_a_file_can_still_collide() {
        let root = indexed_root();
        let repository = Repository::load(&root).unwrap();
        let left = request(1, &["core.rs"]);
        let right = request(2, &["core.rs", "edge.rs"]);
        let analyzed = vec![
            (left.clone(), analyze(&repository, &root, &left).unwrap()),
            (right.clone(), analyze(&repository, &root, &right).unwrap()),
        ];

        let found = overlaps(&analyzed);

        let overlap = found.first().expect("an overlap");
        assert_eq!((overlap.left, overlap.right), (1, 2));
        assert_eq!(overlap.severity(), "conflict", "{overlap:?}");
        assert!(overlap.shared_files.contains(&"core.rs".to_string()));
    }

    #[test]
    fn a_shared_community_alone_is_proximity_and_says_so() {
        let root = indexed_root();
        let repository = Repository::load(&root).unwrap();
        let left = request(1, &["core.rs"]);
        let right = request(2, &["callers.rs"]);
        let analyzed = vec![
            (left.clone(), analyze(&repository, &root, &left).unwrap()),
            (right.clone(), analyze(&repository, &root, &right).unwrap()),
        ];

        let found = overlaps(&analyzed);

        for overlap in &found {
            assert!(
                overlap.shared_files.is_empty(),
                "these two change different files: {overlap:?}"
            );
            assert_ne!(overlap.severity(), "conflict");
        }
    }

    #[test]
    fn the_json_impact_carries_the_score_and_its_reasons() {
        let root = indexed_root();

        let report = impact_files(&root, "7", &["core.rs".into()]).unwrap();

        assert!(report.contains("\"risk\""), "{report}");
        assert!(report.contains("\"band\""), "{report}");
        assert!(report.contains("hub symbol"), "{report}");
        assert!(report.contains("\"blast_radius\""), "{report}");
    }

    #[test]
    fn a_failing_check_is_worth_what_the_rules_say_it_is() {
        let root = indexed_root();
        let repository = Repository::load(&root).unwrap();
        let mut failing = request(1, &["edge.rs"]);
        failing.checks = "FAILURE".to_string();

        let clean = analyze(&repository, &root, &request(2, &["edge.rs"])).unwrap();
        let broken = analyze(&repository, &root, &failing).unwrap();

        assert_eq!(broken.risk, clean.risk + 3, "the rule table is the model");
    }

    #[test]
    fn the_check_rollup_takes_the_worst_state() {
        let entry = json!({"statusCheckRollup": [
            {"conclusion": "SUCCESS"},
            {"conclusion": "FAILURE"},
            {"conclusion": "SUCCESS"},
        ]});
        assert_eq!(rollup_state(&entry), "FAILURE");

        let pending =
            json!({"statusCheckRollup": [{"conclusion": "SUCCESS"}, {"state": "PENDING"}]});
        assert_eq!(rollup_state(&pending), "PENDING");

        assert_eq!(rollup_state(&json!({})), "", "no checks is not a failure");
    }
}
