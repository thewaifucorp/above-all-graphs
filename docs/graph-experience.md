---
wiki: assets/graph.html.template
---

# Graph experience — baseline evidence and design contract

This page is gates 3A (research and task model) and 3B (interaction and
visual specification) of P0.3 in [capability coverage](capability-coverage.md).
It exists so the renderer is rewritten against a written contract instead of
taste, and so the claim "the graph got better" can be checked.

## 3A — how the current graph performs against the five jobs

### Measurement setup

One external repository, not used to design anything here: a TypeScript
monorepo of 1009 TS/JS files, indexed by `aag bigbang`, producing 5971 nodes
and 87 928 edges. The generated `.aag/graph.html` was served over loopback
HTTP and driven in a Chromium at 1440×900.

Recorded from the live page:

| measurement | value |
|---|---|
| `graph.html` transferred | 12.97 MB (single self-contained file) |
| `DOMContentLoaded` | 1373 ms |
| peak JS heap during load | 271.8 MB |
| heap after layout settled | 135.2 MB |
| nodes handed to the scene | 5971 of 5971 |
| edges handed to the scene | 87 928 of 87 928 |
| frame rate once layout settles | 60.7 fps, worst frame gap 17 ms |
| distinct node colours in default state | 16 |
| modules detected | 498 |
| edges visible in default state | 87 928, in one flat colour |
| labels attached in default state | 5971 |

Two of those deserve emphasis because they cut against the obvious
assumption: **rendering is not the bottleneck.** Sigma's WebGL layer holds 60
fps with the entire graph in the scene. The failure is comprehension and
payload, not frame rate.

A first measurement pass wrongly reported one node colour and zero labels.
That was an artifact: `nodeReducer` dims every non-neighbour to `#1a1a18` and
drops its label while a node is hovered, and the pointer happened to rest on
the canvas. The numbers above are from an explicitly cleared hover state.

### Job-by-job result

**Overview — "what are the principal modules?"** Fails. The default camera
sits inside the node cloud at ratio 0.36 rather than fitting the graph, so the
opening view is a dense confetti field of ~6000 same-sized dots. Colour
encodes community, but 498 modules map onto 16 palette entries — roughly 31
modules per colour — and there is no legend or community name anywhere, so the
colour cannot be decoded. Labels are attached to every node at once and
collide into an illegible layer. Nothing is collapsed or aggregated.

**Explore — "show me this symbol and what it touches."** Partly works, for the
wrong reason. Hovering a node highlights its incident edges — by hiding every
other edge in the graph. That is a usable one-hop peek and a dead end for
anything else: you cannot see two symbols' relationship at the same time,
there is no independent upstream/downstream depth control, no ranking of which
neighbours matter, no breadcrumb, and no pinning.

**Path — "how does A reach B?"** No affordance at all. The page contains no
path UI, no second endpoint selection, and no directed layout. The
`aag explore` CLI can answer it; the graph cannot.

**Impact — "what breaks if I change this?"** No affordance. The word appears
only in help text. Upstream dependents and downstream dependencies are not
separated anywhere in the view.

**Contracts — "which endpoints exist and who serves them?"** No affordance.
Endpoint and schema nodes are in the data and are drawn as ordinary circles
with no distinction between a declared contract and an observed
implementation.

### Cross-cutting failures

- No mode concept. One view answers one and a half of five jobs.
- URL state is a single `?focus=` parameter. Camera, filters, isolation,
  selection, and depth are unshareable, so no view can be sent to a colleague
  or pasted into a review.
- Edges carry no visual encoding of relation kind, direction, or confidence in
  the default state — one grey, one width.
- The file tree in the sidebar is unranked and alphabetical, so a real
  repository opens on `.github/`, `.history/`, `.husky/`, `.sisyphus/` before
  any source directory.
- No command palette and no keyboard route through the core tasks.
- Payload scales linearly with the repository: every file's full source text
  and every edge are inlined into one HTML document.

### Benchmark fixtures

Three fixtures, versioned, used for every budget below and for visual
regression:

| fixture | shape | why |
|---|---|---|
| `small` | this repository, first-party files only (~1.9k nodes) | fast feedback, dogfood |
| `medium` | the 5971-node / 87 928-edge TypeScript monorepo above | the realistic case |
| `large` | synthetic, ~25k nodes / ~400k edges | headroom, labelled synthetic |

The synthetic fixture is labelled synthetic wherever it appears. Per the
claim-discipline rules, synthetic scale never substantiates real-world task
accuracy.

## 3B — interaction and visual specification

### Mode state machine

Five modes, one at a time, all reachable from a persistent switcher. Mode is
the only thing that changes what the canvas is *for*; selection, filters, and
camera survive a mode change wherever they still mean something.

