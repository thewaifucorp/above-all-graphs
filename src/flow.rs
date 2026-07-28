//! Statement-level control flow and data flow: basic blocks, a control-flow
//! graph, definitions and uses, reaching definitions, and control dependence.
//!
//! Everything else in `aag` works at symbol granularity — this function calls
//! that function. That granularity cannot answer "what guards this statement"
//! or "where does this value come from", because both questions live *inside*
//! a function body. This module is the layer underneath, per P0.5 of
//! `docs/capability-coverage.md`.
//!
//! Three deliberate limits, so nothing here is mistaken for a compiler:
//!
//! - Blocks are cut at the statements a reader would cut them at (branches,
//!   loops, jumps), not at every expression with a side effect.
//! - A definition is a syntactic assignment or binding. Aliasing through a
//!   reference, a field, or a container is not tracked, so reaching
//!   definitions is an over-approximation of what may reach and an
//!   under-approximation of what does.
//! - Control dependence uses post-dominance over the intraprocedural CFG.
//!   Exceptions unwinding past a caller are not modelled.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::error::{Error, Result};

/// Why a block ended, which is also what its outgoing edges mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BlockExit {
    /// Runs into the next block.
    Fallthrough,
    /// Two-way branch: the true edge first, then the false edge.
    Branch,
    /// Loop header; one edge into the body, one past it.
    Loop,
    /// Leaves the function.
    Return,
    /// Jumps out of the enclosing loop.
    Break,
    /// Jumps to the enclosing loop's header.
    Continue,
    /// The synthetic exit block.
    Exit,
}

impl BlockExit {
    /// Stable string form, for storage and for the query surface.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fallthrough => "fallthrough",
            Self::Branch => "branch",
            Self::Loop => "loop",
            Self::Return => "return",
            Self::Break => "break",
            Self::Continue => "continue",
            Self::Exit => "exit",
        }
    }
}

/// What one CFG edge means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FlowEdge {
    /// Unconditional successor.
    Sequential,
    /// The branch was taken.
    True,
    /// The branch was not taken.
    False,
    /// Back edge to a loop header.
    Back,
}

impl FlowEdge {
    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sequential => "sequential",
            Self::True => "true",
            Self::False => "false",
            Self::Back => "back",
        }
    }
}

/// One basic block: a straight-line run of statements with a single entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// Index in [`Cfg::blocks`], and the block's identity everywhere else.
    pub id: usize,
    /// 1-based first line.
    pub start_line: u32,
    /// 1-based last line.
    pub end_line: u32,
    /// Why the block ended.
    pub exit: BlockExit,
    /// Source text of the statement that ended it, trimmed — enough for a
    /// reader to recognize the guard without opening the file.
    pub terminator: String,
}

/// A variable written in a block.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Definition {
    /// Variable name.
    pub name: String,
    /// Block that writes it.
    pub block: usize,
    /// 1-based line of the write.
    pub line: u32,
}

/// A variable read in a block.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Use {
    /// Variable name.
    pub name: String,
    /// Block that reads it.
    pub block: usize,
    /// 1-based line of the read.
    pub line: u32,
}

/// One function's control and data flow.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cfg {
    /// Enclosing function or method name.
    pub function: String,
    /// Blocks in source order; the last one is the synthetic exit.
    pub blocks: Vec<Block>,
    /// `(from, to, kind)`.
    pub edges: Vec<(usize, usize, FlowEdge)>,
    /// Every syntactic write.
    pub definitions: Vec<Definition>,
    /// Every syntactic read.
    pub uses: Vec<Use>,
}

impl Cfg {
    /// Successors of `block`.
    #[must_use]
    pub fn successors(&self, block: usize) -> Vec<usize> {
        self.edges
            .iter()
            .filter(|(from, _, _)| *from == block)
            .map(|(_, to, _)| *to)
            .collect()
    }

    /// Predecessors of `block`.
    #[must_use]
    pub fn predecessors(&self, block: usize) -> Vec<usize> {
        self.edges
            .iter()
            .filter(|(_, to, _)| *to == block)
            .map(|(from, _, _)| *from)
            .collect()
    }

