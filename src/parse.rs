//! Tree-sitter based structural parsing.
//!
//! A [`LanguageParser`] turns one file's source text into a [`ParsedFile`]:
//! the symbols it declares (functions/structs/methods) plus *unresolved*
//! cross-references — [`ImportRef`], [`CallRef`], [`InheritRef`].
//!
//! Unresolved means the parser reports what the syntax said and nothing
//! more: which module an import names and under what local name, which
//! receiver a call was made on, which base type a class declared. Turning
//! any of that into an edge pointing at a specific node id, and tagging it
//! `EXTRACTED`/`INFERRED`/`AMBIGUOUS`, belongs to `crate::bindings` and
//! `crate::resolve`. What the parser must not do is *discard* the
//! disambiguating context — a bare callee name with its receiver thrown
//! away cannot be resolved by anything downstream.

use tree_sitter::{Node as TsNode, Parser as TsParser};

use crate::error::{Error, Result};
use crate::storage::{Node, NodeKind};

/// One file's extracted symbols plus unresolved cross-references.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ParsedFile {
    /// Symbols declared directly in this file (not yet inserted, no id).
    pub nodes: Vec<Node>,
    /// Imports as written, split into module source and bound name so
    /// `crate::bindings` can resolve them against the repository layout.
    pub imports: Vec<ImportRef>,
    /// Calls found inside function/method bodies, with their receiver when
    /// the call site had one.
    pub calls: Vec<CallRef>,
    /// `extends`/`implements`/`impl … for …` relations declared in this file.
    pub inherits: Vec<InheritRef>,
    /// Which type each declared method/field belongs to.
    pub members: Vec<MemberRef>,
    /// Local variables whose type the syntax states outright.
    pub locals: Vec<LocalTypeRef>,
    /// HTTP routes this file registers with a web framework.
    pub routes: Vec<RouteRef>,
    /// RPC/MCP tools this file exposes by name.
    pub tools: Vec<ToolRef>,
    /// Outbound HTTP calls this file makes — endpoints it consumes.
    pub consumers: Vec<ConsumerRef>,
    /// Events this file publishes or listens for, by name.
    pub events: Vec<EventRef>,
    /// RPC/MCP tools this file invokes by name.
    pub tool_calls: Vec<ToolCallRef>,
}

/// An event published or listened for by name: `emit('order.created')`,
/// `bus.subscribe('order.created', handle)`.
///
/// The two halves are usually in different files, and across a group of
/// repositories they are usually in different repositories — which is the point
/// of recording them separately rather than as a call.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EventRef {
    /// Event or topic name as written.
    pub name: String,
    /// True for a publisher, false for a listener.
    pub emitted: bool,
    /// Symbol the call sits in, when it sits in one.
    pub owner: Option<String>,
    /// 1-based line.
    pub line: u32,
}

/// An RPC/MCP tool invoked by name: `callTool('search')`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ToolCallRef {
    /// Tool name as written.
    pub name: String,
    /// Symbol the call sits in, when it sits in one.
    pub owner: Option<String>,
    /// 1-based line.
    pub line: u32,
}

/// An RPC or MCP tool a program exposes: `server.tool("search", handler)`,
/// `@mcp.tool()`, or a `ToolSpec { name: "search", … }` table entry.
///
/// A tool is a callable contract in the same sense an HTTP route is — something
/// outside the process invokes it by name — so it is modelled as an endpoint
/// whose method is `TOOL` rather than as a second vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ToolRef {
    /// Tool name as callers pass it.
    pub name: String,
    /// Symbol that serves it, when the definition names one.
    pub handler: Option<String>,
    /// 1-based line of the definition.
    pub line: u32,
    /// Whether an unknown handler should be read as the declaration below this
    /// line, which is what a decorator or attribute means.
    pub attach_below: bool,
}

/// An outbound HTTP call: this code *consumes* an endpoint something else
/// serves. `fetch('/api/pets')`, `axios.get('/api/pets')`, `requests.post(url)`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ConsumerRef {
    /// Uppercase HTTP method.
    pub method: String,
    /// Requested path, with any host stripped.
    pub path: String,
    /// Symbol the call sits in, when it sits in one.
    pub owner: Option<String>,
    /// 1-based line of the call.
    pub line: u32,
}

/// An HTTP route a framework registers in code — `app.get('/pets', list)`,
/// `@app.get("/pets")`, `.route("/pets", get(list))`, `@GetMapping("/pets")`.
///
/// A declared contract (`crate::openapi`) says what the API *should* be;
/// this says what the implementation actually wires up. Keeping both lets
/// the graph show an endpoint that exists in code but not in the spec, and
/// the reverse.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RouteRef {
    /// Uppercase HTTP method.
    pub method: String,
    /// Route path as written.
    pub path: String,
    /// Handler symbol, when the registration names one.
    pub handler: Option<String>,
    /// 1-based line of the registration.
    pub line: u32,
    /// Whether an unknown handler should be read as the declaration below
    /// this line. True for a marker (`#[get("/x")] fn handler`), false for a
    /// registration call — `app.get("/x", (req, res) => …)` is served by an
    /// inline closure, and the next function declared in the file has
    /// nothing to do with it.
    pub attach_below: bool,
}

/// Method names a framework registration can use.
const HTTP_METHODS: &[&str] = &[
    "get", "post", "put", "patch", "delete", "head", "options", "trace",
];

/// Whether `name` names an HTTP method (`get`, `Post`, `DELETE`).
fn http_method(name: &str) -> Option<String> {
    let lowered = name.to_ascii_lowercase();
    HTTP_METHODS
        .contains(&lowered.as_str())
        .then(|| lowered.to_ascii_uppercase())
}

/// The route path inside a registration argument, if the argument is a
/// string literal that looks like a path.
fn route_path(raw: &str) -> Option<String> {
    let trimmed = raw
        .trim()
        .trim_matches(|character| matches!(character, '"' | '\'' | '`'));
    (trimmed.starts_with('/') || trimmed.is_empty()).then(|| {
        if trimmed.is_empty() {
            "/".to_string()
        } else {
            trimmed.to_string()
        }
    })
}

/// Splits an argument list on commas that are not nested inside brackets.
fn split_arguments(args: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut in_string: Option<char> = None;
    for (offset, character) in args.char_indices() {
        if let Some(quote) = in_string {
            if character == quote {
                in_string = None;
            }
            continue;
        }
        match character {
            '"' | '\'' | '`' => in_string = Some(character),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(args[start..offset].trim());
                start = offset + 1;
            }
            _ => {}
        }
    }
    parts.push(args[start..].trim());
    parts.into_iter().filter(|part| !part.is_empty()).collect()
}

/// The symbol an argument names, or `None` when it is an inline closure.
fn handler_name(argument: &str) -> Option<&str> {
    let argument = argument.trim();
    if argument.contains("=>")
        || argument.starts_with("function")
        || argument.starts_with('|')
        || argument.starts_with("async")
        || argument.starts_with("lambda")
    {
        return None;
    }
    last_callable_identifier(argument).filter(|name| !name.is_empty())
}

/// Routes and tools an attribute or a struct-literal table entry declares:
/// `#[get("/x")]`, `#[tool]`, `ToolSpec { name: "x", … }`.
fn collect_contract_markers(
    node: TsNode<'_>,
    source: &str,
    current_owner: Option<&str>,
    out: &mut ParsedFile,
) {
    let line = line_range(node).0;
    if node.kind() == "attribute_item" {
        out.routes
            .extend(annotation_route(text(node, source), line, current_owner));
        out.tools
            .extend(annotation_tool(text(node, source), line, current_owner));
        return;
    }
    if let Some((type_text, body)) = node
        .child_by_field_name("name")
        .zip(node.child_by_field_name("body"))
    {
        out.tools.extend(struct_tool(
            text(type_text, source),
            text(body, source),
            line,
        ));
    }
}

/// Routes, tools, and consumed endpoints a single call site declares. Shared by
/// the dedicated JavaScript and Rust walkers, which see the same three things.
fn collect_contract_calls(
    node: TsNode<'_>,
    source: &str,
    current_owner: Option<&str>,
    out: &mut ParsedFile,
) {
    let Some((function, arguments)) = node
        .child_by_field_name("function")
        .zip(node.child_by_field_name("arguments"))
    else {
        return;
    };
    let (callee, args) = (text(function, source), text(arguments, source));
    let line = line_range(node).0;
    out.routes.extend(registration_route(callee, args, line));
    out.tools.extend(registration_tool(callee, args, line));
    out.consumers
        .extend(consumer_call(callee, args, line, current_owner));
    out.events
        .extend(event_call(callee, args, line, current_owner));
    out.tool_calls
        .extend(tool_call(callee, args, line, current_owner));
}

/// The contents of a quoted argument, or `None` when it is not a literal.
fn quoted_argument(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let quote = trimmed
        .chars()
        .next()
        .filter(|character| matches!(character, '"' | '\'' | '`'))?;
    Some(trimmed.trim_matches(quote).to_string())
}

/// Everything before the final identifier of a callee expression, which is what
/// says whether a `.get(...)` is a server registering a route or a client
/// fetching one.
fn receiver_tail(callee_text: &str) -> String {
    let trimmed = callee_text.trim();
    let tail = trimmed
        .rsplit(['.', ':', ' ', '>'])
        .next()
        .unwrap_or_default();
    let head = trimmed.strip_suffix(tail).unwrap_or_default();
    head.trim_end_matches(['.', ':', '>', '-', ' '])
        .rsplit(['.', ':', ' ', '>', '(', ')'])
        .find(|part| !part.is_empty())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// Receivers that serve requests. `app.get("/x", handler)` is a route.
const SERVER_RECEIVERS: &[&str] = &[
    "app",
    "router",
    "server",
    "route",
    "blueprint",
    "api_router",
];

/// Receivers that make requests. `client.get("/x", config)` is a consumer.
const CLIENT_RECEIVERS: &[&str] = &[
    "axios",
    "client",
    "http",
    "https",
    "requests",
    "session",
    "httpx",
    "urllib",
    "request",
    "superagent",
    "got",
    "api",
    "fetcher",
    "reqwest",
];

/// Callee tails that fetch a URL without naming a method.
const FETCHERS: &[&str] = &["fetch", "urlopen", "get_json", "getjson"];

/// Callee tails that register a tool by name.
const TOOL_REGISTRARS: &[&str] = &[
    "tool",
    "registertool",
    "register_tool",
    "addtool",
    "add_tool",
    "registermethod",
    "register_method",
];

/// An RPC/MCP tool registered by a call: `server.tool("search", handler)`.
fn registration_tool(callee_text: &str, args_text: &str, line: u32) -> Option<ToolRef> {
    let callee = callee_text
        .rsplit(['.', ':', ' ', '>'])
        .next()?
        .trim()
        .to_ascii_lowercase();
    if !TOOL_REGISTRARS.contains(&callee.as_str()) {
        return None;
    }
    let args = args_text
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')');
    let parts = split_arguments(args);
    let name = quoted_argument(parts.first()?)?;
    // A path is a route, whatever the call is named.
    if name.is_empty() || name.starts_with('/') {
        return None;
    }
    Some(ToolRef {
        name,
        handler: parts
            .get(1)
            .and_then(|argument| handler_name(argument))
            .map(str::to_string),
        line,
        attach_below: false,
    })
}

/// A tool declared by a decorator or attribute on its handler: `@mcp.tool()`,
/// `@tool("search")`, `#[tool]`. The name defaults to the handler's own.
fn annotation_tool(raw: &str, line: u32, handler: Option<&str>) -> Option<ToolRef> {
    let raw = raw
        .trim()
        .trim_start_matches(['#', '@', '[', ' '])
        .trim_end_matches([']', ' ']);
    let (head, rest) = raw.split_once('(').unwrap_or((raw, ""));
    let marker = head
        .rsplit(['.', ' ', ':'])
        .next()?
        .trim()
        .to_ascii_lowercase();
    if marker != "tool" && marker != "mcp_tool" {
        return None;
    }
    let name = split_arguments(rest.trim_end_matches([')', ']']))
        .first()
        .and_then(|argument| quoted_argument(argument))
        .filter(|name| !name.is_empty())
        .or_else(|| handler.map(str::to_string))?;
    Some(ToolRef {
        name,
        handler: handler.map(str::to_string),
        line,
        attach_below: true,
    })
}

/// A tool table entry: `ToolSpec { name: "explore", … }`, which is how a Rust
/// or Go server usually declares its surface.
fn struct_tool(type_text: &str, body_text: &str, line: u32) -> Option<ToolRef> {
    let type_name = type_text
        .trim()
        .rsplit("::")
        .next()?
        .trim()
        .to_ascii_lowercase();
    if !type_name.ends_with("tool") && !type_name.ends_with("toolspec") {
        return None;
    }
    let body = body_text
        .trim()
        .trim_start_matches('{')
        .trim_end_matches('}');
    let name = split_arguments(body).into_iter().find_map(|field| {
        let (key, value) = field.split_once(':')?;
        (key.trim() == "name").then(|| quoted_argument(value))?
    })?;
    (!name.is_empty()).then_some(ToolRef {
        name,
        handler: None,
        line,
        attach_below: false,
    })
}

/// Callee tails that publish an event.
const EMITTERS: &[&str] = &[
    "emit",
    "publish",
    "dispatch",
    "send_event",
    "sendevent",
    "post_message",
    "postmessage",
    "produce",
    "notify",
];

