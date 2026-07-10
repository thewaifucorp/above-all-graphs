---
wiki: src/install.rs
---

## The install-and-forget layer

`aag install` (called automatically by every `aag bigbang`) detects every coding agent on the machine — config dir in the repo or the home directory — and registers everything each one needs. Nobody configures `aag` by hand; that is the point.

Per agent:

- Claude Code: MCP in `.mcp.json`, three hooks in `.claude/settings.json`, seven skills in `.claude/skills/`
- Cursor: MCP in `.cursor/mcp.json`, an `afterFileEdit` hook in `.cursor/hooks.json`, rules in `.cursor/rules/aag.mdc`
- Gemini CLI: MCP in `.gemini/settings.json`, fenced section in `GEMINI.md`
- Kiro: MCP in `.kiro/settings/mcp.json`, steering file `.kiro/steering/aag.md`
- opencode: `opencode.json` (its own `mcp` shape), fenced section in `AGENTS.md`
- Codex: fenced TOML block in the global `~/.codex/config.toml`, fenced section in `AGENTS.md`
- Antigravity: fenced section in `AGENTS.md` (its MCP config is UI-managed)

## The skill pack

Seven skills, embedded in the binary via `include_str!` from `assets/skills/`, written only if missing so user edits survive:

- `aag-guide` — meta reference: what tools exist, which surface fits which question
- `aag-exploring` — how does X work, architecture, execution flows
- `aag-debugging` — trace a failure from symptom to cause through the call chain
- `aag-impact` — blast radius before changing anything widely used
- `aag-refactoring` — rename/extract/move with the graph checking completeness
- `aag-pr-review` — diff risk: missed call sites, affected tests
- `aag-wiki` — write docs that land in this wiki

## House rules

- Idempotent: re-running never duplicates hook entries or MCP servers
- Additive: existing user config survives byte-for-byte; unparseable files are skipped, never clobbered
- Reversible: `aag uninstall` removes exactly what install wrote
- Hermetic tests: `run_with_home` takes an explicit home so tests never touch the machine's real agent configs
