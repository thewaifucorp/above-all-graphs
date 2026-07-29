//! Repository-area skills, generated from the graph — P1.15 of
//! `docs/capability-coverage.md`.
//!
//! The seven skills in `assets/skills/` teach an agent how to *use* `aag`.
//! They are the same in every repository, because they have to be. What they
//! cannot say is what *this* repository is made of: which areas exist, what
//! runs first in each, which symbols the rest of the code leans on, and which
//! areas are coupled to which.
//!
//! That is derivable — [`crate::analysis`] already finds communities,
//! entrypoints, and processes, and the graph already holds endpoints and
//! cross-area edges. This module turns those into one `SKILL.md` per area, so
//! an agent that has never seen the repository starts oriented instead of
//! grepping for a map.
//!
//! Generation is deterministic: same graph in, byte-identical files out. There
//! are no timestamps, no counts that jitter with hash order, and every list is
//! sorted. That is what makes refresh safe to run on every `bigbang` — a file
//! is rewritten only when the graph actually changed it.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use crate::analysis;
use crate::error::Result;
use crate::storage::{Edge, Graph, Node, NodeKind};

/// Prefix for every generated skill directory. `uninstall` removes exactly
/// this set, and it must not collide with the static `aag-*` pack.
pub const AREA_PREFIX: &str = "aag-area-";

/// Smallest community worth a skill of its own. Below this, an "area" is one
/// file and a page describing it is noise.
const MIN_SYMBOLS: usize = 8;

/// Most areas to generate. A repository split into fifty communities does not
/// want fifty skills competing for the agent's attention; the largest ones are
/// where the code actually is.
const MAX_AREAS: usize = 12;

/// How many symbols, files, and links each section lists.
const LIST_LIMIT: usize = 12;

/// One detected area of the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Area {
    /// Directory-derived slug, e.g. `src-mcp`.
    pub slug: String,
    /// Human-facing name: the shared directory, or the dominant file.
    pub name: String,
    /// Files in the area, sorted.
    pub files: Vec<String>,
    /// Symbols with the most dependents, highest first.
    pub hubs: Vec<(String, usize)>,
    /// Entrypoints declared in the area, sorted.
    pub entrypoints: Vec<String>,
    /// HTTP endpoints the area declares or implements, sorted.
    pub endpoints: Vec<String>,
    /// Call chains rooted at an entrypoint in this area, longest first.
    pub processes: Vec<(String, usize)>,
    /// Other areas this one is coupled to, by edge count, strongest first.
    pub links: Vec<(String, usize)>,
    /// Total symbols in the area.
    pub symbols: usize,
}

/// Detects the repository's areas from the graph.
///
/// # Errors
/// Returns a storage error when the graph cannot be read.
pub fn detect(graph: &Graph) -> Result<Vec<Area>> {
    let nodes = graph.all_nodes()?;
    let edges = graph.all_edges()?;
    Ok(from_graph(&nodes, &edges))
}

/// The pure half of [`detect`] — everything below operates on data, so the
/// tests can build a repository shape without a database.
#[must_use]
pub fn from_graph(nodes: &[Node], edges: &[Edge]) -> Vec<Area> {
    let by_id: BTreeMap<i64, &Node> = nodes
        .iter()
        .filter_map(|node| node.id.map(|id| (id, node)))
        .collect();
    let mut dependents: BTreeMap<i64, usize> = BTreeMap::new();
    for edge in edges {
        *dependents.entry(edge.dst).or_default() += 1;
    }
    let entrypoints = analysis::entrypoints(nodes, edges);
    let processes = analysis::processes(nodes, edges);

    // Community id to the symbols in it, then the same map keyed by node so
    // cross-area edges can be counted in one pass.
    let communities = analysis::communities(nodes, edges);
    let mut area_of: BTreeMap<i64, i64> = BTreeMap::new();
    for community in &communities {
        for &member in &community.members {
            area_of.insert(member, community.id);
        }
    }

    let mut cross: BTreeMap<(i64, i64), usize> = BTreeMap::new();
    for edge in edges {
        let (Some(&left), Some(&right)) = (area_of.get(&edge.src), area_of.get(&edge.dst)) else {
            continue;
        };
        if left != right {
            cross.entry((left, right)).or_default().add_one();
            cross.entry((right, left)).or_default().add_one();
        }
    }

    let mut named: BTreeMap<i64, String> = BTreeMap::new();
    let mut candidates: Vec<(i64, Vec<i64>)> = Vec::new();
    for community in &communities {
        let symbols: Vec<i64> = community
            .members
            .iter()
            .copied()
            .filter(|id| {
                by_id.get(id).is_some_and(|node| {
                    !matches!(node.kind, NodeKind::File | NodeKind::Doc)
                        && !is_vendored(&node.file_path)
                })
            })
            .collect();
        if symbols.len() < MIN_SYMBOLS {
            continue;
        }
        named.insert(community.id, area_name(&symbols, &by_id));
        candidates.push((community.id, symbols));
    }
    disambiguate(&mut named, &candidates, &by_id);
    // Biggest areas first, id as the tie-break so the set never depends on
    // iteration order.
    candidates.sort_by(|left, right| right.1.len().cmp(&left.1.len()).then(left.0.cmp(&right.0)));
    candidates.truncate(MAX_AREAS);

    let mut areas: Vec<Area> = candidates
        .iter()
        .map(|(id, symbols)| {
            build_area(
                *id,
                symbols,
                &by_id,
                &dependents,
                &entrypoints,
                &processes,
                &cross,
                &named,
            )
        })
        .collect();
    areas.sort_by(|left, right| left.slug.cmp(&right.slug));
    areas
}