/// Callee tails that listen for one.
const LISTENERS: &[&str] = &[
    "on",
    "once",
    "addeventlistener",
    "addlistener",
    "subscribe",
    "consume",
    "handle_event",
    "handleevent",
];

/// Callee tails that invoke a tool by name.
const TOOL_CALLERS: &[&str] = &[
    "call_tool",
    "calltool",
    "invoke_tool",
    "invoketool",
    "run_tool",
    "runtool",
    "use_tool",
    "usetool",
];

/// An event published or listened for at this call site.
///
/// The name has to be a literal: an event whose name is computed is a link this
/// cannot make, and guessing one would pair unrelated producers and consumers.
fn event_call(
    callee_text: &str,
    args_text: &str,
    line: u32,
    owner: Option<&str>,
) -> Option<EventRef> {
    let callee = callee_text
        .rsplit(['.', ':', ' ', '>'])
        .next()?
        .trim()
        .to_ascii_lowercase();
    let emitted = if EMITTERS.contains(&callee.as_str()) {
        true
    } else if LISTENERS.contains(&callee.as_str()) {
        false
    } else {
        return None;
    };
    let args = args_text
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')');
    let name = quoted_argument(split_arguments(args).first()?)?;
    // A path is an HTTP route; a bare identifier is not an event name.
    if name.is_empty() || name.starts_with('/') {
        return None;
    }
    Some(EventRef {
        name,
        emitted,
        owner: owner.map(str::to_string),
        line,
    })
}

/// A tool invoked by name at this call site.
fn tool_call(
    callee_text: &str,
    args_text: &str,
    line: u32,
    owner: Option<&str>,
) -> Option<ToolCallRef> {
    let callee = callee_text
        .rsplit(['.', ':', ' ', '>'])
        .next()?
        .trim()
        .to_ascii_lowercase();
    if !TOOL_CALLERS.contains(&callee.as_str()) {
        return None;
    }
    let args = args_text
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')');
    let name = quoted_argument(split_arguments(args).first()?)?;
    (!name.is_empty()).then_some(ToolCallRef {
        name,
        owner: owner.map(str::to_string),
        line,
    })
}

/// An outbound HTTP call, told apart from a route registration by its receiver
/// first and its argument count second: `app.get("/x", handler)` serves, while
/// `client.get("/x")` and `fetch("/x", {method: "POST"})` consume.
fn consumer_call(
    callee_text: &str,
    args_text: &str,
    line: u32,
    owner: Option<&str>,
) -> Option<ConsumerRef> {
    let callee = callee_text.rsplit(['.', ':', ' ', '>']).next()?.trim();
    let receiver = receiver_tail(callee_text);
    if SERVER_RECEIVERS.contains(&receiver.as_str()) {
        return None;
    }
    let args = args_text
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')');
    let parts = split_arguments(args);
    let path = request_path(&quoted_argument(parts.first()?)?)?;
    let lowered = callee.to_ascii_lowercase();
    let method = if let Some(method) = http_method(callee) {
        // Two arguments are a route registration unless the receiver says
        // otherwise — that ambiguity is why the receiver is checked first.
        if parts.len() > 1 && !CLIENT_RECEIVERS.contains(&receiver.as_str()) {
            return None;
        }
        method
    } else if FETCHERS.contains(&lowered.as_str()) {
        parts
            .get(1)
            .and_then(|options| method_option(options))
            .unwrap_or_else(|| "GET".to_string())
    } else {
        return None;
    };
    Some(ConsumerRef {
        method,
        path,
        owner: owner.map(str::to_string),
        line,
    })
}

/// The path part of a request target: `/pets` as written, or the path of an
/// absolute URL. Anything else — a variable, a template with no leading path —
/// is not a target this can name, and is skipped rather than guessed.
fn request_path(target: &str) -> Option<String> {
    if target.starts_with('/') {
        return Some(target.to_string());
    }
    let rest = target
        .strip_prefix("http://")
        .or_else(|| target.strip_prefix("https://"))?;
    let (_, path) = rest.split_once('/')?;
    Some(format!("/{path}"))
}

/// The method inside a `fetch` options object: `{ method: "POST" }`.
fn method_option(options: &str) -> Option<String> {
    let (_, rest) = options.split_once("method")?;
    let trimmed = rest.trim_start();
    let value = trimmed
        .strip_prefix(':')
        .or_else(|| trimmed.strip_prefix('='))?;
    http_method(&quoted_argument(value.split(',').next()?)?)
}

/// A framework route registered by a call: `app.get("/pets", list)` in the
/// Express family, or axum's `.route("/pets", get(list))`.
///
/// Two arguments are required for the method-name form, because a one-argument
/// `client.get("/pets")` is an HTTP *client* call, not a route.
fn registration_route(callee_text: &str, args_text: &str, line: u32) -> Option<RouteRef> {
    let callee = callee_text.rsplit(['.', ':', ' ', '>']).next()?.trim();
    let args = args_text
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')');
    let parts = split_arguments(args);
    let path = route_path(parts.first()?)?;
    let (method, handler) = if let Some(method) = http_method(callee) {
        if parts.len() < 2 {
            return None;
        }
        (
            method,
            parts.get(1).and_then(|argument| handler_name(argument)),
        )
    } else if callee == "route" {
        let (inner_head, inner_rest) = parts.get(1)?.split_once('(')?;
        let method = http_method(inner_head.trim().rsplit(['.', ':']).next()?.trim())?;
        (method, handler_name(inner_rest.trim_end_matches(')')))
    } else {
        return None;
    };
    Some(RouteRef {
        method,
        path,
        handler: handler.map(str::to_string),
        line,
        attach_below: false,
    })
}

/// A framework route declared by a decorator, annotation, or attribute:
/// `@app.get("/pets")`, `@GetMapping("/pets")`, `[HttpGet("/pets")]`,
/// `#[get("/pets")]`.
fn annotation_route(raw: &str, line: u32, handler: Option<&str>) -> Option<RouteRef> {
    let raw = raw
        .trim()
        .trim_start_matches(['#', '@', '[', ' '])
        .trim_end_matches([']', ' ']);
    let (head, rest) = raw.split_once('(')?;
    let path = route_path(split_arguments(rest.trim_end_matches([')', ']'])).first()?)?;
    let method = marker_method(head.rsplit(['.', ' ', ':']).next()?.trim())?;
    Some(RouteRef {
        method,
        path,
        handler: handler.map(str::to_string),
        line,
        attach_below: true,
    })
}

/// HTTP method named by a decorator/annotation/attribute marker, across the
/// naming styles the frameworks use (`get`, `HttpGet`, `GetMapping`), with
/// the method-agnostic markers (`route`, `RequestMapping`, `Path`) reading
/// as GET.
fn marker_method(name: &str) -> Option<String> {
    let lowered = name.to_ascii_lowercase();
    let stem = lowered
        .trim_start_matches("http")
        .trim_end_matches("mapping")
        .trim_end_matches("_route");
    http_method(stem)
        .or_else(|| matches!(stem, "route" | "request" | "path" | "api").then(|| "GET".to_string()))
}

/// Attaches routes and tools whose handler is unknown to the declaration that
/// follows them, which is what an attribute or annotation means (`#[get("/x")]
/// fn handler`). Runs once per file, after every declaration is known.
fn attach_route_handlers(out: &mut ParsedFile) {
    let mut declarations: Vec<(u32, String)> = out
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, NodeKind::Function | NodeKind::Method))
        .map(|node| (node.start_line, node.name.clone()))
        .collect();
    declarations.sort_unstable();
    let below = |line: u32| {
        declarations
            .iter()
            .find(|(declared, _)| *declared >= line)
            .map(|(_, name)| name.clone())
    };
    for route in &mut out.routes {
        if route.handler.is_some() || !route.attach_below {
            continue;
        }
        route.handler = below(route.line);
    }
    for tool in &mut out.tools {
        if tool.handler.is_none() && tool.attach_below {
            tool.handler = below(tool.line);
        }
    }
    // A tool table entry names no handler, and the entry is not inside one
    // either; the handler is whatever dispatch routes the name to, which only
    // resolution can see.
    out.routes.sort_unstable();
    out.routes.dedup();
    out.tools.sort_unstable();
    out.tools.dedup();
    out.consumers.sort_unstable();
    out.consumers.dedup();
    out.events.sort_unstable();
    out.events.dedup();
    out.tool_calls.sort_unstable();
    out.tool_calls.dedup();
}

/// A method or field and the type that declares it. Without this, a call on
/// a receiver can only be narrowed by file; with it, `store.get()` resolves
/// to the `get` that `Store` actually declares.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MemberRef {
    /// Declaring type.
    pub owner_type: String,
    /// Member name.
    pub member: String,
}

/// A local binding whose type is stated by a constructor call or an
/// annotation — `const s = new Store()`, `let s: Store`, `s := Store{}`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LocalTypeRef {
    /// Function or method the binding lives in.
    pub scope: String,
    /// Variable name.
    pub name: String,
    /// Type it was given.
    pub type_name: String,
}

/// One imported name, as written at the import site.
///
/// The split matters: `source` says *where* to look (a module path the
/// language's own conventions map onto a file), `name` says *what* was taken
/// from there, and `alias` says what it is called locally. Name-only
/// resolution throws all three away and matches on `name` repo-wide.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ImportRef {
    /// Module path/specifier exactly as written (`./helper`, `crate::sync`,
    /// `os.path`, `github.com/acme/app/internal/store`).
    pub source: String,
    /// Member taken from `source`, when the syntax names one.
    pub name: Option<String>,
    /// Local binding when it differs from `name` (`as` clauses).
    pub alias: Option<String>,
    /// Wildcard import (`use x::*`, `from x import *`) — binds everything.
    pub glob: bool,
    /// The binding names a module rather than a symbol (`import * as ns`,
    /// Go's `import "fmt"`, Python's `import os`).
    pub namespace: bool,
}

impl ImportRef {
    /// A bare module import with no named member.
    #[must_use]
    pub fn module(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            namespace: true,
            ..Self::default()
        }
    }

    /// The name this import binds in the importing file's scope.
    #[must_use]
    pub fn local_name(&self) -> Option<&str> {
        if let Some(alias) = &self.alias {
            return Some(alias);
        }
        if let Some(name) = &self.name {
            return Some(name);
        }
        if self.glob {
            return None;
        }
        self.source
            .rsplit([':', '/', '.'])
            .next()
            .filter(|segment| !segment.is_empty())
    }
}

/// One call site: who made the call, what was called, and what it was
/// called on. The receiver is the disambiguator name-only matching lacks —
/// `store.get()` and `cache.get()` are different edges.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CallRef {
    /// Enclosing function/method the call was made from.
    pub caller: String,
    /// Type declaring `caller`, when it is a method — what `self`/`this`
    /// refers to at this call site.
    pub caller_type: Option<String>,
    /// Called name, without its receiver or module qualifier.
    pub callee: String,
    /// Receiver or module qualifier as written (`self`, `this`, `store`,
    /// `crate::sync`), or `None` for a bare call.
    pub receiver: Option<String>,
}

/// One declared type relation (`class A extends B`, `impl Trait for Type`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct InheritRef {
    /// The declaring type.
    pub child: String,
    /// The base type or interface named by the declaration.
    pub parent: String,
    /// Interface/trait conformance rather than base-class inheritance.
    pub implements: bool,
}

/// A language-specific structural parser.
pub trait LanguageParser {
    /// File extensions (without the dot) this parser handles, e.g. `["rs"]`.
    fn extensions(&self) -> &'static [&'static str];

    /// Parses `source` (from `file_path`, used only to tag emitted nodes).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Parse`] if the source cannot be parsed.
    fn parse(&self, file_path: &str, source: &str) -> Result<ParsedFile>;
}

/// Picks a registered parser by file extension and runs it.
///
/// Returns `Ok(None)` for files with no registered parser — callers should
/// skip these rather than treat them as an error.
///
/// # Errors
///
/// Returns [`Error::Parse`] if the matched parser fails.
pub fn parse_file(file_path: &str, source: &str) -> Result<Option<ParsedFile>> {
    let extension = file_path.rsplit('.').next().unwrap_or_default();
    let parsers: [&dyn LanguageParser; 2] = [&RustParser, &JavaScriptParser];

    for parser in parsers {
        if parser.extensions().contains(&extension) {
            return parser.parse(file_path, source).map(Some);
        }
    }
    let Some(language) = polyglot_language(file_path) else {
        return Ok(None);
    };
    parse_polyglot(file_path, source, language).map(Some)
}

/// Whether a path has one of the supported source-language extensions.
#[must_use]
pub fn supports_file(file_path: &str) -> bool {
    matches!(
        file_path.rsplit('.').next().unwrap_or_default(),
        "rs" | "js" | "jsx"
    ) || polyglot_language(file_path).is_some()
}

