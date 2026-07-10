---
wiki: src/workspaces.rs
---

## Multi-repo without a unified graph

GitNexus solves multi-repo with a unified enterprise graph server-side. `aag` deliberately does not copy that: every repo keeps its own local `.aag/` graph — zero coupling, zero server — and a lightweight global registry at `~/.config/aag/workspaces.json` (respecting `XDG_CONFIG_HOME`) records every workspace the machine has indexed.

- Registration is a side effect of every `bigbang`/`sync` — maintenance-free
- `aag workspaces` lists name, stats, and path, pruning entries whose `.aag/` vanished
- Any command reaches a specific workspace with `--path`
- The registry is disposable state: corrupt file rebuilds itself, and a registry error never fails an index pass

## The UI

`aag ui` starts a local server (127.0.0.1 only, `crate::hub`) and opens the browser. One bar of lib-level chrome — workspace picker, a `+ index` button, stats — over the selected repo's own site; each embedded page keeps its workspace-level navigation. Routes:

- `GET /` — the shell (the app's only page)
- `GET /api/workspaces` — the live registry as JSON
- `POST /api/index` — index a new repo from the browser (`{"path": "/abs/path"}`); fresh repos get a full `bigbang`, already-indexed ones a `sync` refresh
- `GET /w/<root>/<page>` — a registered workspace's generated site

The registry is read fresh on every request, so the picker always reflects the latest `bigbang`/`sync` — nothing to regenerate. The static per-repo `.aag/index.html` remains as the no-server fallback; the UI is the front door.
