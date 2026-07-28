//! A documented formal subset of Cypher, evaluated as real graph patterns.
//!
//! What this replaces matters more than what it adds. The previous surface
//! sniffed strings: it checked that a query started with `MATCH`, looked for the
//! first `.name = '...'` anywhere in the text, and — if the query mentioned
//! `-[` at all — dumped every edge in the graph regardless of what the pattern
//! actually said. A query and a wrong answer to it were indistinguishable.
//!
//! So this module is a lexer, a parser, and an evaluator for a subset that is
//! written down rather than guessed at, per P0.4 of
//! `docs/capability-coverage.md`:
//!
//! ```text
//! MATCH pattern (, pattern)*
//! [WHERE predicate]
//! RETURN [DISTINCT] item (, item)*
//! [ORDER BY column [ASC|DESC] (, ...)*]
//! [SKIP n] [LIMIT n]
//! ```
//!
//! Anything outside the subset is an error naming what was expected, never a
//! silently different query. Writes are rejected by name: the graph is read-only
//! here.
//!
//! Three deliberate limits, so nothing here is mistaken for a query engine:
//!
//! - No `WITH`, `UNWIND`, `OPTIONAL MATCH`, `UNION`, subqueries, path
//!   functions, or arithmetic. `count` is the only function.
//! - Evaluation loads the graph and matches in memory, bounded by a row budget.
//!   It is not a planner and does not use indexes beyond the label and property
//!   pushdown described in [`Index::node_matches`].
//! - A variable-length relationship never repeats an edge within one path, so a
//!   cycle terminates instead of running to the budget.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Value as Json, json};

use crate::error::{Error, Result};
use crate::storage::{Edge, EdgeKind, Graph, Node, NodeKind};

/// Rows returned when a query does not say otherwise.
const DEFAULT_LIMIT: usize = 100;

/// Hard ceiling on returned rows. A larger `LIMIT` is clamped to this and the
/// result says it was truncated.
const MAX_LIMIT: usize = 1_000;

/// Hops a `*` with no upper bound expands to. Unbounded means unbounded work,
/// and a query surface that can hang is not usable from a hook.
const DEFAULT_MAX_HOPS: u32 = 5;

/// Intermediate rows one query may hold. Beyond this the query is rejected with
/// advice rather than answered slowly.
const ROW_BUDGET: usize = 20_000;

/// Words the subset reserves, which therefore cannot name a variable.
const RESERVED: &[&str] = &[
    "match", "where", "return", "distinct", "order", "by", "skip", "limit", "as", "and", "or",
    "not", "is", "null", "in", "contains", "starts", "ends", "with", "count", "asc", "desc",
];

/// Words that would write to the graph. Rejected before parsing so the message
/// is about intent rather than syntax.
const WRITE_WORDS: &[&str] = &[
    "create", "merge", "delete", "detach", "set", "remove", "drop", "load", "foreach", "call",
];

/// Clauses Cypher has and this subset does not.
const UNSUPPORTED_WORDS: &[&str] = &["unwind", "union", "optional", "case", "exists", "collect"];

/// Node properties a query may read.
const NODE_PROPERTIES: &[&str] = &["id", "kind", "name", "file", "line", "end_line"];

/// Relationship properties a query may read.
const RELATIONSHIP_PROPERTIES: &[&str] = &["type", "confidence"];

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// An identifier, keyword, or label — case is preserved, comparison is not.
    Word(String),
    /// A quoted string literal.
    Text(String),
    /// An integer literal.
    Int(i64),
    /// Punctuation or an operator.
    Symbol(&'static str),
}

/// Longest match first, so `->` never lexes as `-` then `>`.
const SYMBOLS: &[&str] = &[
    "->", "<-", "<>", "<=", ">=", "..", "(", ")", "[", "]", "{", "}", ",", ".", ":", "|", "*", "-",
    "=", "<", ">",
];

/// Splits a query into tokens, each with the byte offset it started at.
fn lex(source: &str) -> Result<Vec<(Token, usize)>> {
    let mut out = Vec::new();
    let mut offset = 0;
    while offset < source.len() {
        let rest = &source[offset..];
        let Some(character) = rest.chars().next() else {
            break;
        };
        if character.is_whitespace() {
            offset += character.len_utf8();
            continue;
        }
        if character == '\'' || character == '"' {
            let (text, length) = lex_text(source, offset, character)?;
            out.push((Token::Text(text), offset));
            offset += length;
            continue;
        }
        if character.is_ascii_digit() {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            let value = digits.parse::<i64>().map_err(|_| {
                error_at(
                    source,
                    offset,
                    format!("`{digits}` is not an integer this subset can read"),
                )
            })?;
            offset += digits.len();
            out.push((Token::Int(value), offset - digits.len()));
            continue;
        }
        if character.is_alphabetic() || character == '_' {
            let word: String = rest
                .chars()
                .take_while(|character| character.is_alphanumeric() || *character == '_')
                .collect();
            out.push((Token::Word(word.clone()), offset));
            offset += word.len();
            continue;
        }
        if let Some(symbol) = SYMBOLS.iter().find(|symbol| rest.starts_with(**symbol)) {
            out.push((Token::Symbol(symbol), offset));
            offset += symbol.len();
            continue;
        }
        return Err(error_at(
            source,
            offset,
            format!("unexpected character `{character}`"),
        ));
    }
    Ok(out)
}

/// Reads a quoted literal, returning its contents and how many bytes it spanned.
///
/// Escapes are not supported: a literal is the text between two quotes. That is
/// a limit worth stating rather than a half-implemented escape table.
fn lex_text(source: &str, offset: usize, quote: char) -> Result<(String, usize)> {
    let body = &source[offset + quote.len_utf8()..];
    let Some(end) = body.find(quote) else {
        return Err(error_at(source, offset, "unterminated string literal"));
    };
    Ok((
        body[..end].to_string(),
        quote.len_utf8() * 2 + body[..end].len(),
    ))
}

/// A query error carrying the line and column it was found at.
fn error_at(source: &str, offset: usize, detail: impl Into<String>) -> Error {
    let consumed = &source[..offset.min(source.len())];
    let line = consumed.matches('\n').count() + 1;
    let column = consumed
        .rsplit_once('\n')
        .map_or(consumed.chars().count(), |(_, last)| last.chars().count())
        + 1;
    Error::Query {
        detail: format!("line {line}, column {column}: {}", detail.into()),
    }
}

// ---------------------------------------------------------------------------
// Syntax tree
// ---------------------------------------------------------------------------

/// One parsed query.
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    patterns: Vec<Pattern>,
    filter: Option<Predicate>,
    projections: Vec<Projection>,
    distinct: bool,
    order: Vec<(String, bool)>,
    skip: usize,
    limit: usize,
    limit_clamped: bool,
}

/// A chain of node patterns joined by relationship patterns.
#[derive(Debug, Clone, PartialEq)]
struct Pattern {
    start: NodePattern,
    hops: Vec<(RelationshipPattern, NodePattern)>,
}

#[derive(Debug, Clone, PartialEq)]
struct NodePattern {
    variable: Option<String>,
    label: Option<NodeKind>,
    properties: Vec<(String, Literal)>,
}

#[derive(Debug, Clone, PartialEq)]
struct RelationshipPattern {
    variable: Option<String>,
    /// Empty means any type.
    types: Vec<EdgeKind>,
    direction: Direction,
    minimum: u32,
    maximum: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    /// `-[]->`
    Out,
    /// `<-[]-`
    In,
    /// `-[]-`
    Either,
}