/// Every pack-backed language, keyed by extension. With the native Rust and
/// JavaScript frontends this is the whole language surface.
///
/// A language is listed here only once a test shows the pack extracts
/// declarations from it — the list is a claim about what works, not about which
/// grammars exist. See the `pack_languages_extract_declarations` test, which is
/// the validation half of P2.13.
fn polyglot_language(file_path: &str) -> Option<&'static str> {
    let extension = file_path.rsplit('.').next()?.to_ascii_lowercase();
    match extension.as_str() {
        // Component frameworks: one file holding markup, style, and script.
        "vue" => Some("vue"),
        "svelte" => Some("svelte"),
        "astro" => Some("astro"),
        // Systems and scientific languages.
        "zig" => Some("zig"),
        "jl" => Some("julia"),
        "f" | "f90" | "f95" | "f03" | "for" => Some("fortran"),
        "nim" => Some("nim"),
        "hs" => Some("haskell"),
        "ml" | "mli" => Some("ocaml"),
        "erl" | "hrl" => Some("erlang"),
        "clj" | "cljs" | "cljc" => Some("clojure"),
        // JVM and .NET adjacent.
        "groovy" | "gradle" => Some("groovy"),
        "cls" | "trigger" => Some("apex"),
        // Hardware description.
        "v" | "vh" => Some("verilog"),
        "sv" | "svh" => Some("systemverilog"),
        // Scripting and legacy.
        "ps1" | "psm1" | "psd1" => Some("powershell"),
        "pas" | "pp" | "dpr" => Some("pascal"),
        "pl" | "pm" => Some("perl"),
        "sol" => Some("solidity"),
        "ts" => Some("typescript"),
        "tsx" => Some("tsx"),
        "py" | "pyw" => Some("python"),
        "java" => Some("java"),
        "c" | "h" => Some("c"),
        "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" => Some("cpp"),
        "cs" => Some("csharp"),
        "go" => Some("go"),
        "php" | "phtml" => Some("php"),
        "rb" | "rake" => Some("ruby"),
        "swift" => Some("swift"),
        "kt" | "kts" => Some("kotlin"),
        "dart" => Some("dart"),
        "scala" | "sc" => Some("scala"),
        "sh" | "bash" | "zsh" => Some("bash"),
        "lua" => Some("lua"),
        "r" => Some("r"),
        "ex" | "exs" => Some("elixir"),
        "m" | "mm" => Some("objc"),
        _ => None,
    }
}

/// Single-file component languages, where the code lives inside a `<script>`
/// block and the rest of the file is markup.
const EMBEDDED_SCRIPT: &[&str] = &["vue", "svelte", "astro"];

/// Parses a component file: the script block goes to the JavaScript frontend,
/// and the markup gives the component itself and what it renders.
///
/// A single-file component *is* a declaration — the file is the component — so
/// it gets one node named after the file, and every component element in the
/// template becomes a call from it to the component that element names. That is
/// the edge nothing else can supply: a globally registered component is used
/// without an import, so without reading the markup the parent and the child
/// look unrelated. Line numbers from the script are shifted so a symbol still
/// points at the right line of the component.
fn parse_embedded_script(file_path: &str, source: &str, language: &str) -> Result<ParsedFile> {
    let mut parser =
        tree_sitter_language_pack::get_parser(language).map_err(|error| Error::Parse {
            file: file_path.to_string(),
            reason: error.to_string(),
        })?;
    let Some(tree) = parser.parse(source) else {
        return Ok(ParsedFile::default());
    };
    let mut blocks = Vec::new();
    collect_script_blocks(&tree.root_node(), source, &mut blocks);
    let mut out = ParsedFile::default();
    let component = component_name(file_path);
    out.nodes.push(Node {
        id: None,
        kind: NodeKind::Component,
        name: component.clone(),
        file_path: file_path.to_string(),
        start_line: 1,
        end_line: u32::try_from(source.lines().count().max(1)).unwrap_or(u32::MAX),
        description: None,
    });
    let mut rendered = Vec::new();
    collect_rendered_components(&tree.root_node(), source, &mut rendered);
    rendered.sort_unstable();
    rendered.dedup();
    for child in rendered {
        if child == component {
            continue;
        }
        out.calls.push(CallRef {
            caller: component.clone(),
            caller_type: None,
            callee: child,
            receiver: None,
        });
    }
    for (script, offset) in blocks {
        let Ok(mut parsed) = JavaScriptParser.parse(file_path, script) else {
            continue;
        };
        for node in &mut parsed.nodes {
            node.start_line += offset;
            node.end_line += offset;
        }
        for route in &mut parsed.routes {
            route.line += offset;
        }
        out.nodes.append(&mut parsed.nodes);
        out.imports.append(&mut parsed.imports);
        out.calls.append(&mut parsed.calls);
        out.inherits.append(&mut parsed.inherits);
        out.members.append(&mut parsed.members);
        out.locals.append(&mut parsed.locals);
        out.routes.append(&mut parsed.routes);
        out.tools.append(&mut parsed.tools);
        out.consumers.append(&mut parsed.consumers);
        out.events.append(&mut parsed.events);
        out.tool_calls.append(&mut parsed.tool_calls);
    }
    Ok(out)
}

/// The component a single-file component declares: its file stem, which is the
/// name every framework in this family uses to refer to it.
fn component_name(file_path: &str) -> String {
    file_path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(file_path)
        .rsplit_once('.')
        .map_or(file_path, |(stem, _)| stem)
        .to_string()
}

/// Component elements used in a template.
///
/// A component is told from an HTML element by its name: `PascalCase` or
/// `kebab-case-with-a-dash` is a component, `div` is not. That is the same rule
/// Vue, Svelte, and Astro themselves use, and it is a rule about the name — an
/// element whose name is built at runtime (`<component :is="x">`) names nothing
/// and is skipped.
fn collect_rendered_components(
    node: &tree_sitter_language_pack::Node,
    source: &str,
    out: &mut Vec<String>,
) {
    if matches!(node.kind().as_str(), "tag_name" | "element_name") {
        let range = node.byte_range();
        if let Some(name) = source
            .get(range.start..range.end)
            .filter(|name| is_component_element(name))
        {
            out.push(name.to_string());
        }
    }
    for index in 0..u32::try_from(node.named_child_count()).unwrap_or(u32::MAX) {
        if let Some(child) = node.named_child(index) {
            collect_rendered_components(&child, source, out);
        }
    }
}

/// Reserved names that look like components but are framework control flow.
const NOT_COMPONENTS: &[&str] = &[
    "template",
    "component",
    "slot",
    "script",
    "style",
    "svelte:self",
    "svelte:component",
    "svelte:fragment",
    "svelte:window",
    "svelte:body",
    "svelte:head",
    "svelte:element",
    "svelte:options",
];

fn is_component_element(name: &str) -> bool {
    if NOT_COMPONENTS.contains(&name) || name.contains(':') {
        return false;
    }
    let pascal = name.chars().next().is_some_and(char::is_uppercase);
    let custom = name.contains('-') && !name.starts_with('-');
    pascal || custom
}

/// `(script text, line offset)` for every script block in a component file.
fn collect_script_blocks<'a>(
    node: &tree_sitter_language_pack::Node,
    source: &'a str,
    out: &mut Vec<(&'a str, u32)>,
) {
    if matches!(node.kind().as_str(), "raw_text" | "frontmatter_js_block") {
        let range = node.byte_range();
        if let Some(text) = source.get(range.start..range.end) {
            out.push((
                text,
                u32::try_from(node.start_position().row).unwrap_or_default(),
            ));
        }
        return;
    }
    for index in 0..u32::try_from(node.named_child_count()).unwrap_or(u32::MAX) {
        if let Some(child) = node.named_child(index) {
            collect_script_blocks(&child, source, out);
        }
    }
}

/// Control flow that takes a parenthesised head and a brace, exactly like a
/// method declaration does.
const GROOVY_KEYWORDS: &[&str] = &[
    "if",
    "else",
    "for",
    "while",
    "switch",
    "catch",
    "synchronized",
    "try",
    "do",
    "return",
];

/// A Groovy method declaration, whatever it is spelled with.
///
/// The grammar is a loose command soup, but a declaration has a shape inside
/// it: somewhere in the command sits a `func` — an identifier with an argument
/// list — followed by a brace. That is true of `def greet() { … }`,
/// `String greet(who) { … }`, `static int add(a, b) { … }`, and
/// `@Override void run() { … }` alike, so the type and the modifiers stop
/// mattering. The brace is what separates a declaration from a call: `greet(1)`
/// has the same `func` and no body.
fn groovy_method(node: &tree_sitter_language_pack::Node, source: &str) -> Option<String> {
    let block = (0..u32::try_from(node.named_child_count()).ok()?)
        .filter_map(|index| node.named_child(index))
        .find(|child| child.kind() == "block")?;
    let func = (0..u32::try_from(block.named_child_count()).ok()?)
        .filter_map(|index| block.named_child(index))
        .filter(|child| child.kind() == "unit")
        .find_map(|unit| {
            (0..u32::try_from(unit.named_child_count()).ok()?)
                .filter_map(|index| unit.named_child(index))
                .find(|child| child.kind() == "func")
        })?;
    if !source
        .get(func.end_byte()..)
        .is_some_and(|rest| rest.trim_start().starts_with('{'))
    {
        return None;
    }
    let name = (0..u32::try_from(func.named_child_count()).ok()?)
        .filter_map(|index| func.named_child(index))
        .find(|child| child.kind() == "identifier")
        .and_then(|identifier| {
            let range = identifier.byte_range();
            source.get(range.start..range.end)
        })?;
    (!name.is_empty() && !GROOVY_KEYWORDS.contains(&name)).then(|| name.to_string())
}

/// Groovy's grammar is a loose command soup, so declarations are recovered
/// from the shapes inside it: a `func` with a brace after it is a method
/// (`groovy_method`), and `class`/`interface`/`trait` followed by a word is a
/// type. See the limits in `docs/parse.md`.
fn collect_groovy_declarations(
    node: &tree_sitter_language_pack::Node,
    source: &str,
    file_path: &str,
    out: &mut ParsedFile,
    owners: &mut Vec<(String, usize, usize)>,
) {
    if node.kind() == "command"
        && let Some(name) = groovy_method(node, source)
        && !out.nodes.iter().any(|existing| existing.name == name)
    {
        out.nodes.push(Node {
            id: None,
            kind: NodeKind::Function,
            name: name.clone(),
            file_path: file_path.to_string(),
            start_line: u32::try_from(node.start_position().row + 1).unwrap_or(u32::MAX),
            end_line: u32::try_from(node.end_position().row + 1).unwrap_or(u32::MAX),
            description: None,
        });
        owners.push((name, node.start_byte(), node.end_byte()));
    }
    if node.kind() == "command" {
        let words: Vec<&str> = (0..u32::try_from(node.named_child_count()).unwrap_or(u32::MAX))
            .filter_map(|index| node.named_child(index))
            .filter_map(|child| {
                let range = child.byte_range();
                source.get(range.start..range.end)
            })
            .collect();
        if let [keyword, rest, ..] = words.as_slice()
            && matches!(keyword.trim(), "def" | "class" | "interface" | "trait")
        {
            let name = rest
                .trim()
                .split(|character: char| !character.is_alphanumeric() && character != '_')
                .find(|part| !part.is_empty())
                .unwrap_or_default();
            let kind = match keyword.trim() {
                "class" => NodeKind::Struct,
                "interface" | "trait" => NodeKind::Interface,
                _ => NodeKind::Function,
            };
            if !name.is_empty() && !out.nodes.iter().any(|existing| existing.name == name) {
                out.nodes.push(Node {
                    id: None,
                    kind,
                    name: name.to_string(),
                    file_path: file_path.to_string(),
                    start_line: u32::try_from(node.start_position().row + 1).unwrap_or(u32::MAX),
                    end_line: u32::try_from(node.end_position().row + 1).unwrap_or(u32::MAX),
                    description: None,
                });
                if kind == NodeKind::Function {
                    owners.push((name.to_string(), node.start_byte(), node.end_byte()));
                }
            }
        }
    }
    for index in 0..u32::try_from(node.named_child_count()).unwrap_or(u32::MAX) {
        if let Some(child) = node.named_child(index) {
            collect_groovy_declarations(&child, source, file_path, out, owners);
        }
    }
}

/// Clojure declares with a form, not a node kind: `(defn greet [] …)` is a list
/// whose first symbol says what the second one is.
fn collect_clojure_declarations(
    node: &tree_sitter_language_pack::Node,
    source: &str,
    file_path: &str,
    out: &mut ParsedFile,
    owners: &mut Vec<(String, usize, usize)>,
) {
    if node.kind() == "list_lit" {
        let symbols: Vec<&str> = (0..u32::try_from(node.named_child_count()).unwrap_or(u32::MAX))
            .filter_map(|index| node.named_child(index))
            .filter(|child| child.kind() == "sym_lit")
            .filter_map(|child| {
                let range = child.byte_range();
                source.get(range.start..range.end)
            })
            .collect();
        if let [form, name, ..] = symbols.as_slice()
            && matches!(
                *form,
                "defn" | "defn-" | "defmacro" | "def" | "defrecord" | "defprotocol"
            )
        {
            let kind = match *form {
                "defrecord" => NodeKind::Struct,
                "defprotocol" => NodeKind::Interface,
                _ => NodeKind::Function,
            };
            if !out.nodes.iter().any(|existing| existing.name == *name) {
                out.nodes.push(Node {
                    id: None,
                    kind,
                    name: (*name).to_string(),
                    file_path: file_path.to_string(),
                    start_line: u32::try_from(node.start_position().row + 1).unwrap_or(u32::MAX),
                    end_line: u32::try_from(node.end_position().row + 1).unwrap_or(u32::MAX),
                    description: None,
                });
                if kind == NodeKind::Function {
                    owners.push(((*name).to_string(), node.start_byte(), node.end_byte()));
                }
            }
        }
    }
    for index in 0..u32::try_from(node.named_child_count()).unwrap_or(u32::MAX) {
        if let Some(child) = node.named_child(index) {
            collect_clojure_declarations(&child, source, file_path, out, owners);
        }
    }
}