/// Tiny helper so the cross-area counting reads as one statement.
trait AddOne {
    fn add_one(&mut self);
}

impl AddOne for usize {
    fn add_one(&mut self) {
        *self += 1;
    }
}

#[allow(clippy::too_many_arguments)]
fn build_area(
    id: i64,
    symbols: &[i64],
    by_id: &BTreeMap<i64, &Node>,
    dependents: &BTreeMap<i64, usize>,
    entrypoints: &BTreeSet<i64>,
    processes: &[analysis::Process],
    cross: &BTreeMap<(i64, i64), usize>,
    named: &BTreeMap<i64, String>,
) -> Area {
    let name = named.get(&id).cloned().unwrap_or_else(|| "area".into());
    let members: BTreeSet<i64> = symbols.iter().copied().collect();

    let files: Vec<String> = symbols
        .iter()
        .filter_map(|id| by_id.get(id).map(|node| node.file_path.clone()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let mut hubs: Vec<(String, usize)> = symbols
        .iter()
        .filter_map(|id| {
            let node = by_id.get(id)?;
            let count = dependents.get(id).copied().unwrap_or_default();
            (count > 0).then(|| (format!("{}:{}", node.file_path, node.name), count))
        })
        .collect();
    hubs.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    hubs.truncate(LIST_LIMIT);

    let entry_names: Vec<String> = symbols
        .iter()
        .filter(|id| entrypoints.contains(id))
        .filter_map(|id| {
            by_id
                .get(id)
                .map(|node| format!("{}:{}", node.file_path, node.name))
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(LIST_LIMIT)
        .collect();

    let endpoints: Vec<String> = symbols
        .iter()
        .filter_map(|id| by_id.get(id))
        .filter(|node| matches!(node.kind, NodeKind::Endpoint))
        .map(|node| node.name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(LIST_LIMIT)
        .collect();

    let mut chains: Vec<(String, usize)> = processes
        .iter()
        .filter(|process| members.contains(&process.entrypoint))
        .filter_map(|process| {
            let node = by_id.get(&process.entrypoint)?;
            Some((
                format!("{}:{}", node.file_path, node.name),
                process.steps.len(),
            ))
        })
        .collect();
    chains.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    chains.truncate(LIST_LIMIT);

    let mut totals: BTreeMap<String, usize> = BTreeMap::new();
    for (other, other_name) in named.iter().filter(|(other, _)| **other != id) {
        if let Some(count) = cross.get(&(id, *other)) {
            *totals.entry(other_name.clone()).or_default() += count;
        }
    }
    let mut links: Vec<(String, usize)> = totals.into_iter().collect();
    links.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    links.truncate(LIST_LIMIT);

    Area {
        slug: slug(&name),
        name,
        files,
        hubs,
        entrypoints: entry_names,
        endpoints,
        processes: chains,
        links,
        symbols: symbols.len(),
    }
}

/// Whether a path is a third-party bundle rather than this repository's code.
/// A minified vendor file is one enormous "community" of symbols nobody here
/// wrote, and a skill page describing it would be a page about someone else's
/// library.
fn is_vendored(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains(".min.")
        || lower.split('/').any(|segment| {
            matches!(
                segment,
                "vendor" | "vendored" | "third_party" | "thirdparty"
            )
        })
}

/// Two communities can live in the same directory — a repository with a flat
/// `src/` has several. They must not collapse onto one name, or one page
/// would overwrite the other and every cross-link would be ambiguous, so a
/// colliding area is renamed after the file that holds most of it.
fn disambiguate(
    named: &mut BTreeMap<i64, String>,
    candidates: &[(i64, Vec<i64>)],
    by_id: &BTreeMap<i64, &Node>,
) {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for name in named.values() {
        *seen.entry(name.clone()).or_default() += 1;
    }
    let colliding: BTreeSet<String> = seen
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(name, _)| name)
        .collect();
    if colliding.is_empty() {
        return;
    }
    let symbols_of: BTreeMap<i64, &Vec<i64>> = candidates
        .iter()
        .map(|(id, symbols)| (*id, symbols))
        .collect();
    let mut taken: BTreeSet<String> = named
        .values()
        .filter(|name| !colliding.contains(*name))
        .cloned()
        .collect();
    // The biggest area in a colliding group keeps the directory name — it is
    // what someone means by "the `src` area" — and the smaller ones are named
    // after the file that holds them.
    let mut keeps_directory_name: BTreeSet<i64> = BTreeSet::new();
    for name in &colliding {
        let winner = named
            .iter()
            .filter(|(_, other)| *other == name)
            .max_by_key(|(id, _)| {
                (
                    symbols_of.get(id).map_or(0, |symbols| symbols.len()),
                    std::cmp::Reverse(**id),
                )
            })
            .map(|(id, _)| *id);
        if let Some(id) = winner {
            keeps_directory_name.insert(id);
        }
    }
    for (id, name) in named.iter_mut() {
        if !colliding.contains(name) {
            continue;
        }
        if keeps_directory_name.contains(id) {
            taken.insert(name.clone());
            continue;
        }
        let candidate = symbols_of
            .get(id)
            .map_or_else(String::new, |symbols| dominant_file(symbols, by_id));
        let mut resolved = if candidate.is_empty() {
            format!("{name} #{id}")
        } else {
            candidate
        };
        if taken.contains(&resolved) {
            resolved = format!("{resolved} #{id}");
        }
        taken.insert(resolved.clone());
        *name = resolved;
    }
}

/// The file holding most of an area's symbols — its centre of gravity, and
/// the most recognizable thing to name it after.
fn dominant_file(symbols: &[i64], by_id: &BTreeMap<i64, &Node>) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for id in symbols {
        if let Some(node) = by_id.get(id) {
            *counts.entry(node.file_path.as_str()).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|(path, count)| (*count, std::cmp::Reverse(*path)))
        .map(|(path, _)| path.to_string())
        .unwrap_or_default()
}

/// An area's name is the directory its symbols share — the deepest one that
/// holds a majority of them, which is what a human would call the area. A
/// community spread across the tree falls back to its most-connected file.
fn area_name(symbols: &[i64], by_id: &BTreeMap<i64, &Node>) -> String {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut total = 0usize;
    for id in symbols {
        let Some(node) = by_id.get(id) else { continue };
        total += 1;
        let mut current = Path::new(&node.file_path);
        while let Some(parent) = current.parent() {
            let text = parent.to_string_lossy().to_string();
            if text.is_empty() {
                break;
            }
            *counts.entry(text).or_default() += 1;
            current = parent;
        }
    }
    let majority = total.div_ceil(2).max(1);
    let best = counts
        .iter()
        .filter(|(_, count)| **count >= majority)
        // Deepest directory that still covers the majority: `src/mcp` beats
        // `src`, because it says more and is equally true.
        .max_by_key(|(path, _)| (path.matches('/').count(), path.len()))
        .map(|(path, _)| path.clone());
    best.unwrap_or_else(|| {
        symbols
            .first()
            .and_then(|id| by_id.get(id))
            .map_or_else(|| "area".into(), |node| node.file_path.clone())
    })
}

fn slug(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-').to_string();
    let mut collapsed = String::with_capacity(trimmed.len());
    let mut last_dash = false;
    for character in trimmed.chars() {
        if character == '-' {
            if !last_dash {
                collapsed.push('-');
            }
            last_dash = true;
        } else {
            collapsed.push(character);
            last_dash = false;
        }
    }
    if collapsed.is_empty() {
        "area".into()
    } else {
        collapsed
    }
}

/// The `SKILL.md` for one area, in the same frontmatter format as the static
/// pack.
#[must_use]
pub fn skill_markdown(area: &Area) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "---\nname: {AREA_PREFIX}{slug}\ndescription: The `{name}` area of this repository — \
         {symbols} symbols across {files} file(s){entry}. Read this before changing anything \
         under `{name}`, and prefer it to grepping for a map.\n---\n\n# Area: {name}\n\n",
        slug = area.slug,
        name = area.name,
        symbols = area.symbols,
        files = area.files.len(),
        entry = first_entrypoint(area)
    );
    out.push_str(
        "Generated from the `aag` graph. Do not edit — every `aag bigbang` rewrites this \
         file from the current graph.\n\n",
    );

    if !area.entrypoints.is_empty() {
        out.push_str("## Starts here\n\n");
        for entry in &area.entrypoints {
            let _ = writeln!(out, "- `{entry}`");
        }
        out.push('\n');
    }

    if !area.hubs.is_empty() {
        out.push_str("## What the rest of the code leans on\n\n");
        for (symbol, count) in &area.hubs {
            let _ = writeln!(out, "- `{symbol}` — {count} dependent(s)");
        }
        out.push_str("\nChanging one of these is a repository-wide event: run `aag impact <symbol>` first.\n\n");
    }

    if !area.endpoints.is_empty() {
        out.push_str("## Contracts it serves\n\n");
        for endpoint in &area.endpoints {
            let _ = writeln!(out, "- `{endpoint}`");
        }
        out.push('\n');
    }

    if !area.processes.is_empty() {
        out.push_str("## Flows rooted here\n\n");
        for (root, steps) in &area.processes {
            let _ = writeln!(out, "- `{root}` reaches {steps} symbol(s)");
        }
        out.push_str("\nWalk one with `aag processes <name>`.\n\n");
    }

    if !area.links.is_empty() {
        out.push_str("## Coupled to\n\n");
        for (other, count) in &area.links {
            let _ = writeln!(out, "- `{other}` — {count} edge(s) across the boundary");
        }
        out.push_str("\nA change here is likely to be felt there.\n\n");
    }

    out.push_str("## Files\n\n");
    for file in area.files.iter().take(LIST_LIMIT) {
        let _ = writeln!(out, "- `{file}`");
    }
    if area.files.len() > LIST_LIMIT {
        let _ = writeln!(out, "- …and {} more", area.files.len() - LIST_LIMIT);
    }
    out.push_str(
        "\n## Asking about it\n\n\
         - `aag explore <symbol or question>` — source, callers, and call paths together\n\
         - `aag impact <symbol>` — what breaks if it changes\n\
         - `aag communities <query>` / `aag processes <query>` — the same clustering this \
         page was built from\n",
    );
    out
}

fn first_entrypoint(area: &Area) -> String {
    area.entrypoints
        .first()
        .map(|entry| format!(", entered through `{entry}`"))
        .unwrap_or_default()
}

/// Writes one `SKILL.md` per area under `skill_root`, removing generated
/// skills for areas that no longer exist. Returns how many files changed.
///
/// Refresh is content-addressed rather than timestamped: a file whose bytes
/// already match is not rewritten, so re-running costs nothing and a watcher
/// never sees a spurious change.
///
/// # Errors
/// Returns a write error when a skill file cannot be created or removed.
pub fn write_area_skills(skill_root: &Path, areas: &[Area]) -> Result<u32> {
    let mut changed = 0u32;
    let mut wanted: BTreeSet<String> = BTreeSet::new();
    for area in areas {
        let directory = format!("{AREA_PREFIX}{}", area.slug);
        wanted.insert(directory.clone());
        let path = skill_root.join(&directory).join("SKILL.md");
        let content = skill_markdown(area);
        if std::fs::read_to_string(&path).is_ok_and(|existing| existing == content) {
            continue;
        }
        crate::install::write_text_public(&path, &content)?;
        changed += 1;
    }
    changed += prune(skill_root, &wanted)?;
    Ok(changed)
}

/// Removes generated area skills that no longer correspond to an area — a
/// merged or deleted module must not leave a page behind claiming it exists.
fn prune(skill_root: &Path, wanted: &BTreeSet<String>) -> Result<u32> {
    let Ok(entries) = std::fs::read_dir(skill_root) else {
        return Ok(0);
    };
    let mut removed = 0u32;
    let mut stale: Vec<std::path::PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(AREA_PREFIX) && !wanted.contains(&name) {
            stale.push(entry.path());
        }
    }
    stale.sort();
    for path in stale {
        std::fs::remove_dir_all(&path).map_err(|source| crate::error::Error::RemoveDir {
            path: path.clone(),
            source,
        })?;
        removed += 1;
    }
    Ok(removed)
}

/// Regenerates area skills into every skill directory `install` writes to.
///
/// # Errors
/// Returns a storage error when the graph cannot be read, or a write error
/// when a skill cannot be written.
pub fn refresh(root: &Path, graph: &Graph) -> Result<u32> {
    let areas = detect(graph)?;
    let mut changed = 0;
    for skill_root in [
        root.join(".claude").join("skills"),
        root.join(".agents").join("skills"),
    ] {
        if skill_root.is_dir() {
            changed += write_area_skills(&skill_root, &areas)?;
        }
    }
    Ok(changed)
}

/// `aag areas` — what the generated skills are built from, printed.
///
/// # Errors
/// Returns [`crate::error::Error::IndexMissing`] when `root` has no index, or
/// a storage error when the graph cannot be read.
pub fn run(root: &Path) -> Result<()> {
    let graph = Graph::open_existing(root)?;
    let areas = detect(&graph)?;
    if areas.is_empty() {
        println!(
            "no areas: this repository has no cluster of {MIN_SYMBOLS}+ symbols that holds \
             together. The static skills still apply."
        );
        return Ok(());
    }
    println!("{} area(s), each with a generated skill:\n", areas.len());
    for area in &areas {
        println!(
            "{} — {} symbols across {} file(s) [{}{}]",
            area.name,
            area.symbols,
            area.files.len(),
            AREA_PREFIX,
            area.slug
        );
        if let Some((hub, count)) = area.hubs.first() {
            println!("     leans on `{hub}` ({count} dependents)");
        }
        for (other, count) in area.links.iter().take(3) {
            println!("     coupled to {other} ({count} edges)");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{Confidence, EdgeKind};

    fn node(id: i64, kind: NodeKind, file: &str, name: &str) -> Node {
        Node {
            id: Some(id),
            kind,
            name: name.to_string(),
            file_path: file.to_string(),
            start_line: 1,
            end_line: 2,
            description: None,
        }
    }

    fn edge(src: i64, dst: i64, kind: EdgeKind) -> Edge {
        Edge {
            src,
            dst,
            kind,
            confidence: Confidence::Extracted,
        }
    }

    /// Two clusters that only touch through one edge: `src/store` and
    /// `src/api`, ten symbols each.
    pub(super) fn two_area_graph() -> (Vec<Node>, Vec<Edge>) {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        for index in 0..10 {
            nodes.push(node(
                index,
                NodeKind::Function,
                "src/store/graph.rs",
                &format!("store{index}"),
            ));
            nodes.push(node(
                100 + index,
                NodeKind::Function,
                "src/api/handler.rs",
                &format!("api{index}"),
            ));
        }
        // Dense inside each cluster — every symbol calls every other one, so
        // label propagation has something to hold onto. A real module looks
        // more like this than like a star.
        for left in 0..10 {
            for right in 0..10 {
                if left != right {
                    edges.push(edge(left, right, EdgeKind::Calls));
                    edges.push(edge(100 + left, 100 + right, EdgeKind::Calls));
                }
            }
        }
        // One thin bridge between the two.
        edges.push(edge(100, 0, EdgeKind::Calls));
        (nodes, edges)
    }

    #[test]
    fn areas_are_named_for_the_directory_their_code_lives_in() {
        let (nodes, edges) = two_area_graph();

        let areas = from_graph(&nodes, &edges);

        let names: Vec<&str> = areas.iter().map(|area| area.name.as_str()).collect();
        assert!(names.contains(&"src/store"), "{names:?}");
        assert!(names.contains(&"src/api"), "{names:?}");
        let slugs: Vec<&str> = areas.iter().map(|area| area.slug.as_str()).collect();
        assert!(slugs.contains(&"src-store"), "{slugs:?}");
    }

    #[test]
    fn an_area_reports_its_hubs_and_its_coupling() {
        let (nodes, edges) = two_area_graph();

        let areas = from_graph(&nodes, &edges);
        let store = areas
            .iter()
            .find(|area| area.name == "src/store")
            .expect("the store area");

        assert_eq!(
            store.hubs.first().map(|(name, _)| name.as_str()),
            Some("src/store/graph.rs:store0"),
            "the symbol the other area also calls ranks first: {:?}",
            store.hubs
        );
        assert!(
            store.links.iter().any(|(other, _)| other == "src/api"),
            "the bridge is reported: {:?}",
            store.links
        );
    }

    #[test]
    fn a_vendored_bundle_is_not_an_area_of_this_repository() {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        for index in 0..12 {
            nodes.push(node(
                index,
                NodeKind::Function,
                "assets/sigma.min.js",
                &format!("minified{index}"),
            ));
        }
        for left in 0..12 {
            for right in 0..12 {
                if left != right {
                    edges.push(edge(left, right, EdgeKind::Calls));
                }
            }
        }

        assert!(
            from_graph(&nodes, &edges).is_empty(),
            "someone else's bundle is not an area of this repository"
        );
    }

    #[test]
    fn two_areas_in_one_directory_get_distinct_names() {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        // Two dense clusters, both directly under `src/`.
        for index in 0..10 {
            nodes.push(node(
                index,
                NodeKind::Function,
                "src/store.rs",
                &format!("s{index}"),
            ));
            nodes.push(node(
                100 + index,
                NodeKind::Function,
                "src/api.rs",
                &format!("a{index}"),
            ));
        }
        for left in 0..10 {
            for right in 0..10 {
                if left != right {
                    edges.push(edge(left, right, EdgeKind::Calls));
                    edges.push(edge(100 + left, 100 + right, EdgeKind::Calls));
                }
            }
        }

        let areas = from_graph(&nodes, &edges);

        let names: BTreeSet<&str> = areas.iter().map(|area| area.name.as_str()).collect();
        assert_eq!(names.len(), areas.len(), "names collide: {names:?}");
        assert!(
            names.contains("src"),
            "the bigger half keeps the directory name: {names:?}"
        );
        assert!(
            names.contains("src/store.rs") || names.contains("src/api.rs"),
            "the other is named for its file: {names:?}"
        );
    }

    #[test]
    fn a_cluster_too_small_to_be_an_area_is_not_one() {
        let nodes = vec![
            node(1, NodeKind::Function, "src/tiny/one.rs", "one"),
            node(2, NodeKind::Function, "src/tiny/one.rs", "two"),
        ];
        let edges = vec![edge(2, 1, EdgeKind::Calls)];

        assert!(from_graph(&nodes, &edges).is_empty());
    }

    #[test]
    fn generation_is_byte_identical_for_the_same_graph() {
        let (nodes, edges) = two_area_graph();

        let first = from_graph(&nodes, &edges);
        let second = from_graph(&nodes, &edges);

        assert_eq!(first, second);
        let left: Vec<String> = first.iter().map(skill_markdown).collect();
        let right: Vec<String> = second.iter().map(skill_markdown).collect();
        assert_eq!(left, right, "the same graph must produce the same bytes");
    }

    #[test]
    fn refresh_rewrites_only_what_changed_and_removes_areas_that_are_gone() {
        let root = std::env::temp_dir().join(format!("aag-areas-{}", std::process::id()));
        let skills = root.join(".claude").join("skills");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&skills).unwrap();
        let (nodes, edges) = two_area_graph();
        let areas = from_graph(&nodes, &edges);

        let first = write_area_skills(&skills, &areas).unwrap();
        let second = write_area_skills(&skills, &areas).unwrap();

        assert_eq!(first, 2, "one skill per area");
        assert_eq!(second, 0, "an unchanged graph rewrites nothing");
        assert!(skills.join("aag-area-src-store").join("SKILL.md").is_file());

        // The API area disappears — so must its page.
        let shrunk: Vec<Area> = areas
            .iter()
            .filter(|area| area.name == "src/store")
            .cloned()
            .collect();
        let third = write_area_skills(&skills, &shrunk).unwrap();

        assert_eq!(third, 1, "the stale area is pruned");
        assert!(!skills.join("aag-area-src-api").exists());
        assert!(skills.join("aag-area-src-store").join("SKILL.md").is_file());
    }

    #[test]
    fn the_page_says_what_an_agent_needs_before_touching_the_area() {
        let (nodes, edges) = two_area_graph();
        let areas = from_graph(&nodes, &edges);
        let store = areas
            .iter()
            .find(|area| area.name == "src/store")
            .expect("the store area");

        let page = skill_markdown(store);

        assert!(
            page.starts_with("---\nname: aag-area-src-store\n"),
            "{page}"
        );
        assert!(page.contains("Do not edit"), "generated files say so");
        assert!(page.contains("aag impact"), "{page}");
        assert!(page.contains("src/store/graph.rs"), "{page}");
    }
}