    /// Which definitions may be live on entry to each block.
    ///
    /// Classic iterative reaching definitions: a block kills every earlier
    /// definition of the names it writes and generates its own. Because a
    /// definition here is syntactic, the result answers "may reach", never
    /// "does reach".
    #[must_use]
    pub fn reaching_definitions(&self) -> Vec<BTreeSet<usize>> {
        let count = self.blocks.len();
        let mut generated: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); count];
        let mut killed: Vec<HashSet<&str>> = vec![HashSet::new(); count];
        for (index, definition) in self.definitions.iter().enumerate() {
            if definition.block >= count {
                continue;
            }
            generated[definition.block].insert(index);
            killed[definition.block].insert(definition.name.as_str());
        }

        let mut entry: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); count];
        let mut exit: Vec<BTreeSet<usize>> = generated.clone();
        let mut changed = true;
        while changed {
            changed = false;
            for block in 0..count {
                let mut incoming = BTreeSet::new();
                for predecessor in self.predecessors(block) {
                    incoming.extend(exit[predecessor].iter().copied());
                }
                if incoming != entry[block] {
                    entry[block] = incoming;
                    changed = true;
                }
                let surviving: BTreeSet<usize> = entry[block]
                    .iter()
                    .copied()
                    .filter(|index| !killed[block].contains(self.definitions[*index].name.as_str()))
                    .collect();
                let mut next = surviving;
                next.extend(generated[block].iter().copied());
                if next != exit[block] {
                    exit[block] = next;
                    changed = true;
                }
            }
        }
        entry
    }

    /// Definitions that may supply each use — the def-use chains.
    #[must_use]
    pub fn def_use_chains(&self) -> Vec<(usize, Vec<usize>)> {
        let reaching = self.reaching_definitions();
        self.uses
            .iter()
            .enumerate()
            .map(|(use_index, usage)| {
                let mut sources: Vec<usize> = Vec::new();
                // A definition earlier in the same block shadows anything that
                // reached the block's entry.
                let local = self
                    .definitions
                    .iter()
                    .enumerate()
                    .filter(|(_, definition)| {
                        definition.block == usage.block
                            && definition.name == usage.name
                            && definition.line <= usage.line
                    })
                    .map(|(index, _)| index)
                    .next_back();
                if let Some(index) = local {
                    sources.push(index);
                } else if let Some(entry) = reaching.get(usage.block) {
                    sources.extend(
                        entry
                            .iter()
                            .copied()
                            .filter(|index| self.definitions[*index].name == usage.name),
                    );
                }
                (use_index, sources)
            })
            .collect()
    }

    /// Which branch each block is controlled by.
    ///
    /// A block is control dependent on a branch when the branch decides
    /// whether it runs: one successor of the branch always reaches the block
    /// and another can avoid it. Computed from post-dominance over this
    /// function's CFG only.
    #[must_use]
    pub fn control_dependence(&self) -> Vec<(usize, usize)> {
        let post = self.post_dominators();
        let mut found = BTreeSet::new();
        for (from, to, _) in &self.edges {
            // Only a branching block can control anything.
            if self.successors(*from).len() < 2 {
                continue;
            }
            // Walk up the post-dominator tree from `to` until the branch's own
            // post-dominator, marking everything on the way as dependent.
            let stop = post.get(from).copied().flatten();
            let mut cursor = Some(*to);
            while let Some(block) = cursor {
                if Some(block) == stop {
                    break;
                }
                // A loop header reached by its own back edge is not guarded by
                // itself; that reads as nonsense and hides the real guard.
                if block != *from {
                    found.insert((block, *from));
                }
                cursor = post.get(&block).copied().flatten();
                if cursor == Some(block) {
                    break;
                }
            }
        }
        found.into_iter().collect()
    }

    /// Immediate post-dominator of each block, if it has one.
    fn post_dominators(&self) -> HashMap<usize, Option<usize>> {
        let count = self.blocks.len();
        let exit = count.saturating_sub(1);
        // Iterative dataflow over the reversed CFG: the post-dominator set of
        // a block is itself plus the intersection of its successors' sets.
        let mut sets: Vec<BTreeSet<usize>> = (0..count)
            .map(|block| {
                if block == exit {
                    BTreeSet::from([exit])
                } else {
                    (0..count).collect()
                }
            })
            .collect();
        let mut changed = true;
        while changed {
            changed = false;
            for block in (0..count.saturating_sub(1)).rev() {
                let successors = self.successors(block);
                let mut intersection: Option<BTreeSet<usize>> = None;
                for successor in successors {
                    intersection = Some(match intersection {
                        None => sets[successor].clone(),
                        Some(current) => current.intersection(&sets[successor]).copied().collect(),
                    });
                }
                let mut next = intersection.unwrap_or_default();
                next.insert(block);
                if next != sets[block] {
                    sets[block] = next;
                    changed = true;
                }
            }
        }
        (0..count)
            .map(|block| {
                // The immediate post-dominator is the nearest strict one, which
                // is the strict post-dominator with the largest set.
                let immediate = sets[block]
                    .iter()
                    .copied()
                    .filter(|candidate| *candidate != block)
                    .max_by_key(|candidate| sets[*candidate].len());
                (block, immediate)
            })
            .collect()
    }
}

