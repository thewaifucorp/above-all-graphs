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
//!
//! Taint crosses function boundaries: each function gets a [`Summary`] of what
//! it does to the values passed into it, and a [`Program`] joins those
//! summaries through the call graph the rest of `aag` already resolved. That is
//! still syntactic — a summary is computed from the same line-granular flow, so
//! crossing a call inherits every limit above rather than escaping them.

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

/// A call site inside a function body, with the names passed to it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Call {
    /// Called name, without receiver.
    pub callee: String,
    /// Block containing the call.
    pub block: usize,
    /// 1-based line.
    pub line: u32,
    /// Identifiers per positional argument, in argument order: `f(a.b, 2, c)`
    /// gives `[["a"], [], ["c"]]`. Grouping is what lets an argument be matched
    /// to the callee's parameter at the same position, so an argument that
    /// mentions no identifier still occupies its slot.
    pub arguments: Vec<Vec<String>>,
}

impl Call {
    /// Position of the first argument that mentions `name`.
    #[must_use]
    pub fn position_of(&self, name: &str) -> Option<usize> {
        self.arguments
            .iter()
            .position(|group| group.iter().any(|argument| argument == name))
    }
}

/// Why one statement depends on another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Dependence {
    /// A branch decides whether the dependent runs.
    Control,
    /// A value the dependent reads is written by the other.
    Data,
}

impl Dependence {
    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Control => "control",
            Self::Data => "data",
        }
    }
}

/// One function's control and data flow.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cfg {
    /// Enclosing function or method name.
    pub function: String,
    /// Declared parameter names, in order. `self`/`this` is excluded: it is not
    /// a value a caller passes at a position.
    pub parameters: Vec<String>,
    /// Blocks in source order; the last one is the synthetic exit.
    pub blocks: Vec<Block>,
    /// `(from, to, kind)`.
    pub edges: Vec<(usize, usize, FlowEdge)>,
    /// Every syntactic write.
    pub definitions: Vec<Definition>,
    /// Every syntactic read.
    pub uses: Vec<Use>,
    /// Every call site.
    pub calls: Vec<Call>,
    /// `(line, identifiers read)` for each explicit `return`. A Rust tail
    /// expression is not a `return` statement and is not captured here.
    pub returns: Vec<(u32, Vec<String>)>,
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

    /// The program dependence graph: control dependence plus data dependence,
    /// as `(dependent line, source line, why)`.
    ///
    /// Lines rather than blocks, because that is the granularity a reader asks
    /// the question at — "what does line 42 depend on".
    #[must_use]
    pub fn dependences(&self) -> Vec<(u32, u32, Dependence)> {
        let mut found = BTreeSet::new();
        for (block, guard) in self.control_dependence() {
            let Some((dependent, owner)) = self.blocks.get(block).zip(self.blocks.get(guard))
            else {
                continue;
            };
            // The guard's own first line, not its last: a loop header spans
            // its whole body, and reporting a dependence on a later line reads
            // backwards.
            if dependent.start_line != owner.start_line {
                found.insert((dependent.start_line, owner.start_line, Dependence::Control));
            }
        }
        for (use_index, sources) in self.def_use_chains() {
            let usage = &self.uses[use_index];
            for source in sources {
                let definition = &self.definitions[source];
                if definition.line != usage.line {
                    found.insert((usage.line, definition.line, Dependence::Data));
                }
            }
        }
        found.into_iter().collect()
    }

    /// What one line depends on, transitively.
    #[must_use]
    pub fn dependences_of(&self, line: u32) -> Vec<(u32, u32, Dependence)> {
        let all = self.dependences();
        let mut seen: BTreeSet<u32> = BTreeSet::from([line]);
        let mut frontier = vec![line];
        let mut found = BTreeSet::new();
        while let Some(current) = frontier.pop() {
            for (dependent, source, why) in &all {
                if *dependent != current {
                    continue;
                }
                found.insert((*dependent, *source, *why));
                if seen.insert(*source) {
                    frontier.push(*source);
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

/// Expressions whose value comes from outside the program. Matching is on the
/// tail identifier of a read, so `req.query`, `process.env`, and
/// `os.environ` all land here.
const TAINT_SOURCES: &[&str] = &[
    "argv", "args", "env", "environ", "query", "body", "params", "headers", "cookies", "stdin",
    "input", "form", "GET", "POST", "request",
];

/// Calls that must not receive attacker-controlled input without a decision in
/// between. The list is deliberately short and specific: a long fuzzy list
/// produces findings nobody reads.
const TAINT_SINKS: &[&str] = &[
    "eval",
    "exec",
    "execSync",
    "execFile",
    "spawn",
    "spawnSync",
    "system",
    "popen",
    "query",
    "execute",
    "executemany",
    "raw",
    "innerHTML",
    "insertAdjacentHTML",
    "writeFile",
    "writeFileSync",
    "readFile",
    "readFileSync",
    "createReadStream",
    "sendFile",
    "send_file",
    "render_template_string",
    "deserialize",
    "loads",
    "unserialize",
    "Function",
];

/// Calls that neutralize a value: escaping, quoting, or narrowing it to a type
/// that cannot carry an injection. Matched on the tail identifier like the
/// source list, so `shlex.quote` and `html.escape` both land here.
///
/// Recognition is line-granular, which means `escape(a) + b` reads as
/// sanitized even though `b` is not. That direction is deliberate: a false
/// negative costs one missed place to look, a false positive costs the reader's
/// trust in the whole list.
const SANITIZERS: &[&str] = &[
    "escape",
    "escapeHtml",
    "escapeHTML",
    "escape_html",
    "escapeString",
    "escape_string",
    "escapeIdentifier",
    "escape_identifier",
    "sanitize",
    "sanitizeHtml",
    "sanitize_html",
    "encodeURI",
    "encodeURIComponent",
    "htmlspecialchars",
    "quote",
    "quote_ident",
    "quoteIdentifier",
    "shlex_quote",
    "parseInt",
    "parseFloat",
    "Number",
    "int",
    "float",
];

/// What a function does to the values passed into it.
///
/// This is the join between the symbol-level call graph and these per-function
/// graphs: a caller does not re-analyze its callee, it reads the callee's
/// summary and asks whether the argument it is about to pass ends up somewhere
/// that matters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    /// Function the summary describes.
    pub function: String,
    /// File it was read from.
    pub file: String,
    /// Declared parameters, in order.
    pub parameters: Vec<String>,
    /// Parameter positions whose value reaches a sink, and where it lands.
    pub sink_reaching: BTreeMap<usize, SinkReach>,
    /// Parameter positions whose value reaches an explicit `return`, so a
    /// tainted argument taints what the caller assigns from the call.
    pub returns_parameter: BTreeSet<usize>,
    /// Parameter positions a sanitizer stopped inside this function, and which
    /// one stopped them. A caller's flow ends here, and saying so is the
    /// difference between "nothing reached a sink" and "something did, and was
    /// escaped on the way".
    pub sanitized_parameters: BTreeMap<usize, String>,
    /// This function reads an external input and returns it, so its result is
    /// tainted for every caller regardless of arguments.
    pub returns_source: Option<(String, u32)>,
    /// This function neutralizes what it is given: a parameter reaches a
    /// `return` only through a sanitizer. Callers treat it as one.
    pub sanitizer: bool,
}

/// Where a parameter's value ends up, and which calls carried it there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinkReach {
    /// The consuming call.
    pub sink: String,
    /// 1-based line of that call.
    pub sink_line: u32,
    /// Function the sink is called from.
    pub sink_function: String,
    /// File that function lives in.
    pub sink_file: String,
    /// `(callee, call line)` for each call boundary crossed, nearest caller
    /// first. Empty when the sink is in the summarized function itself.
    pub via: Vec<(String, u32)>,
    /// Whether a branch decides that the sink runs.
    pub guarded: bool,
}

/// The function a call site was resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    /// File the callee is declared in.
    file: String,
    /// Callee name as the graph knows it.
    function: String,
    /// Whether a resolved `calls` edge produced this, rather than a name that
    /// happened to match.
    by_edge: bool,
}

/// What the rest of the program contributes to one function's taint: which
/// function each call site reaches, and what that function does with a value.
///
/// Both maps are keyed by file *and* name. A repository with two functions named
/// `run` is ordinary, and keying summaries by name alone would let one file's
/// `run` answer for another's — a wrong answer that reads exactly like a right
/// one.
#[derive(Debug, Clone, Default)]
pub struct Context {
    summaries: BTreeMap<(String, String), Summary>,
    /// Every candidate a call site resolves to. More than one means the call
    /// graph tagged the call `AMBIGUOUS`, and following a single candidate
    /// silently would be presenting a guess as an answer.
    targets: BTreeMap<(String, String, String), Vec<Target>>,
}

impl Context {
    /// What the call to `callee` inside `function` may reach.
    fn targets_at(&self, file: &str, function: &str, callee: &str) -> &[Target] {
        self.targets
            .get(&(file.to_string(), function.to_string(), callee.to_string()))
            .map_or(&[], Vec::as_slice)
    }

    /// The summaries of everything that call site may reach, in candidate order.
    fn summaries_at(&self, file: &str, function: &str, callee: &str) -> Vec<(&Target, &Summary)> {
        self.targets_at(file, function, callee)
            .iter()
            .filter_map(|target| {
                self.summaries
                    .get(&(target.file.clone(), target.function.clone()))
                    .map(|summary| (target, summary))
            })
            .collect()
    }

    /// Whether a call neutralizes what it is passed — a name from the built-in
    /// list, or a resolved callee the summaries showed to be one.
    ///
    /// One sanitizing candidate is enough. Where the call is ambiguous that
    /// suppresses a flow the other candidate might carry, which is the same
    /// direction the rest of the sanitizer handling leans.
    fn sanitizes(&self, file: &str, function: &str, callee: &str) -> bool {
        SANITIZERS.contains(&callee)
            || self
                .summaries_at(file, function, callee)
                .iter()
                .any(|(_, summary)| summary.sanitizer)
    }
}

/// Which names carry an external value, and how they got it.
#[derive(Debug, Clone, Default)]
struct Taint {
    /// Tainted name to the `(line, name)` the value entered through.
    origin: BTreeMap<String, (u32, String)>,
    /// Tainted name to the assignments that carried the value to it.
    hops: BTreeMap<String, Vec<(u32, String)>>,
    /// Names that would have been tainted but for a sanitizer, and which one
    /// stopped them. Reported so a reader knows the analysis saw the flow and
    /// dropped it on purpose.
    stopped: BTreeMap<String, (u32, String)>,
}

/// One source-to-sink flow, with the hops that carried it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaintFinding {
    /// Function the flow lives in.
    pub function: String,
    /// Name the value entered through.
    pub source: String,
    /// 1-based line the value entered at.
    pub source_line: u32,
    /// Call that consumed it.
    pub sink: String,
    /// 1-based line of that call.
    pub sink_line: u32,
    /// `(line, name)` for each assignment that carried the value along.
    pub hops: Vec<(u32, String)>,
    /// Whether a branch decides that the sink runs — a validated flow and an
    /// unguarded one deserve different attention.
    pub guarded: bool,
}

