//! Tree-sitter based structural parsing.
//!
//! A [`LanguageParser`] turns one file's source text into a [`ParsedFile`]:
//! the symbols it declares (functions/structs/methods) plus *raw* import
//! paths and call targets. Raw here means unresolved — turning e.g. a call
//! to `bar` into an edge that points at a specific node id, and tagging that
//! edge `EXTRACTED`/`INFERRED`/`AMBIGUOUS`, is cross-file resolution's job
//! (see `crate::resolve`), not the parser's. This keeps each language's
//! parser dumb and swappable without touching the storage layer.

use tree_sitter::{Node as TsNode, Parser as TsParser};

use crate::error::{Error, Result};
use crate::storage::{Node, NodeKind};

/// One file's extracted symbols plus unresolved cross-references.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ParsedFile {
    /// Symbols declared directly in this file (not yet inserted, no id).
    pub nodes: Vec<Node>,
    /// Raw `use`/import paths as written in source (e.g. `std::fs::File`).
    pub imports: Vec<String>,
    /// `(caller_symbol_name, callee_name)` pairs found inside function/method bodies.
    pub calls: Vec<(String, String)>,
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

/// The 18 pack-backed languages plus native Rust and JavaScript frontends
/// make the default top-20 language surface.
fn polyglot_language(file_path: &str) -> Option<&'static str> {
    let extension = file_path.rsplit('.').next()?.to_ascii_lowercase();
    match extension.as_str() {
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

#[allow(clippy::items_after_statements)]
fn parse_polyglot(file_path: &str, source: &str, language: &str) -> Result<ParsedFile> {
    use tree_sitter_language_pack::{ProcessConfig, StructureItem, StructureKind, SymbolKind};

    let processed = tree_sitter_language_pack::process(source, &ProcessConfig::new(language).all())
        .map_err(|error| Error::Parse {
            file: file_path.to_string(),
            reason: error.to_string(),
        })?;
    let mut out = ParsedFile::default();
    let mut owners = Vec::new();

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
    for item in &processed.structure {
        append_structure(item, file_path, &mut out, &mut owners);
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
    append_ast_declarations(language, source, file_path, &mut out, &mut owners)?;
    out.imports = processed
        .imports
        .into_iter()
        .flat_map(|import| {
            if import.items.is_empty() {
                vec![import.source]
            } else {
                import
                    .items
                    .into_iter()
                    .map(|item| format!("{}::{item}", import.source))
                    .collect()
            }
        })
        .collect();
    out.calls = polyglot_calls(language, source, &owners, file_path)?;
    Ok(out)
}

fn append_ast_declarations(
    language: &str,
    source: &str,
    file_path: &str,
    out: &mut ParsedFile,
    owners: &mut Vec<(String, usize, usize)>,
) -> Result<()> {
    let mut parser =
        tree_sitter_language_pack::get_parser(language).map_err(|error| Error::Parse {
            file: file_path.to_string(),
            reason: error.to_string(),
        })?;
    let tree = parser.parse(source).ok_or_else(|| Error::Parse {
        file: file_path.to_string(),
        reason: "tree-sitter returned no tree".to_string(),
    })?;
    collect_ast_declarations(&tree.root_node(), source, file_path, out, owners);
    Ok(())
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

fn declaration_identifier<'a>(
    node: &tree_sitter_language_pack::Node,
    source: &'a str,
) -> Option<&'a str> {
    if matches!(
        node.kind().as_str(),
        "identifier" | "type_identifier" | "field_identifier" | "simple_identifier"
    ) {
        return source.get(node.byte_range().start..node.byte_range().end);
    }
    (0..u32::try_from(node.named_child_count()).unwrap_or(u32::MAX))
        .filter_map(|index| node.named_child(index))
        .find_map(|child| declaration_identifier(&child, source))
}

fn polyglot_calls(
    language: &str,
    source: &str,
    owners: &[(String, usize, usize)],
    file_path: &str,
) -> Result<Vec<(String, String)>> {
    let mut parser =
        tree_sitter_language_pack::get_parser(language).map_err(|error| Error::Parse {
            file: file_path.to_string(),
            reason: error.to_string(),
        })?;
    let tree = parser.parse(source).ok_or_else(|| Error::Parse {
        file: file_path.to_string(),
        reason: "tree-sitter returned no tree".to_string(),
    })?;
    let mut calls = Vec::new();
    collect_polyglot_calls(&tree.root_node(), source, owners, &mut calls);
    calls.sort_unstable();
    calls.dedup();
    Ok(calls)
}

fn collect_polyglot_calls(
    node: &tree_sitter_language_pack::Node,
    source: &str,
    owners: &[(String, usize, usize)],
    out: &mut Vec<(String, String)>,
) {
    let kind = node.kind();
    if matches!(
        kind.as_str(),
        "call_expression" | "invocation_expression" | "function_call" | "call" | "command"
    ) && let Some(owner) = owners
        .iter()
        .filter(|(_, start, end)| *start <= node.start_byte() && node.end_byte() <= *end)
        .min_by_key(|(_, start, end)| end - start)
        && let Some(target) = ["function", "name", "target", "callee", "method"]
            .into_iter()
            .find_map(|field| node.child_by_field_name(field))
        && let Some(name) = source
            .get(target.byte_range().start..target.byte_range().end)
            .and_then(last_callable_identifier)
    {
        out.push((owner.0.clone(), name.to_string()));
    }
    for index in 0..u32::try_from(node.named_child_count()).unwrap_or(u32::MAX) {
        if let Some(child) = node.named_child(index) {
            collect_polyglot_calls(&child, source, owners, out);
        }
    }
}

fn last_callable_identifier(value: &str) -> Option<&str> {
    value
        .trim()
        .rsplit(|character: char| !character.is_alphanumeric() && character != '_')
        .find(|part| !part.is_empty())
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
        walk_javascript(tree.root_node(), source, file_path, None, false, &mut out);
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
        walk(tree.root_node(), source, file_path, false, None, &mut out);
        Ok(out)
    }
}