/// Node kinds that end a basic block, and what they mean.
fn terminator_exit(kind: &str) -> Option<BlockExit> {
    match kind {
        "if_statement"
        | "if_expression"
        | "conditional_expression"
        | "match_expression"
        | "switch_statement"
        | "when_expression" => Some(BlockExit::Branch),
        "for_statement"
        | "while_statement"
        | "loop_expression"
        | "while_expression"
        | "for_expression"
        | "do_statement"
        | "for_in_statement"
        | "for_of_statement"
        | "repeat_statement"
        | "range_for_statement" => Some(BlockExit::Loop),
        "return_statement" | "return_expression" => Some(BlockExit::Return),
        "break_statement" | "break_expression" => Some(BlockExit::Break),
        "continue_statement" | "continue_expression" => Some(BlockExit::Continue),
        _ => None,
    }
}

/// Node kinds that write a variable, paired with the field holding the name.
const DEFINITION_KINDS: &[(&str, &str)] = &[
    ("let_declaration", "pattern"),
    ("variable_declarator", "name"),
    ("assignment", "left"),
    ("assignment_expression", "left"),
    ("augmented_assignment", "left"),
    ("short_var_declaration", "left"),
    ("compound_assignment_expr", "left"),
    ("parameter", "pattern"),
    ("typed_parameter", "name"),
];

fn language_of(file_path: &str) -> Option<&'static str> {
    let extension = file_path.rsplit('.').next()?.to_ascii_lowercase();
    Some(match extension.as_str() {
        "rs" => "rust",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "ts" => "typescript",
        "tsx" => "tsx",
        "py" | "pyw" => "python",
        "java" => "java",
        "cs" => "csharp",
        "go" => "go",
        _ => return None,
    })
}

/// Builds one CFG per function in `source`.
///
/// # Errors
/// Returns [`Error::Parse`] when the language has no grammar available or the
/// file cannot be parsed.
pub fn analyze(file_path: &str, source: &str) -> Result<Vec<Cfg>> {
    let Some(language) = language_of(file_path) else {
        return Ok(Vec::new());
    };
    let mut parser =
        tree_sitter_language_pack::get_parser(language).map_err(|error| Error::Parse {
            file: file_path.to_string(),
            reason: error.to_string(),
        })?;
    let tree = parser.parse(source).ok_or_else(|| Error::Parse {
        file: file_path.to_string(),
        reason: "tree-sitter returned no tree".to_string(),
    })?;
    let mut functions = Vec::new();
    collect_functions(&tree.root_node(), source, &mut functions);
    Ok(functions
        .into_iter()
        .map(|(name, body)| build_cfg(&name, &body, source))
        .collect())
}

/// Wrappers that hold statements rather than being a statement: Rust puts
/// every expression-statement in an `expression_statement`, Go puts a
/// function body's statements in a `statement_list`. Walking these
/// transparently is the difference between finding an `if` and missing it.
const STATEMENT_CONTAINERS: &[&str] = &[
    "block",
    "statement_block",
    "statement_list",
    "compound_statement",
    "declaration_list",
];

const FUNCTION_KINDS: &[&str] = &[
    "function_item",
    "function_declaration",
    "function_definition",
    "method_declaration",
    "method_definition",
    "constructor_declaration",
    "arrow_function",
    "local_function_statement",
    "function_expression",
    "function",
    "generator_function",
    "generator_function_declaration",
    "lambda",
    "closure_expression",
];