/// One source-to-sink flow that may cross call boundaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossFinding {
    /// Function the value entered.
    pub function: String,
    /// File that function lives in.
    pub file: String,
    /// Name the value entered through.
    pub source: String,
    /// 1-based line the value entered at.
    pub source_line: u32,
    /// Call that consumed it.
    pub sink: String,
    /// 1-based line of that call.
    pub sink_line: u32,
    /// Function the sink is called from — the same function when the flow never
    /// left it.
    pub sink_function: String,
    /// File that function lives in.
    pub sink_file: String,
    /// `(callee, call line)` for each call boundary the value crossed, nearest
    /// caller first. Empty for a flow inside one function.
    pub via: Vec<(String, u32)>,
    /// `(line, name)` for each assignment that carried the value along, in the
    /// function it entered.
    pub hops: Vec<(u32, String)>,
    /// Whether a branch decides that the sink runs.
    pub guarded: bool,
    /// Whether a crossed callee was matched by name rather than by a resolved
    /// `calls` edge. A crossing like that can follow the wrong function, since a
    /// call site carries the tail identifier only.
    pub guessed_callee: bool,
    /// How many callees the crossed call site could reach. Above one, the call
    /// graph tagged it `AMBIGUOUS` and this flow is one candidate's behavior,
    /// not the call's only possible behavior.
    pub candidates: usize,
}

/// The first argument of `call` that carries an external value, with the
/// position it occupies — position is what maps it onto a parameter.
fn tainted_argument<'a>(call: &'a Call, state: &Taint) -> Option<(usize, &'a str)> {
    call.arguments
        .iter()
        .enumerate()
        .find_map(|(position, group)| {
            group
                .iter()
                .find(|name| state.origin.contains_key(name.as_str()))
                .map(|name| (position, name.as_str()))
        })
}

impl Cfg {
    /// Source-to-sink flows inside this function.
    ///
    /// Intraprocedural and syntactic: taint spreads when a definition's line
    /// reads an already-tainted name, which is line-granular, not
    /// expression-granular. It cannot follow a value through a field or a
    /// container. Treat a finding as a place to look, never as a proven
    /// vulnerability, and treat the absence of findings as no evidence at all.
    ///
    /// For flows that cross a call, use [`Program::findings`].
    #[must_use]
    pub fn taint_findings(&self) -> Vec<TaintFinding> {
        self.flows("", &Context::default())
            .into_iter()
            .map(|finding| TaintFinding {
                function: finding.function,
                source: finding.source,
                source_line: finding.source_line,
                sink: finding.sink,
                sink_line: finding.sink_line,
                hops: finding.hops,
                guarded: finding.guarded,
            })
            .collect()
    }

    /// Names that carry an external input directly.
    fn source_seeds(&self) -> BTreeMap<String, (u32, String)> {
        let mut seeds = BTreeMap::new();
        for usage in &self.uses {
            if TAINT_SOURCES.contains(&usage.name.as_str()) {
                seeds.insert(usage.name.clone(), (usage.line, usage.name.clone()));
            }
        }
        for definition in &self.definitions {
            if TAINT_SOURCES.contains(&definition.name.as_str()) {
                seeds.insert(
                    definition.name.clone(),
                    (definition.line, definition.name.clone()),
                );
            }
        }
        seeds
    }