```
                 ┌──────────────┐
      ┌─────────▶│   Overview   │◀────────┐
      │          └──────┬───────┘         │
      │   expand/focus  │                 │ back / reset
      │                 ▼                 │
      │          ┌──────────────┐         │
      ├─────────▶│   Explore    │─────────┤
      │          └──┬────────┬──┘         │
      │  set target │        │ inspect    │
      │             ▼        ▼            │
      │      ┌──────────┐  ┌──────────┐   │
      ├─────▶│   Path   │  │  Impact  │───┤
      │      └──────────┘  └──────────┘   │
      │          ┌──────────────┐         │
      └─────────▶│  Contracts   │─────────┘
                 └──────────────┘
```

Transition rules:

- Entering **Explore** requires a focus node; entering from Overview uses the
  clicked node or community representative.
- Entering **Path** requires a source and a target. With only a source set,
  the mode is enterable but shows a "pick a target" state rather than an
  empty canvas.
- Entering **Impact** requires a focus node, file, or diff.
- **Overview** and **Contracts** require nothing.
- Every transition pushes onto a navigation history with working back/forward.
- Filters (node kind, relation kind, confidence) and pinned nodes are global
  and survive all transitions. Camera survives Overview↔Explore; Path and
  Impact own their own camera because their layouts differ.

### URL state schema

Every meaningful state is expressible in the query string, and the page
restores from it. Unknown keys are ignored rather than fatal.

| key | values | applies to |
|---|---|---|
| `mode` | `overview` \| `explore` \| `path` \| `impact` \| `contracts` | all |
| `focus` | node id, symbol name, or file path | explore, impact |
| `from`, `to` | node id, symbol name, or file path | path |
| `up`, `down` | integer depth, 0–6 | explore, impact |
| `rel` | comma list of `calls,imports,inherits,implements,explains,references` | all |
| `conf` | comma list of `extracted,inferred,ambiguous` | all |
| `kind` | comma list of node kinds | all |
| `community` | community id, isolates it | overview, explore |
| `expanded` | comma list of community ids | overview |
| `pin` | comma list of node ids | all |
| `cam` | `x,y,ratio` | all |

Two rules: the URL is written on state change with `replaceState` (no history
spam) and pushed only on a mode change or focus change; and a URL that names a
node no longer in the graph degrades to the nearest valid state with a visible
notice, never a blank canvas.

### Visual grammar

One channel per dimension. Nothing is encoded by colour alone.

| dimension | channel |
|---|---|
| node kind | shape (file square, doc diamond, symbol circle, endpoint hexagon, schema rounded square, infra triangle) |
| community / module | hue, plus an always-available legend with names |
| structural importance | node size (degree-derived, clamped) |
| relation kind | line style — calls solid, imports dashed, inherits/implements double, explains dotted, references thin dotted |
| direction | arrowhead, plus curvature for reciprocal pairs |
| confidence | line opacity *and* a texture cue: EXTRACTED solid, INFERRED single-dash, AMBIGUOUS sparse-dash |
| selection | ring + increased size, never hue change |
| search hit | outline pulse (respects reduced motion: static outline) |
| change state (impact) | fill pattern — changed hatched, directly affected solid, transitive light, ambiguous cross-hatched |
| provenance | declared vs observed as outline style (declared dashed outline, observed solid) |

Colour tokens continue the existing repo identity: surfaces `#0a0a0a` and
`#121212`, text `#f2f2ee`, accent `#ffc600`, alert `#c1121f`, and the spectrum
gradient as the signature line. Community hues come from the existing
desaturated palette, extended to 24 entries with a documented collision rule:
beyond 24 communities, hue repeats and the legend disambiguates by name — hue
is never claimed to be unique.

### Semantic zoom

Three bands, by camera ratio. Thresholds are settings, and the values below
are the defaults:

| band | ratio | nodes drawn | labels | edges |
|---|---|---|---|---|
| far | > 0.6 | communities and files only | community names | aggregated inter-community bundles |
| mid | 0.15–0.6 | files plus symbols above an importance floor | file names, top symbols | aggregated per file pair |
| near | < 0.15 | every node in view | every node in view | every edge in view |

Label collision is resolved greedily by importance, and a label is never drawn
over another label's box. Low-value edges are suppressed until hover,
selection, or the near band.

### Aggregation

An aggregate is always expandable to its parts, and it always carries the
provenance and confidence of what it summarizes:

- A collapsed community carries: contained symbol count, internal cohesion,
  external edge count, entrypoint count, and worst-confidence contained edge.
- An inter-community bundle carries: per relation kind counts, direction
  balance, and the confidence distribution. Expanding lists the underlying
  edges with file and line.
- No aggregate hides a node that the current filters would show at the near
  band without saying so — a visible count of hidden nodes is required.

### Keyboard and accessibility

- Every core task completable from the keyboard: `⌘K`/`Ctrl+K` command
  palette, `/` search, `1`–`5` mode switch, arrow keys to walk neighbours,
  `Enter` to focus, `Esc` to clear, `[`/`]` for history.
