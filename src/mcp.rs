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
        name: "pdg_query",
        description: "Statement-level dependences inside a file: which lines depend on which, by control or by data. Pass `<file>` or `<file>:<line>` to ask what one line depends on.",
        arg: "target",
        arg_description: "Repo-relative file path, optionally `path:line`.",
        implemented: true,
    },
    ToolSpec {
        name: "taint",
        description: "Source-to-sink flows in a file, followed across calls through the indexed call graph. Syntactic: a finding is a place to look, not a proven vulnerability, and no findings is not evidence of safety. Pass `<file>` or `<file>:<call hops>` (default 2).",
        arg: "file",
        arg_description: "Repo-relative file path, optionally `path:hops`.",
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
        description: "Read-only pattern query over the graph, in a documented subset of Cypher: MATCH and OPTIONAL MATCH patterns (labels, relationship types, `*1..3` hops), WHERE, UNION, RETURN with count/collect/min/max/sum/avg, DISTINCT, ORDER BY, SKIP, LIMIT. Anything outside the subset is an error that names what was expected. See docs/query.md.",
        arg: "query",
        arg_description: "A query in the supported subset, e.g. MATCH (f:Function)-[:CALLS]->(g) WHERE f.file STARTS WITH 'src/' RETURN f.name, g.name LIMIT 20.",
        implemented: true,
    },
    ToolSpec {
        name: "route_map",
        description: "Every HTTP endpoint this repository declares in a contract or serves in code, paired by shape, with the handler that serves it, the code that consumes it, and the mismatches: declared-but-unimplemented and served-but-undeclared.",
        arg: "filter",
        arg_description: "Optional substring to narrow the endpoints listed; empty lists all.",
        implemented: true,
    },
    ToolSpec {
        name: "tool_map",
        description: "Every RPC/MCP tool this repository exposes by name, with the symbol that serves it. Recognized from `server.tool(\"name\", …)`, a `@tool`/`#[tool]` marker, or a `ToolSpec { name: … }` table entry.",
        arg: "filter",
        arg_description: "Optional substring to narrow the tools listed; empty lists all.",
        implemented: true,
    },
    ToolSpec {
        name: "shape_check",
        description: "Compares each declared response shape with the fields its handler actually returns, by dotted field path (`customer.name`), following a body the handler assembled in a local variable. Syntactic: a finding is a place to look, not a proven bug.",
        arg: "endpoint",
        arg_description: "Optional substring to narrow which endpoints are checked; empty checks all.",
        implemented: true,
    },
    ToolSpec {
        name: "api_impact",
        description: "Who is on the other side of one endpoint or tool: the handler that serves it, the code that consumes it, and the blast radius of changing that handler.",
        arg: "target",
        arg_description: "Endpoint name (`GET /pets`), tool name (`TOOL explore`), or a path.",
        implemented: true,
    },
    ToolSpec {
        name: "group_links",
        description: "Cross-repository protocol links across a group: API producer to client, package export to import, event producer to consumer, schema to model, tool definition to invocation. Each graph is read separately and never merged; every link is a name agreeing across an ownership boundary and carries the evidence that produced it.",
        arg: "group",
        arg_description: "Group name, or `all` for every registered workspace.",
        implemented: true,
    },
    ToolSpec {
        name: "graph_diff",
        description: "Compare two graph states — the workspace, a branch, a commit, or a pull request head (`pr/42`) — as `before..after` or a single state against the workspace. Reports symbols added, removed, and moved, edges gained and lost, and which symbols the rest of the code started or stopped depending on. Each ref is indexed once through a detached worktree; your checkout is never touched.",
        arg: "states",
        arg_description: "`main..workspace`, `pr/42`, `v0.1.0..main`, or a single ref compared against the workspace.",
        implemented: true,
    },
    ToolSpec {
        name: "pr_dashboard",
        description: "Every open pull request ranked by what the graph says it reaches: hub symbols touched, blast radius, affected tests it does not change, failing checks, and overlaps with other open PRs. The score comes from a stated rule table, and every point is attributed to a rule.",
        arg: "base",
        arg_description: "Optional base branch to filter by; empty covers all open pull requests.",
        implemented: true,
    },
    ToolSpec {
        name: "pr_conflicts",
        description: "Open pull requests that share a file (a merge conflict on the way) or share a symbol without sharing a file (both merge cleanly and still disagree — the one a diff cannot show).",
        arg: "base",
        arg_description: "Optional base branch to filter by; empty covers all open pull requests.",
        implemented: true,
    },
    ToolSpec {
        name: "db_drift",
        description: "Tables this repository's DDL declares against the tables an ingested live catalog actually has, both directions reported. Ingestion itself is CLI-only (`aag db scan --url`): a connection string passed through a tool call is a credential in a transcript.",
        arg: "path",
        arg_description: "Ignored; the drift report covers the indexed repository.",
        implemented: true,
    },
    ToolSpec {
        name: "memory_save",
        description: "Record work: pass `question`, `answer`, and optionally `nodes` (comma-separated symbols the answer rested on), `outcome` (worked|wrong|open), `correction`, and `revision`. Recording the supporting symbols is what lets a later recall tell you the answer is stale.",
        arg: "question",
        arg_description: "What was asked. Pass `answer` alongside it.",
        implemented: true,
    },
    ToolSpec {
        name: "memory_recall",
        description: "Recall earlier work on a question: what was answered, how it turned out, and what corrected it. Every entry is checked against the current graph and marked `stale` when the symbols it rested on are gone. This is recorded experience, not extracted evidence — where it disagrees with the graph, the graph is right.",
        arg: "question",
        arg_description: "The question to match against remembered work.",
        implemented: true,
    },
    ToolSpec {
        name: "memory_lessons",
        description: "Review candidates derived from repeated outcomes: what kinds of answer held up and what kept being wrong, with the entry ids behind each one and how many are still supported by the graph. A lesson is a pattern in what was recorded, not a fact about the code.",
        arg: "subject",
        arg_description: "Optional symbol substring to narrow the lessons; empty returns all.",
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
        description: "List repositories in a named hierarchical group or the full federation.",
        arg: "group",
        arg_description: "Named group or `all`.",
        implemented: true,
    },
    ToolSpec {
        name: "group_query",
        description: "Query a named repository group and all of its descendants.",
        arg: "query",
        arg_description: "Symbol or natural-language search term.",
        implemented: true,
    },
    ToolSpec {
        name: "group_status",
        description: "Index and manifest status across a named repository group.",
        arg: "group",
        arg_description: "Named group or `all`.",
        implemented: true,
    },
    ToolSpec {
        name: "group_contracts",
        description: "OpenAPI, database, and infrastructure contracts across a named group.",
        arg: "group",
        arg_description: "Named group or `all`.",
        implemented: true,
    },
    ToolSpec {
        name: "group_sync",
        description: "Synchronize every repository graph and manifest in a named group.",
        arg: "group",
        arg_description: "Named group or `all`.",
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

/// Runs the Streamable HTTP transport with default options.
///
/// Kept so an embedder that only wants "serve HTTP on this port" does not have
/// to build [`crate::transport::Options`]; everything else lives there.
///
/// # Errors
/// As [`crate::transport::serve`].
pub fn run_http(root: &Path, port: u16, api_key: Option<&str>) -> Result<()> {
    crate::transport::serve(
        root,
        &crate::transport::Options {
            port,
            api_key: api_key.map(str::to_string),
            ..crate::transport::Options::default()
        },
    )
}

/// Handles one JSON-RPC message against `root`, returning the response, or
/// `None` for a notification. Shared with [`crate::transport`] so both
/// transports answer identically.
#[must_use]
pub fn handle_message(root: &Path, request: &Value) -> Option<Value> {
    handle(root, request)
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
            "protocolVersion": "2025-11-25",
            "capabilities": {
                "tools": {},
                // The graph is one resource that changes under the client, so
                // subscription is the capability that matters here, and the
                // HTTP transport actually delivers on it.
                "resources": {"subscribe": true, "listChanged": false},
            },
            "serverInfo": {"name": "aag", "version": env!("CARGO_PKG_VERSION")},
        })),
        // `ping` has nothing to report, and subscription is per stream rather
        // than per resource: there is one resource, and a client that opened a
        // stream is already receiving its updates.
        "ping" | "resources/subscribe" | "resources/unsubscribe" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": listed_tools(&enabled_tool_names()) })),
        "tools/call" => call_tool(root, params),
        "resources/list" => Ok(json!({ "resources": [graph_resource()] })),
        "resources/read" => read_resource(root, params),
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