    /// Propagates taint from `seeds` through this function's assignments.
    ///
    /// A definition becomes tainted when its line reads a tainted name, when it
    /// is assigned from a call that returns one of its tainted arguments, or
    /// when it is assigned from a call that reads an external input of its own.
    /// The last two are what following a value back out of a callee means here.
    /// A sanitizer on the line stops all three.
    fn taint(
        &self,
        file: &str,
        seeds: &BTreeMap<String, (u32, String)>,
        context: &Context,
    ) -> Taint {
        let mut state = Taint {
            origin: seeds.clone(),
            hops: seeds
                .keys()
                .map(|name| (name.clone(), Vec::new()))
                .collect(),
            stopped: BTreeMap::new(),
        };
        // No seeds and no callee summaries means nothing can become tainted. A
        // function whose only input arrives through a callee's return has no
        // seeds of its own, so the summaries have to be checked too.
        if seeds.is_empty() && context.summaries.is_empty() {
            return state;
        }
        // Bounded rather than run to a fixpoint: eight rounds is longer than a
        // chain of assignments a reader would follow, and an unbounded loop over
        // syntactic definitions is a liability in a tool that must not hang.
        for _ in 0..8 {
            let mut grew = false;
            for definition in &self.definitions {
                if state.origin.contains_key(&definition.name) {
                    continue;
                }
                let Some((carrier, origin)) = self.carrier(file, definition, &state, context)
                else {
                    continue;
                };
                if let Some(sanitizer) = self.sanitizer_on(file, definition.line, context) {
                    state
                        .stopped
                        .insert(definition.name.clone(), (definition.line, sanitizer));
                    continue;
                }
                let mut chain = carrier
                    .as_ref()
                    .and_then(|name| state.hops.get(name).cloned())
                    .unwrap_or_default();
                chain.push((definition.line, definition.name.clone()));
                state.hops.insert(definition.name.clone(), chain);
                state.origin.insert(definition.name.clone(), origin);
                grew = true;
            }
            if !grew {
                break;
            }
        }
        state
    }

    /// Why a definition is tainted: the name whose chain it continues, if any,
    /// and the origin it inherits.
    fn carrier(
        &self,
        file: &str,
        definition: &Definition,
        state: &Taint,
        context: &Context,
    ) -> Option<(Option<String>, (u32, String))> {
        let read = self
            .uses
            .iter()
            .find(|usage| usage.line == definition.line && state.origin.contains_key(&usage.name));
        if let Some(usage) = read {
            let origin = state
                .origin
                .get(&usage.name)
                .cloned()
                .unwrap_or((definition.line, definition.name.clone()));
            return Some((Some(usage.name.clone()), origin));
        }
        for call in self
            .calls
            .iter()
            .filter(|call| call.line == definition.line)
        {
            for (_, summary) in context.summaries_at(file, &self.function, &call.callee) {
                if let Some((position, name)) = tainted_argument(call, state)
                    && summary.returns_parameter.contains(&position)
                {
                    let origin = state
                        .origin
                        .get(name)
                        .cloned()
                        .unwrap_or((definition.line, name.to_string()));
                    return Some((Some(name.to_string()), origin));
                }
                if let Some((source, _)) = &summary.returns_source {
                    // The callee's own line number would point into another
                    // file, so the value is reported as entering at the call.
                    return Some((
                        None,
                        (definition.line, format!("{}() -> {source}", call.callee)),
                    ));
                }
            }
        }
        None
    }

    /// A tainted name read on `line`, whatever position it occupies.
    fn tainted_on_line(&self, line: u32, state: &Taint) -> Option<String> {
        self.uses
            .iter()
            .find(|usage| usage.line == line && state.origin.contains_key(&usage.name))
            .map(|usage| usage.name.clone())
    }

    /// The sanitizer called on `line`, if any.
    fn sanitizer_on(&self, file: &str, line: u32, context: &Context) -> Option<String> {
        self.calls
            .iter()
            .find(|call| call.line == line && context.sanitizes(file, &self.function, &call.callee))
            .map(|call| call.callee.clone())
    }

    /// Flows this function's body exposes, seeded by the inputs it reads
    /// itself.
    fn flows(&self, file: &str, context: &Context) -> Vec<CrossFinding> {
        let state = self.taint(file, &self.source_seeds(), context);
        self.flows_from(file, &state, context)
    }

    /// Flows reachable from an already-computed taint state — the shape both a
    /// direct query and a summary need.
    fn flows_from(&self, file: &str, state: &Taint, context: &Context) -> Vec<CrossFinding> {
        if state.origin.is_empty() {
            return Vec::new();
        }
        let guards: HashMap<usize, usize> = self.control_dependence().into_iter().collect();
        let mut findings = Vec::new();
        for call in &self.calls {
            if self.sanitizer_on(file, call.line, context).is_some() {
                continue;
            }
            let is_sink = TAINT_SINKS.contains(&call.callee.as_str());
            // A sink takes any tainted name on its line, because a chain like
            // `Command::new(sh).arg(cmd).spawn()` carries the value in the
            // receiver rather than in the sink call's own arguments. Crossing
            // into a callee still needs a position: that is what a parameter is
            // matched by.
            let carried = match tainted_argument(call, state) {
                Some((position, name)) => Some((Some(position), name.to_string())),
                None if is_sink => self
                    .tainted_on_line(call.line, state)
                    .map(|name| (None, name)),
                None => None,
            };
            let Some((position, name)) = carried else {
                continue;
            };
            let Some((source_line, source)) = state.origin.get(&name) else {
                continue;
            };
            let guarded = guards.contains_key(&call.block);
            let hops = state.hops.get(&name).cloned().unwrap_or_default();
            if is_sink {
                findings.push(CrossFinding {
                    function: self.function.clone(),
                    file: file.to_string(),
                    source: source.clone(),
                    source_line: *source_line,
                    sink: call.callee.clone(),
                    sink_line: call.line,
                    sink_function: self.function.clone(),
                    sink_file: file.to_string(),
                    via: Vec::new(),
                    hops,
                    guarded,
                    guessed_callee: false,
                    candidates: 1,
                });
                continue;
            }
            // Not a sink itself: ask what each callee it may reach does with the
            // argument at that position.
            let Some(position) = position else {
                continue;
            };
            let candidates = context.summaries_at(file, &self.function, &call.callee);
            let Some((target, reach)) = candidates.iter().find_map(|(target, summary)| {
                summary
                    .sink_reaching
                    .get(&position)
                    .map(|reach| (*target, reach))
            }) else {
                continue;
            };
            let mut via = vec![(call.callee.clone(), call.line)];
            via.extend(reach.via.iter().cloned());
            findings.push(CrossFinding {
                function: self.function.clone(),
                file: file.to_string(),
                source: source.clone(),
                source_line: *source_line,
                sink: reach.sink.clone(),
                sink_line: reach.sink_line,
                sink_function: reach.sink_function.clone(),
                sink_file: reach.sink_file.clone(),
                via,
                hops,
                guarded: guarded || reach.guarded,
                guessed_callee: !target.by_edge,
                candidates: context.targets_at(file, &self.function, &call.callee).len(),
            });
        }
        findings
    }

