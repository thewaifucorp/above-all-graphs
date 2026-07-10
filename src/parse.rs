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
    let parsers: [&dyn LanguageParser; 1] = [&RustParser];

    for parser in parsers {
        if parser.extensions().contains(&extension) {
            return parser.parse(file_path, source).map(Some);
        }
    }
    Ok(None)
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
}