fn collect_functions(
    node: &tree_sitter_language_pack::Node,
    source: &str,
    out: &mut Vec<(String, tree_sitter_language_pack::Node)>,
) {
    if FUNCTION_KINDS.contains(&node.kind().as_str())
        && let Some(body) = node.child_by_field_name("body")
    {
        let name = node
            .child_by_field_name("name")
            .and_then(|name| text(&name, source))
            .unwrap_or("<anonymous>")
            .to_string();
        out.push((name, body));
    }
    for index in 0..u32::try_from(node.named_child_count()).unwrap_or(u32::MAX) {
        if let Some(child) = node.named_child(index) {
            collect_functions(&child, source, out);
        }
    }
}

fn text<'a>(node: &tree_sitter_language_pack::Node, source: &'a str) -> Option<&'a str> {
    source.get(node.byte_range().start..node.byte_range().end)
}

fn line_of(node: &tree_sitter_language_pack::Node) -> u32 {
    u32::try_from(node.start_position().row + 1).unwrap_or(u32::MAX)
}

/// Splits one function body into basic blocks and wires the CFG.
///
/// Statements accumulate into the current block until one of them branches,
/// loops, or jumps; that statement terminates the block, and its own
/// sub-statements are walked as nested blocks.
fn build_cfg(function: &str, body: &tree_sitter_language_pack::Node, source: &str) -> Cfg {
    let mut cfg = Cfg {
        function: function.to_string(),
        ..Cfg::default()
    };
    let mut builder = Builder {
        cfg: &mut cfg,
        source,
        pending: Vec::new(),
        loops: Vec::new(),
        current: None,
    };
    builder.open(line_of(body));
    builder.walk_statements(body);
    let exit = builder.close_exit(body.end_position().row.try_into().unwrap_or(u32::MAX) + 1);
    builder.link_pending(exit);
    cfg
}

struct Builder<'c> {
    cfg: &'c mut Cfg,
    source: &'c str,
    /// Blocks waiting for the next block to exist so they can point at it.
    pending: Vec<(usize, FlowEdge)>,
    /// Header and break-target per enclosing loop.
    loops: Vec<(usize, Vec<usize>)>,
    current: Option<usize>,
}