#[allow(clippy::items_after_statements)]
fn parse_polyglot(file_path: &str, source: &str, language: &str) -> Result<ParsedFile> {
    if EMBEDDED_SCRIPT.contains(&language) {
        return parse_embedded_script(file_path, source, language);
    }
    let mut out = ParsedFile::default();
    let mut owners = Vec::new();
    append_pack_symbols(file_path, source, language, &mut out, &mut owners)?;

    // One parse for every AST pass: declarations, heritage, imports, type
    // ranges, typed locals, routes, and calls all read the same tree.
    let mut parser =
        tree_sitter_language_pack::get_parser(language).map_err(|error| Error::Parse {
            file: file_path.to_string(),
            reason: error.to_string(),
        })?;
    let tree = parser.parse(source).ok_or_else(|| Error::Parse {
        file: file_path.to_string(),
        reason: "tree-sitter returned no tree".to_string(),
    })?;
    let root = tree.root_node();

    collect_ast_declarations(&root, source, file_path, &mut out, &mut owners);
    if language == "groovy" {
        collect_groovy_declarations(&root, source, file_path, &mut out, &mut owners);
    }
    if language == "clojure" {
        collect_clojure_declarations(&root, source, file_path, &mut out, &mut owners);
    }
    collect_ast_inheritance(&root, source, &mut out.inherits);
    collect_ast_imports(&root, source, &mut out.imports);
    out.inherits.sort_unstable();
    out.inherits.dedup();

    let types = collect_type_ranges(&root, source);
    out.members = owners
        .iter()
        .filter_map(|(name, start, end)| {
            enclosing_type(&types, *start, *end).map(|owner_type| MemberRef {
                owner_type: owner_type.to_string(),
                member: name.clone(),
            })
        })
        .collect();
    out.members.sort_unstable();
    out.members.dedup();
    collect_ast_locals(&root, source, &owners, &mut out.locals);
    out.locals.sort_unstable();
    out.locals.dedup();
    out.calls = collect_polyglot_calls_from(&root, source, &owners, &types);
    collect_ast_routes(&root, source, &owners, &mut out);
    attach_route_handlers(&mut out);
    Ok(out)
}

/// Declarations the language pack reports directly, before AAG's own AST
/// passes fill in what the pack's structure model does not cover.
fn append_pack_symbols(
    file_path: &str,
    source: &str,
    language: &str,
    out: &mut ParsedFile,
    owners: &mut Vec<(String, usize, usize)>,
) -> Result<()> {
    use tree_sitter_language_pack::{ProcessConfig, StructureItem, StructureKind, SymbolKind};

    fn append_structure(
        item: &StructureItem,
        file_path: &str,
        out: &mut ParsedFile,
        owners: &mut Vec<(String, usize, usize)>,
    ) {
        if let Some(name) = &item.name {
            let kind = match item.kind {
                StructureKind::Function => NodeKind::Function,
                StructureKind::Method => NodeKind::Method,
                StructureKind::Interface | StructureKind::Trait => NodeKind::Interface,
                StructureKind::Class
                | StructureKind::Struct
                | StructureKind::Enum
                | StructureKind::Impl => NodeKind::Struct,
                StructureKind::Module | StructureKind::Namespace | StructureKind::Other(_) => {
                    NodeKind::Interface
                }
            };
            out.nodes.push(Node {
                id: None,
                kind,
                name: name.clone(),
                file_path: file_path.to_string(),
                start_line: u32::try_from(item.span.start_line + 1).unwrap_or(u32::MAX),
                end_line: u32::try_from(item.span.end_line + 1).unwrap_or(u32::MAX),
                description: item.signature.clone().or_else(|| item.doc_comment.clone()),
            });
            if matches!(kind, NodeKind::Function | NodeKind::Method) {
                owners.push((name.clone(), item.span.start_byte, item.span.end_byte));
            }
        }
        for child in &item.children {
            append_structure(child, file_path, out, owners);
        }
    }

    let processed = tree_sitter_language_pack::process(source, &ProcessConfig::new(language).all())
        .map_err(|error| Error::Parse {
            file: file_path.to_string(),
            reason: error.to_string(),
        })?;
    for item in &processed.structure {
        append_structure(item, file_path, out, owners);
    }
    for symbol in &processed.symbols {
        if out.nodes.iter().any(|node| node.name == symbol.name) {
            continue;
        }
        let kind = match symbol.kind {
            SymbolKind::Function => NodeKind::Function,
            SymbolKind::Class | SymbolKind::Type | SymbolKind::Enum => NodeKind::Struct,
            SymbolKind::Interface | SymbolKind::Module | SymbolKind::Other(_) => {
                NodeKind::Interface
            }
            SymbolKind::Variable | SymbolKind::Constant => continue,
        };
        out.nodes.push(Node {
            id: None,
            kind,
            name: symbol.name.clone(),
            file_path: file_path.to_string(),
            start_line: u32::try_from(symbol.span.start_line + 1).unwrap_or(u32::MAX),
            end_line: u32::try_from(symbol.span.end_line + 1).unwrap_or(u32::MAX),
            description: symbol
                .type_annotation
                .clone()
                .or_else(|| symbol.doc.clone()),
        });
        if kind == NodeKind::Function {
            owners.push((
                symbol.name.clone(),
                symbol.span.start_byte,
                symbol.span.end_byte,
            ));
        }
    }
    Ok(())
}

/// Byte ranges of every type declaration, so a function can be attributed to
/// the type that encloses it without each grammar needing its own rule.
fn collect_type_ranges(
    node: &tree_sitter_language_pack::Node,
    source: &str,
) -> Vec<(String, usize, usize)> {
    let mut ranges = Vec::new();
    collect_type_ranges_into(node, source, &mut ranges);
    ranges
}

fn collect_type_ranges_into(
    node: &tree_sitter_language_pack::Node,
    source: &str,
    out: &mut Vec<(String, usize, usize)>,
) {
    if TYPE_DECLARATION_KINDS.contains(&node.kind().as_str())
        && let Some(name) = node
            .child_by_field_name("name")
            .and_then(|name| declaration_identifier(&name, source))
    {
        out.push((name.to_string(), node.start_byte(), node.end_byte()));
    }
    for child in named_children(node) {
        collect_type_ranges_into(&child, source, out);
    }
}

/// Innermost type declaration containing `[start, end]`.
fn enclosing_type(ranges: &[(String, usize, usize)], start: usize, end: usize) -> Option<&str> {
    ranges
        .iter()
        .filter(|(_, open, close)| *open <= start && end <= *close)
        .min_by_key(|(_, open, close)| close - open)
        .map(|(name, _, _)| name.as_str())
}

/// Locals whose type the syntax states: a constructor call, a composite
/// literal, or an explicit annotation. A value returned by an arbitrary
/// function is left untyped rather than guessed at.
fn collect_ast_locals(
    node: &tree_sitter_language_pack::Node,
    source: &str,
    owners: &[(String, usize, usize)],
    out: &mut Vec<LocalTypeRef>,
) {
    if let Some((name, type_name)) = ast_local_binding(node, source)
        && let Some(scope) = innermost_owner(owners, node.start_byte(), node.end_byte())
    {
        out.push(LocalTypeRef {
            scope: scope.to_string(),
            name,
            type_name,
        });
    }
    for child in named_children(node) {
        collect_ast_locals(&child, source, owners, out);
    }
}

fn ast_local_binding(
    node: &tree_sitter_language_pack::Node,
    source: &str,
) -> Option<(String, String)> {
    let (name_node, value_node) = match node.kind().as_str() {
        "variable_declarator" => (
            node.child_by_field_name("name"),
            node.child_by_field_name("value")
                .or_else(|| node.child_by_field_name("init")),
        ),
        "assignment" | "assignment_expression" | "short_var_declaration" => (
            node.child_by_field_name("left"),
            node.child_by_field_name("right"),
        ),
        _ => return None,
    };
    let name = last_callable_identifier(node_text(&name_node?, source)?)?.to_string();
    // `let s: Store = …` annotates the declarator; `Store s = …` annotates
    // the declaration the declarator hangs off.
    let annotated = node
        .child_by_field_name("type")
        .or_else(|| {
            node.parent()
                .and_then(|parent| parent.child_by_field_name("type"))
        })
        .and_then(|annotation| node_text(&annotation, source))
        .and_then(type_head)
        .map(str::to_string);
    let type_name = match annotated {
        Some(annotated) => annotated,
        None => ast_value_type(&value_node?, source)?,
    };
    Some((name, type_name))
}

fn ast_value_type(node: &tree_sitter_language_pack::Node, source: &str) -> Option<String> {
    match node.kind().as_str() {
        "new_expression" => type_head(node_text(
            &node.child_by_field_name("constructor")?,
            source,
        )?)
        .map(str::to_string),
        "object_creation_expression" | "composite_literal" => {
            type_head(node_text(&node.child_by_field_name("type")?, source)?).map(str::to_string)
        }
        "expression_list"
        | "await_expression"
        | "try_expression"
        | "parenthesized_expression"
        | "unary_expression" => ast_value_type(&node.named_child(0)?, source),
        "call_expression" | "call" | "invocation_expression" => {
            let target = ["function", "callee", "name"]
                .into_iter()
                .find_map(|field| node.child_by_field_name(field))?;
            let path = node_text(&target, source)?;
            let head = path
                .rsplit_once("::")
                .or_else(|| path.rsplit_once('.'))
                .map_or(path, |(qualifier, _)| qualifier);
            type_head(head)
                .filter(|name| is_type_name(name))
                .map(str::to_string)
        }
        _ => None,
    }
}

/// Routes registered anywhere in a pack-parsed file, whether by a call
/// (`app.get("/pets", list)`) or by a marker on the handler itself
/// (`@app.get("/pets")`, `@GetMapping("/pets")`, `[HttpGet("/pets")]`).
fn collect_ast_routes(
    node: &tree_sitter_language_pack::Node,
    source: &str,
    owners: &[(String, usize, usize)],
    out: &mut ParsedFile,
) {
    let line = u32::try_from(node.start_position().row + 1).unwrap_or(u32::MAX);
    match node.kind().as_str() {
        "call_expression" | "call" | "invocation_expression" => {
            if let Some((callee, args)) = ["function", "callee", "name"]
                .into_iter()
                .find_map(|field| node.child_by_field_name(field))
                .and_then(|target| node_text(&target, source))
                .zip(
                    ["arguments", "argument_list", "parameters"]
                        .into_iter()
                        .find_map(|field| node.child_by_field_name(field))
                        .and_then(|args| node_text(&args, source)),
                )
            {
                let owner = innermost_owner(owners, node.start_byte(), node.end_byte());
                out.routes.extend(registration_route(callee, args, line));
                out.tools.extend(registration_tool(callee, args, line));
                out.consumers
                    .extend(consumer_call(callee, args, line, owner));
                out.events.extend(event_call(callee, args, line, owner));
                out.tool_calls.extend(tool_call(callee, args, line, owner));
            }
        }
        "decorator" | "annotation" | "marker_annotation" | "attribute" | "attribute_item" => {
            if let Some(text) = node_text(node, source) {
                let owner = innermost_owner(owners, node.start_byte(), node.end_byte());
                out.routes.extend(annotation_route(text, line, owner));
                out.tools.extend(annotation_tool(text, line, owner));
            }
            return;
        }
        "struct_expression" | "composite_literal" | "object_creation_expression" => {
            if let Some((type_text, body)) = ["type", "name", "constructor"]
                .into_iter()
                .find_map(|field| node.child_by_field_name(field))
                .and_then(|target| node_text(&target, source))
                .zip(
                    ["body", "arguments", "literal_value"]
                        .into_iter()
                        .find_map(|field| node.child_by_field_name(field))
                        .and_then(|body| node_text(&body, source)),
                )
            {
                out.tools.extend(struct_tool(type_text, body, line));
            }
        }
        _ => {}
    }
    for child in named_children(node) {
        collect_ast_routes(&child, source, owners, out);
    }
}

fn innermost_owner(owners: &[(String, usize, usize)], start: usize, end: usize) -> Option<&str> {
    owners
        .iter()
        .filter(|(_, open, close)| *open <= start && end <= *close)
        .min_by_key(|(_, open, close)| close - open)
        .map(|(name, _, _)| name.as_str())
}

