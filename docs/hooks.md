---
wiki: src/hook.rs
---

## The three default hooks

`aag install` registers these in Claude Code's `.claude/settings.json`; Cursor gets the post-edit one via `afterFileEdit`. All read a JSON payload on stdin and always exit 0 — a graph hiccup must never block an edit.

- `aag hook pre-edit` (PreToolUse on Edit|Write): if the file about to be edited contains symbols with 5 or more callers, injects a one-line warning as additionalContext, naming the top three. The agent learns the risk before the edit lands, and the warning points it at `aag impact`.
- `aag hook post-edit` (PostToolUse on Write|Edit): spawns a detached `aag sync --file <path>` and returns immediately. Irrelevant paths (`.aag/`, `target/`, agent config dirs) short-circuit before even spawning.
- `aag hook session-start` (SessionStart): reconciles the index (absorbs edits made while nothing was running), then injects a digest — file/symbol/edge counts plus the five most-connected symbols, each qualified by file.

## Hooks and skills are separate channels

A hook is pushed context the harness injects automatically. A skill is pulled guidance the model invokes when a request matches its trigger. They meet in the middle: the pre-edit warning tells the agent to run `aag impact`, which is exactly what the `aag-impact` skill teaches.

## Payload compatibility

Claude Code sends the edited file as `tool_input.file_path`; Cursor sends a top-level `file_path`. `edited_file` accepts both. Malformed or missing payloads degrade to no-op, never to an error.