#[derive(Debug, Clone, PartialEq)]
enum Predicate {
    And(Box<Predicate>, Box<Predicate>),
    Or(Box<Predicate>, Box<Predicate>),
    Not(Box<Predicate>),
    Compare {
        left: Value,
        operator: Operator,
        right: Value,
    },
    IsNull {
        value: Value,
        negated: bool,
    },
    In {
        value: Value,
        options: Vec<Literal>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operator {
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
    Contains,
    StartsWith,
    EndsWith,
}

#[derive(Debug, Clone, PartialEq)]
enum Value {
    Property { variable: String, key: String },
    Variable(String),
    Literal(Literal),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Literal {
    Text(String),
    Int(i64),
}

#[derive(Debug, Clone, PartialEq)]
struct Projection {
    expression: Expression,
    /// Column name: the alias when one was given, otherwise the source text.
    column: String,
}

#[derive(Debug, Clone, PartialEq)]
enum Expression {
    Value(Value),
    /// `count(*)` is `None`; `count(x)` is `Some("x")`.
    Count(Option<String>),
}

/// Maps a pattern label onto a node kind, accepting the spellings a reader would
/// try.
fn label_kind(label: &str) -> Option<NodeKind> {
    Some(match label.to_ascii_lowercase().as_str() {
        "file" => NodeKind::File,
        "function" | "fn" => NodeKind::Function,
        "struct" | "class" => NodeKind::Struct,
        "method" => NodeKind::Method,
        "interface" | "trait" => NodeKind::Interface,
        "doc" => NodeKind::Doc,
        "endpoint" => NodeKind::Endpoint,
        "schema" => NodeKind::Schema,
        "databasetable" | "database_table" | "table" => NodeKind::DatabaseTable,
        "infraresource" | "infra_resource" | "resource" => NodeKind::InfraResource,
        _ => return None,
    })
}

/// Every label the subset accepts, for an error message that teaches.
const LABELS: &str = "File, Function, Struct, Method, Interface, Doc, Endpoint, Schema, \
                      DatabaseTable, InfraResource";

/// Maps a relationship type onto an edge kind.
fn relationship_kind(name: &str) -> Option<EdgeKind> {
    Some(match name.to_ascii_lowercase().as_str() {
        "calls" => EdgeKind::Calls,
        "imports" => EdgeKind::Imports,
        "inherits" => EdgeKind::Inherits,
        "implements" => EdgeKind::Implements,
        "explains" => EdgeKind::Explains,
        "references" => EdgeKind::References,
        _ => return None,
    })
}

/// Every relationship type the subset accepts.
const RELATIONSHIPS: &str = "CALLS, IMPORTS, INHERITS, IMPLEMENTS, EXPLAINS, REFERENCES";

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

struct Parser<'a> {
    source: &'a str,
    tokens: Vec<(Token, usize)>,
    cursor: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.cursor).map(|(token, _)| token)
    }

    fn offset(&self) -> usize {
        self.tokens
            .get(self.cursor)
            .map_or(self.source.len(), |(_, offset)| *offset)
    }

    fn error(&self, detail: impl Into<String>) -> Error {
        let found = match self.peek() {
            Some(Token::Word(word)) => format!(" (found `{word}`)"),
            Some(Token::Text(text)) => format!(" (found the string `{text}`)"),
            Some(Token::Int(value)) => format!(" (found `{value}`)"),
            Some(Token::Symbol(symbol)) => format!(" (found `{symbol}`)"),
            None => " (query ended)".to_string(),
        };
        error_at(
            self.source,
            self.offset(),
            format!("{}{found}", detail.into()),
        )
    }

    fn at_word(&self, word: &str) -> bool {
        matches!(self.peek(), Some(Token::Word(found)) if found.eq_ignore_ascii_case(word))
    }

    fn take_word(&mut self, word: &str) -> bool {
        if self.at_word(word) {
            self.cursor += 1;
            return true;
        }
        false
    }

    fn expect_word(&mut self, word: &str) -> Result<()> {
        if self.take_word(word) {
            return Ok(());
        }
        Err(self.error(format!("expected `{}`", word.to_ascii_uppercase())))
    }

    fn at_symbol(&self, symbol: &str) -> bool {
        matches!(self.peek(), Some(Token::Symbol(found)) if *found == symbol)
    }

    fn take_symbol(&mut self, symbol: &str) -> bool {
        if self.at_symbol(symbol) {
            self.cursor += 1;
            return true;
        }
        false
    }

    fn expect_symbol(&mut self, symbol: &str) -> Result<()> {
        if self.take_symbol(symbol) {
            return Ok(());
        }
        Err(self.error(format!("expected `{symbol}`")))
    }

    /// A name usable as a variable, label, property key, or alias.
    fn take_name(&mut self) -> Option<String> {
        let Some(Token::Word(word)) = self.peek() else {
            return None;
        };
        let word = word.clone();
        self.cursor += 1;
        Some(word)
    }

    fn expect_variable(&mut self) -> Result<String> {
        let offset = self.offset();
        let Some(name) = self.take_name() else {
            return Err(self.error("expected a variable name"));
        };
        if RESERVED.contains(&name.to_ascii_lowercase().as_str()) {
            return Err(error_at(
                self.source,
                offset,
                format!("`{name}` is a reserved word and cannot name a variable"),
            ));
        }
        Ok(name)
    }

    fn expect_integer(&mut self) -> Result<i64> {
        if let Some(Token::Int(value)) = self.peek() {
            let value = *value;
            self.cursor += 1;
            return Ok(value);
        }
        Err(self.error("expected an integer"))
    }

    fn take_literal(&mut self) -> Option<Literal> {
        match self.peek() {
            Some(Token::Text(text)) => {
                let literal = Literal::Text(text.clone());
                self.cursor += 1;
                Some(literal)
            }
            Some(Token::Int(value)) => {
                let literal = Literal::Int(*value);
                self.cursor += 1;
                Some(literal)
            }
            _ => None,
        }
    }
}

/// Parses one query, or explains why it is not in the subset.
///
/// # Errors
/// Returns [`Error::Query`] for a write, an unsupported clause, a syntax error,
/// or an unknown label, relationship type, or property.
pub fn parse(source: &str) -> Result<Query> {
    let tokens = lex(source)?;
    reject_out_of_subset(source, &tokens)?;
    let mut parser = Parser {
        source,
        tokens,
        cursor: 0,
    };
    parser.expect_word("match")?;
    let mut patterns = vec![parse_pattern(&mut parser)?];
    while parser.take_symbol(",") {
        patterns.push(parse_pattern(&mut parser)?);
    }
    let filter = if parser.take_word("where") {
        Some(parse_predicate(&mut parser)?)
    } else {
        None
    };
    parser.expect_word("return")?;
    let distinct = parser.take_word("distinct");
    let mut projections = vec![parse_projection(&mut parser)?];
    while parser.take_symbol(",") {
        projections.push(parse_projection(&mut parser)?);
    }
    let order = parse_order(&mut parser, &projections)?;
    let (skip, limit, limit_clamped) = parse_paging(&mut parser)?;
    if parser.peek().is_some() {
        return Err(parser.error("unexpected input after the end of the query"));
    }
    Ok(Query {
        patterns,
        filter,
        projections,
        distinct,
        order,
        skip,
        limit,
        limit_clamped,
    })
}

/// Rejects writes and unsupported clauses before parsing, so the message is
/// about what the query wanted rather than where it stopped parsing.
fn reject_out_of_subset(source: &str, tokens: &[(Token, usize)]) -> Result<()> {
    let mut previous = String::new();
    for (token, offset) in tokens {
        let Token::Word(word) = token else {
            previous.clear();
            continue;
        };
        let lowered = word.to_ascii_lowercase();
        if WRITE_WORDS.contains(&lowered.as_str()) {
            return Err(error_at(
                source,
                *offset,
                format!(
                    "`{}` writes to the graph; this surface is read-only",
                    word.to_ascii_uppercase()
                ),
            ));
        }
        // `WITH` is part of `STARTS WITH`/`ENDS WITH`; on its own it is the
        // Cypher clause, which this subset does not have.
        let standalone_with = lowered == "with" && previous != "starts" && previous != "ends";
        if standalone_with || UNSUPPORTED_WORDS.contains(&lowered.as_str()) {
            return Err(error_at(
                source,
                *offset,
                format!(
                    "`{}` is outside the supported subset — see docs/query.md",
                    word.to_ascii_uppercase()
                ),
            ));
        }
        previous = lowered;
    }
    Ok(())
}