/// Walks import syntax across the pack grammars.
///
/// The language pack's own import list reports the whole statement as the
/// module source (`from .engine import run`), which is unusable for module
/// resolution — so each family is read off the tree directly. A grammar not
/// covered here simply contributes no import edges, which is better than
/// binding a name to the last word of a statement.
fn collect_ast_imports(
    node: &tree_sitter_language_pack::Node,
    source: &str,
    out: &mut Vec<ImportRef>,
) {
    let kind = node.kind();
    match kind.as_str() {
        // ECMAScript family (TypeScript, TSX): `import x from "y"`.
        "import_statement" | "export_statement" if node.child_by_field_name("source").is_some() => {
            ecmascript_imports(node, source, out);
            return;
        }
        // Python `import a.b as c`.
        "import_statement" => {
            for child in named_children(node) {
                if let Some(import) = python_module_import(&child, source) {
                    out.push(import);
                }
            }
            return;
        }
        "import_from_statement" => {
            python_from_import(node, source, out);
            return;
        }
        // Go groups specs; Java/Swift/Scala write one path per declaration.
        "import_declaration"
        | "import_header"
        | "using_directive"
        | "namespace_use_declaration" => {
            let specs: Vec<_> = descendants_of_kind(node, "import_spec");
            if specs.is_empty() {
                if let Some(import) = qualified_path_import(node, source) {
                    out.push(import);
                }
            } else {
                for spec in specs {
                    if let Some(import) = go_import_spec(&spec, source) {
                        out.push(import);
                    }
                }
            }
            return;
        }
        // `#include "local/header.h"` — angle-bracket includes are system
        // headers and never resolve inside the repository.
        "preproc_include" => {
            if let Some(path) = named_children(node)
                .find(|child| child.kind() == "string_literal")
                .and_then(|literal| literal_text(&literal, source))
            {
                out.push(ImportRef::module(path));
            }
            return;
        }
        _ => {}
    }
    for child in named_children(node) {
        collect_ast_imports(&child, source, out);
    }
}

fn ecmascript_imports(
    node: &tree_sitter_language_pack::Node,
    source: &str,
    out: &mut Vec<ImportRef>,
) {
    let Some(module) = node
        .child_by_field_name("source")
        .and_then(|specifier| literal_text(&specifier, source))
    else {
        return;
    };
    let before = out.len();
    for child in named_children(node) {
        match child.kind().as_str() {
            "import_clause" => {
                for clause in named_children(&child) {
                    match clause.kind().as_str() {
                        "identifier" => out.push(ImportRef {
                            source: module.clone(),
                            name: node_text(&clause, source).map(str::to_string),
                            ..ImportRef::default()
                        }),
                        "namespace_import" => out.push(ImportRef {
                            source: module.clone(),
                            alias: named_children(&clause)
                                .next()
                                .and_then(|alias| node_text(&alias, source))
                                .map(str::to_string),
                            namespace: true,
                            ..ImportRef::default()
                        }),
                        "named_imports" => {
                            ecmascript_specifiers(&clause, source, &module, out);
                        }
                        _ => {}
                    }
                }
            }
            "named_imports" | "export_clause" => {
                ecmascript_specifiers(&child, source, &module, out);
            }
            _ => {}
        }
    }
    if out.len() == before {
        out.push(ImportRef {
            source: module,
            glob: true,
            ..ImportRef::default()
        });
    }
}

fn ecmascript_specifiers(
    node: &tree_sitter_language_pack::Node,
    source: &str,
    module: &str,
    out: &mut Vec<ImportRef>,
) {
    for specifier in named_children(node).filter(|child| {
        matches!(
            child.kind().as_str(),
            "import_specifier" | "export_specifier"
        )
    }) {
        let Some(name) = specifier
            .child_by_field_name("name")
            .and_then(|name| node_text(&name, source))
        else {
            continue;
        };
        out.push(ImportRef {
            source: module.to_string(),
            name: Some(name.to_string()),
            alias: specifier
                .child_by_field_name("alias")
                .and_then(|alias| node_text(&alias, source))
                .map(str::to_string),
            ..ImportRef::default()
        });
    }
}

fn python_module_import(node: &tree_sitter_language_pack::Node, source: &str) -> Option<ImportRef> {
    match node.kind().as_str() {
        "dotted_name" => Some(ImportRef::module(node_text(node, source)?)),
        "aliased_import" => Some(ImportRef {
            source: node_text(&node.child_by_field_name("name")?, source)?.to_string(),
            alias: node
                .child_by_field_name("alias")
                .and_then(|alias| node_text(&alias, source))
                .map(str::to_string),
            namespace: true,
            ..ImportRef::default()
        }),
        _ => None,
    }
}

fn python_from_import(
    node: &tree_sitter_language_pack::Node,
    source: &str,
    out: &mut Vec<ImportRef>,
) {
    let Some(module_node) = node.child_by_field_name("module_name") else {
        return;
    };
    let Some(module) = node_text(&module_node, source).map(str::to_string) else {
        return;
    };
    let module_range = module_node.byte_range();
    for child in named_children(node) {
        if child.byte_range() == module_range {
            continue;
        }
        match child.kind().as_str() {
            "wildcard_import" => out.push(ImportRef {
                source: module.clone(),
                glob: true,
                ..ImportRef::default()
            }),
            "dotted_name" | "identifier" => out.push(ImportRef {
                source: module.clone(),
                name: node_text(&child, source).map(str::to_string),
                ..ImportRef::default()
            }),
            "aliased_import" => out.push(ImportRef {
                source: module.clone(),
                name: child
                    .child_by_field_name("name")
                    .and_then(|name| node_text(&name, source))
                    .map(str::to_string),
                alias: child
                    .child_by_field_name("alias")
                    .and_then(|alias| node_text(&alias, source))
                    .map(str::to_string),
                ..ImportRef::default()
            }),
            _ => {}
        }
    }
}

fn go_import_spec(node: &tree_sitter_language_pack::Node, source: &str) -> Option<ImportRef> {
    let path = node
        .child_by_field_name("path")
        .or_else(|| named_children(node).find(|child| child.kind().contains("string")))?;
    Some(ImportRef {
        source: literal_text(&path, source)?,
        alias: node
            .child_by_field_name("name")
            .and_then(|name| node_text(&name, source))
            .map(str::to_string),
        namespace: true,
        ..ImportRef::default()
    })
}

/// Java/Kotlin/C#/PHP all write `<keyword> a.b.C[;]`, optionally ending in a
/// wildcard. The trailing segment is the imported type; the rest is where it
/// lives.
fn qualified_path_import(
    node: &tree_sitter_language_pack::Node,
    source: &str,
) -> Option<ImportRef> {
    let raw = node_text(node, source)?
        .trim_start_matches("import")
        .trim_start_matches("using")
        .trim_start_matches("use")
        .trim_start_matches("static")
        .trim()
        .trim_end_matches(';')
        .trim();
    // `using Alias = Some.Namespace.Type;`
    let (alias, raw) = raw.split_once('=').map_or((None, raw), |(alias, rest)| {
        (Some(alias.trim().to_string()), rest.trim())
    });
    if raw.is_empty() {
        return None;
    }
    if let Some(prefix) = raw.strip_suffix(".*").or_else(|| raw.strip_suffix('*')) {
        return Some(ImportRef {
            source: prefix.trim_end_matches('.').to_string(),
            glob: true,
            ..ImportRef::default()
        });
    }
    let (module, name) = raw.rsplit_once('.')?;
    Some(ImportRef {
        source: module.to_string(),
        name: Some(name.to_string()),
        alias,
        ..ImportRef::default()
    })
}

fn descendants_of_kind(
    node: &tree_sitter_language_pack::Node,
    kind: &str,
) -> Vec<tree_sitter_language_pack::Node> {
    let mut found = Vec::new();
    for child in named_children(node) {
        if child.kind() == kind {
            found.push(child);
        } else {
            found.extend(descendants_of_kind(&child, kind));
        }
    }
    found
}

fn node_text<'a>(node: &tree_sitter_language_pack::Node, source: &'a str) -> Option<&'a str> {
    source.get(node.byte_range().start..node.byte_range().end)
}

/// Contents of a string literal node, quotes stripped.
fn literal_text(node: &tree_sitter_language_pack::Node, source: &str) -> Option<String> {
    let raw = node_text(node, source)?.trim();
    Some(
        raw.trim_matches(|character| matches!(character, '"' | '\'' | '`'))
            .to_string(),
    )
    .filter(|value| !value.is_empty())
}

/// Declarations that can name a base type or interface, across the pack
/// grammars: classes, structs, interfaces, traits, and Kotlin objects.
const TYPE_DECLARATION_KINDS: &[&str] = &[
    "class_declaration",
    "class_definition",
    "class_specifier",
    "struct_specifier",
    "interface_declaration",
    "object_declaration",
    "trait_declaration",
];

/// Walks type declarations and records what each one extends or implements.
///
/// Each grammar spells heritage differently — Java splits `superclass` from
/// `interfaces`, TypeScript nests both under `class_heritage`, C# puts a
/// single `base_list` where the base class (if any) must come first, and
/// Python passes bases as an `argument_list`. The distinction is worth
/// keeping: `Inherits` and `Implements` answer different questions.
fn collect_ast_inheritance(
    node: &tree_sitter_language_pack::Node,
    source: &str,
    out: &mut Vec<InheritRef>,
) {
    if TYPE_DECLARATION_KINDS.contains(&node.kind().as_str())
        && let Some(child) = node
            .child_by_field_name("name")
            .and_then(|name| declaration_identifier(&name, source))
    {
        for heritage in named_children(node) {
            match heritage.kind().as_str() {
                "superclass"
                | "superclasses"
                | "argument_list"
                | "delegation_specifier"
                | "extends_clause" => push_inherits(child, &heritage, source, false, out),
                "interfaces" | "super_interfaces" | "extends_interfaces" | "implements_clause" => {
                    push_inherits(child, &heritage, source, true, out);
                }
                "class_heritage" => {
                    for clause in named_children(&heritage) {
                        let implements = clause.kind().contains("implement");
                        push_inherits(child, &clause, source, implements, out);
                    }
                }
                // C# lists base class and interfaces together, base first.
                "base_list" => {
                    let mut names = Vec::new();
                    collect_type_names(&heritage, source, &mut names);
                    for (position, parent) in names.into_iter().enumerate() {
                        out.push(InheritRef {
                            child: child.to_string(),
                            parent,
                            implements: position > 0,
                        });
                    }
                }
                _ => {}
            }
        }
    }
    for grandchild in named_children(node) {
        collect_ast_inheritance(&grandchild, source, out);
    }
}

fn push_inherits(
    child: &str,
    heritage: &tree_sitter_language_pack::Node,
    source: &str,
    implements: bool,
    out: &mut Vec<InheritRef>,
) {
    let mut names = Vec::new();
    collect_type_names(heritage, source, &mut names);
    out.extend(names.into_iter().map(|parent| InheritRef {
        child: child.to_string(),
        parent,
        implements,
    }));
}

/// Type names named by a heritage clause. Generic arguments are skipped —
/// `extends Repository<User>` extends `Repository`, not `User`.
fn collect_type_names(node: &tree_sitter_language_pack::Node, source: &str, out: &mut Vec<String>) {
    match node.kind().as_str() {
        "type_identifier"
        | "identifier"
        | "simple_identifier"
        | "constant"
        | "dotted_name"
        | "scoped_type_identifier"
        | "qualified_name" => {
            if let Some(name) = declaration_identifier(node, source) {
                out.push(name.to_string());
            }
        }
        "generic_type" | "user_type" => {
            if let Some(base) = node.named_child(0) {
                collect_type_names(&base, source, out);
            }
        }
        _ => {
            for child in named_children(node) {
                collect_type_names(&child, source, out);
            }
        }
    }
}

fn named_children(
    node: &tree_sitter_language_pack::Node,
) -> impl Iterator<Item = tree_sitter_language_pack::Node> + '_ {
    (0..u32::try_from(node.named_child_count()).unwrap_or(u32::MAX))
        .filter_map(move |index| node.named_child(index))
}

fn collect_ast_declarations(
    node: &tree_sitter_language_pack::Node,
    source: &str,
    file_path: &str,
    out: &mut ParsedFile,
    owners: &mut Vec<(String, usize, usize)>,
) {
    let syntax = node.kind();
    let kind = if matches!(
        syntax.as_str(),
        "function_definition"
            | "function_declaration"
            | "method_declaration"
            | "method_definition"
            | "function_item"
            | "constructor_declaration"
            | "function_signature"
            // Grammars whose declaration node is spelled its own way. Each one
            // is here because a test in this file shows it extracts a symbol.
            | "FnProto"              // Zig
            | "subroutine"           // Fortran
            | "function_statement"   // PowerShell
            | "routine"              // Nim
            | "let_binding"          // OCaml
            | "fun_decl"             // Erlang
            | "declProc"             // Pascal/Delphi
            | "declFunc"
            | "subroutine_declaration_statement" // Perl
            | "bind" // Haskell
    ) {
        Some(
            if syntax.contains("method") || syntax.contains("constructor") {
                NodeKind::Method
            } else {
                NodeKind::Function
            },
        )
    } else if matches!(
        syntax.as_str(),
        "class_declaration"
            | "class_definition"
            | "struct_specifier"
            | "struct_declaration"
            | "object_declaration"
            | "enum_declaration"
            | "module_declaration"   // Verilog/SystemVerilog: a module is the unit
            | "contract_declaration" // Solidity
    ) {
        Some(NodeKind::Struct)
    } else if matches!(
        syntax.as_str(),
        "interface_declaration" | "trait_declaration"
    ) {
        Some(NodeKind::Interface)
    } else {
        None
    };
    let assigned_name = node.parent().and_then(|parent| {
        ["left", "lhs", "name"]
            .into_iter()
            .find_map(|field| parent.child_by_field_name(field))
    });
    if let Some(kind) = kind
        && let Some(name_node) = assigned_name.or_else(|| {
            ["name", "declarator"]
                .into_iter()
                .find_map(|field| node.child_by_field_name(field))
                .or_else(|| Some(node.clone()))
        })
        && let Some(name) = declaration_identifier(&name_node, source)
        && !out.nodes.iter().any(|existing| existing.name == name)
    {
        out.nodes.push(Node {
            id: None,
            kind,
            name: name.to_string(),
            file_path: file_path.to_string(),
            start_line: u32::try_from(node.start_position().row + 1).unwrap_or(u32::MAX),
            end_line: u32::try_from(node.end_position().row + 1).unwrap_or(u32::MAX),
            description: None,
        });
        if matches!(kind, NodeKind::Function | NodeKind::Method) {
            owners.push((name.to_string(), node.start_byte(), node.end_byte()));
        }
    }
    for index in 0..u32::try_from(node.named_child_count()).unwrap_or(u32::MAX) {
        if let Some(child) = node.named_child(index) {
            collect_ast_declarations(&child, source, file_path, out, owners);
        }
    }
}

