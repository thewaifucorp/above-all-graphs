//! Command-line surface for `aag`.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// `AboveAllGraphs` — code knowledge graph, always fresh, MCP-native.
#[derive(Debug, Parser)]
#[command(name = "aag", version, about)]
pub struct Cli {
    /// Subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level `aag` subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Bootstrap: detect agent, register hooks, run the first index. One shot.
    Bigbang {
        /// Repository root to index. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Discard any existing index and rebuild from scratch.
        #[arg(long)]
        force: bool,

        /// Skip writing `index.html`/`graph.html`/report/wiki/`graph.graphml`/`cypher.txt`.
        #[arg(long)]
        no_viz: bool,

        /// Also write an Obsidian-compatible export under `<dir>/aag/`.
        #[arg(long)]
        obsidian: bool,

        /// Obsidian vault directory. Implies `--obsidian`. Defaults to `.aag/obsidian`.
        #[arg(long)]
        obsidian_dir: Option<PathBuf>,

        /// Skip agent integration (MCP config, hooks, skill pack).
        #[arg(long)]
        no_install: bool,
    },

    /// Refresh the index and site in place (what the `PostToolUse` hook runs).
    Sync {
        /// Repository root to sync. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,

        /// The file that changed — sync exits instantly when that path
        /// can't affect the index (e.g. `.aag/`, `target/`).
        #[arg(long)]
        file: Option<PathBuf>,

        /// Skip regenerating the site artifacts, only refresh the graph.
        #[arg(long)]
        no_viz: bool,
    },

    /// Register aag with detected agents: MCP config, hooks, skill pack.
    Install {
        /// Repository root. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Rewrite skills/rules even if the user edited them.
        #[arg(long)]
        force: bool,
    },

    /// List every workspace this machine has indexed (each repo keeps its
    /// own local graph; query one with `--path`).
    Workspaces,

    /// Manage named hierarchical groups of indexed repositories.
    Group {
        /// Group operation.
        #[command(subcommand)]
        command: GroupCommand,
    },

    /// Open the aag UI: a local server (127.0.0.1) browsing every indexed
    /// workspace as one app. Launches your browser automatically.
    #[command(alias = "hub")]
    Ui {
        /// Port to bind. 0 (default) asks the OS for a free port.
        #[arg(long, default_value_t = 0)]
        port: u16,

        /// Don't launch the browser, just print the URL.
        #[arg(long)]
        no_open: bool,
    },

    /// Remove everything `aag install` wrote (hooks, skills, MCP entries).
    Uninstall {
        /// Repository root. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: PathBuf,
    },

    /// Agent hook entry points — called by the agent harness with a JSON
    /// payload on stdin, never by hand. Always exits 0.
    Hook {
        /// Which hook event fired.
        #[command(subcommand)]
        event: HookEvent,
    },

