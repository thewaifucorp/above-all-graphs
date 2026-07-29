//! `aag install` / `aag uninstall` — the install-and-forget layer, per
//! `SPEC.md` section 8: detect every agent the user runs and register
//! everything each one needs to use the graph without any manual step.
//! Agent-agnostic by construction — the MCP server is the common surface,
//! and each agent additionally gets whatever native integration it
//! supports (hooks, skills, rules, steering, context files). `aag bigbang`
//! calls this automatically.
//!
//! Coverage (detection = config dir in the repo or the user's home):
//!
//! | Agent       | MCP config                  | Hooks                       | Guidance                    |
//! |-------------|-----------------------------|-----------------------------|-----------------------------|
//! | Claude Code | `.mcp.json`                 | `.claude/settings.json`     | `.claude/skills/aag-*`      |
//! | Cursor      | `.cursor/mcp.json`          | `.cursor/hooks.json`        | `.cursor/rules/aag.mdc`     |
//! | Gemini CLI  | `.gemini/settings.json`     | —                           | `GEMINI.md` (fenced)        |
//! | Kiro        | `.kiro/settings/mcp.json`   | —                           | `.kiro/steering/aag.md`     |
//! | opencode    | `opencode.json`             | —                           | `AGENTS.md` (fenced)        |
//! | Codex       | `~/.codex/config.toml`      | —                           | `.agents/skills/aag-*` + `AGENTS.md` |
//! | Antigravity | (UI-managed)                | —                           | `AGENTS.md` (fenced)        |
//! | VS Code / Copilot | `.vscode/mcp.json`    | —                           | `.github/copilot-instructions.md` (fenced) |
//! | Windsurf    | `~/.codeium/windsurf/mcp_config.json` | —                 | `.windsurf/rules/aag.md`    |
//! | Zed         | `.zed/settings.json`        | —                           | `AGENTS.md` (fenced)        |
//! | Roo Code    | `.roo/mcp.json`             | —                           | `.roo/rules/aag.md`         |
//! | Cline       | (VS Code globalStorage)     | —                           | `.clinerules/aag.md`        |
//! | Crush       | `.crush.json`               | —                           | `AGENTS.md` (fenced)        |
//! | goose       | `~/.config/goose/config.yaml` | —                         | `.goosehints` (fenced)      |
//!
//! Agents without a hook system still stay fresh: the MCP server itself
//! reconciles on connect and runs the native watcher (`SPEC.md` section 2).
//!
//! House rules:
//! - **Idempotent**: re-running never duplicates entries and never
//!   overwrites a file the user edited (unless `force`).
//! - **Additive**: existing config (other hooks, servers, rules) is
//!   preserved; unparseable files are skipped, never clobbered.
//! - **Reversible**: `uninstall` removes exactly what `install` wrote.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::error::{Error, Result};

/// The skill pack, embedded at compile time (`SPEC.md` section 8: no
/// download, no network). Installed into each detected agent's native skill
/// directory (`.claude/skills` or Codex's `.agents/skills`).
const SKILLS: &[(&str, &str)] = &[
    ("aag-guide", include_str!("../assets/skills/aag-guide.md")),
    (
        "aag-exploring",
        include_str!("../assets/skills/aag-exploring.md"),
    ),
    (
        "aag-debugging",
        include_str!("../assets/skills/aag-debugging.md"),
    ),
    ("aag-impact", include_str!("../assets/skills/aag-impact.md")),
    (
        "aag-refactoring",
        include_str!("../assets/skills/aag-refactoring.md"),
    ),
    (
        "aag-pr-review",
        include_str!("../assets/skills/aag-pr-review.md"),
    ),
    ("aag-wiki", include_str!("../assets/skills/aag-wiki.md")),
];

/// Claude Code hook registrations for `.claude/settings.json`: (event,
/// matcher, command). Empty matcher = match everything (`SessionStart`
/// has no tools).
const HOOKS: &[(&str, &str, &str)] = &[
    ("PreToolUse", "Edit|Write", "aag hook pre-edit"),
    ("PostToolUse", "Write|Edit", "aag hook post-edit"),
    ("SessionStart", "", "aag hook session-start"),
];

/// Markers fencing the sections `install` appends to `AGENTS.md` /
/// `GEMINI.md`, so `uninstall` (and idempotence checks) can find them.
const FENCE_START: &str = "<!-- aag:start -->";
const FENCE_END: &str = "<!-- aag:end -->";

/// Markers fencing the TOML block appended to `~/.codex/config.toml`.
const TOML_FENCE_START: &str = "# aag:start";
const TOML_FENCE_END: &str = "# aag:end";

/// Markers fencing the entries `install` appends to `.gitignore`.
const GITIGNORE_FENCE_START: &str = "# aag:ignore:start";
const GITIGNORE_FENCE_END: &str = "# aag:ignore:end";

/// Every path this installer writes in full, and therefore every path a
/// repository should not be asked to version.
///
/// The rule is the installer's own output: if `install` creates the file, git
/// should not see it. Two deliberate exceptions stay out of this list.
/// `.mcp.json` is versioned on purpose — teams share the server definition. And
/// the markdown context files (`AGENTS.md`, `GEMINI.md`,
/// `.github/copilot-instructions.md`) only receive a fenced section inside a
/// document the repository usually authors itself; ignoring the whole file
/// would hide the author's own writing, and ignoring an already-tracked file
/// does nothing anyway.
const IGNORED_ARTIFACTS: [&str; 16] = [
    ".aag/",
    ".aag.lock",
    ".claude/settings.json",
    ".claude/skills/aag-*/",
    ".agents/skills/aag-*/",
    ".cursor/mcp.json",
    ".cursor/hooks.json",
    ".cursor/rules/aag.mdc",
    ".gemini/settings.json",
    ".kiro/settings/mcp.json",
    ".kiro/steering/aag.md",
    ".roo/mcp.json",
    ".roo/rules/aag.md",
    ".vscode/mcp.json",
    ".zed/settings.json",
    "opencode.json",
];

/// The fenced block as it should read, for comparison as well as for writing.
fn gitignore_block() -> String {
    format!(
        "{GITIGNORE_FENCE_START}\n# Local AAG graph and agent-integration artifacts. Keep .mcp.json versioned.\n{}\n{GITIGNORE_FENCE_END}\n",
        IGNORED_ARTIFACTS.join("\n")
    )
}

/// What one `install` run did — used for logging and tests.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct InstallSummary {
    /// Skills newly written (skipped ones not counted).
    pub skills_written: u32,
    /// Hook entries newly added (Claude Code settings + Cursor hooks.json).
    pub hooks_added: u32,
    /// MCP server entries newly added across all agent configs.
    pub mcp_configs: u32,
    /// Rules/steering/context files or sections newly written.
    pub rules_written: u32,
    /// Agents that were detected and configured this run.
    pub agents: Vec<&'static str>,
}

/// Detects installed agents and registers `aag` with each, rooted at
/// `root`. `force` rewrites skills/rules even when the user edited them.
///
/// # Errors
///
/// Returns a write/create error if a config file cannot be written.
pub fn run(root: &Path, force: bool) -> Result<InstallSummary> {
    run_with_home(root, force, home_dir().as_deref())
}