- Visible focus ring on every interactive element, distinct from the graph
  selection ring.
- `prefers-reduced-motion` disables camera animation, layout animation, and
  pulses; state changes become instant.
- Contrast: text and essential outlines at 4.5:1 or better against their
  surface. Node fills are decorative, so they carry a shape or outline cue too.
- The inspector is a real focusable region with headings, so a screen reader
  can read a node's kind, location, confidence, evidence, and neighbour counts
  without touching the canvas.
- Usable down to 1024 px wide: sidebar collapses to icons, inspector becomes a
  bottom sheet.

### Performance and payload budgets

Measured against the three fixtures, on the recorded reference machine, as
release gates. A regression blocks release rather than being documented
afterwards.

| budget | small | medium | large (synthetic) |
|---|---|---|---|
| document size | ≤ 3 MB | ≤ 6 MB | ≤ 12 MB |
| first interactive | ≤ 600 ms | ≤ 1200 ms | ≤ 2500 ms |
| initial nodes in scene | ≤ 400 | ≤ 800 | ≤ 1200 |
| initial edges in scene | ≤ 1500 | ≤ 3000 | ≤ 5000 |
| search response | ≤ 100 ms | ≤ 150 ms | ≤ 250 ms |
| filter response | ≤ 100 ms | ≤ 150 ms | ≤ 250 ms |
| sustained frame rate while panning | ≥ 50 fps | ≥ 50 fps | ≥ 40 fps |
| peak JS heap | ≤ 150 MB | ≤ 300 MB | ≤ 600 MB |

The document-size budget is the reason the payload changes shape: full source
text for every file cannot stay inlined unconditionally at the medium fixture
and above. Source is embedded up to a per-repository cap, and beyond it the
file viewer degrades to "open in your editor" with the path and line rather
than silently shipping 13 MB.

### Layout caching

Layout is cached in `localStorage` keyed by a fingerprint of
`(node id set, edge count, mode, layout settings)`. A cache hit restores exact
positions, so reloading or returning from another mode does not reshuffle the
user's mental map. Any change to the node set invalidates deterministically —
positions are never partially reused, because a half-stale layout is worse
than an honest relayout.

## 3C — progress

### Landed: payload and aggregates

The page payload was the binding constraint: at the medium fixture it was
12.97 MB, and the object-per-edge encoding plus unconditional source
embedding accounted for almost all of it. Both are now fixed in the exporter,
where they can be tested by `cargo test` rather than by eye.

- `graph_payload_json` encodes nodes and edges columnar with travelling
  dictionaries for node kind, file path, relation kind, and confidence. The
  page decodes it back to the same object shape it always used, so no
  rendering code changed. `graph.json` keeps the readable object form — it is
  a public export, not a page payload.
- Every community now ships with what a collapsed aggregate has to be able to
  explain: a label derived from the deepest shared directory (falling back to
  the dominant file, then the id), member list, internal and external edge
  counts, entrypoint count, worst contained confidence, and its five
  highest-degree representatives.
- Inter-community bundles ship pre-aggregated by relation kind with per
  confidence counts, so Overview can draw ~136 bundles instead of ~88 000
  edges and still expand any of them.
- Node degree ships per node, so importance ranking needs no client-side pass.
- Embedded source is capped by `SOURCE_EMBED_BUDGET_BYTES`, spent
  most-referenced-file-first. Beyond the cap the viewer states that the source
  is not embedded and shows path and line, instead of opening an empty modal.
  One oversized file is still embedded rather than shipping a viewer that
  never works.

Re-measured on the same medium fixture, same browser and viewport:

| measurement | before | after | budget |
|---|---|---|---|
| document transferred | 12.97 MB | 5.35 MB | ≤ 6 MB |
| `DOMContentLoaded` | 1373 ms | 892 ms | ≤ 1200 ms |
| peak JS heap | 271.8 MB | 152.8 MB | ≤ 300 MB |
| communities with a readable label | 0 of 197 | 197 of 197 | all |
| inter-community bundles precomputed | 0 | 136 | — |
| embedded files / omitted | 1675 / 0 | 1410 / 265 | — |

### Still open

Everything the payload was blocking. The scene still receives all 5971 nodes
and all 87 928 edges, because the modes that would consume the aggregates do
not exist yet:

- mode state machine, URL schema, and navigation history as specified above
- Overview drawing collapsed communities and bundles by default
- Explore, Path, Impact, and Contracts views
- unified inspector, command palette, legend
- semantic zoom bands, label collision, edge suppression
- layout caching by fingerprint
- keyboard and screen-reader coverage, reduced motion
- budgets enforced in CI against the three fixtures, and visual regression
  baselines

### Definition of done

Inherited verbatim from P0.3 in [capability coverage](capability-coverage.md),
plus: every number in the 3A table above is re-measured and published for the
new implementation, on the same fixtures, before the gate is called closed.