    /// Answer a question about the codebase: symbols, call paths, blast radius.
    #[command(alias = "query", alias = "explain", alias = "context")]
    Explore {
        /// Symbol name or search term.
        query: String,

        /// Repository root to query. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },

    /// Show what would break if a symbol changed.
    Impact {
        /// Symbol to analyze.
        symbol: String,

        /// Repository root to query. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },

    /// Show statement-level control and data flow for one file: basic blocks,
    /// CFG edges, def-use chains, and what guards each block.
    Flow {
        /// File to analyze.
        file: PathBuf,
        /// Only this function.
        #[arg(long, default_value = "")]
        function: String,
    },

    /// Show the program dependence graph for a file: which lines depend on
    /// which, by control or by data.
    Pdg {
        /// File to analyze.
        file: PathBuf,
        /// Only what this line depends on, transitively.
        #[arg(long)]
        line: Option<u32>,
    },

    /// Show source-to-sink flows in a file, following values across calls.
    /// Syntactic — each finding is a place to look, not a proven vulnerability.
    Taint {
        /// File to analyze.
        file: PathBuf,
        /// How many call hops to follow out of the file. 0 stays inside it.
        #[arg(long, default_value_t = 2)]
        depth: u32,
    },

    /// Run a read-only pattern query over the graph, in a documented subset of
    /// Cypher. See `docs/query.md` for the grammar the parser accepts.
    Cypher {
        /// The query, e.g. `MATCH (f:Function)-[:CALLS*1..3]->(g) RETURN f.name, g.name`.
        query: String,

        /// Repository root to query. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,

        /// Print JSON rows instead of a table.
        #[arg(long)]
        json: bool,
    },

    /// Route, RPC, and tool intelligence: what this repository serves, what
    /// serves it, and who consumes it.
    Api {
        /// Which view to print.
        #[command(subcommand)]
        command: ApiView,
    },

    /// Measure the engine on a repository (Track E of the evaluation
    /// contract) and append an immutable run record.
    Bench {
        /// Repository to measure. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        repo: PathBuf,

        /// Evidence class: `empirical`, `pilot`, or `simulated`. A run
        /// against this engine's own repository is recorded as `pilot`
        /// whatever is asked for.
        #[arg(long, default_value = "empirical")]
        run_kind: String,

        /// How many times each measurement repeats.
        #[arg(long, default_value_t = 3)]
        repetitions: usize,

        /// Where run records are appended.
        #[arg(long, default_value = "bench")]
        out: PathBuf,

        /// Print the recorded runs of this class as a table instead of
        /// measuring anything.
        #[arg(long)]
        report: bool,

        /// Skip the export measurement — the site for a very large repository
        /// can be hundreds of megabytes.
        #[arg(long)]
        skip_export: bool,
    },

    /// Compare two graph states: the workspace, a branch, a commit, or a
    /// pull request's head. Each ref is indexed once into `.aag/refs/`,
    /// through a detached worktree that never touches your checkout.
    GraphDiff {
        /// Earlier state: `workspace`, a git ref, or `pr/<number>`.
        #[arg(default_value = "HEAD")]
        before: String,

        /// Later state: `workspace`, a git ref, or `pr/<number>`.
        #[arg(default_value = "workspace")]
        after: String,

        /// Repository root to query. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },

    /// The repository's areas, as detected from the graph — the same
    /// clustering the generated area skills are built from.
    Areas {
        /// Repository root to query. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },

    /// Pull requests, ranked by what the graph says they reach.
    Pr {
        /// Which pull-request view.
        #[command(subcommand)]
        command: PrView,
    },

    /// Live database catalogs: ingest one, and compare it with the DDL this
    /// repository declares.
    Db {
        /// Which database operation.
        #[command(subcommand)]
        command: DbCommand,
    },

    /// Outcome-backed work memory: record what was asked and answered, how it
    /// turned out, and review the lessons that repeat.
    Memory {
        /// Which memory operation.
        #[command(subcommand)]
        command: MemoryCommand,
    },

    /// Show detected architectural communities.
    Communities {
        /// Optional symbol-name filter.
        #[arg(default_value = "")]
        query: String,
        /// Repository root to query.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },

    /// Show detected entrypoints and execution processes.
    Processes {
        /// Optional entrypoint-name filter.
        #[arg(default_value = "")]
        query: String,
        /// Repository root to query.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },

    /// Show index status and graph counts for a repository.
    Status {
        /// Repository root to inspect.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },

    /// Generate local semantic embeddings for hybrid graph search.
    Embeddings {
        /// Repository root whose graph will be embedded.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },

    /// Run the MCP server over stdio or Streamable HTTP.
    Mcp {
        /// Repository root to query. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,

        /// Transport: `stdio` or `http`.
        #[arg(long, default_value = "stdio")]
        transport: String,

        /// HTTP port (0 asks the OS for a free port).
        #[arg(long, default_value_t = 0)]
        port: u16,

        /// Optional bearer token required by the HTTP transport. Required when
        /// `--bind` is not loopback.
        #[arg(long, env = "AAG_MCP_API_KEY", hide_env_values = true)]
        api_key: Option<String>,

        /// Address the HTTP transport binds. Anything but loopback needs
        /// `--api-key`.
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,

        /// Serve every HTTP request on its own, with no session tracking — for
        /// running behind a load balancer that will not pin a client.
        #[arg(long)]
        stateless: bool,

        /// Largest HTTP request body accepted, in bytes.
        #[arg(long, default_value_t = 1_048_576)]
        max_body: usize,

        /// HTTP requests one client may make per minute.
        #[arg(long, default_value_t = 600)]
        rate_limit: u32,
    },

    /// Record the host agent's vision-pass description of a doc/image, and
    /// link it to any symbol it mentions by name.
    Describe {
        /// Doc path, relative to the repository root (e.g. `docs/arch.png`).
        doc: String,

        /// What the doc shows/says, as seen by the calling agent.
        description: String,

        /// Repository root to query. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },

    /// Coordinated multi-file rename. Previews by default; writes with `--write`.
    Rename {
        /// Current (unique) symbol name.
        old_name: String,

        /// New name.
        new_name: String,

        /// Apply the rename and reindex. Without this, only previews.
        #[arg(long)]
        write: bool,

        /// Repository root to query. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },

    /// List test-looking files transitively affected by a set of changed
    /// files (e.g. `git diff --name-only | aag affected --stdin`).
    Affected {
        /// Read changed file paths (one per line) from stdin.
        #[arg(long)]
        stdin: bool,

        /// Repository root to query. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },

    /// Compile the indexed graph into an AAG Protocol Context Manifest.
    Export {
        /// Repository root to export. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,

        /// Output path. Defaults to `.aag/context.yaml` under the repository.
        #[arg(long)]
        output: Option<PathBuf>,
    },

    /// Validate an AAG Protocol Context Manifest structurally and semantically.
    Validate {
        /// YAML or JSON manifest to validate.
        manifest: PathBuf,
    },
}

/// Operations for persistent repository groups.
#[derive(Debug, Subcommand)]
pub enum GroupCommand {
    /// Create a group (`platform/backend` also establishes hierarchy by name).
    Create {
        /// Slash-separated group name.
        name: String,
    },
    /// Add a registered workspace by unique name or absolute path.
    Add {
        /// Group name.
        name: String,
        /// Registered workspace name or absolute path.
        repository: String,
    },
    /// Remove a workspace from a group without deleting its graph.
    Remove {
        /// Group name.
        name: String,
        /// Registered workspace name or absolute path.
        repository: String,
    },
    /// List groups, or members of one group including child groups.
    List {
        /// Optional group; omitted lists group definitions.
        name: Option<String>,
    },
    /// Cross-repository protocol links: API producer to client, package export
    /// to import, event producer to consumer, schema to model, tool definition
    /// to invocation. Graphs stay separate.
    Links {
        /// Group name, or `all` for every registered workspace.
        #[arg(default_value = "all")]
        name: String,
    },
    /// Query one group and all of its descendants.
    Query {
        /// Group name.
        name: String,
        /// Search question.
        query: String,
    },
    /// Show index/manifest status for a group.
    Status {
        /// Group name.
        name: String,
    },
    /// Collect API/database/infrastructure contracts for a group.
    Contracts {
        /// Group name.
        name: String,
    },
    /// Synchronize every repository in a group.
    Sync {
        /// Group name.
        name: String,
    },
}

/// `aag hook` events, mirroring `crate::hook::Event`.
#[derive(Debug, Subcommand)]
pub enum HookEvent {
    /// `PreToolUse` on Edit|Write — inject a blast-radius warning.
    PreEdit {
        /// Repository root. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },

    /// `PostToolUse` on Write|Edit — kick off a background `aag sync`.
    PostEdit {
        /// Repository root. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },

    /// `SessionStart` — reconcile the index and inject a graph digest.
    SessionStart {
        /// Repository root. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
}

/// Work-memory operations.
#[derive(Subcommand, Debug, Clone)]
pub enum MemoryCommand {
    /// Record a question, its answer, and the symbols it rested on.
    Save {
        /// What was asked.
        #[arg(long)]
        question: String,

        /// What was answered.
        #[arg(long)]
        answer: String,

        /// Comma-separated symbols the answer rested on.
        #[arg(long, default_value = "")]
        nodes: String,

        /// How it turned out: `worked`, `wrong`, or `open`.
        #[arg(long, default_value = "open")]
        outcome: String,

        /// What replaced a wrong answer.
        #[arg(long)]
        correction: Option<String>,

        /// The commit the work landed in.
        #[arg(long)]
        revision: Option<String>,

        /// Repository root. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },

    /// Record how an earlier answer turned out.
    Correct {
        /// Entry id, as printed by `save` and `recall`.
        id: i64,

        /// `worked`, `wrong`, or `open`.
        #[arg(long, default_value = "wrong")]
        outcome: String,

        /// What replaced it.
        #[arg(long)]
        correction: Option<String>,

        /// Repository root. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },

    /// Recall entries relevant to a question, checked against the current graph.
    Recall {
        /// The question to match.
        question: String,

        /// Repository root. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },

    /// Review the lessons that repeated outcomes suggest.
    Lessons {
        /// Repository root. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
}

/// Which pull-request view to print.
#[derive(Subcommand, Debug, Clone)]
pub enum PrView {
    /// Every open pull request, highest risk first, with the rules that
    /// produced each score and the overlaps between them.
    Dashboard {
        /// Only pull requests against this base branch.
        #[arg(long, default_value = "")]
        base: String,

        /// Repository root to query. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },

    /// Open pull requests that share a file or a symbol.
    Conflicts {
        /// Only pull requests against this base branch.
        #[arg(long, default_value = "")]
        base: String,

        /// Repository root to query. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },

    /// Local git worktrees, mapped to the pull request on each branch.
    Worktrees {
        /// Only pull requests against this base branch.
        #[arg(long, default_value = "")]
        base: String,

        /// Repository root to query. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },

    /// One pull request's blast radius, risk score, and reasons, as JSON.
    Impact {
        /// Pull request number.
        number: String,

        /// Repository root to query. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
}

/// Live database catalog operations.
#[derive(Subcommand, Debug, Clone)]
pub enum DbCommand {
    /// Read a live `PostgreSQL` catalog into the graph: schemas, tables, views,
    /// columns, constraints, indexes, and foreign keys.
    ///
    /// The connection string is used to connect and then dropped. Nodes are
    /// filed under `postgres/<database>/<schema>`, which carries no host, user,
    /// or password.
    Scan {
        /// `PostgreSQL` connection string. Defaults to `AAG_DATABASE_URL`, then
        /// `DATABASE_URL`.
        #[arg(long, default_value = "", env = "AAG_DATABASE_URL")]
        url: String,

        /// Repository root whose graph receives the catalog.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },

    /// Compare the tables this repository's DDL declares with the ones an
    /// ingested catalog actually has.
    Drift {
        /// Repository root to query. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
}

/// Which slice of the API surface to print.
#[derive(Subcommand, Debug, Clone)]
pub enum ApiView {
    /// Every HTTP endpoint, declared and observed, with handler and consumers.
    Routes {
        /// Only endpoints whose name contains this.
        #[arg(default_value = "")]
        filter: String,

        /// Repository root to query. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },

    /// Every RPC/MCP tool this repository exposes, with its handler.
    Tools {
        /// Only tools whose name contains this.
        #[arg(default_value = "")]
        filter: String,

        /// Repository root to query. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },

    /// Emit an `OpenAPI` 3.1 document for the routes this repository serves.
    Spec {
        /// Only endpoints whose name contains this.
        #[arg(default_value = "")]
        filter: String,

        /// Repository root to query. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,

        /// Also include endpoints a contract declares but no code serves.
        #[arg(long)]
        include_declared: bool,

        /// Write to this file instead of standard output.
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Compare declared response shapes with what the handlers return.
    Shapes {
        /// Only endpoints whose name contains this.
        #[arg(default_value = "")]
        filter: String,

        /// Repository root to query. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },

    /// Who is on the other side of one endpoint, tool, or path.
    Impact {
        /// Endpoint name (`GET /pets`), tool name (`TOOL explore`), or path.
        target: String,

        /// Repository root to query. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
}
