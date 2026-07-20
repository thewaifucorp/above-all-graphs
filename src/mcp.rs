//! MCP server: newline-delimited JSON-RPC 2.0 over stdio.
//!
//! Per `SPEC.md` section 4: `explore` is the one tool listed by default —
//! an agent choosing between many similarly-named tools mis-picks more
//! often than one that just answers "how does X work" (this was validated
//! by `CodeGraph`). The other tools stay registered and callable via
//! `tools/call` regardless, but only show up in `tools/list` once named in
//! the `AAG_MCP_TOOLS` env var (comma-separated), so an agent's tool menu
//! doesn't grow unless someone opts in.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::{self, BufRead, Write as _};
use std::path::Path;

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::storage::Graph;
use crate::{docs, explore, export, impact};

struct ToolSpec {
    name: &'static str,
    description: &'static str,
    arg: &'static str,
    arg_description: &'static str,
    implemented: bool,
}

const TOOL_SPECS: &[ToolSpec] = &[
    ToolSpec {
        name: "explore",
        description: "Answer how code works: symbol source verbatim, grouped by file, plus call paths.",
        arg: "query",
        arg_description: "Symbol name or search term.",
        implemented: true,
    },
    ToolSpec {
        name: "node",
        description: "Source of one exact symbol plus its direct callers.",
        arg: "name",
        arg_description: "Exact symbol name.",
        implemented: true,
    },
    ToolSpec {
        name: "search",
        description: "Full-text search over symbol names.",
        arg: "query",
        arg_description: "Search term.",
        implemented: true,
    },
    ToolSpec {
        name: "callers",
        description: "Who calls or imports this symbol.",
        arg: "name",
        arg_description: "Exact symbol name.",
        implemented: true,
    },
    ToolSpec {
        name: "callees",
        description: "What this symbol calls or imports.",
        arg: "name",
        arg_description: "Exact symbol name.",
        implemented: true,
    },
    ToolSpec {
        name: "impact",
        description: "Blast radius of changing a symbol: every caller/importer, transitively.",
        arg: "symbol",
        arg_description: "Exact symbol name.",
        implemented: true,
    },
    ToolSpec {
        name: "rename",
        description: "Coordinated multi-file rename. Applies immediately and writes to disk — pass `name` (current) and `new_name`.",
        arg: "name",
        arg_description: "Current (unique) symbol name. Pass `new_name` alongside it.",
        implemented: true,
    },
    ToolSpec {
        name: "affected",
        description: "Test-looking files transitively affected by a set of changed files.",
        arg: "changed_files",
        arg_description: "Changed file paths, one per line.",
        implemented: true,
    },
    ToolSpec {
        name: "cypher",
        description: "Direct query against the graph layer.",
        arg: "query",
        arg_description: "Cypher query.",
        implemented: true,
    },
    ToolSpec {
        name: "detect_changes",
        description: "Pre-commit risk analysis via git diff.",
        arg: "diff",
        arg_description: "Git diff text.",
        implemented: true,
    },
    ToolSpec {
        name: "wiki",
        description: "Generate a wiki-style export of the graph under `.aag/wiki/`.",
        arg: "out_dir",
        arg_description: "Ignored — always writes to `.aag/wiki/` relative to the indexed root.",
        implemented: true,
    },
    ToolSpec {
        name: "communities",
        description: "Detected architectural communities and their member symbols.",
        arg: "query",
        arg_description: "Optional name filter; pass an empty string for all communities.",
        implemented: true,
    },
    ToolSpec {
        name: "processes",
        description: "Detected entrypoints and their reachable execution flows.",
        arg: "query",
        arg_description: "Optional entrypoint filter; pass an empty string for all processes.",
        implemented: true,
    },
    ToolSpec {
        name: "neighbors",
        description: "All incoming and outgoing neighbors of a symbol.",
        arg: "name",
        arg_description: "Exact symbol name.",
        implemented: true,
    },
    ToolSpec {
        name: "shortest_path",
        description: "Shortest graph path between two symbols.",
        arg: "query",
        arg_description: "Source and target separated by `->`, for example `main -> save`.",
        implemented: true,
    },
    ToolSpec {
        name: "god_nodes",
        description: "Most-connected symbols in the graph.",
        arg: "top_n",
        arg_description: "Maximum number of nodes, for example `10`.",
        implemented: true,
    },
    ToolSpec {
        name: "graph_stats",
        description: "Graph counts, confidence distribution, communities, and processes.",
        arg: "query",
        arg_description: "Pass an empty string; reserved for future filters.",
        implemented: true,
    },
    ToolSpec {
        name: "list_prs",
        description: "Open GitHub PRs with CI and review state.",
        arg: "base",
        arg_description: "Optional base branch; pass an empty string for the default.",
        implemented: true,
    },
    ToolSpec {
        name: "get_pr_impact",
        description: "Changed files, graph communities, touched nodes, and affected tests for a PR.",
        arg: "pr_number",
        arg_description: "GitHub pull request number.",
        implemented: true,
    },
    ToolSpec {
        name: "triage_prs",
        description: "Non-draft open PRs ready for graph-aware triage.",
        arg: "base",
        arg_description: "Optional base branch; pass an empty string for the default.",
        implemented: true,
    },
    ToolSpec {
        name: "group_list",
        description: "List every repository in the local AAG federation.",
        arg: "group",
        arg_description: "Pass `all`; named subgroups are reserved for future policy filters.",
        implemented: true,
    },
    ToolSpec {
        name: "group_query",
        description: "Query all registered repository graphs.",
        arg: "query",
        arg_description: "Symbol or natural-language search term.",
        implemented: true,
    },
    ToolSpec {
        name: "group_status",
        description: "Index and manifest status across registered repositories.",
        arg: "group",
        arg_description: "Pass `all`.",
        implemented: true,
    },
    ToolSpec {
        name: "group_contracts",
        description: "OpenAPI, database, and infrastructure contracts across repositories.",
        arg: "group",
        arg_description: "Pass `all`.",
        implemented: true,
    },
    ToolSpec {
        name: "group_sync",
        description: "Synchronize every registered repository graph and manifest.",
        arg: "group",
        arg_description: "Pass `all`.",
        implemented: true,
    },
    ToolSpec {
        name: "describe_doc",
        description: "Record the host agent's vision-pass description of a doc/image, linking it to symbols it mentions by name.",
        arg: "doc",
        arg_description: "Doc path, relative to the repository root (e.g. `docs/arch.png`). Pass `description` alongside it.",
        implemented: true,
    },
];