/// Node kinds that *are* a name in one grammar or another. A declaration whose
/// name is not in a `name` field is found by looking for one of these among its
/// nearest children — nearest, so a body identifier is never mistaken for the
/// declaration's own name.
const NAME_KINDS: &[&str] = &[
    "identifier",
    "type_identifier",
    "field_identifier",
    "simple_identifier",
    "IDENTIFIER",
    "name",
    "symbol",
    "value_name",
    "atom",
    "variable",
    "function_name",
    "moduleName",
    "sym_name",
    "bareword",
];

fn declaration_identifier<'a>(
    node: &tree_sitter_language_pack::Node,
    source: &'a str,
) -> Option<&'a str> {
    if NAME_KINDS.contains(&node.kind().as_str()) {
        return source.get(node.byte_range().start..node.byte_range().end);
    }
    (0..u32::try_from(node.named_child_count()).unwrap_or(u32::MAX))
        .filter_map(|index| node.named_child(index))
        .find_map(|child| declaration_identifier(&child, source))
}

fn collect_polyglot_calls_from(
    node: &tree_sitter_language_pack::Node,
    source: &str,
    owners: &[(String, usize, usize)],
    types: &[(String, usize, usize)],
) -> Vec<CallRef> {
    let mut calls = Vec::new();
    collect_polyglot_calls(node, source, owners, types, &mut calls);
    calls.sort_unstable();
    calls.dedup();
    calls
}

fn collect_polyglot_calls(
    node: &tree_sitter_language_pack::Node,
    source: &str,
    owners: &[(String, usize, usize)],
    types: &[(String, usize, usize)],
    out: &mut Vec<CallRef>,
) {
    let kind = node.kind();
    if matches!(
        kind.as_str(),
        "call_expression"
            | "invocation_expression"
            | "function_call"
            | "call"
            | "command"
            | "object_creation_expression"
    ) && let Some(owner) = innermost_owner(owners, node.start_byte(), node.end_byte())
        && let Some(target) = ["function", "name", "target", "callee", "method", "type"]
            .into_iter()
            .find_map(|field| node.child_by_field_name(field))
        && let Some((receiver, callee)) = source
            .get(target.byte_range().start..target.byte_range().end)
            .and_then(split_callee)
    {
        out.push(CallRef {
            caller: owner.to_string(),
            caller_type: enclosing_type(types, node.start_byte(), node.end_byte())
                .map(str::to_string),
            callee,
            receiver,
        });
    }
    for index in 0..u32::try_from(node.named_child_count()).unwrap_or(u32::MAX) {
        if let Some(child) = node.named_child(index) {
            collect_polyglot_calls(&child, source, owners, types, out);
        }
    }
}

/// The name of a type as written, minus its generic arguments and module
/// path (`std::fmt::Display` → `Display`, `Graph<T>` → `Graph`).
fn type_head(value: &str) -> Option<&str> {
    last_callable_identifier(value.split('<').next().unwrap_or(value))
}

fn last_callable_identifier(value: &str) -> Option<&str> {
    value
        .trim()
        .rsplit(|character: char| !character.is_alphanumeric() && character != '_')
        .find(|part| !part.is_empty())
}

/// Splits a callee expression into `(receiver, callee)`: everything before
/// the final identifier, minus the separator that joined them. Keeps the
/// receiver verbatim (`self`, `crate::sync`, `this.store`) so resolution can
/// decide whether it is a module path, a variable, or a `self` reference.
fn split_callee(expression: &str) -> Option<(Option<String>, String)> {
    let expression = expression.trim();
    let callee = last_callable_identifier(expression)?;
    let head = expression
        .strip_suffix(callee)?
        .trim_end_matches(|character: char| {
            matches!(character, '.' | ':' | '>' | '-' | '\\' | '@' | '$' | ' ')
        })
        .trim();
    let receiver = (!head.is_empty()).then(|| head.to_string());
    Some((receiver, callee.to_string()))
}

/// Tree-sitter-backed parser for JavaScript modules.
pub struct JavaScriptParser;

impl LanguageParser for JavaScriptParser {
    fn extensions(&self) -> &'static [&'static str] {
        &["js", "mjs", "cjs", "jsx"]
    }

    fn parse(&self, file_path: &str, source: &str) -> Result<ParsedFile> {
        let mut parser = TsParser::new();
        parser
            .set_language(&tree_sitter_javascript::LANGUAGE.into())
            .map_err(|source| Error::Parse {
                file: file_path.to_string(),
                reason: source.to_string(),
            })?;

        let tree = parser.parse(source, None).ok_or_else(|| Error::Parse {
            file: file_path.to_string(),
            reason: "tree-sitter returned no tree".to_string(),
        })?;

        let mut out = ParsedFile::default();
        walk_javascript(tree.root_node(), source, file_path, None, None, &mut out);
        attach_route_handlers(&mut out);
        Ok(out)
    }
}

/// Tree-sitter-backed parser for Rust.
pub struct RustParser;

impl LanguageParser for RustParser {
    fn extensions(&self) -> &'static [&'static str] {
        &["rs"]
    }

    fn parse(&self, file_path: &str, source: &str) -> Result<ParsedFile> {
        let mut parser = TsParser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .map_err(|source| Error::Parse {
                file: file_path.to_string(),
                reason: source.to_string(),
            })?;

        let tree = parser.parse(source, None).ok_or_else(|| Error::Parse {
            file: file_path.to_string(),
            reason: "tree-sitter returned no tree".to_string(),
        })?;

        let mut out = ParsedFile::default();
        walk(tree.root_node(), source, file_path, None, None, &mut out);
        attach_route_handlers(&mut out);
        Ok(out)
    }
}

fn text<'a>(node: TsNode<'_>, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or_default()
}

/// Splits a Rust call target into `(receiver, callee)`.
///
/// The qualifier of a scoped call (`bigbang::run`, `crate::sync::run`) is
/// exactly what disambiguates same-named functions across modules, and the
/// receiver of a method call (`graph.insert_node()`) is what disambiguates
/// same-named methods across types — so both are kept rather than reduced
/// to a bare name.
fn callee_name(func_node: TsNode<'_>, source: &str) -> Option<(Option<String>, String)> {
    match func_node.kind() {
        "identifier" => Some((None, text(func_node, source).to_string())),
        "scoped_identifier" => {
            let path = text(func_node, source);
            let (receiver, callee) = path.rsplit_once("::")?;
            let receiver = receiver.trim();
            Some((
                (!receiver.is_empty()).then(|| receiver.to_string()),
                callee.trim().to_string(),
            ))
        }
        "field_expression" => {
            let callee = text(func_node.child_by_field_name("field")?, source).to_string();
            let receiver = func_node
                .child_by_field_name("value")
                .map(|value| text(value, source).trim().to_string())
                .filter(|value| !value.is_empty());
            Some((receiver, callee))
        }
        _ => None,
    }
}

/// Expands a Rust `use` tree into one path per bound name, flattening
/// nested groups (`a::{b, c::{d, e}}` → `a::b`, `a::c::d`, `a::c::e`).
fn expand_use_tree(raw: &str) -> Vec<String> {
    let raw = raw.trim();
    let Some(open) = raw.find('{') else {
        return vec![raw.to_string()];
    };
    let prefix = &raw[..open];
    let close = matching_brace(raw, open).unwrap_or(raw.len());
    raw.get(open + 1..close)
        .map(split_top_level)
        .unwrap_or_default()
        .into_iter()
        .flat_map(|part| expand_use_tree(&part))
        .map(|part| {
            if part == "self" {
                prefix.trim_end_matches("::").to_string()
            } else {
                format!("{prefix}{part}")
            }
        })
        .collect()
}

fn matching_brace(raw: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, character) in raw.char_indices().skip(open) {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(offset);
                }
            }
            _ => {}
        }
    }
    None
}

