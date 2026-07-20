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

        /// Optional bearer token required by the HTTP transport.
        #[arg(long, env = "AAG_MCP_API_KEY", hide_env_values = true)]
        api_key: Option<String>,
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