/// [`run`] with an explicit home directory — the testable core. `home:
/// None` disables home-based detection and home-file writes entirely.
///
/// # Errors
///
/// Returns a write/create error if a config file cannot be written.
pub fn run_with_home(root: &Path, force: bool, home: Option<&Path>) -> Result<InstallSummary> {
    let mut summary = InstallSummary::default();

    // The graph and agent integration are local machine state. Keep those
    // artifacts out of a project's commits, but deliberately leave the
    // root `.mcp.json` visible: teams often version its server definition.
    upsert_gitignore(&root.join(".gitignore"))?;

    let hooked = register_hooked_agents(root, home, force, &mut summary)?;
    let config_only = register_config_only_agents(root, home, force, &mut summary)?;

    // AGENTS.md is written only when something reads it.
    summary.rules_written += u32::from(upsert_fenced_md(
        &root.join("AGENTS.md"),
        hooked || config_only,
    )?);

    if !summary.agents.is_empty() {
        tracing::info!(
            agents = ?summary.agents,
            skills = summary.skills_written,
            hooks = summary.hooks_added,
            mcp = summary.mcp_configs,
            rules = summary.rules_written,
            "agent integration installed"
        );
    }
    Ok(summary)
}

/// The two agents with a native hook system: edits they make re-index the
/// graph without waiting for the watcher. Returns whether any of them reads
/// `AGENTS.md`.
fn register_hooked_agents(
    root: &Path,
    home: Option<&Path>,
    force: bool,
    summary: &mut InstallSummary,
) -> Result<bool> {
    if detected(root, home, ".claude") {
        summary.agents.push("claude-code");
        summary.skills_written += install_skills_at(&root.join(".claude").join("skills"), force)?;
        summary.hooks_added += register_claude_hooks(root)?;
        summary.mcp_configs += u32::from(register_mcp(&root.join(".mcp.json"))?);
    }

    if detected(root, home, ".cursor") {
        summary.agents.push("cursor");
        summary.mcp_configs += u32::from(register_mcp(&root.join(".cursor").join("mcp.json"))?);
        summary.hooks_added += u32::from(register_cursor_hooks(root)?);
        summary.rules_written += u32::from(write_if_missing(
            &root.join(".cursor").join("rules").join("aag.mdc"),
            &cursor_rules(),
            force,
        )?);
    }

    Ok(false)
}

/// Everything else: MCP registration plus whatever guidance file the agent
/// reads. Returns whether any of them reads `AGENTS.md`.
fn register_config_only_agents(
    root: &Path,
    home: Option<&Path>,
    force: bool,
    summary: &mut InstallSummary,
) -> Result<bool> {
    let mut wants_agents_md = false;
    if detected(root, home, ".gemini") {
        summary.agents.push("gemini-cli");
        summary.mcp_configs +=
            u32::from(register_mcp(&root.join(".gemini").join("settings.json"))?);
        summary.rules_written += u32::from(upsert_fenced_md(&root.join("GEMINI.md"), true)?);
    }

    if detected(root, home, ".kiro") {
        summary.agents.push("kiro");
        summary.mcp_configs += u32::from(register_mcp(
            &root.join(".kiro").join("settings").join("mcp.json"),
        )?);
        summary.rules_written += u32::from(write_if_missing(
            &root.join(".kiro").join("steering").join("aag.md"),
            &format!("# aag — code knowledge graph\n\n{}", agent_blurb()),
            force,
        )?);
    }

    if detected_opencode(root, home) {
        summary.agents.push("opencode");
        summary.mcp_configs += u32::from(register_opencode(&root.join("opencode.json"))?);
        wants_agents_md = true;
    }

    if let Some(home) = home
        && home.join(".codex").is_dir()
    {
        summary.agents.push("codex");
        summary.mcp_configs += u32::from(register_codex(&home.join(".codex").join("config.toml"))?);
        summary.skills_written += install_skills_at(&root.join(".agents").join("skills"), force)?;
        wants_agents_md = true;
    }

    if detected(root, home, ".antigravity") {
        // Antigravity manages MCP servers through its UI (no stable config
        // file to write) — the fenced AGENTS.md section carries the CLI
        // surface instead.
        summary.agents.push("antigravity");
        wants_agents_md = true;
    }
    wants_agents_md |= register_editor_agents(root, home, force, summary)?;
    Ok(wants_agents_md)
}

/// The editors: VS Code and the forks and IDE agents that followed it. Each
/// keys its servers differently — `servers`, `context_servers`, `mcp` — so
/// the shape is passed in rather than assumed. Returns whether any of them
/// reads `AGENTS.md`.
fn register_editor_agents(
    root: &Path,
    home: Option<&Path>,
    force: bool,
    summary: &mut InstallSummary,
) -> Result<bool> {
    let mut wants_agents_md = false;
    // VS Code's own MCP surface (GitHub Copilot agent mode) keys its servers
    // under `servers`, not `mcpServers` — a distinct shape, same idea.
    if detected(root, home, ".vscode") {
        summary.agents.push("vscode-copilot");
        summary.mcp_configs += u32::from(register_keyed(
            &root.join(".vscode").join("mcp.json"),
            "servers",
            &json!({"type": "stdio", "command": "aag", "args": ["mcp"]}),
        )?);
        summary.rules_written += u32::from(upsert_fenced_md(
            &root.join(".github").join("copilot-instructions.md"),
            true,
        )?);
    }

    // Windsurf reads one global MCP file; its per-repo surface is rules.
    if detected(root, home, ".windsurf") || home.is_some_and(|home| home.join(".codeium").is_dir())
    {
        summary.agents.push("windsurf");
        if let Some(home) = home {
            summary.mcp_configs += u32::from(register_mcp(
                &home
                    .join(".codeium")
                    .join("windsurf")
                    .join("mcp_config.json"),
            )?);
        }
        summary.rules_written += u32::from(write_if_missing(
            &root.join(".windsurf").join("rules").join("aag.md"),
            &format!("# aag — code knowledge graph\n\n{}", agent_blurb()),
            force,
        )?);
    }

    // Zed calls them context servers and runs them over stdio.
    if detected(root, home, ".zed")
        || home.is_some_and(|home| home.join(".config").join("zed").is_dir())
    {
        summary.agents.push("zed");
        summary.mcp_configs += u32::from(register_keyed(
            &root.join(".zed").join("settings.json"),
            "context_servers",
            &json!({"source": "custom", "command": "aag", "args": ["mcp"]}),
        )?);
        wants_agents_md = true;
    }

    if detected(root, home, ".roo") {
        summary.agents.push("roo-code");
        summary.mcp_configs += u32::from(register_mcp(&root.join(".roo").join("mcp.json"))?);
        summary.rules_written += u32::from(write_if_missing(
            &root.join(".roo").join("rules").join("aag.md"),
            &format!("# aag — code knowledge graph\n\n{}", agent_blurb()),
            force,
        )?);
    }

    // Cline keeps its MCP list in VS Code's extension globalStorage, whose
    // path differs per platform, per VS Code flavour, and per fork. Writing
    // there would be a guess, so only the rules directory is written and the
    // MCP server is left to Cline's own UI.
    if detected(root, home, ".clinerules") {
        summary.agents.push("cline");
        summary.rules_written += u32::from(write_if_missing(
            &root.join(".clinerules").join("aag.md"),
            &format!("# aag — code knowledge graph\n\n{}", agent_blurb()),
            force,
        )?);
    }

    if root.join(".crush.json").is_file()
        || root.join("crush.json").is_file()
        || home.is_some_and(|home| home.join(".config").join("crush").is_dir())
    {
        summary.agents.push("crush");
        summary.mcp_configs += u32::from(register_keyed(
            &root.join(".crush.json"),
            "mcp",
            &json!({"type": "stdio", "command": "aag", "args": ["mcp"]}),
        )?);
        wants_agents_md = true;
    }

    if root.join(".goosehints").is_file()
        || home.is_some_and(|home| home.join(".config").join("goose").is_dir())
    {
        summary.agents.push("goose");
        if let Some(home) = home {
            summary.mcp_configs += u32::from(register_goose(
                &home.join(".config").join("goose").join("config.yaml"),
            )?);
        }
        summary.rules_written += u32::from(upsert_fenced_md(&root.join(".goosehints"), true)?);
    }

    Ok(wants_agents_md)
}