/// Splits on commas that are not inside a nested brace group.
fn split_top_level(inner: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    for character in inner.chars() {
        match character {
            '{' => {
                depth += 1;
                current.push(character);
            }
            '}' => {
                depth = depth.saturating_sub(1);
                current.push(character);
            }
            ',' if depth == 0 => parts.push(std::mem::take(&mut current)),
            _ => current.push(character),
        }
    }
    parts.push(current);
    parts
        .into_iter()
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

/// Turns one expanded `use` path into its bound name plus the module it
/// came from (`crate::sync::run as go` → source `crate::sync`, name `run`,
/// alias `go`).
fn rust_import_ref(path: &str) -> Option<ImportRef> {
    let (path, alias) = path
        .split_once(" as ")
        .map_or((path, None), |(path, alias)| {
            (path.trim(), Some(alias.trim().to_string()))
        });
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    if let Some(source) = path.strip_suffix("::*").or_else(|| path.strip_suffix('*')) {
        return Some(ImportRef {
            source: source.trim_end_matches("::").to_string(),
            glob: true,
            ..ImportRef::default()
        });
    }
    let (source, name) = path.rsplit_once("::")?;
    Some(ImportRef {
        source: source.trim().to_string(),
        name: Some(name.trim().to_string()),
        alias,
        ..ImportRef::default()
    })
}

fn line_range(node: TsNode<'_>) -> (u32, u32) {
    let start = u32::try_from(node.start_position().row).unwrap_or(u32::MAX);
    let end = u32::try_from(node.end_position().row).unwrap_or(u32::MAX);
    (start + 1, end + 1)
}

fn children(node: TsNode<'_>) -> impl Iterator<Item = TsNode<'_>> {
    let count = u32::try_from(node.child_count()).unwrap_or(u32::MAX);
    (0..count).filter_map(move |i| node.child(i))
}

fn javascript_callee_name(func_node: TsNode<'_>, source: &str) -> Option<(Option<String>, String)> {
    match func_node.kind() {
        "identifier" => Some((None, text(func_node, source).to_string())),
        "member_expression" => {
            let callee = text(func_node.child_by_field_name("property")?, source).to_string();
            let receiver = func_node
                .child_by_field_name("object")
                .map(|object| text(object, source).trim().to_string())
                .filter(|object| !object.is_empty());
            Some((receiver, callee))
        }
        _ => None,
    }
}

/// Builds one [`ImportRef`] per name bound by an `import`/`export … from`
/// statement. A statement with a module source but no clause (a side-effect
/// import, or `export * from`) still records the dependency.
fn javascript_imports(node: TsNode<'_>, source: &str, out: &mut Vec<ImportRef>) {
    let Some(module) = node.child_by_field_name("source").map(|specifier| {
        text(specifier, source)
            .trim_matches(|character| matches!(character, '"' | '\'' | '`'))
            .to_string()
    }) else {
        return;
    };
    let before = out.len();
    for child in children(node) {
        match child.kind() {
            "import_clause" => {
                for clause in children(child) {
                    javascript_import_clause(clause, source, &module, out);
                }
            }
            "export_clause" | "named_imports" => {
                javascript_named_imports(child, source, &module, out);
            }
            _ => {}
        }
    }
    if out.len() == before {
        let glob = children(node).any(|child| child.kind() == "*");
        out.push(ImportRef {
            source: module,
            glob,
            namespace: !glob,
            ..ImportRef::default()
        });
    }
}

fn javascript_import_clause(
    node: TsNode<'_>,
    source: &str,
    module: &str,
    out: &mut Vec<ImportRef>,
) {
    match node.kind() {
        // `import Widget from './widget'` — the default export, which in
        // practice is the symbol the file is named after.
        "identifier" => {
            let local = text(node, source).to_string();
            out.push(ImportRef {
                source: module.to_string(),
                name: Some(local),
                ..ImportRef::default()
            });
        }
        "namespace_import" => {
            let alias = children(node)
                .find(|child| child.kind() == "identifier")
                .map(|child| text(child, source).to_string());
            out.push(ImportRef {
                source: module.to_string(),
                alias,
                namespace: true,
                ..ImportRef::default()
            });
        }
        "named_imports" => javascript_named_imports(node, source, module, out),
        _ => {}
    }
}

fn javascript_named_imports(
    node: TsNode<'_>,
    source: &str,
    module: &str,
    out: &mut Vec<ImportRef>,
) {
    for specifier in children(node)
        .filter(|child| matches!(child.kind(), "import_specifier" | "export_specifier"))
    {
        let Some(name) = specifier
            .child_by_field_name("name")
            .map(|name| text(name, source).to_string())
        else {
            continue;
        };
        let alias = specifier
            .child_by_field_name("alias")
            .map(|alias| text(alias, source).to_string());
        out.push(ImportRef {
            source: module.to_string(),
            name: Some(name),
            alias,
            ..ImportRef::default()
        });
    }
}

/// Records a class declaration and the base class it extends.
fn declare_javascript_class(
    node: TsNode<'_>,
    source: &str,
    file_path: &str,
    name: &str,
    out: &mut ParsedFile,
) {
    let (start_line, end_line) = line_range(node);
    out.nodes.push(Node {
        id: None,
        kind: NodeKind::Struct,
        name: name.to_string(),
        file_path: file_path.to_string(),
        start_line,
        end_line,
        description: None,
    });
    if let Some(parent) = children(node)
        .find(|child| child.kind() == "class_heritage")
        .and_then(|heritage| {
            last_callable_identifier(text(heritage, source).trim_start_matches("extends"))
        })
    {
        out.inherits.push(InheritRef {
            child: name.to_string(),
            parent: parent.to_string(),
            implements: false,
        });
    }
}

/// Records `const store = new Store()` (and its assignment form) as a typed
/// local. Only construction is trusted: a value returned by an arbitrary
/// call has no type the syntax can vouch for.
fn javascript_local_type(node: TsNode<'_>, source: &str, scope: &str, out: &mut Vec<LocalTypeRef>) {
    let (name_node, value_node) = match node.kind() {
        "variable_declarator" => (
            node.child_by_field_name("name"),
            node.child_by_field_name("value"),
        ),
        _ => (
            node.child_by_field_name("left"),
            node.child_by_field_name("right"),
        ),
    };
    let Some((name, value)) = name_node.zip(value_node) else {
        return;
    };
    let Some((name, type_name)) =
        last_callable_identifier(text(name, source)).zip(javascript_value_type(value, source))
    else {
        return;
    };
    out.push(LocalTypeRef {
        scope: scope.to_string(),
        name: name.to_string(),
        type_name,
    });
}

fn javascript_value_type(value: TsNode<'_>, source: &str) -> Option<String> {
    match value.kind() {
        "new_expression" => {
            type_head(text(value.child_by_field_name("constructor")?, source)).map(str::to_string)
        }
        "await_expression" | "parenthesized_expression" => {
            javascript_value_type(value.named_child(0)?, source)
        }
        _ => None,
    }
}

fn javascript_name<'a>(node: TsNode<'_>, source: &'a str) -> Option<&'a str> {
    node.child_by_field_name("name")
        .or_else(|| node.child_by_field_name("property"))
        .map(|name| text(name, source))
        .filter(|name| !name.is_empty())
}

/// Recursive-descent walk for the JavaScript grammar.
fn walk_javascript<'a>(
    node: TsNode<'_>,
    source: &'a str,
    file_path: &str,
    current_owner: Option<&'a str>,
    current_type: Option<&'a str>,
    out: &mut ParsedFile,
) {
    match node.kind() {
        "class_declaration" => {
            let Some(name) = javascript_name(node, source) else {
                return;
            };
            declare_javascript_class(node, source, file_path, name, out);
            for child in children(node) {
                walk_javascript(child, source, file_path, current_owner, Some(name), out);
            }
            return;
        }
        "function_declaration" | "generator_function_declaration" | "method_definition" => {
            let Some(name) = javascript_name(node, source) else {
                return;
            };
            let (start_line, end_line) = line_range(node);
            out.nodes.push(Node {
                id: None,
                kind: if current_type.is_some() {
                    NodeKind::Method
                } else {
                    NodeKind::Function
                },
                name: name.to_string(),
                file_path: file_path.to_string(),
                start_line,
                end_line,
                description: None,
            });
            if let Some(owner_type) = current_type {
                out.members.push(MemberRef {
                    owner_type: owner_type.to_string(),
                    member: name.to_string(),
                });
            }
            if let Some(body) = node.child_by_field_name("body") {
                walk_javascript(body, source, file_path, Some(name), current_type, out);
            }
            return;
        }
        // `const store = new Store()` types `store` for every later call on
        // it — the single most common way a receiver's type is knowable
        // without a type checker.
        "variable_declarator" | "assignment_expression" => {
            if let Some(owner) = current_owner {
                javascript_local_type(node, source, owner, &mut out.locals);
            }
        }
        "import_statement" | "export_statement" => {
            javascript_imports(node, source, &mut out.imports);
        }
        "call_expression" => {
            collect_contract_calls(node, source, current_owner, out);
            if let Some((caller, (receiver, callee))) = node
                .child_by_field_name("function")
                .and_then(|function| current_owner.zip(javascript_callee_name(function, source)))
            {
                out.calls.push(CallRef {
                    caller: caller.to_string(),
                    caller_type: current_type.map(str::to_string),
                    callee,
                    receiver,
                });
            }
        }
        // `new Widget()` is a call into `Widget` — without it the class that
        // a factory function instantiates has no incoming edge at all.
        "new_expression" => {
            if let Some((caller, callee)) = current_owner.zip(
                node.child_by_field_name("constructor")
                    .map(|constructor| text(constructor, source))
                    .and_then(last_callable_identifier),
            ) {
                out.calls.push(CallRef {
                    caller: caller.to_string(),
                    caller_type: current_type.map(str::to_string),
                    callee: callee.to_string(),
                    receiver: None,
                });
            }
        }
        _ => {}
    }

    for child in children(node) {
        walk_javascript(child, source, file_path, current_owner, current_type, out);
    }
}

/// Recursive-descent walk building a [`ParsedFile`] from a tree-sitter tree.
///
/// `in_impl` marks whether we're inside an `impl` block (so nested
/// `function_item`s are tagged `Method` rather than `Function`);
/// `current_owner` is the enclosing symbol name calls get attributed to.
fn walk<'a>(
    node: TsNode<'_>,
    source: &'a str,
    file_path: &str,
    current_type: Option<&'a str>,
    current_owner: Option<&'a str>,
    out: &mut ParsedFile,
) {
    match node.kind() {
        // Traits and enums are declarations like any other: without them a
        // trait has no node for `impl … for …` to point at.
        "struct_item" | "enum_item" | "union_item" | "trait_item" => {
            let Some(name) = node.child_by_field_name("name").map(|n| text(n, source)) else {
                return;
            };
            let is_trait = node.kind() == "trait_item";
            declare_rust_type(node, file_path, name, is_trait, out);
            if is_trait {
                for child in children(node) {
                    walk(child, source, file_path, Some(name), current_owner, out);
                }
                return;
            }
        }
        // Both `impl Type` and `impl Trait for Type` declare members of
        // `Type`; only the second declares a relation.
        "impl_item" => {
            let implementer = node
                .child_by_field_name("type")
                .map(|target| text(target, source))
                .and_then(type_head);
            if let Some((implementer, trait_name)) = implementer.zip(
                node.child_by_field_name("trait")
                    .map(|target| text(target, source))
                    .and_then(type_head),
            ) {
                out.inherits.push(InheritRef {
                    child: implementer.to_string(),
                    parent: trait_name.to_string(),
                    implements: true,
                });
            }
            for child in children(node) {
                walk(child, source, file_path, implementer, current_owner, out);
            }
            return;
        }
        "function_item" => {
            let name = node
                .child_by_field_name("name")
                .map(|n| text(n, source))
                .unwrap_or_default();
            declare_rust_function(node, file_path, name, current_type, out);
            if let Some(body) = node.child_by_field_name("body") {
                walk(body, source, file_path, current_type, Some(name), out);
            }
            return;
        }
        // `let graph = Graph::open(..)` / `let graph: Graph = ..` — the
        // annotation or the constructing path types the binding.
        "let_declaration" => {
            if let Some(owner) = current_owner {
                rust_local_type(node, source, owner, &mut out.locals);
            }
        }
        "use_declaration" => {
            let raw = text(node, source)
                .trim_start_matches("use")
                .trim()
                .trim_end_matches(';')
                .trim();
            out.imports.extend(
                expand_use_tree(raw)
                    .iter()
                    .filter_map(|path| rust_import_ref(path)),
            );
        }
        "attribute_item" | "struct_expression" => {
            collect_contract_markers(node, source, current_owner, out);
        }
        "call_expression" => {
            collect_contract_calls(node, source, current_owner, out);
            if let Some((caller, (receiver, callee))) = node
                .child_by_field_name("function")
                .and_then(|func| current_owner.zip(callee_name(func, source)))
            {
                out.calls.push(CallRef {
                    caller: caller.to_string(),
                    caller_type: current_type.map(str::to_string),
                    callee,
                    receiver,
                });
            }
        }
        _ => {}
    }

    for child in children(node) {
        walk(child, source, file_path, current_type, current_owner, out);
    }
}

/// Records a Rust type declaration (`struct`/`enum`/`union`/`trait`).
fn declare_rust_type(
    node: TsNode<'_>,
    file_path: &str,
    name: &str,
    is_trait: bool,
    out: &mut ParsedFile,
) {
    let (start_line, end_line) = line_range(node);
    out.nodes.push(Node {
        id: None,
        kind: if is_trait {
            NodeKind::Interface
        } else {
            NodeKind::Struct
        },
        name: name.to_string(),
        file_path: file_path.to_string(),
        start_line,
        end_line,
        description: None,
    });
}

/// Records a Rust function, as a method when an `impl`/`trait` encloses it.
fn declare_rust_function(
    node: TsNode<'_>,
    file_path: &str,
    name: &str,
    current_type: Option<&str>,
    out: &mut ParsedFile,
) {
    let (start_line, end_line) = line_range(node);
    out.nodes.push(Node {
        id: None,
        kind: if current_type.is_some() {
            NodeKind::Method
        } else {
            NodeKind::Function
        },
        name: name.to_string(),
        file_path: file_path.to_string(),
        start_line,
        end_line,
        description: None,
    });
    if let Some(owner_type) = current_type {
        out.members.push(MemberRef {
            owner_type: owner_type.to_string(),
            member: name.to_string(),
        });
    }
}

/// Types a `let` binding from its annotation, or from the path that
/// constructed it (`Graph::open(..)` → `Graph`, `Config { .. }` → `Config`).
fn rust_local_type(node: TsNode<'_>, source: &str, scope: &str, out: &mut Vec<LocalTypeRef>) {
    let Some(name) = node
        .child_by_field_name("pattern")
        .map(|pattern| text(pattern, source))
        .and_then(last_callable_identifier)
    else {
        return;
    };
    let type_name = node
        .child_by_field_name("type")
        .map(|annotation| text(annotation, source))
        .and_then(type_head)
        .map(str::to_string)
        .or_else(|| rust_value_type(node.child_by_field_name("value")?, source));
    if let Some(type_name) = type_name {
        out.push(LocalTypeRef {
            scope: scope.to_string(),
            name: name.to_string(),
            type_name,
        });
    }
}

fn rust_value_type(value: TsNode<'_>, source: &str) -> Option<String> {
    match value.kind() {
        "struct_expression" => {
            type_head(text(value.child_by_field_name("name")?, source)).map(str::to_string)
        }
        // `Graph::open(..)` — the qualifier is the constructed type.
        "call_expression" => {
            let function = value.child_by_field_name("function")?;
            if function.kind() != "scoped_identifier" {
                return None;
            }
            let path = text(function, source);
            let (qualifier, _) = path.rsplit_once("::")?;
            type_head(qualifier)
                .filter(|head| is_type_name(head))
                .map(str::to_string)
        }
        // `Graph::open(..)?` / `.await` / `.unwrap()` wrappers keep the type.
        "try_expression" | "await_expression" => rust_value_type(value.named_child(0)?, source),
        _ => None,
    }
}