const DEFAULT_LISTED_TOOLS: &[&str] = &["explore"];

/// Runs the MCP server against the index under `root`, reading JSON-RPC
/// requests from stdin and writing responses to stdout until stdin closes.
///
/// Before serving requests, this reconciles the index against the working
/// tree once (absorbing any edits made while nothing was watching — see
/// `crate::watch::reconcile`) and spawns the background watcher that keeps
/// it fresh for the rest of the session.
///
/// # Errors
///
/// Never returns `Err` in practice — malformed input lines are skipped, and
/// tool/domain errors are reported back as JSON-RPC responses rather than
/// killing the server.
pub fn run(root: &Path) -> Result<()> {
    let root = root.to_path_buf();

    if let Err(error) = crate::watch::reconcile(&root) {
        tracing::warn!(%error, "startup reconciliation failed");
    }
    crate::watch::spawn(root.clone());

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(response) = handle(&root, &request) {
            let _ = writeln!(stdout, "{response}");
            let _ = stdout.flush();
        }
    }
    Ok(())
}

fn handle(root: &Path, request: &Value) -> Option<Value> {
    let id = request.get("id").cloned();
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = request.get("params");

    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "aag", "version": env!("CARGO_PKG_VERSION")},
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": listed_tools(&enabled_tool_names()) })),
        "tools/call" => call_tool(root, params),
        _ if id.is_none() => return None,
        _ => Err(format!("method not found: {method}")),
    };

    let id = id?;
    Some(match result {
        Ok(value) => json!({"jsonrpc": "2.0", "id": id, "result": value}),
        Err(message) => {
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32603, "message": message}})
        }
    })
}

fn enabled_tool_names() -> HashSet<String> {
    std::env::var("AAG_MCP_TOOLS")
        .unwrap_or_default()
        .split(',')
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect()
}