    /// What this function does to the values passed into it, given what it
    /// knows about its own callees.
    fn summarize(&self, file: &str, context: &Context) -> Summary {
        let mut summary = Summary {
            function: self.function.clone(),
            file: file.to_string(),
            parameters: self.parameters.clone(),
            ..Summary::default()
        };
        for (position, parameter) in self.parameters.iter().enumerate() {
            let seeds = BTreeMap::from([(
                parameter.clone(),
                (self.parameter_line(parameter), parameter.clone()),
            )]);
            let state = self.taint(file, &seeds, context);
            if let Some((_, sanitizer)) = state.stopped.values().next() {
                summary
                    .sanitized_parameters
                    .insert(position, sanitizer.clone());
            }
            if let Some(finding) = self.flows_from(file, &state, context).into_iter().next() {
                summary.sink_reaching.insert(
                    position,
                    SinkReach {
                        sink: finding.sink,
                        sink_line: finding.sink_line,
                        sink_function: finding.sink_function,
                        sink_file: finding.sink_file,
                        via: finding.via,
                        guarded: finding.guarded,
                    },
                );
            }
            for (line, names) in &self.returns {
                let sanitized = self.sanitizer_on(file, *line, context).is_some();
                let returns_value = names.iter().any(|name| state.origin.contains_key(name));
                if returns_value && !sanitized {
                    summary.returns_parameter.insert(position);
                }
                // Returning the parameter only after escaping it — either on the
                // return line itself or through an assignment a sanitizer
                // stopped — is what makes a function a sanitizer to its callers.
                if (returns_value && sanitized)
                    || names.iter().any(|name| state.stopped.contains_key(name))
                {
                    summary.sanitizer = true;
                }
            }
        }
        let state = self.taint(file, &self.source_seeds(), context);
        summary.returns_source = self.returns.iter().find_map(|(line, names)| {
            names.iter().find_map(|name| {
                state
                    .origin
                    .get(name)
                    .map(|(_, source)| (source.clone(), *line))
            })
        });
        // A function that hands a parameter back unchanged is not a sanitizer,
        // even if it escapes something else on the way.
        if !summary.returns_parameter.is_empty() {
            summary.sanitizer = false;
        }
        summary
    }