fn parse_pattern(parser: &mut Parser<'_>) -> Result<Pattern> {
    let start = parse_node_pattern(parser)?;
    let mut hops = Vec::new();
    while parser.at_symbol("-") || parser.at_symbol("<-") {
        let relationship = parse_relationship_pattern(parser)?;
        hops.push((relationship, parse_node_pattern(parser)?));
    }
    Ok(Pattern { start, hops })
}

fn parse_node_pattern(parser: &mut Parser<'_>) -> Result<NodePattern> {
    parser.expect_symbol("(")?;
    let variable = if parser.at_symbol(":") || parser.at_symbol(")") || parser.at_symbol("{") {
        None
    } else {
        Some(parser.expect_variable()?)
    };
    let mut label = None;
    if parser.take_symbol(":") {
        let offset = parser.offset();
        let Some(name) = parser.take_name() else {
            return Err(parser.error("expected a label after `:`"));
        };
        label = Some(label_kind(&name).ok_or_else(|| {
            error_at(
                parser.source,
                offset,
                format!("unknown label `{name}` — the graph has: {LABELS}"),
            )
        })?);
    }
    let properties = parse_property_map(parser, NODE_PROPERTIES, "node")?;
    parser.expect_symbol(")")?;
    Ok(NodePattern {
        variable,
        label,
        properties,
    })
}

fn parse_property_map(
    parser: &mut Parser<'_>,
    allowed: &[&str],
    owner: &str,
) -> Result<Vec<(String, Literal)>> {
    let mut properties = Vec::new();
    if !parser.take_symbol("{") {
        return Ok(properties);
    }
    loop {
        let offset = parser.offset();
        let Some(key) = parser.take_name() else {
            return Err(parser.error("expected a property name"));
        };
        check_property(parser.source, offset, &key, allowed, owner)?;
        parser.expect_symbol(":")?;
        let Some(literal) = parser.take_literal() else {
            return Err(parser.error("expected a string or integer literal"));
        };
        properties.push((key.to_ascii_lowercase(), literal));
        if !parser.take_symbol(",") {
            break;
        }
    }
    parser.expect_symbol("}")?;
    Ok(properties)
}

/// Rejects a property the graph does not have, rather than matching nothing and
/// letting the reader believe the answer.
fn check_property(
    source: &str,
    offset: usize,
    key: &str,
    allowed: &[&str],
    owner: &str,
) -> Result<()> {
    if allowed.contains(&key.to_ascii_lowercase().as_str()) {
        return Ok(());
    }
    Err(error_at(
        source,
        offset,
        format!(
            "a {owner} has no property `{key}` — it has: {}",
            allowed.join(", ")
        ),
    ))
}

fn parse_relationship_pattern(parser: &mut Parser<'_>) -> Result<RelationshipPattern> {
    let leading_left = parser.take_symbol("<-");
    if !leading_left {
        parser.expect_symbol("-")?;
    }
    let mut variable = None;
    let mut types = Vec::new();
    let mut minimum = 1;
    let mut maximum = 1;
    if parser.take_symbol("[") {
        if !parser.at_symbol(":") && !parser.at_symbol("*") && !parser.at_symbol("]") {
            variable = Some(parser.expect_variable()?);
        }
        if parser.take_symbol(":") {
            loop {
                let offset = parser.offset();
                let Some(name) = parser.take_name() else {
                    return Err(parser.error("expected a relationship type after `:`"));
                };
                types.push(relationship_kind(&name).ok_or_else(|| {
                    error_at(
                        parser.source,
                        offset,
                        format!(
                            "unknown relationship type `{name}` — the graph has: {RELATIONSHIPS}"
                        ),
                    )
                })?);
                if !parser.take_symbol("|") {
                    break;
                }
            }
        }
        if parser.take_symbol("*") {
            (minimum, maximum) = parse_hop_range(parser)?;
        }
        parser.expect_symbol("]")?;
    }
    let direction = if leading_left {
        parser.expect_symbol("-")?;
        Direction::In
    } else if parser.take_symbol("->") {
        Direction::Out
    } else if parser.take_symbol("-") {
        Direction::Either
    } else {
        return Err(parser.error("expected `->` or `-` to close the relationship"));
    };
    Ok(RelationshipPattern {
        variable,
        types,
        direction,
        minimum,
        maximum,
    })
}

/// `*`, `*2`, `*1..3`, `*..3`, `*2..` — an absent upper bound becomes
/// [`DEFAULT_MAX_HOPS`], because unbounded means unbounded work.
fn parse_hop_range(parser: &mut Parser<'_>) -> Result<(u32, u32)> {
    let mut minimum: u32 = 1;
    let mut maximum: u32 = DEFAULT_MAX_HOPS;
    let mut saw_bound = false;
    if let Some(Token::Int(_)) = parser.peek() {
        let offset = parser.offset();
        minimum = hop_count(parser.expect_integer()?, parser.source, offset)?;
        maximum = minimum;
        saw_bound = true;
    }
    if parser.take_symbol("..") {
        maximum = DEFAULT_MAX_HOPS;
        if let Some(Token::Int(_)) = parser.peek() {
            let offset = parser.offset();
            maximum = hop_count(parser.expect_integer()?, parser.source, offset)?;
        }
        if !saw_bound {
            minimum = 1;
        }
    }
    if minimum == 0 {
        return Err(Error::Query {
            detail: "a variable-length relationship must span at least one hop \
                     (`*0..` is not in the subset)"
                .to_string(),
        });
    }
    if maximum < minimum {
        return Err(Error::Query {
            detail: format!("hop range {minimum}..{maximum} is empty"),
        });
    }
    Ok((minimum, maximum))
}

fn hop_count(value: i64, source: &str, offset: usize) -> Result<u32> {
    u32::try_from(value)
        .ok()
        .filter(|hops| *hops <= DEFAULT_MAX_HOPS)
        .ok_or_else(|| {
            error_at(
                source,
                offset,
                format!("a hop count must be between 1 and {DEFAULT_MAX_HOPS}"),
            )
        })
}

fn parse_predicate(parser: &mut Parser<'_>) -> Result<Predicate> {
    let mut left = parse_conjunction(parser)?;
    while parser.take_word("or") {
        let right = parse_conjunction(parser)?;
        left = Predicate::Or(Box::new(left), Box::new(right));
    }
    Ok(left)
}

fn parse_conjunction(parser: &mut Parser<'_>) -> Result<Predicate> {
    let mut left = parse_unary(parser)?;
    while parser.take_word("and") {
        let right = parse_unary(parser)?;
        left = Predicate::And(Box::new(left), Box::new(right));
    }
    Ok(left)
}

fn parse_unary(parser: &mut Parser<'_>) -> Result<Predicate> {
    if parser.take_word("not") {
        return Ok(Predicate::Not(Box::new(parse_unary(parser)?)));
    }
    if parser.take_symbol("(") {
        let inner = parse_predicate(parser)?;
        parser.expect_symbol(")")?;
        return Ok(inner);
    }
    parse_comparison(parser)
}

fn parse_comparison(parser: &mut Parser<'_>) -> Result<Predicate> {
    let left = parse_value(parser)?;
    if parser.take_word("is") {
        let negated = parser.take_word("not");
        parser.expect_word("null")?;
        return Ok(Predicate::IsNull {
            value: left,
            negated,
        });
    }
    if parser.take_word("in") {
        parser.expect_symbol("[")?;
        let mut options = Vec::new();
        loop {
            let Some(literal) = parser.take_literal() else {
                return Err(parser.error("expected a literal inside the list"));
            };
            options.push(literal);
            if !parser.take_symbol(",") {
                break;
            }
        }
        parser.expect_symbol("]")?;
        return Ok(Predicate::In {
            value: left,
            options,
        });
    }
    let operator = parse_operator(parser)?;
    let right = parse_value(parser)?;
    Ok(Predicate::Compare {
        left,
        operator,
        right,
    })
}