fn listed_tools(enabled: &HashSet<String>) -> Vec<Value> {
    TOOL_SPECS
        .iter()
        .filter(|spec| DEFAULT_LISTED_TOOLS.contains(&spec.name) || enabled.contains(spec.name))
        .map(tool_schema)
        .collect()
}

fn tool_schema(spec: &ToolSpec) -> Value {
    if spec.name == "describe_doc" {
        return json!({
            "name": spec.name,
            "description": spec.description,
            "inputSchema": {
                "type": "object",
                "properties": {
                    "doc": {"type": "string", "description": "Doc path, relative to the repository root (e.g. `docs/arch.png`)."},
                    "description": {"type": "string", "description": "What the doc shows/says, as seen by the calling agent."},
                },
                "required": ["doc", "description"],
            },
        });
    }
    if spec.name == "rename" {
        return json!({
            "name": spec.name,
            "description": spec.description,
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Current (unique) symbol name."},
                    "new_name": {"type": "string", "description": "New name."},
                },
                "required": ["name", "new_name"],
            },
        });
    }
    json!({
        "name": spec.name,
        "description": spec.description,
        "inputSchema": {
            "type": "object",
            "properties": { spec.arg: {"type": "string", "description": spec.arg_description} },
            "required": [spec.arg],
        },
    })
}

fn call_tool(root: &Path, params: Option<&Value>) -> std::result::Result<Value, String> {
    let params = params.ok_or_else(|| "missing params".to_string())?;
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing tool name".to_string())?;

    if name == "describe_doc" {
        return call_describe_doc(root, params);
    }
    if name == "rename" {
        return call_rename(root, params);
    }

    let spec = TOOL_SPECS
        .iter()
        .find(|spec| spec.name == name)
        .ok_or_else(|| format!("unknown tool: {name}"))?;
    let arg = params
        .get("arguments")
        .and_then(|arguments| arguments.get(spec.arg))
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing argument `{}`", spec.arg))?;

    let (text, is_error) = if spec.implemented {
        match dispatch(root, name, arg) {
            Ok(text) => (text, false),
            Err(error) => (error.to_string(), true),
        }
    } else {
        (format!("`{name}` is not implemented yet"), true)
    };

    Ok(json!({
        "content": [{"type": "text", "text": text}],
        "isError": is_error,
    }))
}

fn dispatch(root: &Path, name: &str, arg: &str) -> Result<String> {
    match name {
        "explore" => explore::format(root, arg),
        "node" => explore::format_node(root, arg),
        "search" => search_text(root, arg),
        "callers" => edges_text(root, arg, &Direction::Callers),
        "callees" => edges_text(root, arg, &Direction::Callees),
        "impact" => impact::format(root, arg),
        "wiki" => write_wiki(root),
        "affected" => affected_text(root, arg),
        "detect_changes" => detect_changes_text(root, arg),
        "cypher" => cypher_text(root, arg),
        "communities" => communities_text(root, arg),
        "processes" => processes_text(root, arg),
        "neighbors" => neighbors_text(root, arg),
        "shortest_path" => shortest_path_text(root, arg),
        "god_nodes" => god_nodes_text(root, arg),
        "graph_stats" => graph_stats_text(root),
        "list_prs" => crate::pr::list(root, arg),
        "get_pr_impact" => crate::pr::impact(root, arg),
        "triage_prs" => crate::pr::triage(root, arg),
        "group_list" => Ok(crate::federation::list()),
        "group_query" => crate::federation::query(arg),
        "group_status" => Ok(crate::federation::status()),
        "group_contracts" => crate::federation::contracts(),
        "group_sync" => crate::federation::sync(),
        _ => unreachable!("dispatch only called for implemented tools"),
    }
}