    /// Line a parameter is bound on, or 0 when the binding was not recorded.
    fn parameter_line(&self, parameter: &str) -> u32 {
        self.definitions
            .iter()
            .find(|definition| definition.name == parameter)
            .map_or(0, |definition| definition.line)
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
        .filter_map(|(name, function)| {
            let body = function.child_by_field_name("body")?;
            Some(build_cfg(&name, &function, &body, source))
        })
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
    if FUNCTION_KINDS.contains(&node.kind().as_str()) && node.child_by_field_name("body").is_some()
    {
        let name = node
            .child_by_field_name("name")
            .and_then(|name| text(&name, source))
            .unwrap_or("<anonymous>")
            .to_string();
        out.push((name, node.clone()));
    }
    for index in 0..u32::try_from(node.named_child_count()).unwrap_or(u32::MAX) {
        if let Some(child) = node.named_child(index) {
            collect_functions(&child, source, out);
        }
    }
}

/// Fields that hold a function's parameter list. `parameter` is last because
/// JavaScript's parenthesis-free arrow function puts a single identifier there
/// instead of a list.
const PARAMETER_FIELDS: &[&str] = &[
    "parameters",
    "parameter_list",
    "formal_parameters",
    "parameter",
];

/// Fields inside one parameter that hold its name.
const PARAMETER_NAME_FIELDS: &[&str] = &["pattern", "name", "declarator"];

/// Declared parameter names in order, which is what maps a caller's argument
/// position onto a callee's value.
///
/// A parameter that declares several names at once (Go's `func f(a, b int)`)
/// contributes only its first, and a receiver (`self`, `this`) contributes
/// nothing: the caller does not pass it at a position.
fn parameters_of(declaration: &tree_sitter_language_pack::Node, source: &str) -> Vec<String> {
    let Some(list) = PARAMETER_FIELDS
        .iter()
        .find_map(|field| declaration.child_by_field_name(field))
    else {
        return Vec::new();
    };
    if list.kind() == "identifier" {
        return text(&list, source)
            .and_then(identifier_head)
            .map(|name| vec![name.to_string()])
            .unwrap_or_default();
    }
    (0..u32::try_from(list.named_child_count()).unwrap_or(u32::MAX))
        .filter_map(|index| list.named_child(index))
        .filter_map(|parameter| {
            if parameter.kind() == "self_parameter" {
                return None;
            }
            let name = PARAMETER_NAME_FIELDS
                .iter()
                .find_map(|field| parameter.child_by_field_name(field))
                .and_then(|node| text(&node, source))
                .or_else(|| text(&parameter, source))
                .and_then(identifier_head)?;
            if matches!(name, "self" | "this") {
                return None;
            }
            Some(name.to_string())
        })
        .collect()
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
fn build_cfg(
    function: &str,
    declaration: &tree_sitter_language_pack::Node,
    body: &tree_sitter_language_pack::Node,
    source: &str,
) -> Cfg {
    let mut cfg = Cfg {
        function: function.to_string(),
        parameters: parameters_of(declaration, source),
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
        // A branch or loop owns only its condition: its body becomes its own
        // blocks, and recording the whole subtree here would count every
        // nested definition, use, and call twice — once in the guard's block
        // and once in the body's.
        match condition_of(statement) {
            Some(condition) => self.record_data_flow(&condition, block),
            None => self.record_data_flow(statement, block),
        }
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
            Some(BlockExit::Return) => self.walk_return(block, statement, line),
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

    /// Ends a block at a `return`, recording what the function hands back —
    /// that is what a caller's assignment inherits, and what a summary reads.
    fn walk_return(
        &mut self,
        block: usize,
        statement: &tree_sitter_language_pack::Node,
        line: u32,
    ) {
        let mut returned = Vec::new();
        collect_identifiers(statement, self.source, &mut returned);
        self.cfg.returns.push((line, returned));
        self.terminate(block, BlockExit::Return, statement);
        self.pending.push((block, FlowEdge::Sequential));
    }

    /// Records the writes, reads, and calls a statement performs.
    fn record_data_flow(&mut self, statement: &tree_sitter_language_pack::Node, block: usize) {
        let mut defined: HashSet<String> = HashSet::new();
        self.collect_definitions(statement, block, &mut defined);
        self.collect_uses(statement, block, &defined);
        self.collect_calls(statement, block);
    }

    fn collect_calls(&mut self, node: &tree_sitter_language_pack::Node, block: usize) {
        if matches!(
            node.kind().as_str(),
            "call_expression" | "call" | "invocation_expression" | "macro_invocation"
        ) {
            let callee = ["function", "callee", "name", "macro"]
                .into_iter()
                .find_map(|field| node.child_by_field_name(field))
                .and_then(|target| text(&target, self.source))
                .and_then(callee_tail)
                .unwrap_or_default()
                .to_string();
            // Grouped per argument, keeping empty groups: a literal argument
            // still occupies the position its parameter is matched by.
            let arguments = ["arguments", "argument_list", "parameters"]
                .into_iter()
                .find_map(|field| node.child_by_field_name(field))
                .map(|args| {
                    (0..u32::try_from(args.named_child_count()).unwrap_or(u32::MAX))
                        .filter_map(|index| args.named_child(index))
                        .map(|argument| {
                            let mut names = Vec::new();
                            collect_identifiers(&argument, self.source, &mut names);
                            names
                        })
                        .collect()
                })
                .unwrap_or_default();
            if !callee.is_empty() {
                self.cfg.calls.push(Call {
                    callee,
                    block,
                    line: line_of(node),
                    arguments,
                });
            }
        }
        for index in 0..u32::try_from(node.named_child_count()).unwrap_or(u32::MAX) {
            if let Some(child) = node.named_child(index) {
                if FUNCTION_KINDS.contains(&child.kind().as_str()) {
                    continue;
                }
                self.collect_calls(&child, block);
            }
        }
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
        // A property read is a read: `req.query` reads `query`. Such a name
        // rarely has a local definition, so def-use chains simply find nothing
        // for it, while taint seeding gets the input it needs.
        if matches!(
            node.kind().as_str(),
            "identifier" | "property_identifier" | "field_identifier"
        ) && let Some(name) = text(node, self.source)
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
/// The part of a branch or loop that runs in the guard's own block: its
/// condition. `None` for a statement that owns its whole subtree.
fn condition_of(node: &tree_sitter_language_pack::Node) -> Option<tree_sitter_language_pack::Node> {
    if !matches!(
        terminator_exit(node.kind().as_str()),
        Some(BlockExit::Branch | BlockExit::Loop)
    ) {
        return None;
    }
    ["condition", "value", "left", "initializer"]
        .into_iter()
        .find_map(|field| node.child_by_field_name(field))
        .or_else(|| node.named_child(0))
}

/// The final identifier of a callee expression (`fs.writeFileSync` →
/// `writeFileSync`, `Command::new` → `new`).
fn callee_tail(raw: &str) -> Option<&str> {
    raw.trim()
        .rsplit(|character: char| !character.is_alphanumeric() && character != '_')
        .find(|part| !part.is_empty())
}

fn collect_identifiers(
    node: &tree_sitter_language_pack::Node,
    source: &str,
    out: &mut Vec<String>,
) {
    if node.kind() == "identifier"
        && let Some(name) = text(node, source)
    {
        out.push(name.to_string());
    }
    for index in 0..u32::try_from(node.named_child_count()).unwrap_or(u32::MAX) {
        if let Some(child) = node.named_child(index) {
            collect_identifiers(&child, source, out);
        }
    }
}

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
    let source = read(path)?;
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

/// Renders the dependence graph for one file, or for one line of it.
///
/// # Errors
/// Returns an error when the file cannot be read or parsed.
pub fn format_pdg(path: &std::path::Path, line: Option<u32>) -> Result<String> {
    let source = read(path)?;
    let name = path.to_string_lossy();
    let mut out = Vec::new();
    for cfg in analyze(&name, &source)? {
        let dependences = match line {
            Some(line) => cfg.dependences_of(line),
            None => cfg.dependences(),
        };
        if dependences.is_empty() {
            continue;
        }
        out.push(format!("## {}", cfg.function));
        for (dependent, source_line, why) in dependences {
            out.push(format!(
                "line {dependent} depends on line {source_line} ({})",
                why.as_str()
            ));
        }
        out.push(String::new());
    }
    Ok(if out.is_empty() {
        match line {
            Some(line) => format!("nothing in {name} depends on line {line}"),
            None => format!("no dependences extracted from {name}"),
        }
    } else {
        out.join("\n").trim_end().to_string()
    })
}

/// Functions joined by the call graph: one entry file's functions plus the
/// callees they reach, each summarized so a flow can cross a call.
#[derive(Debug, Clone, Default)]
pub struct Program {
    /// `(file, cfg)` in load order — the entry file's functions first.
    functions: Vec<(String, Cfg)>,
    /// How many distinct files contributed.
    files: usize,
    context: Context,
}

/// Cap on how many functions one run joins. A bound the analysis can state
/// beats a run that quietly takes minutes on a large repository.
const PROGRAM_BUDGET: usize = 400;

/// How many times summaries are recomputed so a callee's summary can inform its
/// caller's. Three rounds carry a sink three calls up the chain, which is as far
/// as the loader's default depth reaches anyway.
const SUMMARY_ROUNDS: usize = 3;

impl Program {
    /// Cross-function source-to-sink flows, entry file first.
    #[must_use]
    pub fn findings(&self) -> Vec<CrossFinding> {
        self.functions
            .iter()
            .flat_map(|(file, cfg)| cfg.flows(file, &self.context))
            .collect()
    }

    /// Flows a sanitizer stopped, as `(function, line, sanitizer)`. Reported so
    /// a reader can tell "nothing reaches a sink" from "something did, and was
    /// escaped on the way".
    #[must_use]
    pub fn stopped(&self) -> Vec<(String, u32, String)> {
        let mut out = Vec::new();
        for (file, cfg) in &self.functions {
            let state = cfg.taint(file, &cfg.source_seeds(), &self.context);
            for (line, sanitizer) in state.stopped.values() {
                out.push((cfg.function.clone(), *line, format!("{sanitizer}()")));
            }
            // A tainted value handed to a sanitizer leaves no assignment behind
            // when the call is a statement of its own, and a value stopped
            // *inside* a callee leaves nothing in this function at all — so the
            // call sites are scanned against the callee summaries too.
            for call in &cfg.calls {
                let Some((position, _)) = tainted_argument(call, &state) else {
                    continue;
                };
                if self.context.sanitizes(file, &cfg.function, &call.callee) {
                    out.push((
                        cfg.function.clone(),
                        call.line,
                        format!("{}()", call.callee),
                    ));
                    continue;
                }
                if let Some(sanitizer) = self
                    .context
                    .summaries_at(file, &cfg.function, &call.callee)
                    .iter()
                    .find_map(|(_, summary)| summary.sanitized_parameters.get(&position))
                {
                    out.push((
                        cfg.function.clone(),
                        call.line,
                        format!("{sanitizer}() inside {}()", call.callee),
                    ));
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// What one joined function does to the values passed into it.
    #[must_use]
    pub fn summary(&self, file: &str, function: &str) -> Option<&Summary> {
        self.context
            .summaries
            .get(&(file.to_string(), function.to_string()))
    }

    /// Functions joined, and how many files they came from.
    #[must_use]
    pub const fn reach(&self) -> (usize, usize) {
        (self.functions.len(), self.files)
    }
}

/// Reads flow graphs for a repository, resolving a call site's callee through
/// the indexed graph when there is one.
struct Loader {
    /// Directory paths are relative to: the indexed repository root when one was
    /// found above the entry file, otherwise the entry file's own directory.
    root: std::path::PathBuf,
    /// The indexed graph, when the repository has one. Its absence is not an
    /// error — it only limits joining to calls within one file.
    graph: Option<crate::storage::Graph>,
    /// Parsed files, keyed by the relative path they were read from.
    parsed: BTreeMap<String, BTreeMap<String, Cfg>>,
}

impl Loader {
    /// Opens the repository containing `file`, if it is indexed.
    fn open(file: &std::path::Path) -> Self {
        let absolute = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
        let indexed = absolute
            .ancestors()
            .skip(1)
            .find(|ancestor| ancestor.join(".aag").join("graph.db").is_file());
        let root = indexed
            .or_else(|| absolute.parent())
            .map(std::path::Path::to_path_buf)
            .unwrap_or_default();
        let graph = indexed.and_then(|root| crate::storage::Graph::open_existing(root).ok());
        Self {
            root,
            graph,
            parsed: BTreeMap::new(),
        }
    }

    /// `file` as the graph would name it: relative to the root, forward slashes.
    fn relative(&self, file: &std::path::Path) -> String {
        let absolute = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
        absolute
            .strip_prefix(&self.root)
            .unwrap_or(&absolute)
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// Flow graphs for one relative path, parsed once and cached. A file that
    /// cannot be read or has no flow frontend contributes nothing rather than
    /// failing the run — the entry file is the only one a caller asked for.
    fn flow(&mut self, file: &str) -> &BTreeMap<String, Cfg> {
        if !self.parsed.contains_key(file) {
            let graphs = read(&self.root.join(file))
                .and_then(|source| analyze_map(file, &source))
                .unwrap_or_default();
            self.parsed.insert(file.to_string(), graphs);
        }
        &self.parsed[file]
    }

    /// Where a call site's callee is defined: whatever the indexed graph
    /// resolved that call to, then the origin file's own functions.
    ///
    /// Resolution is not reimplemented here. The graph already applied the
    /// language-aware ladder — receiver type, import binding, module qualifier —
    /// so this reads its answer, and only guesses when there is none. The graph
    /// comes first for a reason: a call site here carries the tail identifier
    /// only, so `crate::bigbang::run` and a local `run` are the same string, and
    /// preferring the local one silently follows the wrong function.
    /// Every candidate, because a call the graph could not narrow to one symbol
    /// has more than one, and picking one silently would hide that.
    fn resolve(&mut self, target: &str, origin_file: &str, origin: &str) -> Vec<Target> {
        let by_edge = self.resolve_through_graph(target, origin_file, origin);
        if !by_edge.is_empty() {
            return by_edge;
        }
        if self.flow(origin_file).contains_key(target) {
            return vec![Target {
                file: origin_file.to_string(),
                function: target.to_string(),
                by_edge: false,
            }];
        }
        self.graph
            .as_ref()
            .and_then(|graph| graph.find_by_name(target).ok().flatten())
            .map(|node| {
                vec![Target {
                    file: node.file_path,
                    function: node.name,
                    by_edge: false,
                }]
            })
            .unwrap_or_default()
    }

    /// The callees indexed `calls` edges point at. More than one means the call
    /// graph could not narrow the call to a single symbol and tagged it
    /// `AMBIGUOUS`.
    fn resolve_through_graph(&self, target: &str, origin_file: &str, origin: &str) -> Vec<Target> {
        let Some(graph) = self.graph.as_ref() else {
            return Vec::new();
        };
        let Some(id) = graph
            .find_in_file(origin, origin_file)
            .ok()
            .flatten()
            .and_then(|node| node.id)
        else {
            return Vec::new();
        };
        graph
            .callees(id)
            .unwrap_or_default()
            .into_iter()
            .filter(|(node, kind, _)| {
                *kind == crate::storage::EdgeKind::Calls && node.name == target
            })
            .map(|(node, _, _)| Target {
                file: node.file_path,
                function: node.name,
                by_edge: true,
            })
            .collect()
    }
}

/// Joins `file`'s functions with the ones they call, within `depth` call hops.
///
/// Depth 0 is one file's functions on their own; the callees of the entry file
/// are one hop away. Cross-file joining needs an index under an ancestor of
/// `file` — without one, only calls to functions in the same file are followed,
/// which the caller can see in [`Program::reach`].
///
/// # Errors
/// Returns [`Error::Parse`] when the entry file cannot be read or parsed.
pub fn program(file: &std::path::Path, depth: u32) -> Result<Program> {
    let mut loader = Loader::open(file);
    let entry = loader.relative(file);
    let source = read(&loader.root.join(&entry)).or_else(|_| read(file))?;
    let mut functions: Vec<(String, Cfg)> = analyze(&entry, &source)?
        .into_iter()
        .map(|cfg| (entry.clone(), cfg))
        .collect();
    loader
        .parsed
        .insert(entry.clone(), analyze_map(&entry, &source)?);

    let mut seen: BTreeSet<(String, String)> = functions
        .iter()
        .map(|(file, cfg)| (file.clone(), cfg.function.clone()))
        .collect();
    let mut frontier = functions.clone();
    let mut targets: BTreeMap<(String, String, String), Vec<Target>> = BTreeMap::new();
    for _ in 0..depth {
        let mut next: Vec<(String, Cfg)> = Vec::new();
        for (file_path, cfg) in &frontier {
            for call in &cfg.calls {
                if functions.len() + next.len() >= PROGRAM_BUDGET {
                    break;
                }
                // A known sink or sanitizer is answered by its name; descending
                // into its body would only spend time.
                if TAINT_SINKS.contains(&call.callee.as_str())
                    || SANITIZERS.contains(&call.callee.as_str())
                {
                    continue;
                }
                let candidates = loader.resolve(&call.callee, file_path, &cfg.function);
                if candidates.is_empty() {
                    continue;
                }
                targets.insert(
                    (file_path.clone(), cfg.function.clone(), call.callee.clone()),
                    candidates.clone(),
                );
                for candidate in candidates {
                    if !seen.insert((candidate.file.clone(), candidate.function.clone())) {
                        continue;
                    }
                    if let Some(callee) = loader
                        .flow(&candidate.file)
                        .get(&candidate.function)
                        .cloned()
                    {
                        next.push((candidate.file, callee));
                    }
                }
            }
        }
        if next.is_empty() {
            break;
        }
        functions.extend(next.iter().cloned());
        frontier = next;
    }

    // Summaries iterate so that a callee summarized in one round informs its
    // caller's summary in the next, which is what carries a sink up a chain.
    let mut context = Context {
        targets,
        ..Context::default()
    };
    for _ in 0..SUMMARY_ROUNDS {
        let mut summaries = BTreeMap::new();
        for (file_path, cfg) in &functions {
            summaries.insert(
                (file_path.clone(), cfg.function.clone()),
                cfg.summarize(file_path, &context),
            );
        }
        if summaries == context.summaries {
            break;
        }
        context.summaries = summaries;
    }

    let files = functions
        .iter()
        .map(|(file, _)| file.clone())
        .collect::<BTreeSet<_>>()
        .len();
    Ok(Program {
        functions,
        files,
        context,
    })
}

/// Renders source-to-sink findings for one file, following values across calls.
///
/// # Errors
/// Returns an error when the file cannot be read or parsed.
pub fn format_taint(path: &std::path::Path, depth: u32) -> Result<String> {
    let name = path.to_string_lossy();
    let program = program(path, depth)?;
    let findings = program.findings();
    let stopped = program.stopped();
    let (joined, files) = program.reach();

    let mut out = Vec::new();
    for finding in &findings {
        let hops = if finding.hops.is_empty() {
            "direct".to_string()
        } else {
            finding
                .hops
                .iter()
                .map(|(line, name)| format!("{name}@{line}"))
                .collect::<Vec<_>>()
                .join(" -> ")
        };
        let crossing = if finding.via.is_empty() {
            String::new()
        } else {
            let chain = finding
                .via
                .iter()
                .map(|(callee, line)| format!("{callee}() at line {line}"))
                .collect::<Vec<_>>()
                .join(" -> ");
            let elsewhere = if finding.sink_file == finding.file {
                String::new()
            } else {
                format!(" in {}", finding.sink_file)
            };
            let caveat = if finding.guessed_callee {
                " (callee matched by name, not by a resolved edge)".to_string()
            } else if finding.candidates > 1 {
                format!(
                    " (one of {} callees this ambiguous call may reach)",
                    finding.candidates
                )
            } else {
                String::new()
            };
            format!(
                " — through {chain}, sinking in {}{elsewhere}{caveat}",
                finding.sink_function
            )
        };
        out.push(format!(
            "{}: {} (line {}) reaches {}() at line {} via {}{}{}",
            finding.function,
            finding.source,
            finding.source_line,
            finding.sink,
            finding.sink_line,
            hops,
            crossing,
            if finding.guarded {
                " — guarded by a branch"
            } else {
                " — no branch in between"
            }
        ));
    }

    if out.is_empty() {
        let mut lines = vec![format!(
            "no source-to-sink flows found in {name}. This analysis is syntactic \
             and line-granular, so absence of findings is not evidence of safety."
        )];
        lines.extend(sanitizer_notes(&stopped));
        return Ok(lines.join("\n"));
    }

    let mut lines = vec![
        "Syntactic taint: each finding is a place to look, not a proven \
         vulnerability."
            .to_string(),
        format!(
            "{joined} function{} joined from {files} file{}, {} call hop{}.",
            if joined == 1 { "" } else { "s" },
            if files == 1 { "" } else { "s" },
            depth,
            if depth == 1 { "" } else { "s" }
        ),
        String::new(),
    ];
    lines.extend(out);
    let notes = sanitizer_notes(&stopped);
    if !notes.is_empty() {
        lines.push(String::new());
        lines.extend(notes);
    }
    Ok(lines.join("\n"))
}

/// The tail of a taint report: what a sanitizer stopped, so silence is
/// distinguishable from suppression.
fn sanitizer_notes(stopped: &[(String, u32, String)]) -> Vec<String> {
    stopped
        .iter()
        .map(|(function, line, sanitizer)| {
            format!("stopped at a sanitizer: {sanitizer} at line {line} in {function}")
        })
        .collect()
}

fn read(path: &std::path::Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|error| Error::Parse {
        file: path.display().to_string(),
        reason: error.to_string(),
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
    fn dependences_separate_control_from_data() {
        let source = "fn run(flag: bool) {\n    let value = 1;\n    if flag {\n        let out = value;\n    }\n}";
        let cfg = cfg_of("a.rs", source, "run");
        let kinds: BTreeSet<Dependence> =
            cfg.dependences().iter().map(|(_, _, why)| *why).collect();
        assert!(
            kinds.contains(&Dependence::Data),
            "`out` reads `value`: {:?}",
            cfg.dependences()
        );
        assert!(
            kinds.contains(&Dependence::Control),
            "the guarded statement depends on the branch: {:?}",
            cfg.dependences()
        );
    }

    #[test]
    fn transitive_dependences_walk_backwards() {
        let source = "fn run() {\n    let a = 1;\n    let b = a;\n    let c = b;\n}";
        let cfg = cfg_of("a.rs", source, "run");
        let sources: BTreeSet<u32> = cfg
            .dependences_of(4)
            .iter()
            .map(|(_, source, _)| *source)
            .collect();
        assert!(
            !sources.is_empty(),
            "`c` depends on `b` which depends on `a`: {:?}",
            cfg.dependences()
        );
    }

    #[test]
    fn taint_reports_an_unguarded_flow_from_input_to_a_sink() {
        let source = "function handle(req) {\n  const cmd = req.query;\n  exec(cmd);\n}";
        let cfg = cfg_of("a.js", source, "handle");
        let findings = cfg.taint_findings();
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].sink, "exec");
        assert!(!findings[0].guarded, "nothing decides whether exec runs");
        assert!(
            findings[0].hops.iter().any(|(_, name)| name == "cmd"),
            "the carrying assignment must be in the chain: {:?}",
            findings[0].hops
        );
    }

    #[test]
    fn taint_marks_a_flow_that_passes_a_branch_as_guarded() {
        let source = "function handle(req) {\n  const cmd = req.query;\n  if (allowed(cmd)) {\n    exec(cmd);\n  }\n}";
        let cfg = cfg_of("a.js", source, "handle");
        let findings = cfg.taint_findings();
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert!(
            findings[0].guarded,
            "a branch decides whether the sink runs, which is worth distinguishing"
        );
    }

    #[test]
    fn clean_function_produces_no_findings() {
        let cfg = cfg_of("a.js", "function run() { const a = 1; helper(a); }", "run");
        assert!(cfg.taint_findings().is_empty());
    }

    #[test]
    fn a_sink_reached_by_a_constant_is_not_a_finding() {
        let cfg = cfg_of(
            "a.js",
            "function run() { const cmd = \"ls\"; exec(cmd); }",
            "run",
        );
        assert!(
            cfg.taint_findings().is_empty(),
            "nothing external reaches this sink"
        );
    }

    /// Writes `files` into a scratch directory and joins the first one's flow.
    fn program_of(label: &str, files: &[(&str, &str)], depth: u32) -> Program {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("aag-flow-{label}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        for (name, source) in files {
            std::fs::write(root.join(name), source).unwrap();
        }
        program(&root.join(files[0].0), depth).unwrap()
    }

    #[test]
    fn parameters_are_ordered_and_exclude_the_receiver() {
        assert_eq!(
            cfg_of(
                "a.js",
                "function run(first, second) { return first; }",
                "run"
            )
            .parameters,
            vec!["first".to_string(), "second".to_string()]
        );
        assert_eq!(
            cfg_of(
                "a.rs",
                "struct S;\nimpl S {\n    fn run(&self, value: u8) -> u8 { value }\n}",
                "run"
            )
            .parameters,
            vec!["value".to_string()],
            "`self` is not a value a caller passes at a position"
        );
    }

    #[test]
    fn taint_crosses_a_call_into_the_callee_that_sinks_it() {
        let program = program_of(
            "cross",
            &[(
                "a.js",
                "function run(command) {\n  exec(command);\n}\n\
                 function handle(req) {\n  const cmd = req.query;\n  run(cmd);\n}\n",
            )],
            2,
        );
        let findings = program.findings();
        let crossing = findings
            .iter()
            .find(|finding| finding.function == "handle")
            .unwrap_or_else(|| panic!("the flow must cross into run: {findings:?}"));
        assert_eq!(crossing.sink, "exec");
        assert_eq!(crossing.sink_function, "run");
        assert_eq!(crossing.via, vec![("run".to_string(), 6)]);
    }

    #[test]
    fn a_tainted_argument_at_the_wrong_position_is_not_a_finding() {
        let program = program_of(
            "position",
            &[(
                "a.js",
                "function run(command, label) {\n  exec(command);\n}\n\
                 function handle(req) {\n  const cmd = req.query;\n  run(\"ls\", cmd);\n}\n",
            )],
            2,
        );
        assert!(
            program
                .findings()
                .iter()
                .all(|finding| finding.function != "handle"),
            "the tainted argument lands on `label`, which never reaches the sink: {:?}",
            program.findings()
        );
    }

    #[test]
    fn a_value_returned_by_a_callee_carries_that_callee_s_input_back() {
        let program = program_of(
            "return",
            &[(
                "a.js",
                "function readInput() {\n  const raw = process.argv;\n  return raw;\n}\n\
                 function handle() {\n  const cmd = readInput();\n  exec(cmd);\n}\n",
            )],
            2,
        );
        let findings = program.findings();
        assert!(
            findings
                .iter()
                .any(|finding| finding.function == "handle" && finding.sink == "exec"),
            "the input entered through the callee's return: {findings:?}"
        );
    }

    #[test]
    fn a_sanitizer_stops_the_flow_and_says_so() {
        let program = program_of(
            "sanitizer",
            &[(
                "a.js",
                "function handle(req) {\n  const cmd = req.query;\n  const safe = escape(cmd);\n  exec(safe);\n}\n",
            )],
            0,
        );
        assert!(
            program.findings().is_empty(),
            "the value was escaped before the sink: {:?}",
            program.findings()
        );
        assert_eq!(
            program.stopped(),
            vec![("handle".to_string(), 3, "escape()".to_string())],
            "silence and suppression must not read the same"
        );
    }

    #[test]
    fn a_function_that_escapes_what_it_returns_is_treated_as_a_sanitizer() {
        let program = program_of(
            "inferred",
            &[(
                "a.js",
                "function clean(value) {\n  return escape(value);\n}\n\
                 function handle(req) {\n  const cmd = req.query;\n  const safe = clean(cmd);\n  exec(safe);\n}\n",
            )],
            2,
        );
        assert!(
            program
                .summary("a.js", "clean")
                .expect("a summary for clean")
                .sanitizer,
            "a parameter that reaches the return only through a sanitizer makes one: {:?}",
            program.summary("a.js", "clean")
        );
        assert!(
            program.findings().is_empty(),
            "so the caller's flow ends at it: {:?}",
            program.findings()
        );
    }

    #[test]
    fn a_function_that_returns_its_parameter_unchanged_is_not_a_sanitizer() {
        let program = program_of(
            "passthrough",
            &[(
                "a.js",
                "function wrap(value) {\n  return value;\n}\n\
                 function handle(req) {\n  const cmd = req.query;\n  const same = wrap(cmd);\n  exec(same);\n}\n",
            )],
            2,
        );
        assert!(
            !program
                .summary("a.js", "wrap")
                .expect("a summary for wrap")
                .sanitizer
        );
        assert!(
            program
                .findings()
                .iter()
                .any(|finding| finding.function == "handle"),
            "the value came back unchanged: {:?}",
            program.findings()
        );
    }

    #[test]
    fn a_flow_two_calls_deep_still_reports_the_real_sink() {
        let program = program_of(
            "chain",
            &[(
                "a.js",
                "function sink(command) {\n  exec(command);\n}\n\
                 function middle(command) {\n  sink(command);\n}\n\
                 function handle(req) {\n  const cmd = req.query;\n  middle(cmd);\n}\n",
            )],
            3,
        );
        let findings = program.findings();
        let crossing = findings
            .iter()
            .find(|finding| finding.function == "handle")
            .unwrap_or_else(|| panic!("two hops must still land on exec: {findings:?}"));
        assert_eq!(crossing.sink_function, "sink");
        assert_eq!(
            crossing.via,
            vec![("middle".to_string(), 9), ("sink".to_string(), 5)],
            "the chain names every boundary the value crossed"
        );
    }

    #[test]
    fn a_sink_fed_through_a_method_chain_is_still_a_finding() {
        let program = program_of(
            "chain-receiver",
            &[(
                "a.rs",
                "fn run() {\n    let args: Vec<String> = std::env::args().collect();\n    \
                 std::process::Command::new(\"sh\").arg(&args[0]).spawn().unwrap();\n}\n",
            )],
            0,
        );
        let findings = program.findings();
        assert!(
            findings.iter().any(|finding| finding.sink == "spawn"),
            "the value rides the receiver, not the sink's own arguments: {findings:?}"
        );
    }

    #[test]
    fn without_an_index_only_the_entry_file_is_joined() {
        let program = program_of(
            "unindexed",
            &[
                (
                    "a.js",
                    "function handle(req) {\n  const cmd = req.query;\n  run(cmd);\n}\n",
                ),
                ("b.js", "function run(command) {\n  exec(command);\n}\n"),
            ],
            2,
        );
        assert_eq!(program.reach(), (1, 1));
        assert!(
            program.findings().is_empty(),
            "the callee lives in a file no index pointed at: {:?}",
            program.findings()
        );
    }

    #[test]
    fn an_indexed_repository_joins_a_callee_in_another_file() {
        let root = std::env::temp_dir().join(format!("aag-flow-crossfile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("handler.js"),
            "const { run } = require(\"./runner\");\n\
             function handle(req) {\n  const cmd = req.query;\n  run(cmd);\n}\n\
             module.exports = { handle };\n",
        )
        .unwrap();
        std::fs::write(
            root.join("runner.js"),
            "function run(command) {\n  exec(command);\n}\nmodule.exports = { run };\n",
        )
        .unwrap();
        crate::bigbang::run(
            &root,
            &crate::bigbang::Options {
                no_viz: true,
                no_install: true,
                ..crate::bigbang::Options::default()
            },
        )
        .unwrap();

        let program = program(&root.join("handler.js"), 2).unwrap();

        let findings = program.findings();
        let crossing = findings
            .iter()
            .find(|finding| finding.function == "handle")
            .unwrap_or_else(|| {
                panic!(
                    "the indexed call graph resolves `run` into runner.js: {findings:?}, reach {:?}",
                    program.reach()
                )
            });
        assert_eq!(crossing.sink_file, "runner.js");
        assert_eq!(crossing.sink_function, "run");
    }

    #[test]
    fn an_ambiguous_call_is_followed_but_reported_as_one_candidate() {
        let root = std::env::temp_dir().join(format!("aag-flow-samename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("handler.js"),
            // Both `run`s are loaded — the aliased call pulls danger.js in — so a
            // summary keyed by name alone would let it answer for safe.js.
            "const { run } = require(\"./safe\");\n\
             const { run: unsafeRun } = require(\"./danger\");\n\
             function handle(req) {\n  const cmd = req.query;\n  run(cmd);\n}\n\
             function admin() {\n  unsafeRun(\"ls\");\n}\n\
             module.exports = { handle, admin };\n",
        )
        .unwrap();
        std::fs::write(
            root.join("safe.js"),
            "function run(value) {\n  log(value);\n}\nmodule.exports = { run };\n",
        )
        .unwrap();
        std::fs::write(
            root.join("danger.js"),
            "function run(value) {\n  exec(value);\n}\nmodule.exports = { run };\n",
        )
        .unwrap();
        crate::bigbang::run(
            &root,
            &crate::bigbang::Options {
                no_viz: true,
                no_install: true,
                ..crate::bigbang::Options::default()
            },
        )
        .unwrap();

        let program = program(&root.join("handler.js"), 2).unwrap();

        // Two files export `run`, and the call graph could not narrow the call
        // to one of them. Following a single candidate silently would present a
        // guess as an answer; following none would hide a real sink.
        let findings = program.findings();
        let crossing = findings
            .iter()
            .find(|finding| finding.function == "handle")
            .unwrap_or_else(|| panic!("the sinking candidate must be reported: {findings:?}"));
        assert_eq!(crossing.sink_file, "danger.js");
        assert_eq!(
            crossing.candidates, 2,
            "and reported as one candidate of two"
        );
        assert!(
            program
                .summary("safe.js", "run")
                .expect("safe.js declares its own run")
                .sink_reaching
                .is_empty(),
            "each file's `run` keeps its own summary"
        );
    }

    #[test]
    fn calls_are_collected_with_their_arguments() {
        let cfg = cfg_of("a.js", "function run(x) { helper(x, 2); }", "run");
        let call = cfg
            .calls
            .iter()
            .find(|call| call.callee == "helper")
            .expect("the call site");
        assert_eq!(call.position_of("x"), Some(0), "{call:?}");
        assert_eq!(
            call.arguments.len(),
            2,
            "a literal argument keeps its position: {call:?}"
        );
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