/// Removes everything `run` writes across every agent. Files that end up
/// empty are left in place (still-valid JSON), never deleted wholesale —
/// they may predate `aag`.
///
/// # Errors
///
/// Returns a write error if a config file cannot be rewritten.
pub fn uninstall(root: &Path) -> Result<()> {
    uninstall_with_home(root, home_dir().as_deref())
}

/// [`uninstall`] with an explicit home directory — the testable core.
///
/// # Errors
///
/// Returns a write error if a config file cannot be rewritten.
pub fn uninstall_with_home(root: &Path, home: Option<&Path>) -> Result<()> {
    for (name, _) in SKILLS {
        for skill_root in [
            root.join(".claude").join("skills"),
            root.join(".agents").join("skills"),
        ] {
            let dir = skill_root.join(name);
            if dir.is_dir() {
                fs::remove_dir_all(&dir).map_err(|source| Error::RemoveDir {
                    path: dir.clone(),
                    source,
                })?;
            }
        }
    }

    // Generated area skills are ours too — they exist only because `aag`
    // wrote them, so `uninstall` takes them with the rest.
    for skill_root in [
        root.join(".claude").join("skills"),
        root.join(".agents").join("skills"),
    ] {
        remove_generated_area_skills(&skill_root)?;
    }

    unregister_claude_hooks(&root.join(".claude").join("settings.json"))?;
    unregister_cursor_hooks(&root.join(".cursor").join("hooks.json"))?;

    unregister_mcp(&root.join(".mcp.json"))?;
    unregister_mcp(&root.join(".cursor").join("mcp.json"))?;
    unregister_mcp(&root.join(".gemini").join("settings.json"))?;
    unregister_mcp(&root.join(".kiro").join("settings").join("mcp.json"))?;
    unregister_opencode(&root.join("opencode.json"))?;
    unregister_mcp(&root.join(".roo").join("mcp.json"))?;
    unregister_keyed(&root.join(".vscode").join("mcp.json"), "servers")?;
    unregister_keyed(&root.join(".zed").join("settings.json"), "context_servers")?;
    unregister_keyed(&root.join(".crush.json"), "mcp")?;
    if let Some(home) = home {
        unregister_codex(&home.join(".codex").join("config.toml"))?;
        unregister_mcp(
            &home
                .join(".codeium")
                .join("windsurf")
                .join("mcp_config.json"),
        )?;
        unregister_goose(&home.join(".config").join("goose").join("config.yaml"))?;
    }

    remove_file_if_present(&root.join(".cursor").join("rules").join("aag.mdc"))?;
    remove_file_if_present(&root.join(".kiro").join("steering").join("aag.md"))?;
    remove_file_if_present(&root.join(".windsurf").join("rules").join("aag.md"))?;
    remove_file_if_present(&root.join(".roo").join("rules").join("aag.md"))?;
    remove_file_if_present(&root.join(".clinerules").join("aag.md"))?;
    remove_fenced_md(&root.join(".github").join("copilot-instructions.md"))?;
    remove_fenced_md(&root.join(".goosehints"))?;
    remove_fenced_md(&root.join("AGENTS.md"))?;
    remove_fenced_md(&root.join("GEMINI.md"))?;
    remove_gitignore_block(&root.join(".gitignore"))?;

    tracing::info!("uninstalled aag agent integration");
    Ok(())
}

/// Whether agent config dir `name` exists in the repo or the user's home —
/// SPEC section 8: "Detecção: presença de dir/arquivo de config do agente
/// no repo ou no home".
fn detected(root: &Path, home: Option<&Path>, name: &str) -> bool {
    root.join(name).is_dir() || home.is_some_and(|home| home.join(name).is_dir())
}

/// opencode keeps its project config at `opencode.json` and its global
/// state under `~/.config/opencode`.
fn detected_opencode(root: &Path, home: Option<&Path>) -> bool {
    root.join("opencode.json").is_file()
        || home.is_some_and(|home| home.join(".config").join("opencode").is_dir())
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Writes each embedded skill under `skill_root`, skipping any that already
/// exist (the user may have edited them) unless `force`.
fn install_skills_at(skill_root: &Path, force: bool) -> Result<u32> {
    let mut written = 0;
    for (name, content) in SKILLS {
        let path = skill_root.join(name).join("SKILL.md");
        written += u32::from(write_if_missing(&path, content, force)?);
    }
    Ok(written)
}

/// Merges the three `aag hook` entries into `.claude/settings.json`,
/// preserving everything already there. An entry is "ours" iff its command
/// matches exactly — re-running never duplicates.
fn register_claude_hooks(root: &Path) -> Result<u32> {
    let path = root.join(".claude").join("settings.json");
    let Some(mut settings) = read_json_object(&path)? else {
        tracing::warn!(path = %path.display(), "unparseable settings.json — skipping hook registration");
        return Ok(0);
    };

    let hooks = settings
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| malformed(&path, "`hooks` is not an object"))?;

    let mut added = 0;
    for (event, matcher, command) in HOOKS {
        let entries = hooks
            .entry(*event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| malformed(&path, "hook event is not an array"))?;
        if entries.iter().any(|entry| has_command(entry, command)) {
            continue;
        }
        let mut entry = Map::new();
        if !matcher.is_empty() {
            entry.insert("matcher".into(), json!(matcher));
        }
        entry.insert(
            "hooks".into(),
            json!([{"type": "command", "command": command}]),
        );
        entries.push(Value::Object(entry));
        added += 1;
    }

    write_json(&path, &Value::Object(settings))?;
    Ok(added)
}