impl Builder<'_> {
    fn open(&mut self, line: u32) -> usize {
        let id = self.cfg.blocks.len();
        self.cfg.blocks.push(Block {
            id,
            start_line: line,
            end_line: line,
            exit: BlockExit::Fallthrough,
            terminator: String::new(),
        });
        let pending = std::mem::take(&mut self.pending);
        for (from, kind) in pending {
            self.cfg.edges.push((from, id, kind));
        }
        self.current = Some(id);
        id
    }

    fn ensure_current(&mut self, line: u32) -> usize {
        match self.current {
            Some(id) => id,
            None => self.open(line),
        }
    }

    fn close_exit(&mut self, line: u32) -> usize {
        let id = self.cfg.blocks.len();
        self.cfg.blocks.push(Block {
            id,
            start_line: line,
            end_line: line,
            exit: BlockExit::Exit,
            terminator: String::new(),
        });
        id
    }

    fn link_pending(&mut self, target: usize) {
        let pending = std::mem::take(&mut self.pending);
        for (from, kind) in pending {
            self.cfg.edges.push((from, target, kind));
        }
        if let Some(current) = self.current.take()
            && !self
                .cfg
                .edges
                .iter()
                .any(|(from, _, _)| *from == current && current != target)
        {
            self.cfg.edges.push((current, target, FlowEdge::Sequential));
        }
    }

    fn terminate(&mut self, block: usize, exit: BlockExit, node: &tree_sitter_language_pack::Node) {
        let terminator = text(node, self.source)
            .unwrap_or_default()
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        if let Some(slot) = self.cfg.blocks.get_mut(block) {
            slot.exit = exit;
            slot.end_line = u32::try_from(node.end_position().row + 1).unwrap_or(u32::MAX);
            slot.terminator = terminator;
        }
        self.current = None;
    }

    fn walk_statements(&mut self, parent: &tree_sitter_language_pack::Node) {
        for index in 0..u32::try_from(parent.named_child_count()).unwrap_or(u32::MAX) {
            let Some(statement) = parent.named_child(index) else {
                continue;
            };
            self.walk_statement(&statement);
        }
    }

    fn walk_statement(&mut self, statement: &tree_sitter_language_pack::Node) {
        // A container is not a statement; its children are.
        if STATEMENT_CONTAINERS.contains(&statement.kind().as_str()) {
            self.walk_statements(statement);
            return;
        }
        // Unwrap `expression_statement` so the expression inside can terminate
        // the block, then work with whichever node actually decides control.
        let statement = &unwrap_statement(statement);
        let line = line_of(statement);
        let block = self.ensure_current(line);
        self.record_data_flow(statement, block);
        if let Some(slot) = self.cfg.blocks.get_mut(block) {
            slot.end_line = slot
                .end_line
                .max(u32::try_from(statement.end_position().row + 1).unwrap_or(u32::MAX));
        }

        match terminator_exit(statement.kind().as_str()) {
            Some(BlockExit::Branch) => {
                self.terminate(block, BlockExit::Branch, statement);
                let consequence = statement
                    .child_by_field_name("consequence")
                    .or_else(|| statement.child_by_field_name("body"));
                let alternative = statement
                    .child_by_field_name("alternative")
                    .or_else(|| statement.child_by_field_name("else"));
                let mut joins = Vec::new();
                if let Some(branch) = consequence {
                    self.pending.push((block, FlowEdge::True));
                    self.open(line_of(&branch));
                    self.walk_statements(&branch);
                    if let Some(open) = self.current.take() {
                        joins.push((open, FlowEdge::Sequential));
                    }
                    joins.extend(std::mem::take(&mut self.pending));
                }
                if let Some(branch) = alternative {
                    self.pending.push((block, FlowEdge::False));
                    self.open(line_of(&branch));
                    self.walk_statements(&branch);
                    if let Some(open) = self.current.take() {
                        joins.push((open, FlowEdge::Sequential));
                    }
                    joins.extend(std::mem::take(&mut self.pending));
                } else {
                    joins.push((block, FlowEdge::False));
                }
                self.pending = joins;
            }
            Some(BlockExit::Loop) => {
                self.terminate(block, BlockExit::Loop, statement);
                let body = statement
                    .child_by_field_name("body")
                    .or_else(|| statement.child_by_field_name("consequence"));
                self.loops.push((block, Vec::new()));
                if let Some(body) = body {
                    self.pending.push((block, FlowEdge::True));
                    self.open(line_of(&body));
                    self.walk_statements(&body);
                    // Whatever is still open loops back to the header.
                    if let Some(open) = self.current.take() {
                        self.cfg.edges.push((open, block, FlowEdge::Back));
                    }
                    for (from, _) in std::mem::take(&mut self.pending) {
                        self.cfg.edges.push((from, block, FlowEdge::Back));
                    }
                }
                let (_, breaks) = self.loops.pop().unwrap_or((block, Vec::new()));
                self.pending.push((block, FlowEdge::False));
                for from in breaks {
                    self.pending.push((from, FlowEdge::Sequential));
                }
            }
            Some(BlockExit::Return) => {
                self.terminate(block, BlockExit::Return, statement);
                self.pending.push((block, FlowEdge::Sequential));
            }
            Some(BlockExit::Break) => {
                self.terminate(block, BlockExit::Break, statement);
                if let Some((_, breaks)) = self.loops.last_mut() {
                    breaks.push(block);
                } else {
                    self.pending.push((block, FlowEdge::Sequential));
                }
            }
            Some(BlockExit::Continue) => {
                self.terminate(block, BlockExit::Continue, statement);
                if let Some(header) = self.loops.last().map(|(header, _)| *header) {
                    self.cfg.edges.push((block, header, FlowEdge::Back));
                }
            }
            Some(BlockExit::Fallthrough | BlockExit::Exit) | None => {
                // A plain statement: keep filling this block, but still walk
                // nested blocks so an inner `if` inside an expression is not
                // lost.
                for index in 0..u32::try_from(statement.named_child_count()).unwrap_or(u32::MAX) {
                    if let Some(child) = statement.named_child(index)
                        && STATEMENT_CONTAINERS.contains(&child.kind().as_str())
                    {
                        self.walk_statements(&child);
                    }
                }
            }
        }
    }

    /// Records the writes and reads a statement performs.
    fn record_data_flow(&mut self, statement: &tree_sitter_language_pack::Node, block: usize) {
        let mut defined: HashSet<String> = HashSet::new();
        self.collect_definitions(statement, block, &mut defined);
        self.collect_uses(statement, block, &defined);
    }

    fn collect_definitions(
        &mut self,
        node: &tree_sitter_language_pack::Node,
        block: usize,
        defined: &mut HashSet<String>,
    ) {
        if let Some((_, field)) = DEFINITION_KINDS
            .iter()
            .find(|(kind, _)| *kind == node.kind().as_str())
            && let Some(name) = node
                .child_by_field_name(field)
                .and_then(|target| text(&target, self.source))
                .and_then(identifier_head)
        {
            defined.insert(name.to_string());
            self.cfg.definitions.push(Definition {
                name: name.to_string(),
                block,
                line: line_of(node),
            });
        }
        for index in 0..u32::try_from(node.named_child_count()).unwrap_or(u32::MAX) {
            if let Some(child) = node.named_child(index) {
                // Do not descend into a nested function: its flow is its own.
                if FUNCTION_KINDS.contains(&child.kind().as_str()) {
                    continue;
                }
                self.collect_definitions(&child, block, defined);
            }
        }
    }

    fn collect_uses(
        &mut self,
        node: &tree_sitter_language_pack::Node,
        block: usize,
        defined: &HashSet<String>,
    ) {
        if node.kind() == "identifier"
            && let Some(name) = text(node, self.source)
        {
            // The left-hand side of an assignment is a write, not a read.
            let is_target = node
                .parent()
                .and_then(|parent| {
                    DEFINITION_KINDS
                        .iter()
                        .find(|(kind, _)| *kind == parent.kind().as_str())
                        .map(|(_, field)| (parent, *field))
                })
                .and_then(|(parent, field)| parent.child_by_field_name(field))
                .is_some_and(|target| target.byte_range() == node.byte_range());
            if !is_target {
                self.cfg.uses.push(Use {
                    name: name.to_string(),
                    block,
                    line: line_of(node),
                });
            }
        }
        let _ = defined;
        for index in 0..u32::try_from(node.named_child_count()).unwrap_or(u32::MAX) {
            if let Some(child) = node.named_child(index) {
                if FUNCTION_KINDS.contains(&child.kind().as_str()) {
                    continue;
                }
                self.collect_uses(&child, block, defined);
            }
        }
    }
}