fn text<'a>(node: TsNode<'_>, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or_default()
}

fn callee_name(func_node: TsNode<'_>, source: &str) -> Option<String> {
    match func_node.kind() {
        // For scoped calls, keep the full qualified path (`bigbang::run`,
        // `crate::sync::run`): the qualifier is exactly what disambiguates
        // same-named functions across modules, and `crate::resolve` uses it
        // to pick the right file instead of fanning out AMBIGUOUS edges to
        // every `run`.
        "identifier" | "scoped_identifier" => Some(text(func_node, source).to_string()),
        "field_expression" => func_node
            .child_by_field_name("field")
            .map(|n| text(n, source).to_string()),
        _ => None,
    }
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

fn javascript_callee_name(func_node: TsNode<'_>, source: &str) -> Option<String> {
    match func_node.kind() {
        "identifier" => Some(text(func_node, source).to_string()),
        "member_expression" => func_node
            .child_by_field_name("property")
            .map(|node| text(node, source).to_string()),
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
    in_class: bool,
    out: &mut ParsedFile,
) {
    match node.kind() {
        "class_declaration" => {
            if let Some(name) = javascript_name(node, source) {
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
            }
            for child in children(node) {
                walk_javascript(child, source, file_path, current_owner, true, out);
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
                kind: if in_class {
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
            if let Some(body) = node.child_by_field_name("body") {
                walk_javascript(body, source, file_path, Some(name), in_class, out);
            }
            return;
        }
        "import_statement" => {
            out.imports.push(text(node, source).to_string());
        }
        "call_expression" => {
            if let Some((caller, callee)) = node
                .child_by_field_name("function")
                .and_then(|function| current_owner.zip(javascript_callee_name(function, source)))
            {
                out.calls.push((caller.to_string(), callee));
            }
        }
        _ => {}
    }

    for child in children(node) {
        walk_javascript(child, source, file_path, current_owner, in_class, out);
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
    in_impl: bool,
    current_owner: Option<&'a str>,
    out: &mut ParsedFile,
) {
    match node.kind() {
        "struct_item" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let (start_line, end_line) = line_range(node);
                out.nodes.push(Node {
                    id: None,
                    kind: NodeKind::Struct,
                    name: text(name_node, source).to_string(),
                    file_path: file_path.to_string(),
                    start_line,
                    end_line,
                    description: None,
                });
            }
        }
        "impl_item" => {
            for child in children(node) {
                walk(child, source, file_path, true, current_owner, out);
            }
            return;
        }
        "function_item" => {
            let name = node
                .child_by_field_name("name")
                .map(|n| text(n, source))
                .unwrap_or_default();
            let (start_line, end_line) = line_range(node);
            out.nodes.push(Node {
                id: None,
                kind: if in_impl {
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
            if let Some(body) = node.child_by_field_name("body") {
                walk(body, source, file_path, in_impl, Some(name), out);
            }
            return;
        }
        "use_declaration" => {
            let raw = text(node, source)
                .trim_start_matches("use")
                .trim()
                .trim_end_matches(';')
                .trim();
            out.imports.push(raw.to_string());
        }
        "call_expression" => {
            if let Some((caller, callee)) = node
                .child_by_field_name("function")
                .and_then(|func| current_owner.zip(callee_name(func, source)))
            {
                out.calls.push((caller.to_string(), callee));
            }
        }
        _ => {}
    }

    for child in children(node) {
        walk(child, source, file_path, in_impl, current_owner, out);
    }
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
    fn extracts_raw_import_path() {
        let parsed = parse_file("src/lib.rs", "use std::fs::File;")
            .unwrap()
            .unwrap();

        assert_eq!(parsed.imports, vec!["std::fs::File".to_string()]);
    }

    #[test]
    fn attributes_call_to_enclosing_function() {
        let source = "fn caller() { callee(); }";
        let parsed = parse_file("src/lib.rs", source).unwrap().unwrap();

        assert_eq!(
            parsed.calls,
            vec![("caller".to_string(), "callee".to_string())]
        );
    }

    #[test]
    fn attributes_method_call_by_field_name() {
        let source = "fn caller() { graph.insert_node(); }";
        let parsed = parse_file("src/lib.rs", source).unwrap().unwrap();

        assert_eq!(
            parsed.calls,
            vec![("caller".to_string(), "insert_node".to_string())]
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
                ("render".to_string(), "helper".to_string()),
                ("save".to_string(), "render".to_string())
            ]
        );
        assert_eq!(
            parsed.imports,
            vec!["import { helper } from './helper.mjs';".to_string()]
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