/// Drops every hook entry whose commands all start with `aag hook` from
/// `settings.json`, pruning empty arrays and the `hooks` object itself.
fn unregister_claude_hooks(path: &Path) -> Result<()> {
    let Some(mut settings) = read_json_object(path)? else {
        return Ok(());
    };
    let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(());
    };

    for (_, entries) in hooks.iter_mut() {
        if let Some(entries) = entries.as_array_mut() {
            entries.retain(|entry| !is_ours(entry));
        }
    }
    hooks.retain(|_, entries| entries.as_array().is_none_or(|list| !list.is_empty()));
    if hooks.is_empty() {
        settings.remove("hooks");
    }

    write_json(path, &Value::Object(settings))
}

/// Merges an `afterFileEdit` entry into `.cursor/hooks.json` (Cursor's
/// hook system) so edits made in Cursor trigger the same background sync.
fn register_cursor_hooks(root: &Path) -> Result<bool> {
    let path = root.join(".cursor").join("hooks.json");
    let Some(mut config) = read_json_object(&path)? else {
        tracing::warn!(path = %path.display(), "unparseable hooks.json — skipping");
        return Ok(false);
    };
    config.entry("version").or_insert_with(|| json!(1));
    let hooks = config
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| malformed(&path, "`hooks` is not an object"))?;
    let entries = hooks
        .entry("afterFileEdit")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| malformed(&path, "`afterFileEdit` is not an array"))?;
    let command = "aag hook post-edit";
    if entries
        .iter()
        .any(|entry| entry.get("command").and_then(Value::as_str) == Some(command))
    {
        return Ok(false);
    }
    entries.push(json!({"command": command}));
    write_json(&path, &Value::Object(config))?;
    Ok(true)
}

/// Drops our `afterFileEdit` entry from `.cursor/hooks.json`.
fn unregister_cursor_hooks(path: &Path) -> Result<()> {
    let Some(mut config) = read_json_object(path)? else {
        return Ok(());
    };
    let Some(hooks) = config.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(());
    };
    for (_, entries) in hooks.iter_mut() {
        if let Some(entries) = entries.as_array_mut() {
            entries.retain(|entry| {
                entry
                    .get("command")
                    .and_then(Value::as_str)
                    .is_none_or(|cmd| !cmd.starts_with("aag hook"))
            });
        }
    }
    hooks.retain(|_, entries| entries.as_array().is_none_or(|list| !list.is_empty()));
    write_json(path, &Value::Object(config))
}

/// Whether a hook entry contains a command hook with exactly `command`.
fn has_command(entry: &Value, command: &str) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| {
            hooks
                .iter()
                .any(|hook| hook.get("command").and_then(Value::as_str) == Some(command))
        })
}

/// Whether a hook entry is one `register_claude_hooks` wrote: every
/// command in it starts with `aag hook`.
fn is_ours(entry: &Value) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| {
            !hooks.is_empty()
                && hooks.iter().all(|hook| {
                    hook.get("command")
                        .and_then(Value::as_str)
                        .is_some_and(|cmd| cmd.starts_with("aag hook"))
                })
        })
}

/// Adds the `aag` server to a `mcpServers`-shaped config file (Claude
/// Code, Cursor, Gemini CLI, and Kiro all share this shape), preserving
/// other servers and unrelated settings. Returns whether the entry was
/// added (false = already present or file unparseable).
fn register_mcp(path: &Path) -> Result<bool> {
    let Some(mut config) = read_json_object(path)? else {
        tracing::warn!(path = %path.display(), "unparseable MCP config — skipping");
        return Ok(false);
    };
    let servers = config
        .entry("mcpServers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| malformed(path, "`mcpServers` is not an object"))?;
    if servers.contains_key("aag") {
        return Ok(false);
    }
    servers.insert("aag".into(), json!({"command": "aag", "args": ["mcp"]}));
    write_json(path, &Value::Object(config))?;
    Ok(true)
}

/// Adds the `aag` server to a JSON config that keys its servers under
/// `container` — VS Code uses `servers`, Zed uses `context_servers`, Crush
/// uses `mcp`, each with its own entry shape. Same additive, idempotent,
/// skip-what-we-cannot-parse rules as [`register_mcp`].
fn register_keyed(path: &Path, container: &str, entry: &Value) -> Result<bool> {
    let Some(mut config) = read_json_object(path)? else {
        tracing::warn!(path = %path.display(), container, "unparseable config — skipping");
        return Ok(false);
    };
    let servers = config
        .entry(container.to_string())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| malformed(path, "server container is not an object"))?;
    if servers.contains_key("aag") {
        return Ok(false);
    }
    servers.insert("aag".into(), entry.clone());
    write_json(path, &Value::Object(config))?;
    Ok(true)
}

/// Removes the `aag` entry from a `container`-keyed JSON config.
fn unregister_keyed(path: &Path, container: &str) -> Result<()> {
    let Some(mut config) = read_json_object(path)? else {
        return Ok(());
    };
    let Some(servers) = config.get_mut(container).and_then(Value::as_object_mut) else {
        return Ok(());
    };
    if servers.remove("aag").is_none() {
        return Ok(());
    }
    write_json(path, &Value::Object(config))
}

/// Adds the `aag` stdio extension to goose's YAML config, preserving the
/// provider settings and other extensions around it.
fn register_goose(path: &Path) -> Result<bool> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(Error::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let mut config = serde_yaml_ng::Mapping::new();
    if !raw.trim().is_empty() {
        let Ok(serde_yaml_ng::Value::Mapping(parsed)) = serde_yaml_ng::from_str(&raw) else {
            tracing::warn!(path = %path.display(), "unparseable goose config — skipping");
            return Ok(false);
        };
        config = parsed;
    }
    let key = serde_yaml_ng::Value::String("extensions".into());
    let extensions = config
        .entry(key)
        .or_insert_with(|| serde_yaml_ng::Value::Mapping(serde_yaml_ng::Mapping::new()));
    let Some(extensions) = extensions.as_mapping_mut() else {
        return Err(malformed(path, "`extensions` is not a mapping"));
    };
    let name = serde_yaml_ng::Value::String("aag".into());
    if extensions.contains_key(&name) {
        return Ok(false);
    }
    let mut entry = serde_yaml_ng::Mapping::new();
    entry.insert("name".into(), "aag".into());
    entry.insert("type".into(), "stdio".into());
    entry.insert("cmd".into(), "aag".into());
    entry.insert(
        "args".into(),
        serde_yaml_ng::Value::Sequence(vec!["mcp".into()]),
    );
    entry.insert("enabled".into(), true.into());
    extensions.insert(name, serde_yaml_ng::Value::Mapping(entry));
    let rendered = serde_yaml_ng::to_string(&serde_yaml_ng::Value::Mapping(config))
        .map_err(|_| malformed(path, "goose config could not be serialized"))?;
    write_text(path, &rendered)?;
    Ok(true)
}