/// Whether an identifier looks like a type rather than a module or value.
/// Every supported language capitalizes type names; a lowercase qualifier is
/// a module path (`crate::sync`), which resolution already handles.
fn is_type_name(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_top_level_function() {
        let parsed = parse_file("src/lib.rs", "fn run() {}").unwrap().unwrap();

        assert_eq!(parsed.nodes.len(), 1);
        assert_eq!(parsed.nodes[0].kind, NodeKind::Function);
        assert_eq!(parsed.nodes[0].name, "run");
    }

    #[test]
    fn extracts_struct() {
        let parsed = parse_file("src/lib.rs", "struct Graph { conn: i32 }")
            .unwrap()
            .unwrap();

        assert_eq!(parsed.nodes.len(), 1);
        assert_eq!(parsed.nodes[0].kind, NodeKind::Struct);
        assert_eq!(parsed.nodes[0].name, "Graph");
    }

    #[test]
    fn extracts_method_inside_impl_as_method_not_function() {
        let source = "struct Graph; impl Graph { fn open() {} }";
        let parsed = parse_file("src/lib.rs", source).unwrap().unwrap();

        let method = parsed
            .nodes
            .iter()
            .find(|n| n.name == "open")
            .expect("method node present");
        assert_eq!(method.kind, NodeKind::Method);
    }

    #[test]
    fn extracts_import_source_and_name() {
        let parsed = parse_file("src/lib.rs", "use std::fs::File;")
            .unwrap()
            .unwrap();

        assert_eq!(
            parsed.imports,
            vec![ImportRef {
                source: "std::fs".into(),
                name: Some("File".into()),
                ..ImportRef::default()
            }]
        );
    }

    #[test]
    fn expands_grouped_and_nested_use_trees() {
        let parsed = parse_file(
            "src/lib.rs",
            "use crate::{a::{b, c as d}, e::*, f::{self}};",
        )
        .unwrap()
        .unwrap();

        let rendered: Vec<(String, Option<String>, Option<String>, bool)> = parsed
            .imports
            .iter()
            .map(|import| {
                (
                    import.source.clone(),
                    import.name.clone(),
                    import.alias.clone(),
                    import.glob,
                )
            })
            .collect();
        assert_eq!(
            rendered,
            vec![
                ("crate::a".into(), Some("b".into()), None, false),
                ("crate::a".into(), Some("c".into()), Some("d".into()), false),
                ("crate::e".into(), None, None, true),
                ("crate".into(), Some("f".into()), None, false),
            ]
        );
    }

    #[test]
    fn records_impl_of_trait_as_implements() {
        let parsed = parse_file("src/lib.rs", "impl Display for Graph<T> { }")
            .unwrap()
            .unwrap();

        assert_eq!(
            parsed.inherits,
            vec![InheritRef {
                child: "Graph".into(),
                parent: "Display".into(),
                implements: true,
            }]
        );
    }

    #[test]
    fn inherent_impl_declares_no_relation() {
        let parsed = parse_file("src/lib.rs", "impl Graph { fn open() {} }")
            .unwrap()
            .unwrap();

        assert!(parsed.inherits.is_empty());
    }

    #[test]
    fn attributes_call_to_enclosing_function() {
        let source = "fn caller() { callee(); }";
        let parsed = parse_file("src/lib.rs", source).unwrap().unwrap();

        assert_eq!(
            parsed.calls,
            vec![CallRef {
                caller: "caller".into(),
                caller_type: None,
                callee: "callee".into(),
                receiver: None,
            }]
        );
    }

    #[test]
    fn keeps_the_module_qualifier_of_a_scoped_call() {
        let source = "fn caller() { crate::sync::run(); }";
        let parsed = parse_file("src/lib.rs", source).unwrap().unwrap();

        assert_eq!(
            parsed.calls,
            vec![CallRef {
                caller: "caller".into(),
                caller_type: None,
                callee: "run".into(),
                receiver: Some("crate::sync".into()),
            }]
        );
    }

    #[test]
    fn attributes_method_call_by_field_name() {
        let source = "fn caller() { graph.insert_node(); }";
        let parsed = parse_file("src/lib.rs", source).unwrap().unwrap();

        assert_eq!(
            parsed.calls,
            vec![CallRef {
                caller: "caller".into(),
                caller_type: None,
                callee: "insert_node".into(),
                receiver: Some("graph".into()),
            }]
        );
    }

    #[test]
    fn a_component_declares_itself_and_the_components_its_template_renders() {
        let parsed = parse_file(
            "src/OrderPage.vue",
            "<script setup>\nimport OrderRow from './OrderRow.vue';\n</script>\n\
             <template>\n  <div class=\"page\">\n    <OrderRow :order=\"o\" />\n    \
             <order-total :sum=\"s\" />\n    <p>plain</p>\n  </div>\n</template>\n",
        )
        .unwrap()
        .unwrap();

        assert!(
            parsed
                .nodes
                .iter()
                .any(|node| node.kind == NodeKind::Component && node.name == "OrderPage"),
            "the file is the component: {:?}",
            parsed.nodes
        );
        let rendered: Vec<&str> = parsed
            .calls
            .iter()
            .filter(|call| call.caller == "OrderPage")
            .map(|call| call.callee.as_str())
            .collect();
        assert!(
            rendered.contains(&"OrderRow") && rendered.contains(&"order-total"),
            "both spellings of a component element are a use: {rendered:?}"
        );
        assert!(
            !rendered.contains(&"div") && !rendered.contains(&"p"),
            "an HTML element is not a component: {rendered:?}"
        );
    }

    #[test]
    fn groovy_methods_are_found_without_the_def_keyword() {
        let parsed = parse_file(
            "Main.groovy",
            "class Main {\n  String greet(String who) { return who }\n  \
             static int add(a, b) { a + b }\n  void run() { greet('x') }\n}\n\
             def helper() { return 1 }\nprintln greet('y')\n",
        )
        .unwrap()
        .unwrap();

        let names: Vec<&str> = parsed.nodes.iter().map(|node| node.name.as_str()).collect();
        for expected in ["Main", "greet", "add", "run", "helper"] {
            assert!(
                names.contains(&expected),
                "{expected} missing from {names:?}"
            );
        }
        assert!(
            !names.contains(&"println"),
            "a call is not a declaration: {names:?}"
        );
    }

    #[test]
    fn a_svelte_control_element_is_not_a_component() {
        let parsed = parse_file(
            "src/List.svelte",
            "<script>\n  let items = [];\n</script>\n\
             <svelte:head><title>x</title></svelte:head>\n<Row />\n",
        )
        .unwrap()
        .unwrap();

        let rendered: Vec<&str> = parsed.calls.iter().map(|c| c.callee.as_str()).collect();
        assert!(rendered.contains(&"Row"), "{rendered:?}");
        assert!(
            !rendered.iter().any(|name| name.starts_with("svelte:")),
            "framework control flow is not a component: {rendered:?}"
        );
    }

    /// The validation half of P2.13: a language is only listed once a snippet
    /// from it yields a declaration. A grammar existing in the pack is not
    /// coverage — extracting a symbol is.
    #[test]
    fn pack_languages_extract_declarations() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "app.vue",
                "<script>\nexport default { methods: { greet() { return 1; } } }\n</script>\n<template><p/></template>",
                "greet",
            ),
            (
                "app.svelte",
                "<script>\n  function greet() { return 1; }\n</script>\n<p>hi</p>",
                "greet",
            ),
            (
                "page.astro",
                "---\nfunction greet() { return 1; }\n---\n<p>hi</p>",
                "greet",
            ),
            (
                "main.zig",
                "pub fn greet() u8 {\n    return 1;\n}\n",
                "greet",
            ),
            ("main.jl", "function greet()\n    return 1\nend\n", "greet"),
            (
                "main.f90",
                "subroutine greet()\nend subroutine greet\n",
                "greet",
            ),
            ("main.nim", "proc greet(): int =\n  1\n", "greet"),
            ("main.hs", "greet :: Int\ngreet = 1\n", "greet"),
            ("main.ml", "let greet () = 1\n", "greet"),
            ("main.erl", "-module(main).\ngreet() -> 1.\n", "greet"),
            ("main.clj", "(defn greet [] 1)\n", "greet"),
            (
                "Main.groovy",
                "class Main {\n  def greet() { return 1 }\n}\n",
                "greet",
            ),
            (
                "Main.cls",
                "public class Main {\n  public Integer greet() { return 1; }\n}\n",
                "greet",
            ),
            ("main.v", "module greet;\nendmodule\n", "greet"),
            ("main.sv", "module greet;\nendmodule\n", "greet"),
            ("main.ps1", "function Greet {\n  return 1\n}\n", "Greet"),
            (
                "main.pas",
                "program Main;\nprocedure Greet;\nbegin\nend;\nbegin\nend.\n",
                "Greet",
            ),
            ("main.pl", "sub greet {\n  return 1;\n}\n", "greet"),
            (
                "main.sol",
                "contract Greeter {\n  function greet() public pure returns (uint) { return 1; }\n}\n",
                "greet",
            ),
            ("build.gradle", "def greet() { return 1 }\n", "greet"),
        ];
        let mut silent = Vec::new();
        for (path, source, expected) in cases {
            assert!(
                supports_file(path),
                "{path} must be recognized as a source file"
            );
            let parsed = parse_file(path, source)
                .unwrap_or_else(|error| panic!("{path} failed to parse: {error}"))
                .unwrap_or_else(|| panic!("{path} has no parser"));
            if !parsed.nodes.iter().any(|node| node.name == *expected) {
                silent.push(format!(
                    "{path}: expected `{expected}`, got {:?}",
                    parsed
                        .nodes
                        .iter()
                        .map(|node| node.name.as_str())
                        .collect::<Vec<_>>()
                ));
            }
        }
        assert!(
            silent.is_empty(),
            "every listed language must extract a declaration:\n{}",
            silent.join("\n")
        );
    }

    #[test]
    fn unknown_extension_returns_none() {
        assert_eq!(parse_file("README.md", "# hi").unwrap(), None);
    }

    #[test]
    fn extracts_javascript_module_symbols_and_calls() {
        let source = "import { helper } from './helper.mjs'; export function render() { helper(); } class Studio { save() { render(); } }";
        let parsed = parse_file("src/app.mjs", source).unwrap().unwrap();

        assert!(
            parsed
                .nodes
                .iter()
                .any(|node| node.name == "render" && node.kind == NodeKind::Function)
        );
        assert!(
            parsed
                .nodes
                .iter()
                .any(|node| node.name == "Studio" && node.kind == NodeKind::Struct)
        );
        assert!(
            parsed
                .nodes
                .iter()
                .any(|node| node.name == "save" && node.kind == NodeKind::Method)
        );
        assert_eq!(
            parsed.calls,
            vec![
                CallRef {
                    caller: "render".into(),
                    caller_type: None,
                    callee: "helper".into(),
                    receiver: None,
                },
                CallRef {
                    caller: "save".into(),
                    caller_type: Some("Studio".into()),
                    callee: "render".into(),
                    receiver: None,
                }
            ]
        );
        assert_eq!(
            parsed.imports,
            vec![ImportRef {
                source: "./helper.mjs".into(),
                name: Some("helper".into()),
                ..ImportRef::default()
            }]
        );
    }

    #[test]
    fn extracts_javascript_default_namespace_and_aliased_imports() {
        let source = "import Widget from './widget.js';\nimport * as utils from './utils.js';\nimport { parse as read, load } from './io.js';\nexport * from './extra.js';\n";
        let parsed = parse_file("src/app.js", source).unwrap().unwrap();

        assert_eq!(
            parsed.imports,
            vec![
                ImportRef {
                    source: "./widget.js".into(),
                    name: Some("Widget".into()),
                    ..ImportRef::default()
                },
                ImportRef {
                    source: "./utils.js".into(),
                    alias: Some("utils".into()),
                    namespace: true,
                    ..ImportRef::default()
                },
                ImportRef {
                    source: "./io.js".into(),
                    name: Some("parse".into()),
                    alias: Some("read".into()),
                    ..ImportRef::default()
                },
                ImportRef {
                    source: "./io.js".into(),
                    name: Some("load".into()),
                    ..ImportRef::default()
                },
                ImportRef {
                    source: "./extra.js".into(),
                    glob: true,
                    ..ImportRef::default()
                },
            ]
        );
    }

    #[test]
    fn extracts_javascript_receiver_and_construction() {
        let source =
            "class Store extends Base { load() { this.db.query(); const w = new Widget(); } }";
        let parsed = parse_file("src/app.js", source).unwrap().unwrap();

        assert_eq!(
            parsed.calls,
            vec![
                CallRef {
                    caller: "load".into(),
                    caller_type: Some("Store".into()),
                    callee: "query".into(),
                    receiver: Some("this.db".into()),
                },
                CallRef {
                    caller: "load".into(),
                    caller_type: Some("Store".into()),
                    callee: "Widget".into(),
                    receiver: None,
                }
            ]
        );
        assert_eq!(
            parsed.inherits,
            vec![InheritRef {
                child: "Store".into(),
                parent: "Base".into(),
                implements: false,
            }]
        );
    }

    #[test]
    fn parses_top_twenty_languages() {
        let fixtures = [
            ("main.rs", "fn greet() { helper(); }", "greet"),
            ("main.js", "function greet() { helper(); }", "greet"),
            ("main.ts", "function greet(): void { helper(); }", "greet"),
            ("main.py", "def greet():\n    helper()\n", "greet"),
            (
                "Main.java",
                "class Main { void greet() { helper(); } }",
                "greet",
            ),
            ("main.c", "void greet(void) { helper(); }", "greet"),
            ("main.cpp", "void greet() { helper(); }", "greet"),
            (
                "Main.cs",
                "class Main { void Greet() { Helper(); } }",
                "Greet",
            ),
            (
                "main.go",
                "package main\nfunc greet() { helper() }",
                "greet",
            ),
            ("main.php", "<?php function greet() { helper(); }", "greet"),
            ("main.rb", "def greet\n  helper\nend\n", "greet"),
            ("main.swift", "func greet() { helper() }", "greet"),
            ("Main.kt", "fun greet() { helper() }", "greet"),
            ("main.dart", "void greet() { helper(); }", "greet"),
            ("Main.scala", "def greet(): Unit = helper()", "greet"),
            ("main.sh", "greet() { helper; }", "greet"),
            ("main.lua", "function greet() helper() end", "greet"),
            ("main.r", "greet <- function() { helper() }", "greet"),
            (
                "main.ex",
                "defmodule Main do\n  def greet, do: helper()\nend",
                "greet",
            ),
            ("main.m", "void greet(void) { helper(); }", "greet"),
        ];

        for (path, source, expected) in fixtures {
            let parsed = parse_file(path, source)
                .unwrap_or_else(|error| panic!("{path}: {error}"))
                .unwrap_or_else(|| panic!("{path}: language not detected"));
            assert!(
                parsed.nodes.iter().any(|node| node.name == expected),
                "{path}: expected {expected}, got {:?}; syntax {}",
                parsed
                    .nodes
                    .iter()
                    .map(|node| &node.name)
                    .collect::<Vec<_>>(),
                polyglot_language(path)
                    .and_then(|language| tree_sitter_language_pack::get_parser(language).ok())
                    .and_then(|mut parser| parser.parse(source))
                    .map_or_else(|| "native".into(), |tree| tree.root_node().to_sexp())
            );
        }
    }
}