fn parse_operator(parser: &mut Parser<'_>) -> Result<Operator> {
    for (symbol, operator) in [
        ("<>", Operator::NotEqual),
        ("<=", Operator::LessOrEqual),
        (">=", Operator::GreaterOrEqual),
        ("=", Operator::Equal),
        ("<", Operator::Less),
        (">", Operator::Greater),
    ] {
        if parser.take_symbol(symbol) {
            return Ok(operator);
        }
    }
    if parser.take_word("contains") {
        return Ok(Operator::Contains);
    }
    if parser.take_word("starts") {
        parser.expect_word("with")?;
        return Ok(Operator::StartsWith);
    }
    if parser.take_word("ends") {
        parser.expect_word("with")?;
        return Ok(Operator::EndsWith);
    }
    Err(parser.error(
        "expected a comparison: `=`, `<>`, `<`, `<=`, `>`, `>=`, CONTAINS, \
         STARTS WITH, ENDS WITH, IN, or IS NULL",
    ))
}

fn parse_value(parser: &mut Parser<'_>) -> Result<Value> {
    if let Some(literal) = parser.take_literal() {
        return Ok(Value::Literal(literal));
    }
    let variable = parser.expect_variable()?;
    if !parser.take_symbol(".") {
        return Ok(Value::Variable(variable));
    }
    let offset = parser.offset();
    let Some(key) = parser.take_name() else {
        return Err(parser.error("expected a property name after `.`"));
    };
    // Which properties are legal depends on what the variable is bound to,
    // which is not known until evaluation; both tables are accepted here and
    // the unknown-property error is raised there.
    let known = NODE_PROPERTIES
        .iter()
        .chain(RELATIONSHIP_PROPERTIES)
        .any(|allowed| allowed.eq_ignore_ascii_case(&key));
    if !known {
        return Err(error_at(
            parser.source,
            offset,
            format!(
                "unknown property `{key}` — a node has {}; a relationship has {}",
                NODE_PROPERTIES.join(", "),
                RELATIONSHIP_PROPERTIES.join(", ")
            ),
        ));
    }
    Ok(Value::Property {
        variable,
        key: key.to_ascii_lowercase(),
    })
}

fn parse_projection(parser: &mut Parser<'_>) -> Result<Projection> {
    let start = parser.offset();
    let expression = if parser.at_word("count") {
        parser.cursor += 1;
        parser.expect_symbol("(")?;
        let counted = if parser.take_symbol("*") {
            None
        } else {
            Some(parser.expect_variable()?)
        };
        parser.expect_symbol(")")?;
        Expression::Count(counted)
    } else {
        Expression::Value(parse_value(parser)?)
    };
    let end = parser.offset();
    let column = if parser.take_word("as") {
        parser.expect_variable()?
    } else {
        parser.source[start..end.min(parser.source.len())]
            .trim()
            .to_string()
    };
    Ok(Projection { expression, column })
}