/// Removes the `aag` extension from goose's YAML config.
fn unregister_goose(path: &Path) -> Result<()> {
    let Ok(raw) = fs::read_to_string(path) else {
        return Ok(());
    };
    let Ok(serde_yaml_ng::Value::Mapping(mut config)) = serde_yaml_ng::from_str(&raw) else {
        return Ok(());
    };
    let Some(extensions) = config
        .get_mut(serde_yaml_ng::Value::String("extensions".into()))
        .and_then(serde_yaml_ng::Value::as_mapping_mut)
    else {
        return Ok(());
    };
    if extensions
        .remove(serde_yaml_ng::Value::String("aag".into()))
        .is_none()
    {
        return Ok(());
    }
    let rendered = serde_yaml_ng::to_string(&serde_yaml_ng::Value::Mapping(config))
        .map_err(|_| malformed(path, "goose config could not be serialized"))?;
    write_text(path, &rendered)
}

/// Removes the `aag` server entry, leaving everything else intact.
fn unregister_mcp(path: &Path) -> Result<()> {
    let Some(mut config) = read_json_object(path)? else {
        return Ok(());
    };
    let Some(servers) = config.get_mut("mcpServers").and_then(Value::as_object_mut) else {
        return Ok(());
    };
    if servers.remove("aag").is_none() {
        return Ok(());
    }
    write_json(path, &Value::Object(config))
}

/// Adds the `aag` server to `opencode.json` (opencode's own shape: an
/// `mcp` object with `type: local` entries).
fn register_opencode(path: &Path) -> Result<bool> {
    let Some(mut config) = read_json_object(path)? else {
        tracing::warn!(path = %path.display(), "unparseable opencode.json — skipping");
        return Ok(false);
    };
    if config.is_empty() {
        config.insert("$schema".into(), json!("https://opencode.ai/config.json"));
    }
    let servers = config
        .entry("mcp")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| malformed(path, "`mcp` is not an object"))?;
    if servers.contains_key("aag") {
        return Ok(false);
    }
    servers.insert(
        "aag".into(),
        json!({"type": "local", "command": ["aag", "mcp"], "enabled": true}),
    );
    write_json(path, &Value::Object(config))?;
    Ok(true)
}

/// Removes the `aag` entry from `opencode.json`.
fn unregister_opencode(path: &Path) -> Result<()> {
    let Some(mut config) = read_json_object(path)? else {
        return Ok(());
    };
    let Some(servers) = config.get_mut("mcp").and_then(Value::as_object_mut) else {
        return Ok(());
    };
    if servers.remove("aag").is_none() {
        return Ok(());
    }
    write_json(path, &Value::Object(config))
}

/// Appends a fenced `[mcp_servers.aag]` block to Codex's global
/// `~/.codex/config.toml` (Codex has no per-project MCP config). The fence
/// comments make removal exact without a TOML parser.
fn register_codex(path: &Path) -> Result<bool> {
    let existing = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(Error::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if existing.contains("[mcp_servers.aag]") {
        return Ok(false);
    }
    let block = format!(
        "\n{TOML_FENCE_START}\n[mcp_servers.aag]\ncommand = \"aag\"\nargs = [\"mcp\"]\n{TOML_FENCE_END}\n"
    );
    write_text(path, &(existing + &block))?;
    Ok(true)
}

/// Removes the fenced block written by `register_codex`, if present.
fn unregister_codex(path: &Path) -> Result<()> {
    let Ok(existing) = fs::read_to_string(path) else {
        return Ok(());
    };
    let Some(cleaned) = strip_fenced(&existing, TOML_FENCE_START, TOML_FENCE_END) else {
        return Ok(());
    };
    write_text(path, &cleaned)
}

/// Ensures the AAG-generated local artifacts are ignored without hiding the
/// root `.mcp.json`, which is intentionally suitable for version control.
fn upsert_gitignore(path: &Path) -> Result<bool> {
    let existing = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(Error::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let block = gitignore_block();
    // An existing block is brought up to date rather than left alone. The list
    // grows as `install` learns new agents, and a repository that installed
    // once would otherwise be asked forever to version the artifacts added
    // since — which is how `.vscode/mcp.json` and `.zed/settings.json` ended up
    // in front of a reader as untracked files.
    if let Some(cleaned) = strip_fenced(&existing, GITIGNORE_FENCE_START, GITIGNORE_FENCE_END) {
        let wanted = format!("{}\n{block}", cleaned.trim_end());
        if existing == wanted {
            return Ok(false);
        }
        write_text(path, &wanted)?;
        return Ok(true);
    }
    let separator = if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    write_text(path, &format!("{existing}{separator}\n{block}"))?;
    Ok(true)
}

/// Removes only the fenced `.gitignore` block written by [`upsert_gitignore`].
fn remove_gitignore_block(path: &Path) -> Result<()> {
    let Ok(existing) = fs::read_to_string(path) else {
        return Ok(());
    };
    let Some(cleaned) = strip_fenced(&existing, GITIGNORE_FENCE_START, GITIGNORE_FENCE_END) else {
        return Ok(());
    };
    if cleaned.trim().is_empty() {
        return remove_file_if_present(path);
    }
    write_text(path, &cleaned)
}

/// Ensures `path` (a markdown context file: `AGENTS.md` / `GEMINI.md`)
/// carries the fenced aag section. Appends when the file exists; creates
/// the file only when `create` (an agent that reads it was detected).
/// Returns whether anything was written.
fn upsert_fenced_md(path: &Path, create: bool) -> Result<bool> {
    let existing = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if !create {
                return Ok(false);
            }
            String::new()
        }
        Err(source) => {
            return Err(Error::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if existing.contains(FENCE_START) {
        return Ok(false);
    }
    let section = format!(
        "\n{FENCE_START}\n## aag — code knowledge graph\n\n{}{FENCE_END}\n",
        agent_blurb()
    );
    write_text(path, &(existing + &section))?;
    Ok(true)
}

/// Drops the fenced section written by `upsert_fenced_md`, if present.
fn remove_fenced_md(path: &Path) -> Result<()> {
    let Ok(existing) = fs::read_to_string(path) else {
        return Ok(());
    };
    let Some(cleaned) = strip_fenced(&existing, FENCE_START, FENCE_END) else {
        return Ok(());
    };
    if cleaned.trim().is_empty() {
        // The file existed only to carry our section — remove it entirely.
        return remove_file_if_present(path);
    }
    write_text(path, &cleaned)
}

/// `text` minus the `start`..`end` fenced block (inclusive), normalizing
/// the surrounding blank lines. `None` when no complete fence is present.
fn strip_fenced(text: &str, start: &str, end: &str) -> Option<String> {
    let start_at = text.find(start)?;
    let end_at = text.find(end)?;
    if end_at < start_at {
        return None;
    }
    let mut cleaned = String::new();
    cleaned.push_str(text[..start_at].trim_end_matches('\n'));
    let tail = &text[end_at + end.len()..];
    if !cleaned.is_empty() && !tail.trim().is_empty() {
        cleaned.push('\n');
    }
    cleaned.push_str(tail.trim_start_matches('\n'));
    if !cleaned.is_empty() && !cleaned.ends_with('\n') {
        cleaned.push('\n');
    }
    Some(cleaned)
}

/// The Cursor rules file (`.mdc` frontmatter + shared blurb).
fn cursor_rules() -> String {
    format!(
        "---\ndescription: Query the aag code knowledge graph before exploring or refactoring\nalwaysApply: true\n---\n\n{}",
        agent_blurb()
    )
}

/// The shared "how to use aag" blurb for rules/steering/context files.
fn agent_blurb() -> &'static str {
    "This repo has an `aag` knowledge graph (`.aag/graph.db`), kept fresh automatically.\n\
     \n\
     - How does X work / what calls X: `aag explore <query>`\n\
     - What breaks if X changes: `aag impact <symbol>`\n\
     - Safe multi-file rename: `aag rename <old> <new> [--write]`\n\
     - Tests affected by a diff: `git diff --name-only | aag affected --stdin`\n\
     \n\
     Prefer these over manual grepping for call-graph questions; edges are\n\
     confidence-tagged (EXTRACTED/INFERRED/AMBIGUOUS) — verify AMBIGUOUS ones.\n"
}

/// Writes `content` to `path` unless it already exists (unless `force`),
/// creating parent dirs. Returns whether it wrote.
fn write_if_missing(path: &Path, content: &str, force: bool) -> Result<bool> {
    if path.exists() && !force {
        return Ok(false);
    }
    write_text(path, content)?;
    Ok(true)
}

/// Reads `path` as a JSON object. `Ok(None)` = exists but is not valid
/// JSON or not an object (caller should skip rather than clobber a file it
/// doesn't understand). Missing file = empty object.
fn read_json_object(path: &Path) -> Result<Option<Map<String, Value>>> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Some(Map::new())),
        Err(source) => {
            return Err(Error::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    match serde_json::from_str::<Value>(&raw) {
        Ok(Value::Object(map)) => Ok(Some(map)),
        _ => Ok(None),
    }
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    let pretty = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    write_text(path, &(pretty + "\n"))
}

/// [`write_text`] for `crate::areas`, which writes generated skills into the
/// same directories with the same parent-creating semantics.
pub(crate) fn write_text_public(path: &Path, content: &str) -> Result<()> {
    write_text(path, content)
}

fn write_text(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| Error::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(path, content).map_err(|source| Error::Write {
        path: path.to_path_buf(),
        source,
    })
}

/// Removes every `aag-area-*` directory under `skill_root`.
fn remove_generated_area_skills(skill_root: &Path) -> Result<()> {
    let Ok(entries) = fs::read_dir(skill_root) else {
        return Ok(());
    };
    let mut stale: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(crate::areas::AREA_PREFIX))
        })
        .collect();
    stale.sort();
    for path in stale {
        fs::remove_dir_all(&path).map_err(|source| Error::RemoveDir {
            path: path.clone(),
            source,
        })?;
    }
    Ok(())
}