/// The URI of the one resource this server publishes: the indexed graph.
pub const GRAPH_RESOURCE_URI: &str = "aag://graph";

fn graph_resource() -> Value {
    json!({
        "uri": GRAPH_RESOURCE_URI,
        "name": "code knowledge graph",
        "description": "Counts and most-connected symbols for the indexed repository. \
                        Changes whenever the index does; a client on an SSE stream is \
                        told when.",
        "mimeType": "application/json",
    })
}

/// Reads the graph resource: what the index currently holds, not a dump of it.
fn read_resource(root: &Path, params: Option<&Value>) -> std::result::Result<Value, String> {
    let uri = params
        .and_then(|params| params.get("uri"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if uri != GRAPH_RESOURCE_URI {
        return Err(format!(
            "unknown resource: {uri}. This server publishes {GRAPH_RESOURCE_URI}."
        ));
    }
    let graph = crate::storage::Graph::open_existing(root).map_err(|error| error.to_string())?;
    let nodes = graph.all_nodes().map_err(|error| error.to_string())?;
    let edges = graph.all_edges().map_err(|error| error.to_string())?.len();
    let files = nodes
        .iter()
        .filter(|node| node.kind == crate::storage::NodeKind::File)
        .count();
    let summary = json!({
        "files": files,
        "symbols": nodes.len() - files,
        "edges": edges,
        // The revision a reader can compare against the one carried by the
        // `notifications/resources/updated` that woke them.
        "revision": crate::watch::revision(),
    });
    Ok(json!({
        "contents": [{
            "uri": GRAPH_RESOURCE_URI,
            "mimeType": "application/json",
            "text": summary.to_string(),
        }]
    }))
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
    if matches!(
        spec.name,
        "group_list" | "group_status" | "group_contracts" | "group_sync"
    ) {
        return json!({
            "name": spec.name,
            "description": spec.description,
            "inputSchema": {
                "type": "object",
                "properties": {"group": {"type": "string", "description": "Named group or `all`."}},
                "required": ["group"]
            }
        });
    }
    if spec.name == "group_query" {
        return json!({
            "name": spec.name,
            "description": spec.description,
            "inputSchema": {
                "type": "object",
                "properties": {
                    "group": {"type": "string", "description": "Named group or `all`."},
                    "query": {"type": "string", "description": "Symbol or natural-language search term."}
                },
                "required": ["group", "query"]
            }
        });
    }
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
    if name == "memory_save" {
        return call_memory_save(root, params);
    }
    if name.starts_with("group_") {
        return call_group(params, name);
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
        "pdg_query" => {
            let (file, line) = arg
                .rsplit_once(':')
                .and_then(|(file, line)| line.parse::<u32>().ok().map(|line| (file, Some(line))))
                .unwrap_or((arg, None));
            crate::flow::format_pdg(&root.join(file), line)
        }
        "taint" => {
            // `<file>` or `<file>:<hops>`, so an agent can widen the search
            // without a second tool.
            let (file, depth) = arg
                .rsplit_once(':')
                .and_then(|(file, depth)| depth.parse::<u32>().ok().map(|depth| (file, depth)))
                .unwrap_or((arg, 2));
            crate::flow::format_taint(&root.join(file), depth)
        }
        "wiki" => write_wiki(root),
        "affected" => affected_text(root, arg),
        "detect_changes" => detect_changes_text(root, arg),
        "cypher" => crate::query::run_json(root, arg),
        "memory_recall" => crate::memory::recall_json(root, arg),
        "memory_lessons" => crate::memory::format_lessons(root, arg),
        "db_drift" => crate::database::format_drift(root),
        "route_map" => crate::api::route_map(root, arg),
        "tool_map" => crate::api::tool_map(root, arg),
        "shape_check" => crate::api::format_shape_check(root, arg),
        "api_impact" => crate::api::impact(root, arg),
        "communities" => communities_text(root, arg),
        "processes" => processes_text(root, arg),
        "neighbors" => neighbors_text(root, arg),
        "shortest_path" => shortest_path_text(root, arg),
        "god_nodes" => god_nodes_text(root, arg),
        "graph_stats" => graph_stats_text(root),
        "graph_diff" => {
            // `before..after`, or a single state compared against the
            // workspace — the common question is "what did this branch do".
            let (before, after) = arg
                .split_once("..")
                .map_or((arg, "workspace"), |(left, right)| (left, right));
            crate::refs::format(
                root,
                &crate::refs::State::parse(before),
                &crate::refs::State::parse(after),
            )
        }
        "pr_dashboard" => crate::pr::dashboard(root, arg),
        "pr_conflicts" => crate::pr::conflicts(root, arg),
        "pr_worktrees" => crate::pr::worktrees(root, arg),
        "list_prs" => crate::pr::list(root, arg),
        "get_pr_impact" => crate::pr::impact(root, arg),
        "triage_prs" => crate::pr::triage(root, arg),
        _ => unreachable!("dispatch only called for implemented tools"),
    }
}

fn call_memory_save(root: &Path, params: &Value) -> std::result::Result<Value, String> {
    let arguments = params
        .get("arguments")
        .ok_or_else(|| "missing arguments".to_string())?;
    let text = |key: &str| {
        arguments
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let question = text("question").ok_or_else(|| "missing argument `question`".to_string())?;
    let answer = text("answer").ok_or_else(|| "missing argument `answer`".to_string())?;
    let record = crate::memory::Record {
        question,
        answer,
        nodes: text("nodes")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect(),
        outcome: text("outcome").unwrap_or_else(|| "open".to_string()),
        correction: text("correction"),
        revision: text("revision"),
    };
    let (text, is_error) = match crate::memory::save(root, &record) {
        Ok(id) => (format!("saved memory entry #{id}"), false),
        Err(error) => (error.to_string(), true),
    };
    Ok(json!({"content": [{"type": "text", "text": text}], "isError": is_error}))
}

fn call_group(params: &Value, name: &str) -> std::result::Result<Value, String> {
    let arguments = params
        .get("arguments")
        .ok_or_else(|| "missing arguments".to_string())?;
    let group = arguments
        .get("group")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing argument `group`".to_string())?;
    let result = match name {
        "group_list" => crate::federation::list_group((group != "all").then_some(group)),
        "group_query" => {
            let query = arguments
                .get("query")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing argument `query`".to_string())?;
            crate::federation::query_group(group, query)
        }
        "group_status" => crate::federation::status_group(group),
        "group_contracts" => crate::federation::contracts_group(group),
        "group_sync" => crate::federation::sync_group(group),
        "group_links" => crate::federation::links_group(group),
        _ => unreachable!("group dispatcher called with non-group tool"),
    };
    let (text, is_error) = match result {
        Ok(text) => (text, false),
        Err(error) => (error.to_string(), true),
    };
    Ok(json!({"content": [{"type": "text", "text": text}], "isError": is_error}))
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
            crate::query::run_json(&root, "MATCH (n) WHERE n.name = 'helper' RETURN n LIMIT 5")
                .unwrap();
        assert!(text.contains("helper"));
        assert!(!text.contains("caller"));
        assert!(crate::query::run_json(&root, "MATCH (n) DELETE n").is_err());
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