fn neighbors_text(root: &Path, name: &str) -> Result<String> {
    let graph = Graph::open_existing(root)?;
    let node = graph
        .find_by_name(name)?
        .ok_or_else(|| Error::SymbolNotFound { name: name.into() })?;
    let id = node.id.expect("stored nodes have ids");
    let incoming = graph.callers(id)?.into_iter().map(|(node, kind, confidence)| json!({"direction": "incoming", "name": node.name, "relation": kind.as_str(), "confidence": confidence.as_str()}));
    let outgoing = graph.callees(id)?.into_iter().map(|(node, kind, confidence)| json!({"direction": "outgoing", "name": node.name, "relation": kind.as_str(), "confidence": confidence.as_str()}));
    let rows = incoming.chain(outgoing).collect::<Vec<_>>();
    Ok(serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".into()))
}

fn shortest_path_text(root: &Path, query: &str) -> Result<String> {
    let (source_name, target_name) = query.split_once("->").ok_or_else(|| Error::Protocol {
        context: "shortest path parse failed",
        detail: "expected `source -> target`".into(),
    })?;
    let graph = Graph::open_existing(root)?;
    let source = graph
        .find_by_name(source_name.trim())?
        .ok_or_else(|| Error::SymbolNotFound {
            name: source_name.trim().into(),
        })?;
    let target = graph
        .find_by_name(target_name.trim())?
        .ok_or_else(|| Error::SymbolNotFound {
            name: target_name.trim().into(),
        })?;
    let nodes = graph.all_nodes()?;
    let edges = graph.all_edges()?;
    let by_id: std::collections::HashMap<i64, &crate::storage::Node> = nodes
        .iter()
        .filter_map(|node| node.id.map(|id| (id, node)))
        .collect();
    let start = source.id.expect("stored nodes have ids");
    let goal = target.id.expect("stored nodes have ids");
    let mut adjacency: std::collections::HashMap<i64, Vec<i64>> = std::collections::HashMap::new();
    for edge in &edges {
        adjacency.entry(edge.src).or_default().push(edge.dst);
        adjacency.entry(edge.dst).or_default().push(edge.src);
    }
    let mut queue = std::collections::VecDeque::from([start]);
    let mut previous = std::collections::HashMap::from([(start, start)]);
    while let Some(current) = queue.pop_front() {
        if current == goal {
            break;
        }
        for next in adjacency.get(&current).into_iter().flatten() {
            if !previous.contains_key(next) {
                previous.insert(*next, current);
                queue.push_back(*next);
            }
        }
    }
    if !previous.contains_key(&goal) {
        return Ok("no path found".into());
    }
    let mut path = vec![goal];
    while *path.last().unwrap_or(&start) != start {
        path.push(previous[path.last().unwrap_or(&start)]);
    }
    path.reverse();
    Ok(path
        .into_iter()
        .filter_map(|id| {
            by_id
                .get(&id)
                .map(|node| format!("{} ({}:{})", node.name, node.file_path, node.start_line))
        })
        .collect::<Vec<_>>()
        .join(" -> "))
}

fn god_nodes_text(root: &Path, top_n: &str) -> Result<String> {
    let graph = Graph::open_existing(root)?;
    let nodes = graph.all_nodes()?;
    let edges = graph.all_edges()?;
    let mut degree = std::collections::HashMap::<i64, usize>::new();
    for edge in edges {
        *degree.entry(edge.src).or_default() += 1;
        *degree.entry(edge.dst).or_default() += 1;
    }
    let mut ranked = nodes
        .into_iter()
        .filter_map(|node| Some((degree.get(&node.id?).copied().unwrap_or(0), node)))
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| left.1.name.cmp(&right.1.name))
    });
    let limit = top_n.parse::<usize>().unwrap_or(10).min(100);
    Ok(ranked
        .into_iter()
        .take(limit)
        .map(|(count, node)| {
            format!(
                "{} — {count} edges ({}:{})",
                node.name, node.file_path, node.start_line
            )
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

fn graph_stats_text(root: &Path) -> Result<String> {
    let graph = Graph::open_existing(root)?;
    let nodes = graph.all_nodes()?;
    let edges = graph.all_edges()?;
    let mut confidence = std::collections::BTreeMap::<&str, usize>::new();
    for edge in &edges {
        *confidence.entry(edge.confidence.as_str()).or_default() += 1;
    }
    Ok(serde_json::to_string_pretty(&json!({
        "nodes": nodes.len(), "edges": edges.len(),
        "communities": crate::analysis::communities(&nodes, &edges).len(),
        "processes": crate::analysis::processes(&nodes, &edges).len(),
        "confidence": confidence
    }))
    .unwrap_or_else(|_| "{}".into()))
}

fn communities_text(root: &Path, query: &str) -> Result<String> {
    let graph = Graph::open_existing(root)?;
    let nodes = graph.all_nodes()?;
    let edges = graph.all_edges()?;
    let by_id: std::collections::HashMap<i64, _> = nodes
        .iter()
        .filter_map(|node| node.id.map(|id| (id, node)))
        .collect();
    let rows = crate::analysis::communities(&nodes, &edges)
        .into_iter()
        .filter_map(|community| {
            let members = community
                .members
                .iter()
                .filter_map(|id| by_id.get(id).map(|node| node.name.clone()))
                .collect::<Vec<_>>();
            (query.is_empty() || members.iter().any(|name| name.contains(query)))
                .then_some(json!({"id": community.id, "members": members}))
        })
        .collect::<Vec<_>>();
    Ok(serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".into()))
}

fn processes_text(root: &Path, query: &str) -> Result<String> {
    let graph = Graph::open_existing(root)?;
    let nodes = graph.all_nodes()?;
    let edges = graph.all_edges()?;
    let by_id: std::collections::HashMap<i64, _> = nodes
        .iter()
        .filter_map(|node| node.id.map(|id| (id, node)))
        .collect();
    let rows = crate::analysis::processes(&nodes, &edges)
        .into_iter()
        .filter_map(|process| {
            let entrypoint = by_id.get(&process.entrypoint)?.name.clone();
            let steps = process
                .steps
                .iter()
                .filter_map(|id| by_id.get(id).map(|node| node.name.clone()))
                .collect::<Vec<_>>();
            (query.is_empty() || entrypoint.contains(query))
                .then_some(json!({"entrypoint": entrypoint, "steps": steps}))
        })
        .collect::<Vec<_>>();
    Ok(serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".into()))
}

fn detect_changes_text(root: &Path, diff: &str) -> Result<String> {
    let mut changed: Vec<String> = diff
        .lines()
        .filter_map(|line| line.strip_prefix("+++ b/"))
        .filter(|path| *path != "/dev/null")
        .map(str::to_string)
        .collect();
    changed.sort_unstable();
    changed.dedup();
    if changed.is_empty() {
        return Ok("no changed files found in diff".to_string());
    }
    let affected = crate::refactor::affected(root, &changed)?;
    Ok(format!(
        "changed files:\n{}\n\naffected tests:\n{}",
        changed.join("\n"),
        if affected.is_empty() {
            "none".into()
        } else {
            affected.join("\n")
        }
    ))
}

fn cypher_text(root: &Path, query: &str) -> Result<String> {
    let normalized = query.split_whitespace().collect::<Vec<_>>().join(" ");
    let upper = normalized.to_ascii_uppercase();
    if !upper.starts_with("MATCH ")
        || !upper.contains(" RETURN ")
        || [" CREATE ", " DELETE ", " SET ", " REMOVE ", " MERGE "]
            .iter()
            .any(|keyword| upper.contains(keyword))
    {
        return Err(Error::Protocol {
            context: "Cypher query rejected",
            detail: "only read-only MATCH ... RETURN queries are supported".into(),
        });
    }
    let limit = upper
        .rsplit_once(" LIMIT ")
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(100)
        .min(1_000);
    let graph = Graph::open_existing(root)?;
    if normalized.contains("-[") || normalized.contains("]->") {
        let nodes = graph.all_nodes()?;
        let by_id: std::collections::HashMap<i64, _> = nodes
            .iter()
            .filter_map(|node| node.id.map(|id| (id, node)))
            .collect();
        let rows = graph
            .all_edges()?
            .into_iter()
            .take(limit)
            .filter_map(|edge| {
                Some(json!({
                    "source": by_id.get(&edge.src)?.name,
                    "relationship": edge.kind.as_str(),
                    "target": by_id.get(&edge.dst)?.name,
                    "confidence": edge.confidence.as_str()
                }))
            })
            .collect::<Vec<_>>();
        return Ok(serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".into()));
    }
    let name_filter = cypher_string_filter(&normalized, ".name");
    let kind_filter = cypher_string_filter(&normalized, ".kind");
    let rows = graph
        .all_nodes()?
        .into_iter()
        .filter(|node| name_filter.as_ref().is_none_or(|name| node.name == *name))
        .filter(|node| {
            kind_filter
                .as_ref()
                .is_none_or(|kind| node.kind.as_str() == kind)
        })
        .take(limit)
        .map(|node| {
            json!({
                "id": node.id,
                "kind": node.kind.as_str(),
                "name": node.name,
                "file": node.file_path,
                "line": node.start_line
            })
        })
        .collect::<Vec<_>>();
    Ok(serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".into()))
}

fn cypher_string_filter(query: &str, field: &str) -> Option<String> {
    let start = query.find(field)? + field.len();
    let value = query
        .get(start..)?
        .trim_start()
        .strip_prefix('=')?
        .trim_start();
    let quote = value
        .chars()
        .next()
        .filter(|character| matches!(character, '\'' | '"'))?;
    value.get(1..)?.split(quote).next().map(str::to_string)
}

fn write_wiki(root: &Path) -> Result<String> {
    let graph = Graph::open_existing(root)?;
    let out_dir = root.join(".aag").join("wiki");
    export::write_wiki_html(&out_dir, &graph)?;
    Ok(format!("wrote wiki to {}", out_dir.display()))
}

fn affected_text(root: &Path, changed_files: &str) -> Result<String> {
    let changed: Vec<String> = changed_files
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    let files = crate::refactor::affected(root, &changed)?;
    if files.is_empty() {
        return Ok("no affected test files found".to_string());
    }
    Ok(files.join("\n"))
}

fn call_rename(root: &Path, params: &Value) -> std::result::Result<Value, String> {
    let arguments = params
        .get("arguments")
        .ok_or_else(|| "missing arguments".to_string())?;
    let old_name = arguments
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing argument `name`".to_string())?;
    let new_name = arguments
        .get("new_name")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing argument `new_name`".to_string())?;

    let (text, is_error) = match crate::refactor::rename_plan(root, old_name, new_name) {
        Ok(changes) => match crate::refactor::rename_apply(root, &changes, old_name, new_name) {
            Ok(()) => (
                format!(
                    "renamed `{old_name}` to `{new_name}` in {} file(s); reindexed",
                    changes.len()
                ),
                false,
            ),
            Err(error) => (error.to_string(), true),
        },
        Err(error) => (error.to_string(), true),
    };

    Ok(json!({
        "content": [{"type": "text", "text": text}],
        "isError": is_error,
    }))
}

fn call_describe_doc(root: &Path, params: &Value) -> std::result::Result<Value, String> {
    let arguments = params
        .get("arguments")
        .ok_or_else(|| "missing arguments".to_string())?;
    let doc = arguments
        .get("doc")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing argument `doc`".to_string())?;
    let description = arguments
        .get("description")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing argument `description`".to_string())?;

    let (text, is_error) = match docs::format(root, doc, description) {
        Ok(text) => (text, false),
        Err(error) => (error.to_string(), true),
    };

    Ok(json!({
        "content": [{"type": "text", "text": text}],
        "isError": is_error,
    }))
}

fn search_text(root: &Path, query: &str) -> Result<String> {
    let graph = Graph::open_existing(root)?;
    let results = graph.search(&format!("\"{}\"*", query.replace('"', "\"\"")), 20)?;
    if results.is_empty() {
        return Ok(format!("no matches for `{query}`"));
    }
    let mut out = String::new();
    for node in results {
        let _ = writeln!(
            out,
            "- {} ({}) {}:{}",
            node.name,
            node.kind.as_str(),
            node.file_path,
            node.start_line
        );
    }
    Ok(out)
}

enum Direction {
    Callers,
    Callees,
}

fn edges_text(root: &Path, name: &str, direction: &Direction) -> Result<String> {
    let graph = Graph::open_existing(root)?;
    let node = graph
        .find_by_name(name)?
        .ok_or_else(|| Error::SymbolNotFound {
            name: name.to_string(),
        })?;
    let id = node.id.expect("node loaded from storage always has an id");
    let edges = match direction {
        Direction::Callers => graph.callers(id)?,
        Direction::Callees => graph.callees(id)?,
    };

    if edges.is_empty() {
        let label = match direction {
            Direction::Callers => "callers",
            Direction::Callees => "callees",
        };
        return Ok(format!("no {label} found for `{name}`"));
    }

    let mut out = String::new();
    for (other, kind, confidence) in edges {
        let _ = writeln!(
            out,
            "- {} ({}:{}) [{} {}]",
            other.name,
            other.file_path,
            other.start_line,
            kind.as_str(),
            confidence.as_str()
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn indexed_root() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("aag-mcp-test-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.rs"), "fn caller() { helper(); }").unwrap();
        fs::write(dir.join("b.rs"), "fn helper() {}").unwrap();
        crate::bigbang::run(&dir, &crate::bigbang::Options::default()).unwrap();
        dir
    }

    #[test]
    fn initialize_returns_server_info() {
        let response = handle(
            Path::new("."),
            &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}),
        )
        .unwrap();
        assert_eq!(response["result"]["serverInfo"]["name"], "aag");
    }

    #[test]
    fn tools_list_only_shows_explore_by_default() {
        let response = handle(
            Path::new("."),
            &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
        )
        .unwrap();
        let tools = response["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "explore");
    }

    #[test]
    fn every_advertised_tool_is_implemented() {
        assert!(TOOL_SPECS.iter().all(|tool| tool.implemented));
    }

    #[test]
    fn notification_without_id_gets_no_response() {
        let response = handle(
            Path::new("."),
            &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        );
        assert!(response.is_none());
    }

    #[test]
    fn unknown_method_with_id_returns_error() {
        let response = handle(
            Path::new("."),
            &json!({"jsonrpc": "2.0", "id": 1, "method": "bogus"}),
        )
        .unwrap();
        assert!(response["error"].is_object());
    }

    #[test]
    fn explore_tool_call_returns_source_and_callers() {
        let root = indexed_root();
        let response = handle(
            &root,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {"name": "explore", "arguments": {"query": "helper"}},
            }),
        )
        .unwrap();

        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("fn helper"));
        assert!(text.contains("caller"));
        assert_eq!(response["result"]["isError"], false);
    }

    #[test]
    fn cypher_tool_returns_read_only_graph_rows() {
        let root = indexed_root();
        let text =
            cypher_text(&root, "MATCH (n) WHERE n.name = 'helper' RETURN n LIMIT 5").unwrap();
        assert!(text.contains("helper"));
        assert!(!text.contains("caller"));
        assert!(cypher_text(&root, "MATCH (n) DELETE n").is_err());
    }

    #[test]
    fn detect_changes_maps_diff_to_changed_files() {
        let root = indexed_root();
        let text = detect_changes_text(
            &root,
            "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n",
        )
        .unwrap();
        assert!(text.contains("a.rs"));
        assert!(text.contains("affected tests"));
    }

    #[test]
    fn callees_tool_call_reflects_calls_direction() {
        let root = indexed_root();
        let response = handle(
            &root,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {"name": "callees", "arguments": {"name": "caller"}},
            }),
        )
        .unwrap();

        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("helper"));
    }

    #[test]
    fn cypher_tool_is_available_over_mcp() {
        let root = indexed_root();
        let response = handle(
            &root,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {"name": "cypher", "arguments": {"query": "MATCH (n) RETURN n"}},
            }),
        )
        .unwrap();

        assert_eq!(response["result"]["isError"], false);
        assert!(
            response["result"]["content"][0]["text"]
                .as_str()
                .is_some_and(|text| text.contains("helper"))
        );
    }

    #[test]
    fn listed_tools_includes_default_plus_explicitly_enabled() {
        let enabled: HashSet<String> = ["search".to_string(), "impact".to_string()].into();
        let tools = listed_tools(&enabled);
        let names: Vec<&str> = tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();

        assert!(
            names.contains(&"explore"),
            "default-listed tool must stay listed"
        );
        assert!(names.contains(&"search"));
        assert!(names.contains(&"impact"));
        assert_eq!(names.len(), 3);
    }
}