/// The bare name a binding pattern introduces (`mut total` → `total`).
/// Peels `expression_statement`-style wrappers off a statement.
fn unwrap_statement(node: &tree_sitter_language_pack::Node) -> tree_sitter_language_pack::Node {
    let mut current = node.clone();
    while current.kind() == "expression_statement" && current.named_child_count() == 1 {
        match current.named_child(0) {
            Some(child) => current = child,
            None => break,
        }
    }
    current
}

fn identifier_head(raw: &str) -> Option<&str> {
    raw.split(|character: char| !character.is_alphanumeric() && character != '_')
        .find(|part| {
            !part.is_empty() && !matches!(*part, "mut" | "let" | "const" | "var" | "final")
        })
}

/// Every function's flow in one file, keyed by function name — the shape the
/// query surface and storage both want.
///
/// # Errors
/// Returns [`Error::Parse`] when the file cannot be parsed.
pub fn analyze_map(file_path: &str, source: &str) -> Result<BTreeMap<String, Cfg>> {
    Ok(analyze(file_path, source)?
        .into_iter()
        .map(|cfg| (cfg.function.clone(), cfg))
        .collect())
}

/// Renders one file's flow for the CLI.
///
/// # Errors
/// Returns an error when the file cannot be read or parsed.
pub fn format_file(path: &std::path::Path, function_filter: &str) -> Result<String> {
    let source = std::fs::read_to_string(path).map_err(|error| Error::Parse {
        file: path.display().to_string(),
        reason: error.to_string(),
    })?;
    let name = path.to_string_lossy();
    let graphs = analyze(&name, &source)?;
    if graphs.is_empty() {
        return Ok(format!(
            "no control flow extracted from {name} — the language has no flow frontend yet"
        ));
    }
    let mut out = Vec::new();
    for cfg in graphs {
        if !function_filter.is_empty() && cfg.function != function_filter {
            continue;
        }
        out.push(format!(
            "## {} — {} blocks, {} edges",
            cfg.function,
            cfg.blocks.len(),
            cfg.edges.len()
        ));
        let guards: HashMap<usize, usize> = cfg.control_dependence().into_iter().collect();
        for block in &cfg.blocks {
            let guard = guards.get(&block.id).map_or_else(String::new, |guard| {
                let terminator = cfg
                    .blocks
                    .get(*guard)
                    .map(|owner| owner.terminator.as_str())
                    .unwrap_or_default();
                format!("  guarded by b{guard} `{terminator}`")
            });
            let successors: Vec<String> = cfg
                .edges
                .iter()
                .filter(|(from, _, _)| *from == block.id)
                .map(|(_, to, kind)| format!("{}->b{to}", kind.as_str()))
                .collect();
            out.push(format!(
                "b{} lines {}-{} {}{}{}",
                block.id,
                block.start_line,
                block.end_line,
                block.exit.as_str(),
                if successors.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", successors.join(", "))
                },
                guard
            ));
            if !block.terminator.is_empty() {
                out.push(format!("     {}", block.terminator));
            }
        }
        for (use_index, sources) in cfg.def_use_chains() {
            let usage = &cfg.uses[use_index];
            if sources.is_empty() {
                continue;
            }
            let from: Vec<String> = sources
                .iter()
                .map(|index| format!("line {}", cfg.definitions[*index].line))
                .collect();
            out.push(format!(
                "use {} at line {} <- {}",
                usage.name,
                usage.line,
                from.join(", ")
            ));
        }
        out.push(String::new());
    }
    Ok(if out.is_empty() {
        format!("no function named {function_filter} in {name}")
    } else {
        out.join("\n").trim_end().to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_of(path: &str, source: &str, function: &str) -> Cfg {
        analyze_map(path, source)
            .unwrap()
            .remove(function)
            .unwrap_or_else(|| panic!("no cfg for {function}"))
    }

    #[test]
    fn straight_line_function_is_one_block_plus_exit() {
        let cfg = cfg_of("a.rs", "fn run() { let a = 1; let b = a + 1; }", "run");
        assert_eq!(cfg.blocks.len(), 2, "one body block and the exit");
        assert_eq!(cfg.blocks[1].exit, BlockExit::Exit);
        assert_eq!(cfg.edges, vec![(0, 1, FlowEdge::Sequential)]);
    }

    #[test]
    fn branch_produces_true_and_false_edges_that_rejoin() {
        let source = "fn run(flag: bool) { let a = 1; if flag { let b = 2; } let c = 3; }";
        let cfg = cfg_of("a.rs", source, "run");
        let branch = cfg
            .blocks
            .iter()
            .find(|block| block.exit == BlockExit::Branch)
            .expect("a branching block");
        let kinds: BTreeSet<FlowEdge> = cfg
            .edges
            .iter()
            .filter(|(from, _, _)| *from == branch.id)
            .map(|(_, _, kind)| *kind)
            .collect();
        assert!(
            kinds.contains(&FlowEdge::True) && kinds.contains(&FlowEdge::False),
            "a branch must have both outcomes, got {kinds:?}"
        );
        assert!(
            branch.terminator.starts_with("if flag"),
            "the guard must be readable without opening the file: {:?}",
            branch.terminator
        );
    }

    #[test]
    fn loop_gets_a_back_edge_and_an_exit_edge() {
        let source = "fn run() { let mut i = 0; while i < 3 { i = i + 1; } }";
        let cfg = cfg_of("a.rs", source, "run");
        let header = cfg
            .blocks
            .iter()
            .find(|block| block.exit == BlockExit::Loop)
            .expect("a loop header");
        assert!(
            cfg.edges
                .iter()
                .any(|(_, to, kind)| *to == header.id && *kind == FlowEdge::Back),
            "the body must loop back to the header: {:?}",
            cfg.edges
        );
        assert!(
            cfg.edges
                .iter()
                .any(|(from, _, kind)| *from == header.id && *kind == FlowEdge::False),
            "the header must have a way out"
        );
    }

    #[test]
    fn return_leaves_the_function() {
        let cfg = cfg_of("a.rs", "fn run(x: i32) -> i32 { return x; }", "run");
        assert!(
            cfg.blocks
                .iter()
                .any(|block| block.exit == BlockExit::Return)
        );
    }

    #[test]
    fn definitions_and_uses_are_separated() {
        let cfg = cfg_of(
            "a.rs",
            "fn run() { let total = 1; let other = total; }",
            "run",
        );
        let defined: Vec<&str> = cfg.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(
            defined.contains(&"total") && defined.contains(&"other"),
            "{defined:?}"
        );
        assert!(
            cfg.uses.iter().any(|usage| usage.name == "total"),
            "`total` is read on the second line"
        );
        assert!(
            !cfg.uses.iter().any(|usage| usage.name == "other"),
            "the left-hand side of a binding is a write, not a read"
        );
    }

    #[test]
    fn reaching_definitions_follow_a_branch() {
        // `value` is defined before the branch and redefined inside it, so both
        // definitions may reach the use afterwards.
        let source =
            "fn run(flag: bool) { let value = 1; if flag { value = 2; } let out = value; }";
        let cfg = cfg_of("a.rs", source, "run");
        let chains = cfg.def_use_chains();
        let final_use = cfg
            .uses
            .iter()
            .enumerate()
            .filter(|(_, usage)| usage.name == "value")
            .max_by_key(|(_, usage)| usage.line)
            .map(|(index, _)| index)
            .expect("a use of value");
        let sources = chains
            .iter()
            .find(|(index, _)| *index == final_use)
            .map(|(_, sources)| sources.clone())
            .unwrap_or_default();
        assert!(
            sources.len() >= 2,
            "both the pre-branch and in-branch definitions may reach: {sources:?}"
        );
    }

    #[test]
    fn local_definition_shadows_what_reached_the_block() {
        let source = "fn run() { let value = 1; let value = 2; let out = value; }";
        let cfg = cfg_of("a.rs", source, "run");
        let chains = cfg.def_use_chains();
        let last = cfg
            .uses
            .iter()
            .enumerate()
            .filter(|(_, usage)| usage.name == "value")
            .max_by_key(|(_, usage)| usage.line)
            .map(|(index, _)| index)
            .unwrap();
        let sources = chains
            .iter()
            .find(|(index, _)| *index == last)
            .unwrap()
            .1
            .clone();
        assert_eq!(sources.len(), 1, "the nearer definition wins: {sources:?}");
        assert_eq!(cfg.definitions[sources[0]].line, 1);
    }

    #[test]
    fn control_dependence_names_the_guard() {
        let source = "fn run(flag: bool) { if flag { let inner = 1; } let after = 2; }";
        let cfg = cfg_of("a.rs", source, "run");
        let branch = cfg
            .blocks
            .iter()
            .find(|block| block.exit == BlockExit::Branch)
            .unwrap()
            .id;
        let dependence = cfg.control_dependence();
        assert!(
            dependence.iter().any(|(_, guard)| *guard == branch),
            "the guarded block must be control dependent on the branch: {dependence:?}"
        );
    }

    #[test]
    fn javascript_and_python_produce_flow_too() {
        for (path, source, function) in [
            (
                "a.js",
                "function run(flag) { let a = 1; if (flag) { a = 2; } return a; }",
                "run",
            ),
            (
                "a.py",
                "def run(flag):\n    a = 1\n    if flag:\n        a = 2\n    return a\n",
                "run",
            ),
            (
                "a.go",
                "func run(flag bool) int {\n\ta := 1\n\tif flag {\n\t\ta = 2\n\t}\n\treturn a\n}",
                "run",
            ),
        ] {
            let cfg = cfg_of(path, source, function);
            assert!(
                cfg.blocks.len() >= 3,
                "{path}: a branch means at least three blocks, got {}",
                cfg.blocks.len()
            );
            assert!(
                cfg.definitions.iter().any(|d| d.name == "a"),
                "{path}: `a` is defined"
            );
        }
    }

    #[test]
    fn unsupported_language_yields_nothing_rather_than_failing() {
        assert!(analyze("notes.md", "# hello").unwrap().is_empty());
        assert!(analyze("main.lua", "print(1)").unwrap().is_empty());
    }

    #[test]
    fn nested_function_flow_is_not_folded_into_its_parent() {
        let source = "function outer() { let a = 1; const inner = function () { let b = 2; return b; }; return a; }";
        let map = analyze_map("a.js", source).unwrap();
        let outer = map.get("outer").expect("outer");
        assert!(
            !outer.definitions.iter().any(|d| d.name == "b"),
            "the inner function's binding belongs to the inner function"
        );
    }
}