fn remove_file_if_present(path: &Path) -> Result<()> {
    if path.is_file() {
        fs::remove_file(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

fn malformed(path: &Path, what: &str) -> Error {
    Error::Write {
        path: path.to_path_buf(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, what.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Scratch repo + scratch home, so detection and home-file writes are
    /// fully hermetic — tests never see or touch the machine's real home.
    fn scratch() -> (PathBuf, PathBuf) {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base =
            std::env::temp_dir().join(format!("aag-install-test-{}-{n}", std::process::id()));
        let root = base.join("repo");
        let home = base.join("home");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&home).unwrap();
        (root, home)
    }

    fn install(root: &Path, home: &Path, force: bool) -> InstallSummary {
        run_with_home(root, force, Some(home)).unwrap()
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn no_agents_detected_writes_nothing() {
        let (root, home) = scratch();
        let summary = install(&root, &home, false);
        assert_eq!(summary, InstallSummary::default());
        assert!(!root.join(".mcp.json").exists());
        assert!(!root.join("AGENTS.md").exists());
        assert!(root.join(".gitignore").is_file());
    }

    #[test]
    fn every_generated_artifact_is_ignored() {
        // The list and the writers have to stay in step: an agent added to
        // `install` without an entry here leaves its config in front of the
        // reader as an untracked file.
        let (root, home) = scratch();
        install(&root, &home, false);
        let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap();
        for artifact in IGNORED_ARTIFACTS {
            assert!(
                gitignore.lines().any(|line| line == artifact),
                "{artifact} is written by install but not ignored"
            );
        }
    }

    #[test]
    fn an_older_block_is_brought_up_to_date() {
        // What a repository that installed an earlier version has on disk: the
        // fence, with fewer entries inside it.
        let (root, home) = scratch();
        fs::write(
            root.join(".gitignore"),
            format!("dist/\n\n{GITIGNORE_FENCE_START}\n.aag/\n{GITIGNORE_FENCE_END}\n"),
        )
        .unwrap();

        install(&root, &home, false);

        let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(
            gitignore.contains("dist/"),
            "user entries survive: {gitignore}"
        );
        assert!(gitignore.contains(".vscode/mcp.json"), "{gitignore}");
        assert!(gitignore.contains(".zed/settings.json"), "{gitignore}");
        assert_eq!(gitignore.matches(GITIGNORE_FENCE_START).count(), 1);
    }

    #[test]
    fn a_context_document_is_never_ignored_wholesale() {
        // `install` only appends a fenced section to these, so ignoring the file
        // would hide whatever the repository wrote in it.
        for document in ["AGENTS.md", "GEMINI.md", ".github/copilot-instructions.md"] {
            assert!(
                !IGNORED_ARTIFACTS.contains(&document),
                "{document} carries repository-authored content"
            );
        }
    }

    #[test]
    fn install_ignores_local_aag_artifacts_but_not_root_mcp_config() {
        let (root, home) = scratch();
        fs::write(root.join(".gitignore"), "dist/\n").unwrap();

        install(&root, &home, false);
        install(&root, &home, false);

        let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(gitignore.contains("dist/"));
        assert!(gitignore.contains(".aag/"));
        assert!(gitignore.contains(".aag.lock"));
        assert!(gitignore.contains(".agents/skills/aag-*/"));
        assert!(gitignore.contains(".cursor/hooks.json"));
        assert!(!gitignore.lines().any(|line| line == ".mcp.json"));
        assert_eq!(gitignore.matches(GITIGNORE_FENCE_START).count(), 1);

        uninstall_with_home(&root, Some(&home)).unwrap();
        assert_eq!(
            fs::read_to_string(root.join(".gitignore")).unwrap(),
            "dist/\n"
        );
    }

    #[test]
    fn claude_gets_skills_hooks_and_mcp() {
        let (root, home) = scratch();
        fs::create_dir_all(root.join(".claude")).unwrap();

        let summary = install(&root, &home, false);

        assert_eq!(summary.agents, vec!["claude-code"]);
        assert_eq!(summary.skills_written, u32::try_from(SKILLS.len()).unwrap());
        assert_eq!(summary.hooks_added, u32::try_from(HOOKS.len()).unwrap());
        assert_eq!(summary.mcp_configs, 1);
        for (name, _) in SKILLS {
            assert!(
                root.join(".claude")
                    .join("skills")
                    .join(name)
                    .join("SKILL.md")
                    .is_file(),
                "missing skill {name}"
            );
        }
        let settings = read_json(&root.join(".claude").join("settings.json"));
        assert!(settings["hooks"]["PreToolUse"].is_array());
        assert!(settings["hooks"]["SessionStart"].is_array());
    }

    #[test]
    fn detection_works_from_home_too() {
        let (root, home) = scratch();
        fs::create_dir_all(home.join(".claude")).unwrap();

        let summary = install(&root, &home, false);
        assert_eq!(summary.agents, vec!["claude-code"]);
        assert!(root.join(".mcp.json").is_file());
    }

    #[test]
    fn install_is_idempotent_across_all_agents() {
        let (root, home) = scratch();
        for dir in [
            ".claude",
            ".cursor",
            ".gemini",
            ".kiro",
            ".antigravity",
            ".vscode",
            ".windsurf",
            ".zed",
            ".roo",
            ".clinerules",
        ] {
            fs::create_dir_all(home.join(dir)).unwrap();
        }
        fs::create_dir_all(home.join(".codex")).unwrap();
        for dir in ["opencode", "crush", "goose"] {
            fs::create_dir_all(home.join(".config").join(dir)).unwrap();
        }

        let first = install(&root, &home, false);
        assert_eq!(first.agents.len(), 14, "agents: {:?}", first.agents);

        let second = install(&root, &home, false);
        assert_eq!(second.skills_written, 0);
        assert_eq!(second.hooks_added, 0);
        assert_eq!(second.mcp_configs, 0);
        assert_eq!(second.rules_written, 0);

        let settings = read_json(&root.join(".claude").join("settings.json"));
        assert_eq!(
            settings["hooks"]["PostToolUse"].as_array().unwrap().len(),
            1
        );
        let cursor_hooks = read_json(&root.join(".cursor").join("hooks.json"));
        assert_eq!(
            cursor_hooks["hooks"]["afterFileEdit"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let codex = fs::read_to_string(home.join(".codex").join("config.toml")).unwrap();
        assert_eq!(codex.matches("[mcp_servers.aag]").count(), 1);
        // Each newer agent gets its own shape, not a copy of `mcpServers`.
        let vscode = read_json(&root.join(".vscode").join("mcp.json"));
        assert_eq!(vscode["servers"]["aag"]["command"], "aag");
        assert_eq!(vscode["servers"]["aag"]["type"], "stdio");
        let zed = read_json(&root.join(".zed").join("settings.json"));
        assert_eq!(zed["context_servers"]["aag"]["source"], "custom");
        let crush = read_json(&root.join(".crush.json"));
        assert_eq!(crush["mcp"]["aag"]["type"], "stdio");
        let roo = read_json(&root.join(".roo").join("mcp.json"));
        assert_eq!(roo["mcpServers"]["aag"]["command"], "aag");
        let windsurf = read_json(
            &home
                .join(".codeium")
                .join("windsurf")
                .join("mcp_config.json"),
        );
        assert_eq!(windsurf["mcpServers"]["aag"]["args"][0], "mcp");
        let goose =
            fs::read_to_string(home.join(".config").join("goose").join("config.yaml")).unwrap();
        assert_eq!(goose.matches("cmd: aag").count(), 1, "{goose}");
        for rules in [
            root.join(".windsurf").join("rules").join("aag.md"),
            root.join(".roo").join("rules").join("aag.md"),
            root.join(".clinerules").join("aag.md"),
        ] {
            assert!(rules.is_file(), "missing rules file {}", rules.display());
        }
        assert!(
            fs::read_to_string(root.join(".github").join("copilot-instructions.md"))
                .unwrap()
                .contains("aag explore")
        );
        assert!(
            fs::read_to_string(root.join(".goosehints"))
                .unwrap()
                .contains("aag impact")
        );
        for (name, _) in SKILLS {
            assert!(
                root.join(".agents")
                    .join("skills")
                    .join(name)
                    .join("SKILL.md")
                    .is_file(),
                "missing Codex skill {name}"
            );
        }
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md"))
                .unwrap()
                .matches(FENCE_START)
                .count(),
            1
        );
    }

    #[test]
    fn install_preserves_existing_config() {
        let (root, home) = scratch();
        fs::create_dir_all(root.join(".claude")).unwrap();
        fs::create_dir_all(root.join(".gemini")).unwrap();
        fs::write(
            root.join(".claude").join("settings.json"),
            r#"{"hooks":{"PostToolUse":[{"matcher":"Write|Edit","hooks":[{"type":"command","command":"./mine.sh"}]}]},"other":true}"#,
        )
        .unwrap();
        fs::write(
            root.join(".gemini").join("settings.json"),
            r#"{"theme":"dark","mcpServers":{"other":{"command":"other"}}}"#,
        )
        .unwrap();

        install(&root, &home, false);

        let settings = read_json(&root.join(".claude").join("settings.json"));
        assert_eq!(settings["other"], json!(true));
        assert_eq!(
            settings["hooks"]["PostToolUse"].as_array().unwrap().len(),
            2,
            "user hook must survive"
        );
        let gemini = read_json(&root.join(".gemini").join("settings.json"));
        assert_eq!(gemini["theme"], json!("dark"));
        assert!(gemini["mcpServers"]["other"].is_object());
        assert!(gemini["mcpServers"]["aag"].is_object());
    }

    #[test]
    fn user_edited_skill_survives_reinstall_unless_forced() {
        let (root, home) = scratch();
        fs::create_dir_all(root.join(".claude")).unwrap();
        install(&root, &home, false);

        let skill = root
            .join(".claude")
            .join("skills")
            .join("aag-guide")
            .join("SKILL.md");
        fs::write(&skill, "user edit").unwrap();

        install(&root, &home, false);
        assert_eq!(fs::read_to_string(&skill).unwrap(), "user edit");

        install(&root, &home, true);
        assert_ne!(fs::read_to_string(&skill).unwrap(), "user edit");
    }

    #[test]
    fn unparseable_settings_is_left_alone() {
        let (root, home) = scratch();
        fs::create_dir_all(root.join(".claude")).unwrap();
        let path = root.join(".claude").join("settings.json");
        fs::write(&path, "{ not json").unwrap();

        let summary = install(&root, &home, false);
        assert_eq!(summary.hooks_added, 0);
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json");
    }

    #[test]
    fn opencode_gets_its_own_config_shape() {
        let (root, home) = scratch();
        fs::create_dir_all(home.join(".config").join("opencode")).unwrap();

        install(&root, &home, false);

        let config = read_json(&root.join("opencode.json"));
        assert_eq!(config["mcp"]["aag"]["type"], json!("local"));
        assert_eq!(config["mcp"]["aag"]["command"], json!(["aag", "mcp"]));
        // Created from scratch — must carry the schema pointer.
        assert!(config["$schema"].is_string());
        // Codex not detected — AGENTS.md still created for opencode.
        assert!(root.join("AGENTS.md").is_file());
    }

    #[test]
    fn codex_toml_appends_and_removes_fenced_block() {
        let (root, home) = scratch();
        fs::create_dir_all(home.join(".codex")).unwrap();
        let toml = home.join(".codex").join("config.toml");
        fs::write(&toml, "model = \"gpt-5\"\n").unwrap();

        install(&root, &home, false);
        let content = fs::read_to_string(&toml).unwrap();
        assert!(content.starts_with("model = \"gpt-5\""));
        assert!(content.contains("[mcp_servers.aag]"));
        assert_eq!(
            fs::read_to_string(
                root.join(".agents")
                    .join("skills")
                    .join("aag-exploring")
                    .join("SKILL.md")
            )
            .unwrap()
            .matches("name: aag-exploring")
            .count(),
            1
        );

        uninstall_with_home(&root, Some(&home)).unwrap();
        let cleaned = fs::read_to_string(&toml).unwrap();
        assert!(!cleaned.contains("aag"), "cleaned was: {cleaned}");
        assert!(cleaned.contains("model = \"gpt-5\""));
        assert!(
            !root
                .join(".agents")
                .join("skills")
                .join("aag-exploring")
                .exists()
        );
    }

    #[test]
    fn kiro_gets_mcp_and_steering() {
        let (root, home) = scratch();
        fs::create_dir_all(root.join(".kiro")).unwrap();

        install(&root, &home, false);

        let mcp = read_json(&root.join(".kiro").join("settings").join("mcp.json"));
        assert!(mcp["mcpServers"]["aag"].is_object());
        assert!(root.join(".kiro").join("steering").join("aag.md").is_file());
    }

    #[test]
    fn gemini_md_created_and_fenced_once() {
        let (root, home) = scratch();
        fs::create_dir_all(home.join(".gemini")).unwrap();

        install(&root, &home, false);
        install(&root, &home, false);

        let content = fs::read_to_string(root.join("GEMINI.md")).unwrap();
        assert_eq!(content.matches(FENCE_START).count(), 1);
    }

    #[test]
    fn uninstall_removes_ours_keeps_theirs() {
        let (root, home) = scratch();
        for dir in [".claude", ".cursor", ".gemini", ".kiro"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::write(
            root.join(".claude").join("settings.json"),
            r#"{"hooks":{"PostToolUse":[{"hooks":[{"type":"command","command":"./mine.sh"}]}]}}"#,
        )
        .unwrap();
        fs::write(root.join("AGENTS.md"), "# My agents file\n").unwrap();
        install(&root, &home, false);

        uninstall_with_home(&root, Some(&home)).unwrap();

        for (name, _) in SKILLS {
            assert!(!root.join(".claude").join("skills").join(name).exists());
            assert!(!root.join(".agents").join("skills").join(name).exists());
        }
        let settings = read_json(&root.join(".claude").join("settings.json"));
        assert_eq!(
            settings["hooks"]["PostToolUse"].as_array().unwrap().len(),
            1
        );
        assert!(settings["hooks"].get("PreToolUse").is_none());
        for mcp_path in [
            root.join(".mcp.json"),
            root.join(".cursor").join("mcp.json"),
            root.join(".gemini").join("settings.json"),
            root.join(".kiro").join("settings").join("mcp.json"),
        ] {
            let config = read_json(&mcp_path);
            assert!(
                config["mcpServers"].get("aag").is_none(),
                "aag survives in {}",
                mcp_path.display()
            );
        }
        assert!(!root.join(".cursor").join("rules").join("aag.mdc").exists());
        assert!(!root.join(".kiro").join("steering").join("aag.md").exists());
        let agents_md = fs::read_to_string(root.join("AGENTS.md")).unwrap();
        assert!(!agents_md.contains("aag"), "was: {agents_md}");
        assert!(agents_md.contains("# My agents file"));
        // GEMINI.md existed only for our section — gone entirely.
        assert!(!root.join("GEMINI.md").exists());
    }

    /// Every integration added in the second wave is reversible too, and
    /// none of them takes a neighbouring entry with it on the way out.
    #[test]
    fn uninstall_reverses_the_newer_integrations() {
        let (root, home) = scratch();
        for dir in [".vscode", ".windsurf", ".zed", ".roo", ".clinerules"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::create_dir_all(home.join(".config").join("goose")).unwrap();
        fs::write(root.join(".crush.json"), "{}").unwrap();
        // Foreign entries that must survive, in each config's own shape.
        fs::write(
            root.join(".vscode").join("mcp.json"),
            r#"{"servers":{"theirs":{"command":"other"}}}"#,
        )
        .unwrap();
        fs::write(
            root.join(".zed").join("settings.json"),
            r#"{"theme":"One Dark","context_servers":{"theirs":{"source":"custom"}}}"#,
        )
        .unwrap();
        fs::write(
            home.join(".config").join("goose").join("config.yaml"),
            "GOOSE_PROVIDER: anthropic\nextensions:\n  developer:\n    type: builtin\n",
        )
        .unwrap();
        let summary = install(&root, &home, false);
        assert!(summary.agents.contains(&"goose"), "{:?}", summary.agents);

        uninstall_with_home(&root, Some(&home)).unwrap();

        let vscode = read_json(&root.join(".vscode").join("mcp.json"));
        assert!(vscode["servers"].get("aag").is_none());
        assert_eq!(vscode["servers"]["theirs"]["command"], "other");
        let zed = read_json(&root.join(".zed").join("settings.json"));
        assert!(zed["context_servers"].get("aag").is_none());
        assert_eq!(zed["theme"], "One Dark", "unrelated settings survive");
        assert!(zed["context_servers"].get("theirs").is_some());
        let crush = read_json(&root.join(".crush.json"));
        assert!(crush["mcp"].get("aag").is_none());
        let roo = read_json(&root.join(".roo").join("mcp.json"));
        assert!(roo["mcpServers"].get("aag").is_none());
        let windsurf = read_json(
            &home
                .join(".codeium")
                .join("windsurf")
                .join("mcp_config.json"),
        );
        assert!(windsurf["mcpServers"].get("aag").is_none());
        let goose =
            fs::read_to_string(home.join(".config").join("goose").join("config.yaml")).unwrap();
        assert!(!goose.contains("cmd: aag"), "{goose}");
        assert!(
            goose.contains("developer"),
            "their extension stays: {goose}"
        );
        assert!(goose.contains("anthropic"), "their provider stays: {goose}");
        for rules in [
            root.join(".windsurf").join("rules").join("aag.md"),
            root.join(".roo").join("rules").join("aag.md"),
            root.join(".clinerules").join("aag.md"),
            root.join(".github").join("copilot-instructions.md"),
            root.join(".goosehints"),
        ] {
            assert!(!rules.exists(), "{} survives", rules.display());
        }
    }

    #[test]
    fn agents_md_not_created_when_no_reader_detected() {
        let (root, home) = scratch();
        fs::create_dir_all(root.join(".claude")).unwrap();
        install(&root, &home, false);
        assert!(!root.join("AGENTS.md").exists());
    }
}