/// `ORDER BY` names returned columns, not arbitrary expressions: with `count`
/// in the projection, ordering by anything else has no defined meaning here.
fn parse_order(parser: &mut Parser<'_>, projections: &[Projection]) -> Result<Vec<(String, bool)>> {
    let mut order = Vec::new();
    if !parser.take_word("order") {
        return Ok(order);
    }
    parser.expect_word("by")?;
    loop {
        let offset = parser.offset();
        let Some(mut column) = parser.take_name() else {
            return Err(parser.error("expected a returned column name"));
        };
        if parser.take_symbol(".") {
            let Some(key) = parser.take_name() else {
                return Err(parser.error("expected a property name after `.`"));
            };
            column = format!("{column}.{key}");
        }
        if !projections
            .iter()
            .any(|projection| projection.column.eq_ignore_ascii_case(&column))
        {
            return Err(error_at(
                parser.source,
                offset,
                format!(
                    "ORDER BY must name a returned column; this query returns: {}",
                    projections
                        .iter()
                        .map(|projection| projection.column.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
        let descending = if parser.take_word("desc") {
            true
        } else {
            parser.take_word("asc");
            false
        };
        order.push((column, descending));
        if !parser.take_symbol(",") {
            break;
        }
    }
    Ok(order)
}

fn parse_paging(parser: &mut Parser<'_>) -> Result<(usize, usize, bool)> {
    let mut skip = 0;
    let mut limit = DEFAULT_LIMIT;
    let mut clamped = false;
    for _ in 0..2 {
        if parser.take_word("skip") {
            let offset = parser.offset();
            skip = usize::try_from(parser.expect_integer()?)
                .map_err(|_| error_at(parser.source, offset, "SKIP must not be negative"))?;
        } else if parser.take_word("limit") {
            let offset = parser.offset();
            let requested = usize::try_from(parser.expect_integer()?)
                .map_err(|_| error_at(parser.source, offset, "LIMIT must not be negative"))?;
            limit = requested.min(MAX_LIMIT);
            clamped = requested > MAX_LIMIT;
        }
    }
    Ok((skip, limit, clamped))
}

// ---------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------

/// What a variable is bound to in one candidate row.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Bound {
    /// Position in [`Index::nodes`].
    Node(usize),
    /// Positions in [`Index::edges`], in traversal order. A fixed-length hop
    /// binds one; a variable-length hop binds the whole path.
    Path(Vec<usize>),
}

type Row = BTreeMap<String, Bound>;

/// One evaluated value.
#[derive(Debug, Clone, PartialEq)]
enum Val {
    Null,
    Int(i64),
    Text(String),
    Node(usize),
    Path(Vec<usize>),
}

/// The graph, in the shape pattern matching needs: nodes in a stable order and
/// adjacency by node id.
struct Index {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    position: HashMap<i64, usize>,
    outgoing: HashMap<i64, Vec<usize>>,
    incoming: HashMap<i64, Vec<usize>>,
}

impl Index {
    /// Loads the graph once. Node order is by id, so equal queries return equal
    /// rows in equal order.
    fn build(graph: &Graph) -> Result<Self> {
        let mut nodes = graph.all_nodes()?;
        nodes.sort_by_key(|node| node.id.unwrap_or_default());
        let position = nodes
            .iter()
            .enumerate()
            .filter_map(|(index, node)| node.id.map(|id| (id, index)))
            .collect();
        let edges = graph.all_edges()?;
        let mut outgoing: HashMap<i64, Vec<usize>> = HashMap::new();
        let mut incoming: HashMap<i64, Vec<usize>> = HashMap::new();
        for (index, edge) in edges.iter().enumerate() {
            outgoing.entry(edge.src).or_default().push(index);
            incoming.entry(edge.dst).or_default().push(index);
        }
        Ok(Self {
            nodes,
            edges,
            position,
            outgoing,
            incoming,
        })
    }

    /// Whether one node satisfies a node pattern.
    ///
    /// This is the whole of the query's pushdown: a label and a literal property
    /// map are checked before the node is bound, so `(:Function {name: 'x'})`
    /// never builds a row per node in the repository. `WHERE` runs later, on
    /// rows.
    fn node_matches(&self, index: usize, pattern: &NodePattern) -> bool {
        let Some(node) = self.nodes.get(index) else {
            return false;
        };
        if let Some(label) = pattern.label
            && node.kind != label
        {
            return false;
        }
        pattern.properties.iter().all(|(key, literal)| {
            compare(
                &node_property(node, key),
                Operator::Equal,
                &literal_value(literal),
            )
        })
    }

    /// Edges leaving `index` in the direction and of the types a pattern asks
    /// for, as `(edge position, other node position)`.
    fn step(&self, index: usize, pattern: &RelationshipPattern) -> Vec<(usize, usize)> {
        let Some(id) = self.nodes.get(index).and_then(|node| node.id) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let forward = matches!(pattern.direction, Direction::Out | Direction::Either);
        let backward = matches!(pattern.direction, Direction::In | Direction::Either);
        if forward {
            self.collect_step(self.outgoing.get(&id), pattern, true, &mut out);
        }
        if backward {
            self.collect_step(self.incoming.get(&id), pattern, false, &mut out);
        }
        out
    }

    fn collect_step(
        &self,
        candidates: Option<&Vec<usize>>,
        pattern: &RelationshipPattern,
        forward: bool,
        out: &mut Vec<(usize, usize)>,
    ) {
        for edge_index in candidates.into_iter().flatten() {
            let Some(edge) = self.edges.get(*edge_index) else {
                continue;
            };
            if !pattern.types.is_empty() && !pattern.types.contains(&edge.kind) {
                continue;
            }
            let other = if forward { edge.dst } else { edge.src };
            if let Some(node_index) = self.position.get(&other) {
                out.push((*edge_index, *node_index));
            }
        }
    }

    /// Paths from `index` whose length is inside the pattern's hop range.
    ///
    /// An edge is never repeated within one path, which is what makes a cycle
    /// terminate. Traversal is depth-first over a stack so a wide graph does not
    /// recurse as deep as it is broad, and the expansion is budgeted here rather
    /// than only where rows are collected: a hub node at five hops can hold more
    /// paths than the whole answer would ever contain.
    fn paths(
        &self,
        index: usize,
        pattern: &RelationshipPattern,
    ) -> Result<Vec<(Vec<usize>, usize)>> {
        let mut found = Vec::new();
        let mut stack = vec![(index, Vec::new())];
        while let Some((node, trail)) = stack.pop() {
            budget(found.len() + stack.len())?;
            let depth = u32::try_from(trail.len()).unwrap_or(u32::MAX);
            if depth >= pattern.minimum && !trail.is_empty() {
                found.push((trail.clone(), node));
            }
            if depth >= pattern.maximum {
                continue;
            }
            for (edge_index, next) in self.step(node, pattern) {
                if trail.contains(&edge_index) {
                    continue;
                }
                let mut extended = trail.clone();
                extended.push(edge_index);
                stack.push((next, extended));
            }
        }
        Ok(found)
    }

    /// Every row that satisfies the patterns and the `WHERE` clause.
    fn rows(&self, query: &Query) -> Result<Vec<Row>> {
        let mut rows = vec![Row::new()];
        for pattern in &query.patterns {
            rows = self.expand(pattern, rows)?;
            if rows.is_empty() {
                return Ok(rows);
            }
        }
        if let Some(filter) = &query.filter {
            let mut kept = Vec::new();
            for row in rows {
                if self.truth(filter, &row)? {
                    kept.push(row);
                }
            }
            return Ok(kept);
        }
        Ok(rows)
    }

    /// Extends every row with the bindings one pattern adds.
    fn expand(&self, pattern: &Pattern, rows: Vec<Row>) -> Result<Vec<Row>> {
        let mut out = Vec::new();
        for row in rows {
            for start in self.candidates(&pattern.start, &row) {
                let mut seeded = row.clone();
                bind_node(&mut seeded, pattern.start.variable.as_deref(), start);
                self.walk(pattern, 0, start, seeded, &mut out)?;
            }
        }
        Ok(out)
    }

    /// Nodes a pattern may bind, honoring a binding the row already has.
    fn candidates(&self, pattern: &NodePattern, row: &Row) -> Vec<usize> {
        if let Some(variable) = &pattern.variable
            && let Some(Bound::Node(index)) = row.get(variable)
        {
            return if self.node_matches(*index, pattern) {
                vec![*index]
            } else {
                Vec::new()
            };
        }
        (0..self.nodes.len())
            .filter(|index| self.node_matches(*index, pattern))
            .collect()
    }

    /// Walks the hops of one pattern from an already-bound node.
    fn walk(
        &self,
        pattern: &Pattern,
        hop: usize,
        from: usize,
        row: Row,
        out: &mut Vec<Row>,
    ) -> Result<()> {
        let Some((relationship, target)) = pattern.hops.get(hop) else {
            out.push(row);
            return budget(out.len());
        };
        for (trail, end) in self.paths(from, relationship)? {
            if !self.node_matches(end, target) {
                continue;
            }
            let mut extended = row.clone();
            if let Some(variable) = &relationship.variable {
                match extended.get(variable) {
                    Some(Bound::Path(bound)) if *bound != trail => continue,
                    Some(Bound::Node(_)) => continue,
                    _ => {
                        extended.insert(variable.clone(), Bound::Path(trail));
                    }
                }
            }
            if let Some(variable) = &target.variable {
                match extended.get(variable) {
                    Some(Bound::Node(bound)) if *bound != end => continue,
                    Some(Bound::Path(_)) => continue,
                    _ => {
                        extended.insert(variable.clone(), Bound::Node(end));
                    }
                }
            }
            budget(out.len())?;
            self.walk(pattern, hop + 1, end, extended, out)?;
        }
        Ok(())
    }

    /// Truth of a predicate for one row.
    fn truth(&self, predicate: &Predicate, row: &Row) -> Result<bool> {
        Ok(match predicate {
            Predicate::And(left, right) => self.truth(left, row)? && self.truth(right, row)?,
            Predicate::Or(left, right) => self.truth(left, row)? || self.truth(right, row)?,
            Predicate::Not(inner) => !self.truth(inner, row)?,
            Predicate::Compare {
                left,
                operator,
                right,
            } => compare(&self.value(left, row)?, *operator, &self.value(right, row)?),
            Predicate::IsNull { value, negated } => {
                (self.value(value, row)? == Val::Null) != *negated
            }
            Predicate::In { value, options } => {
                let found = self.value(value, row)?;
                options
                    .iter()
                    .any(|option| compare(&found, Operator::Equal, &literal_value(option)))
            }
        })
    }

    /// One value in the context of a row.
    fn value(&self, value: &Value, row: &Row) -> Result<Val> {
        Ok(match value {
            Value::Literal(literal) => literal_value(literal),
            Value::Variable(variable) => match row.get(variable) {
                Some(Bound::Node(index)) => Val::Node(*index),
                Some(Bound::Path(trail)) => Val::Path(trail.clone()),
                None => return Err(unbound(variable)),
            },
            Value::Property { variable, key } => match row.get(variable) {
                Some(Bound::Node(index)) => self
                    .nodes
                    .get(*index)
                    .map_or(Val::Null, |node| node_property(node, key)),
                Some(Bound::Path(trail)) => self.path_property(trail, key),
                None => return Err(unbound(variable)),
            },
        })
    }

    /// A relationship property, which a multi-hop path does not have: the value
    /// would have to be one edge's, and picking one silently is a wrong answer.
    fn path_property(&self, trail: &[usize], key: &str) -> Val {
        if !RELATIONSHIP_PROPERTIES.contains(&key) {
            return Val::Null;
        }
        let [single] = trail else {
            return Val::Null;
        };
        self.edges.get(*single).map_or(Val::Null, |edge| match key {
            "type" => Val::Text(edge.kind.as_str().to_string()),
            _ => Val::Text(edge.confidence.as_str().to_string()),
        })
    }

    /// Renders a value as JSON for output.
    fn json(&self, value: &Val) -> Json {
        match value {
            Val::Null => Json::Null,
            Val::Int(number) => json!(number),
            Val::Text(text) => json!(text),
            Val::Node(index) => self.nodes.get(*index).map_or(Json::Null, |node| {
                json!({
                    "id": node.id,
                    "kind": node.kind.as_str(),
                    "name": node.name,
                    "file": node.file_path,
                    "line": node.start_line,
                    "end_line": node.end_line,
                })
            }),
            Val::Path(trail) => Json::Array(
                trail
                    .iter()
                    .filter_map(|index| self.edges.get(*index))
                    .map(|edge| {
                        json!({
                            "type": edge.kind.as_str(),
                            "confidence": edge.confidence.as_str(),
                            "source": self.name_of(edge.src),
                            "target": self.name_of(edge.dst),
                        })
                    })
                    .collect(),
            ),
        }
    }

    fn name_of(&self, id: i64) -> Option<&str> {
        self.position
            .get(&id)
            .and_then(|index| self.nodes.get(*index))
            .map(|node| node.name.as_str())
    }
}

fn bind_node(row: &mut Row, variable: Option<&str>, index: usize) {
    if let Some(variable) = variable {
        row.insert(variable.to_string(), Bound::Node(index));
    }
}

fn unbound(variable: &str) -> Error {
    Error::Query {
        detail: format!("`{variable}` is not bound by any pattern in this query"),
    }
}

/// Rejects a query that would hold more intermediate rows than the budget,
/// with the advice that would make it answerable.
fn budget(rows: usize) -> Result<()> {
    if rows <= ROW_BUDGET {
        return Ok(());
    }
    Err(Error::Query {
        detail: format!(
            "the pattern produced more than {ROW_BUDGET} intermediate rows — \
             narrow it with a label, a property, or a shorter hop range"
        ),
    })
}

fn literal_value(literal: &Literal) -> Val {
    match literal {
        Literal::Text(text) => Val::Text(text.clone()),
        Literal::Int(number) => Val::Int(*number),
    }
}

fn node_property(node: &Node, key: &str) -> Val {
    match key {
        "id" => node.id.map_or(Val::Null, Val::Int),
        "kind" => Val::Text(node.kind.as_str().to_string()),
        "name" => Val::Text(node.name.clone()),
        "file" => Val::Text(node.file_path.clone()),
        "line" => Val::Int(i64::from(node.start_line)),
        "end_line" => Val::Int(i64::from(node.end_line)),
        _ => Val::Null,
    }
}

/// Compares two values. Mismatched types and nulls are never equal and never
/// ordered, which keeps a typo in a query from reading as a real answer.
fn compare(left: &Val, operator: Operator, right: &Val) -> bool {
    match (left, right) {
        (Val::Int(left), Val::Int(right)) => match operator {
            Operator::Equal => left == right,
            Operator::NotEqual => left != right,
            Operator::Less => left < right,
            Operator::LessOrEqual => left <= right,
            Operator::Greater => left > right,
            Operator::GreaterOrEqual => left >= right,
            Operator::Contains | Operator::StartsWith | Operator::EndsWith => false,
        },
        (Val::Text(left), Val::Text(right)) => match operator {
            Operator::Equal => left == right,
            Operator::NotEqual => left != right,
            Operator::Less => left < right,
            Operator::LessOrEqual => left <= right,
            Operator::Greater => left > right,
            Operator::GreaterOrEqual => left >= right,
            Operator::Contains => left.contains(right.as_str()),
            Operator::StartsWith => left.starts_with(right.as_str()),
            Operator::EndsWith => left.ends_with(right.as_str()),
        },
        _ => operator == Operator::NotEqual && left != right,
    }
}

// ---------------------------------------------------------------------------
// Projection and output
// ---------------------------------------------------------------------------

/// A result set: the columns in the order the query asked for them, the rows,
/// and whether the answer was cut short.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// Column names, in `RETURN` order.
    pub columns: Vec<String>,
    /// One entry per row, aligned with [`Answer::columns`].
    pub rows: Vec<Vec<Json>>,
    /// Whether rows were dropped by the limit — so a reader never mistakes a
    /// page for the whole answer.
    pub truncated: bool,
}

impl Answer {
    /// The answer as JSON: `{"columns": [...], "rows": [...], "truncated": ...}`.
    ///
    /// Rows are arrays rather than objects because column order is part of the
    /// answer, and a JSON object's key order is not.
    #[must_use]
    pub fn to_json(&self) -> String {
        let payload = json!({
            "columns": self.columns,
            "rows": self.rows,
            "truncated": self.truncated,
        });
        serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
    }

    /// The answer as aligned text, for a terminal.
    #[must_use]
    pub fn to_table(&self) -> String {
        if self.rows.is_empty() {
            return "no rows".to_string();
        }
        let cells: Vec<Vec<String>> = self
            .rows
            .iter()
            .map(|row| row.iter().map(cell_text).collect())
            .collect();
        let widths: Vec<usize> = self
            .columns
            .iter()
            .enumerate()
            .map(|(index, column)| {
                cells
                    .iter()
                    .filter_map(|row| row.get(index))
                    .map(|cell| cell.chars().count())
                    .chain(std::iter::once(column.chars().count()))
                    .max()
                    .unwrap_or_default()
            })
            .collect();
        let mut lines = vec![join_row(&self.columns, &widths)];
        lines.push(
            widths
                .iter()
                .map(|width| "-".repeat(*width))
                .collect::<Vec<_>>()
                .join("  "),
        );
        for row in &cells {
            lines.push(join_row(row, &widths));
        }
        if self.truncated {
            lines.push(format!(
                "({} rows shown; more were dropped by the limit)",
                self.rows.len()
            ));
        }
        lines.join("\n")
    }
}

fn join_row<S: AsRef<str>>(cells: &[S], widths: &[usize]) -> String {
    cells
        .iter()
        .enumerate()
        .map(|(index, cell)| {
            let text = cell.as_ref();
            let width = widths.get(index).copied().unwrap_or_default();
            let padding = width.saturating_sub(text.chars().count());
            format!("{text}{}", " ".repeat(padding))
        })
        .collect::<Vec<_>>()
        .join("  ")
        .trim_end()
        .to_string()
}

/// One JSON value as a table cell: a node reads as `name (file:line)`, because
/// a table of nested objects is not a table.
fn cell_text(value: &Json) -> String {
    match value {
        Json::Null => "null".to_string(),
        Json::String(text) => text.clone(),
        Json::Object(fields) => match (fields.get("name"), fields.get("file")) {
            (Some(Json::String(name)), Some(Json::String(file))) => {
                let line = fields
                    .get("line")
                    .and_then(Json::as_u64)
                    .unwrap_or_default();
                format!("{name} ({file}:{line})")
            }
            _ => value.to_string(),
        },
        Json::Array(items) => items.iter().map(cell_text).collect::<Vec<_>>().join(" -> "),
        other => other.to_string(),
    }
}

/// Evaluates a parsed query against an open graph.
///
/// # Errors
/// Returns [`Error::Query`] when a variable is unbound or the pattern exceeds
/// the row budget, or [`Error::Storage`] when the graph cannot be read.
pub fn evaluate(graph: &Graph, query: &Query) -> Result<Answer> {
    let index = Index::build(graph)?;
    let rows = index.rows(query)?;
    let mut projected = project(&index, query, &rows)?;
    for (column, descending) in query.order.iter().rev() {
        let Some(position) = query
            .projections
            .iter()
            .position(|projection| projection.column.eq_ignore_ascii_case(column))
        else {
            continue;
        };
        projected.sort_by(|left, right| {
            let ordering = order_json(left.get(position), right.get(position));
            if *descending {
                ordering.reverse()
            } else {
                ordering
            }
        });
    }
    if query.distinct {
        let mut seen = std::collections::HashSet::new();
        projected.retain(|row| seen.insert(row.iter().map(Json::to_string).collect::<Vec<_>>()));
    }
    let total = projected.len();
    let page: Vec<Vec<Json>> = projected
        .into_iter()
        .skip(query.skip)
        .take(query.limit)
        .collect();
    let truncated = query.limit_clamped || query.skip + page.len() < total;
    Ok(Answer {
        columns: query
            .projections
            .iter()
            .map(|projection| projection.column.clone())
            .collect(),
        rows: page,
        truncated,
    })
}

/// Applies the projection, folding rows into groups when `count` is present.
///
/// Grouping is by the non-aggregate columns, which is the one aggregation rule
/// this subset has and the only one it claims.
fn project(index: &Index, query: &Query, rows: &[Row]) -> Result<Vec<Vec<Json>>> {
    let aggregating = query
        .projections
        .iter()
        .any(|projection| matches!(projection.expression, Expression::Count(_)));
    if !aggregating {
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(project_row(index, query, row)?);
        }
        return Ok(out);
    }
    // `BTreeMap` so groups come out in a stable order rather than a hashed one.
    let mut groups: BTreeMap<Vec<String>, (Vec<Json>, usize, Vec<usize>)> = BTreeMap::new();
    for row in rows {
        let mut key = Vec::new();
        let mut values = Vec::new();
        let mut counted = Vec::new();
        for (position, projection) in query.projections.iter().enumerate() {
            match &projection.expression {
                Expression::Value(value) => {
                    let rendered = index.json(&index.value(value, row)?);
                    key.push(rendered.to_string());
                    values.push(rendered);
                }
                Expression::Count(variable) => {
                    values.push(Json::Null);
                    if variable
                        .as_ref()
                        .is_none_or(|variable| row.contains_key(variable))
                    {
                        counted.push(position);
                    }
                }
            }
        }
        let entry = groups.entry(key).or_insert((values, 0, Vec::new()));
        entry.1 += 1;
        entry.2 = counted;
    }
    Ok(groups
        .into_values()
        .map(|(mut values, rows_in_group, counted)| {
            for position in counted {
                if let Some(slot) = values.get_mut(position) {
                    *slot = json!(rows_in_group);
                }
            }
            values
        })
        .collect())
}

fn project_row(index: &Index, query: &Query, row: &Row) -> Result<Vec<Json>> {
    let mut out = Vec::with_capacity(query.projections.len());
    for projection in &query.projections {
        match &projection.expression {
            Expression::Value(value) => out.push(index.json(&index.value(value, row)?)),
            Expression::Count(_) => out.push(Json::Null),
        }
    }
    Ok(out)
}

/// Orders two projected cells: numbers numerically, strings lexicographically,
/// nulls last.
fn order_json(left: Option<&Json>, right: Option<&Json>) -> std::cmp::Ordering {
    match (left, right) {
        (Some(Json::Number(left)), Some(Json::Number(right))) => left
            .as_f64()
            .partial_cmp(&right.as_f64())
            .unwrap_or(std::cmp::Ordering::Equal),
        (Some(Json::String(left)), Some(Json::String(right))) => left.cmp(right),
        (Some(Json::Null) | None, Some(Json::Null) | None) => std::cmp::Ordering::Equal,
        (Some(Json::Null) | None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), Some(Json::Null) | None) => std::cmp::Ordering::Less,
        (Some(left), Some(right)) => left.to_string().cmp(&right.to_string()),
    }
}

/// Parses and evaluates a query against the graph indexed under `root`.
///
/// # Errors
/// Returns [`Error::Query`] for a query outside the subset, or
/// [`Error::IndexMissing`] when `root` has no graph yet.
pub fn run(root: &std::path::Path, source: &str) -> Result<Answer> {
    let query = parse(source)?;
    let graph = Graph::open_existing(root)?;
    evaluate(&graph, &query)
}

/// The JSON form of [`run`], for the MCP tool.
///
/// # Errors
/// As [`run`].
pub fn run_json(root: &std::path::Path, source: &str) -> Result<String> {
    Ok(run(root, source)?.to_json())
}

/// The table form of [`run`], for the CLI.
///
/// # Errors
/// As [`run`].
pub fn run_table(root: &std::path::Path, source: &str) -> Result<String> {
    Ok(run(root, source)?.to_table())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{Confidence, EdgeKind, NodeKind};

    /// A graph with two files, three functions, and a call chain
    /// `caller -> helper -> leaf`, plus an import and a doc.
    fn graph() -> Graph {
        let graph = Graph::open_in_memory().unwrap();
        let mut ids = Vec::new();
        for (kind, name, file, line) in [
            (NodeKind::File, "a.rs", "a.rs", 1),
            (NodeKind::Function, "caller", "a.rs", 10),
            (NodeKind::Function, "helper", "a.rs", 20),
            (NodeKind::Function, "leaf", "b.rs", 5),
            (NodeKind::Struct, "Widget", "b.rs", 1),
            (NodeKind::Doc, "README.md", "README.md", 1),
        ] {
            ids.push(
                graph
                    .insert_node(&Node {
                        id: None,
                        kind,
                        name: name.to_string(),
                        file_path: file.to_string(),
                        start_line: line,
                        end_line: line + 5,
                        description: None,
                    })
                    .unwrap(),
            );
        }
        for (src, dst, kind, confidence) in [
            (1, 2, EdgeKind::Calls, Confidence::Extracted),
            (2, 3, EdgeKind::Calls, Confidence::Inferred),
            (1, 3, EdgeKind::Calls, Confidence::Ambiguous),
            (0, 4, EdgeKind::Imports, Confidence::Extracted),
            (5, 2, EdgeKind::Explains, Confidence::Inferred),
        ] {
            graph
                .insert_edge(&Edge {
                    src: ids[src],
                    dst: ids[dst],
                    kind,
                    confidence,
                })
                .unwrap();
        }
        graph
    }

    fn answer(source: &str) -> Answer {
        let query = parse(source).unwrap_or_else(|error| panic!("parse `{source}`: {error}"));
        evaluate(&graph(), &query).unwrap_or_else(|error| panic!("evaluate `{source}`: {error}"))
    }

    fn column(source: &str, index: usize) -> Vec<String> {
        answer(source)
            .rows
            .iter()
            .map(|row| cell_text(&row[index]))
            .collect()
    }

    fn rejected(source: &str) -> String {
        match parse(source) {
            Err(error) => error.to_string(),
            Ok(_) => panic!("`{source}` should have been rejected"),
        }
    }

    #[test]
    fn a_label_and_a_property_narrow_the_match() {
        assert_eq!(
            column("MATCH (f:Function {name: 'helper'}) RETURN f.name", 0),
            vec!["helper".to_string()]
        );
        assert_eq!(
            column("MATCH (n:Doc) RETURN n.name", 0),
            vec!["README.md".to_string()]
        );
    }

    #[test]
    fn a_relationship_type_is_actually_honored() {
        // The whole point of the rewrite: the previous surface answered this
        // with every edge in the graph, `IMPORTS` and `EXPLAINS` included.
        let calls = column("MATCH (a)-[:CALLS]->(b) RETURN b.name", 0);
        assert_eq!(calls.len(), 3, "{calls:?}");
        assert!(!calls.contains(&"Widget".to_string()), "{calls:?}");
        let imports = column("MATCH (a)-[:IMPORTS]->(b) RETURN b.name", 0);
        assert_eq!(imports, vec!["Widget".to_string()]);
    }

    #[test]
    fn direction_is_part_of_the_pattern() {
        assert_eq!(
            column("MATCH (a)-[:CALLS]->(b {name: 'helper'}) RETURN a.name", 0),
            vec!["caller".to_string()]
        );
        assert_eq!(
            column("MATCH (a)<-[:CALLS]-(b {name: 'helper'}) RETURN a.name", 0),
            vec!["leaf".to_string()]
        );
        let either = column("MATCH (a {name: 'helper'})-[:CALLS]-(b) RETURN b.name", 0);
        assert_eq!(
            either.len(),
            2,
            "an undirected hop sees both sides: {either:?}"
        );
    }

    #[test]
    fn a_variable_length_hop_walks_the_chain() {
        let reached = column(
            "MATCH (f {name: 'caller'})-[:CALLS*1..2]->(g) RETURN DISTINCT g.name ORDER BY g.name",
            0,
        );
        assert_eq!(
            reached,
            vec!["helper".to_string(), "leaf".to_string()],
            "one and two hops from caller"
        );
        let one_hop = column(
            "MATCH (f {name: 'caller'})-[:CALLS*1]->(g) RETURN DISTINCT g.name",
            0,
        );
        assert_eq!(one_hop.len(), 2, "caller calls helper and leaf directly");
    }

    #[test]
    fn where_filters_on_properties_and_relationship_confidence() {
        assert_eq!(
            column("MATCH (f:Function) WHERE f.file = 'b.rs' RETURN f.name", 0),
            vec!["leaf".to_string()]
        );
        assert_eq!(
            column(
                "MATCH (a)-[r:CALLS]->(b) WHERE r.confidence = 'AMBIGUOUS' RETURN b.name",
                0
            ),
            vec!["leaf".to_string()]
        );
        assert_eq!(
            column(
                "MATCH (f:Function) WHERE f.name STARTS WITH 'h' OR f.name ENDS WITH 'f' \
                 RETURN f.name ORDER BY f.name",
                0
            ),
            vec!["helper".to_string(), "leaf".to_string()]
        );
        assert_eq!(
            column(
                "MATCH (f:Function) WHERE f.line > 15 AND NOT f.name CONTAINS 'x' RETURN f.name",
                0
            ),
            vec!["helper".to_string()]
        );
        assert_eq!(
            column(
                "MATCH (n) WHERE n.name IN ['leaf', 'Widget'] RETURN n.name ORDER BY n.name",
                0
            ),
            vec!["Widget".to_string(), "leaf".to_string()]
        );
    }

    #[test]
    fn count_groups_by_the_other_returned_columns() {
        let rows =
            answer("MATCH (a)-[:CALLS]->(b) RETURN a.name, count(*) AS calls ORDER BY calls DESC")
                .rows;
        assert_eq!(cell_text(&rows[0][0]), "caller");
        assert_eq!(cell_text(&rows[0][1]), "2");
        assert_eq!(cell_text(&rows[1][0]), "helper");
        assert_eq!(cell_text(&rows[1][1]), "1");
    }

    #[test]
    fn paging_reports_that_it_paged() {
        let first = answer("MATCH (n) RETURN n.name ORDER BY n.name LIMIT 2");
        assert_eq!(first.rows.len(), 2);
        assert!(first.truncated, "two of six rows is a page, and says so");
        let all = answer("MATCH (n) RETURN n.name LIMIT 100");
        assert!(!all.truncated);
        let skipped = answer("MATCH (n) RETURN n.name ORDER BY n.name SKIP 5 LIMIT 10");
        assert_eq!(skipped.rows.len(), 1);
    }

    #[test]
    fn a_node_column_carries_its_evidence() {
        let rows = answer("MATCH (f:Function {name: 'leaf'}) RETURN f").rows;
        let node = &rows[0][0];
        assert_eq!(node["name"], json!("leaf"));
        assert_eq!(node["file"], json!("b.rs"));
        assert_eq!(node["line"], json!(5));
        assert_eq!(node["kind"], json!("function"));
    }

    #[test]
    fn a_path_column_lists_the_edges_it_crossed() {
        let rows = answer("MATCH (f {name: 'caller'})-[r:CALLS*2..2]->(g) RETURN r").rows;
        let path = rows[0][0].as_array().expect("a path is an array of edges");
        assert_eq!(path.len(), 2);
        assert_eq!(path[0]["source"], json!("caller"));
        assert_eq!(path[1]["target"], json!("leaf"));
    }

    #[test]
    fn writes_are_rejected_by_name() {
        for source in [
            "MATCH (n) DELETE n",
            "MATCH (n) SET n.name = 'x' RETURN n",
            "CREATE (n:Function {name: 'x'}) RETURN n",
            "MATCH (n) DETACH DELETE n",
            "MERGE (n:Function) RETURN n",
        ] {
            let message = rejected(source);
            assert!(
                message.contains("read-only"),
                "`{source}` must be refused as a write: {message}"
            );
        }
    }

    #[test]
    fn an_unsupported_clause_says_so_instead_of_guessing() {
        assert!(rejected("MATCH (n) WITH n RETURN n").contains("outside the supported subset"));
        assert!(rejected("UNWIND [1] AS x RETURN x").contains("outside the supported subset"));
        assert!(
            rejected("MATCH (n) RETURN n UNION MATCH (m) RETURN m")
                .contains("outside the supported subset")
        );
        // `STARTS WITH` still parses: `WITH` there is part of the operator.
        assert!(parse("MATCH (n) WHERE n.name STARTS WITH 'a' RETURN n").is_ok());
    }

    #[test]
    fn an_unknown_label_type_or_property_names_what_exists() {
        assert!(rejected("MATCH (n:Widget) RETURN n").contains("unknown label"));
        assert!(rejected("MATCH (a)-[:USES]->(b) RETURN a").contains("unknown relationship type"));
        assert!(rejected("MATCH (n {colour: 'red'}) RETURN n").contains("has no property"));
        assert!(rejected("MATCH (n) WHERE n.colour = 'red' RETURN n").contains("unknown property"));
    }

    #[test]
    fn a_syntax_error_reports_where_it_is_and_what_was_expected() {
        let message = rejected("MATCH (n:Function RETURN n");
        assert!(message.contains("line 1, column"), "{message}");
        assert!(message.contains("expected `)`"), "{message}");
        assert!(rejected("MATCH (n) RETURN").contains("query ended"));
        assert!(rejected("RETURN 1").contains("expected `MATCH`"));
    }

    #[test]
    fn order_by_must_name_a_returned_column() {
        let message = rejected("MATCH (n) RETURN n.name ORDER BY n.line");
        assert!(
            message.contains("ORDER BY must name a returned column"),
            "{message}"
        );
        assert!(parse("MATCH (n) RETURN n.line, n.name ORDER BY n.line DESC").is_ok());
    }

    #[test]
    fn a_hop_range_is_bounded_and_checked() {
        assert!(rejected("MATCH (a)-[:CALLS*0..2]->(b) RETURN b").contains("at least one hop"));
        assert!(rejected("MATCH (a)-[:CALLS*3..1]->(b) RETURN b").contains("empty"));
        assert!(rejected("MATCH (a)-[:CALLS*1..40]->(b) RETURN b").contains("between 1 and"));
        // A bare `*` is bounded rather than unbounded.
        assert!(parse("MATCH (a)-[:CALLS*]->(b) RETURN b").is_ok());
    }

    #[test]
    fn a_cycle_terminates_instead_of_running_to_the_budget() {
        let graph = Graph::open_in_memory().unwrap();
        let mut ids = Vec::new();
        for name in ["a", "b"] {
            ids.push(
                graph
                    .insert_node(&Node {
                        id: None,
                        kind: NodeKind::Function,
                        name: name.to_string(),
                        file_path: "a.rs".to_string(),
                        start_line: 1,
                        end_line: 2,
                        description: None,
                    })
                    .unwrap(),
            );
        }
        for (src, dst) in [(0, 1), (1, 0)] {
            graph
                .insert_edge(&Edge {
                    src: ids[src],
                    dst: ids[dst],
                    kind: EdgeKind::Calls,
                    confidence: Confidence::Extracted,
                })
                .unwrap();
        }

        let query = parse("MATCH (a {name: 'a'})-[:CALLS*1..5]->(b) RETURN b.name").unwrap();

        let answer = evaluate(&graph, &query).expect("a cycle must not exhaust the budget");
        assert_eq!(answer.rows.len(), 2, "a->b and a->b->a, no further");
    }

    #[test]
    fn an_unbound_variable_is_an_error_not_an_empty_column() {
        let query = parse("MATCH (n) RETURN m.name").unwrap();
        let message = evaluate(&graph(), &query).unwrap_err().to_string();
        assert!(message.contains("not bound by any pattern"), "{message}");
    }

    #[test]
    fn two_patterns_join_on_a_shared_variable() {
        let rows = answer(
            "MATCH (a)-[:CALLS]->(b), (d:Doc)-[:EXPLAINS]->(b) RETURN a.name, b.name, d.name",
        )
        .rows;
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(cell_text(&rows[0][0]), "caller");
        assert_eq!(cell_text(&rows[0][1]), "helper");
        assert_eq!(cell_text(&rows[0][2]), "README.md");
    }

    #[test]
    fn the_table_form_is_aligned_and_names_its_columns() {
        let table = answer("MATCH (f:Function) RETURN f.name, f.line ORDER BY f.name").to_table();
        let lines: Vec<&str> = table.lines().collect();
        assert!(lines[0].starts_with("f.name"), "{table}");
        assert!(lines[1].starts_with("---"), "{table}");
        assert!(lines[2].starts_with("caller"), "{table}");
        assert_eq!(
            answer("MATCH (n:Endpoint) RETURN n.name").to_table(),
            "no rows"
        );
    }
}
