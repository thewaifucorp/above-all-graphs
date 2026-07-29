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

### Landed: mode state, URL state, and Overview

Community detection had been happening twice — once in the exporter and again
in the page, with its own label propagation and its own renumbering. The page
therefore coloured by one numbering while the payload's aggregates and bundles
used another. It is now computed once, in the exporter, including the two
things the page used to do on its own: folding communities below
`MIN_COMMUNITY_MEMBERS` into the dominant community of their own file, and
renumbering by size so id 0 is the largest. The page consumes the result.

- **State store.** One writer for mode, focus, isolated community, expanded
  communities, relation/confidence/node-kind filters, pins, and camera. Every
  field round-trips through the query string per the schema above. A mode or
  focus change pushes history; a filter tweak replaces it, so the back button
  does not walk one checkbox at a time. `popstate` restores state, and a URL
  naming a node that no longer exists shows a notice and falls back to the
  whole graph rather than a blank canvas.
- **Mode switcher.** Overview and Explore, keyboard `1`/`2`, `Esc` back to
  Overview. Path, Impact, and Contracts are deliberately absent from the
  switcher until they work — a dead button is worse than a missing one.
- **Overview.** Opens on collapsed community aggregates and the bundles
  between them. Aggregate size encodes member count, colour encodes the
  module, and edge width encodes bundle volume. Clicking an aggregate opens an
  inspector with symbol count, internal and external edge counts, cohesion,
  entrypoint count, worst contained confidence, its most connected members,
  and what it connects to — every fact a collapsed thing owes its reader.
  Three actions: expand in place, isolate, explore. A named legend lists the
  40 largest modules and isolates on click.
- **Deferred detail.** The force layout over every symbol runs on first entry
  into Explore, not on load, so opening the page costs the Overview only.
- **Overview layout.** Deterministic seeded ring, size-aware repulsion, then a
  no-overlap pass — two aggregates drawn on top of each other read as one
  module, which is a wrong answer rather than a cosmetic one.

Measured on the medium fixture, 1440×900:

| measurement | baseline | payload slice | now | budget |
|---|---|---|---|---|
| document transferred | 12.97 MB | 5.35 MB | 5.36 MB | ≤ 6 MB |
| `DOMContentLoaded` | 1373 ms | 892 ms | 449 ms | ≤ 1200 ms |
| peak JS heap | 271.8 MB | 152.8 MB | 84.4 MB | ≤ 300 MB |
| nodes in the initial scene | 5971 | 5971 | 146 | ≤ 800 |
| edges in the initial scene | 87 928 | 87 928 | 151 | ≤ 3000 |
| sustained fps while panning | 60.7 | — | 61.0 | ≥ 50 |
| labels drawn at rest | 5971 attached, colliding | — | 54 | legible |
| console errors | 0 | 0 | 0 | 0 |

Three real defects were found and fixed by looking at the rendered page rather
than at the code: a hidden container made sigma throw "container has no width"
on every refresh after a mode switch; the "organizing graph" overlay was
dismissed only by the full-graph layout, so it covered the Overview forever
once that layout stopped running on load; and `community_label` fell back to an
arbitrary member file, naming an 867-symbol module after `language-config.ts`.

### Landed: five modes on one scene engine

The page no longer has a whole-repository view at all. Every mode builds a
*scene* — a node list, an edge list, and a layout kind — and one renderer draws
it. That is what makes the budgets reachable: no view needs 6000 nodes on
screen, so none of them asks for that.

- **Explore** takes a focus symbol and walks upstream and downstream to
  separate depths, ranking each hop by relation confidence, entrypoint and
  contract status, and degree, then caps the scene and reports what it dropped.
  Hover now *dims* instead of hiding, so the surrounding structure stays
  visible while one node is highlighted — the old behaviour hid every
  non-incident edge, which made it impossible to see two things relate.
- **Path** runs a directed BFS between two endpoints and draws the result as
  layers, left to right, because a path has an order that a force layout
  destroys. No path, same endpoint, and missing endpoint are all distinct
  stated outcomes rather than an empty canvas.
- **Impact** separates dependents from dependencies, groups by depth, ranks
  tests, entrypoints, and contracts ahead of generic transitive nodes, and
  marks change state with a glyph and a size step — the module already owns
  hue, so impact cannot borrow it.
- **Contracts** pairs each endpoint, schema, table, and infrastructure
  resource with the implementations that serve it, counts the ones with no
  implementation, and separates declared from observed using the `perspective`
  the exporter now ships per node.
- **Command palette** (`⌘K`/`Ctrl+K`) over both commands and symbols, plus `/`
  to search, `1`–`5` for modes, arrow keys to walk neighbours, and `Esc` to
  unwind one layer at a time.
- **Semantic zoom** by camera band, with a floor that stands down for small
  scenes — a nine-node impact fan-out with no labels answers nothing.
- **Explore overflow is aggregated, not dropped.** The ranking that keeps the
  top neighbours per hop is where a hub actually loses neighbours — 898 of them
  becomes 40 — so whatever it cuts becomes one expandable aggregate per module,
  labelled with its count, listing its members in the inspector, and expandable
  in place. The scene cap feeds the same aggregation.
- **Path shows every equally short route**, not the first one BFS happened to
  find: two equally short routes are two different answers, and showing one
  silently is showing half the truth. The primary route is drawn heavier so the
  alternatives are distinguishable without a legend.
- **The visual grammar gained a relation channel.** Sigma's default programs
  draw circles and solid lines, so node kind lived only in a label glyph and
  relation kind only in a hue. A 2D canvas pinned over the WebGL layers, redrawn
  after each sigma frame, adds a dash pattern per relation kind (`calls` stays
  solid; imports, inherits, implements, explains, and references each get their
  own). Measured with the overlay redrawing every frame while panning: 60.7 fps
  on a 31-node scene and 60.7 fps on a 423-node one, worst frame 17 ms in both.
  It also drew an outline shape per node kind; that half was removed later, for
  the reasons in the second pass log below.
- **Layout cache** in `localStorage`, keyed by scene shape and force settings,
  so a reload or a return from another mode lands on the same picture. A
  changed node set invalidates rather than partially reusing.
- **Accessibility**: visible focus rings, a `prefers-reduced-motion` block,
  the inspector as a labelled region, the notice as a live region, and
  keyboard activation on every list row.
- **Header** relaid out as an honest flex row. The search box had been
  absolutely centred, so it sat on top of the mode switcher as soon as there
  were five modes; low-priority chrome now sheds at narrow widths.
- **D3 is gone.** It was vendored solely for `d3.quadtree` in the retired
  whole-repository force layout — 280 KB off every generated page and out of
  the binary.

Final measurements, medium fixture, 1440×900:

| measurement | baseline | now | budget |
|---|---|---|---|
| document transferred | 12.97 MB | 5.17 MB | ≤ 6 MB |
| `DOMContentLoaded` | 1373 ms | 400 ms | ≤ 1200 ms |
| peak JS heap | 271.8 MB | 125 MB | ≤ 300 MB |
| nodes in the initial scene | 5971 | 146 | ≤ 800 |
| edges in the initial scene | 87 928 | 151 | ≤ 3000 |
| Explore scene (depth 2/2) | n/a | 9 nodes / 18 edges | ≤ 800 |
| Contracts scene | n/a | 31 nodes | ≤ 800 |
| sustained fps while panning | 60.7 | 61.0 | ≥ 50 |
| console errors across all modes | 0 | 0 | 0 |

Two budgets are now enforced by `cargo test` rather than measured by hand:
the edge table's cost per edge, and the whole generated page against the
medium ceiling using a synthetic 6000-node / 87 000-edge graph. Those run in
CI with everything else.

### Still open

- Visual regression baselines and browser-side budget enforcement. The payload
  and page-size budgets are in CI; frame rate, interaction latency, and how the
  page *looks* still need a Playwright harness this repository does not have.
- The `large` synthetic fixture is only exercised through the page-size test,
  not through the interaction budgets.
- Overview at 146 modules is still a lot of small aggregates; the fold
  threshold and an "others" bucket want revisiting, and the outermost aggregate
  can sit partly off-screen.
- Node kind is carried by a label glyph only. The shape grammar in the
  specification above needs a custom WebGL node program: an overlay ring cannot
  be made to sit on the circle, because the radius sigma draws is decided inside
  its shader and every reconstruction of it from outside drifted (see the second
  pass log below). Relation kind has both a hue and a dash.

### Landed: the second pass over what the redesign lost

Five complaints from using the page on this repository, all of them things the
first pass either dropped or never wired to a control anyone could find.

- **Module labels are unique.** `community_label` names a community after the
  directory its members share, so four communities inside `src/` all came out
  as `src/` and the legend named nothing. The largest keeps the bare directory
  — it is what someone means by "the `src` module" — and the rest are
  qualified by the file most of their symbols live in, falling back to the
  community id when a vendored bundle splits across communities that share
  both a directory and a dominant file.
- **Clicking a module bubble opens it.** Expand-in-place existed but was
  reachable only through a button in the inspector, so the aggregate read as a
  dead end. The click now toggles the expansion, and an **Expand all modules**
  control in the Overview toolbar opens every one of them — the whole graph,
  which is what the scene cap had made unreachable. Past 5000 nodes it warns
  first: rendering everything is a deliberate expense, not a surprise.
- **The sidebar is a tree again.** It had become a flat list of every file
  sorted by symbol count, which is a search box with extra steps. Files are
  grouped under their directory with per-directory totals, collapsible, with
  the focused file's directory opened automatically.
- **Source is one gesture away.** The status bar had promised "click node for
  source" while the click opened the inspector instead. Double-click opens the
  file at the symbol's lines, and the status bar says what the controls
  actually do.
- **Path endpoints are pickable.** Both fields are backed by a datalist of
  node names, take the current selection through an arrow button, and swap.
  Endpoint resolution stopped being exact-match-only: `bigbang` finds
  `src/bigbang.rs`, case no longer decides whether a path resolves, and a
  value that resolves to nothing stays in the field instead of silently
  blanking.
- **The hover label is readable.** Sigma's default hover renderer hardcodes
  a white plate and then paints the label with `labelColor` on top of it —
  near-white text on white, on a dark page. A local `hoverRenderer` draws the
  plate in the page's own surface colour with its border and text colours.
- **Long labels stopped drawing white bars.** Sigma renders labels on an
  opaque plate, so a 78-character test function name became a bar across the
  scene. Canvas labels truncate at 34 characters; the inspector and the search
  results still show the real name.

### Landed: the panel nobody could see, and the debris around small nodes

- **The inspector was invisible.** `.fpanel` hides every floating panel and each
  one shows through `.open`; the class was toggled on the inspector but no rule
  matched it, so clicking a node filled a panel with `display: none`. One CSS
  rule brings back the kind, location, perspective, degree, edge counts, the
  action row, and the incoming/outgoing lists.
- **Relation rows had no styling at all.** `.callrow`, `.cname`, and `.cfile`
  were markup without CSS, so the name and its edge metadata ran together as
  `src/bigbang.rsimports`. They are now a two-line row: name, then relation,
  confidence, and file, dimmed.
- **The wiki link looked like a stray hyperlink** wedged between buttons: it
  carried the header's `hbtn` class inside a panel that styles only `button`.
  The action row now styles its anchor like its buttons.
- **The kind outline channel is gone.** Reproducing sigma's node radius from
  outside its shader never lined up: the ring was floored at a fixed radius, so
  a node too small to see still got a full sized outline floating in open space
  and not clickable; sized from `renderer.scaleSize` it used a ratio cached at
  refresh time, so a camera-only repaint left the outlines describing the
  previous zoom and they stopped following the graph; and matching the shader's
  own `size / sqrt(ratio)` still came out about a fifth short of the circle
  sigma drew. The channel was also redundant — the label already opens with a
  kind glyph. The dash-per-relation channel stays: it is drawn from endpoint
  positions through `graphToViewport`, which is exact.
- **The overlay is cleared in device pixels** with no transform in effect, and
  is discarded together with the renderer that drew it. Clearing in CSS pixels
  under a scaled transform only covers the canvas while the transform and the
  backing size agree.

### Landed: a layout that survives repository scale, plus pin and path

- **Repulsion goes through a quadtree again.** Every node pushing every other
  node is what made the layout O(n²): 5753 nodes is 16.5 million pairs per
  round, times ~180 rounds. Distant nodes are now summed into one mass and
  pushed against once — the optimisation `d3.quadtree` used to provide before d3
  was dropped, reimplemented in ~110 lines rather than vendored, because
  `graphology-layout-forceatlas2` publishes no browser build and shimming its
  `require` graph is more code than the algorithm.

  Measured in-page on the same machine, synthetic scenes at 2 edges per node:

  | nodes | pairwise | quadtree |
  |---|---|---|
  | 420 | 153 ms | 74 ms |
  | 1 685 | 2 589 ms | 318 ms |
  | 3 000 | 7 956 ms | 628 ms |
  | 5 753 (bastion's size) | 52 850 ms | 1 367 ms |
  | 10 000 | not measured | 2 531 ms |

  "Expand all modules" at bastion's size went from freezing the tab for the best
  part of a minute to about a second and a half.

- **Overlap separation is gridded.** It was the second O(n²) pass, 40 rounds of
  it. Overlap is local, so each node is now compared against its own cell and
  the eight around it. Exactly coincident points also get a direction to be
  pushed along; without one they divided by a zero-length vector and stayed
  stacked forever.

- **Pin marks something.** The state was wired — toggled, persisted in `?pin=`,
  and honoured when Explore builds its scene — but the only visual effect was
  `result.type = "circle"`, which is sigma's default type, so pinning changed
  nothing on screen. A pinned node now carries a 📌 in its label, keeps that
  label past the semantic-zoom threshold, and is highlighted the way a selection
  is: sigma's own channels, no geometry of ours to keep in sync. Pins are marked
  in every mode, and Overview shows a pinned node individually even when its
  module is collapsed. The inspector's button also re-reads after the toggle,
  instead of still offering "Pin" for a node that is now pinned.

- **Path can ignore direction.** The walk followed `outgoing` only, so
  `A → X ← B` reported nothing at all: a real connection through X, refused
  because no directed chain runs end to end. There is now a toggle — *following
  arrows* / *any direction* (`?dir=any`) — and when a directed search comes back
  empty the page checks the undirected one before reporting failure, so "no path"
  and "no path following the arrows, but one hop if you ignore them" are
  different answers. Hops walked against an edge are marked in the caption.

### Landed: layout off the main thread, and a path search that answers

- **The layout runs in a worker.** 1.4 s at bastion's size was a tab frozen for
  1.4 s. The worker source is assembled from the layout functions themselves via
  `Function.prototype.toString`, not a second copy of the physics that would
  drift, and only what the physics reads crosses the boundary: keys, sizes, layer
  hints, endpoints. Scenes under 300 nodes stay on the main thread, where the
  round trip costs more than the work.

  A blob worker is refused from an opaque origin in some browsers, so
  construction is attempted once and any failure — construction or runtime —
  falls back to laying out on the main thread permanently. Verified over `http://`
  (615 ms for the 1685-node expand-all, against 3058 ms before), from `file://`
  (586 ms, no console errors), and with `window.Worker` sabotaged to throw
  (499 ms, no page errors, same picture).

  Renders take a ticket: an off-thread layout can land after the reader has
  already switched modes, and a stale scene overwriting the current one is worse
  than a slow one.

- **Path answers with a path.** Following the arrows is still tried first,
  because it is the better answer when it exists, but failing to find one is no
  longer a reason to show an empty canvas. The search falls back to ignoring
  direction, draws that route, and says so — in the caption, and once in the
  notice. `arrows only` (`?dir=arrows`) refuses the fallback for readers who want
  the strictly directed answer.

  Nothing is drawn only when nothing connects the two, and that message
  distinguishes the two ways it happens: an endpoint with no edges at all is
  named as such, otherwise they are in unconnected parts of the graph.

### Landed: routes of any length, not just the shortest

Breadth-first search answers "what is the shortest way" and stops at the depth
where the target first appears, which hides every longer route. Those are often
the interesting ones: the shortest way from a parser to an exporter may be one
incidental helper both happen to call, while the route that explains the codebase
is six hops through the pipeline.

The walk is Yen's algorithm now — the `k` shortest loopless routes, shortest
first. **There is no hop limit.** The bound is on how many routes come back,
because the number of simple paths between two nodes in a real graph is
astronomically large; `routes` in the Path toolbar takes 1 to 12 (`?routes=`),
default 4. The caption reports a range when the routes differ in length
(`1–2 hops · 7 routes`).

Measured in-page on this repository, k = 12, ignoring direction: 7 ms for a pair
one hop apart, 7 ms across the export subsystem, 14 ms from `bigbang` to
`write_file`, 1 ms between two files. Yen runs one breadth-first search per spur
node per accepted route, so the cost scales with `k` and with route length, not
with how far apart the endpoints are.

`write_default` → `write_file` used to draw a single edge. It now draws the
direct call plus the six two-hop routes through `write_index`, `write_wiki_html`,
`write_cypher`, `write_graphml`, `write_html`, and `write_json`.

### Landed: the sidebar was hiding most of the repository

- **The file tree listed 45 of 107 indexed files.** It was built by counting
  symbols per file, so a file that produced none never appeared: three source
  files here (`src/lib.rs`, `npm/bin/aag.js`, `npm/install.test.js`) and every
  one of the 59 documentation files, which are `doc` nodes with nothing under
  them. The tree is now built from the indexed files themselves, symbol counts
  attached where there are any and no count shown where there are none. The
  header pill counted only `file` nodes, so it said 48 while the tree said 45 —
  two numbers, both wrong, disagreeing. Both now say 107.
- **The row cap was 400 and silent.** It is 2000 and, when it truncates, the
  tree says "N of M files" instead of quietly presenting a subset as the whole.
- **The Filters tab could not be shown.** `#tab-filters { display: none }` — an
  id rule, specificity 100 — outranked `.tab-body.on { display: block }` at 20,
  so selecting the tab switched the highlight and left the panel blank. Node
  kinds, relation kinds, and edge confidence had been unreachable since the
  redesign. The id rule is gone; the class pair decides.

### Landed: files read as files, methods recede

The page drew every node the same way, so nothing said which circle was a file
and which was a method inside it. What a reader wants from this graph is which
files and modules reach each other; a method is where that happens, not what it
is about.

- **Two tiers.** `KIND_TIER` scales the radius and how much of the module colour
  survives against the background: a file is 1.75× and full ink, a type 1× at
  0.72, a function 0.68× at 0.5, a method 0.6× at 0.44. Degree still decides size
  within a tier — the busiest file is the biggest file — but it can no longer make
  a method outweigh the file holding it. Containers also paint above their
  symbols, so a file is never buried under its own dots, and the legend shows the
  scale rather than only the hues.
- **`structure only`** in the toolbar narrows the graph to files, docs, schemas,
  tables, infra and endpoints, which is the import-and-reference skeleton without
  the thousand functions hanging off it. It rides the existing kind filter, so it
  is one click for what was three checkboxes in a tab that could not be opened.
- **Edges favour the crossings.** A call between two methods of one file is
  structure a reader already assumes; one that crosses a file boundary is what
  they came to see. Internal edges keep 45% of their ink and 70% of their width.
- **Cross-module edges are drawn at all.** Overview required both ends of an edge
  to share a module, so expanding two modules drew nothing between them — the
  import tying them together, which is the reason to expand two modules, was
  missing. Both ends being on screen is the only condition now.
- **The camera fits the scene.** `fitScene` set a fixed ratio, which is not a fit:
  a dense module sat in a corner of an empty canvas while a three-node path
  filled the screen. The extent is measured from where the nodes landed, at the
  2nd and 98th percentile so one stray node cannot zoom everything out.
- **The label threshold calibrates itself.** It was a constant tuned against the
  old sizes; with the tiers in place it landed at 24.2 while the largest node in a
  90-file scene rendered at 24, so that view drew no names at all. A zoom band now
  asks for a number of labels and the threshold is read off the scene's own size
  distribution, which cannot drift when sizing changes again.
- **Chrome is set in a text face**, identifiers and source stay monospaced, and
  numbers are tabular. The header and status bar were retuned to hold one line,
  since a proportional face fits fewer characters per pixel than the monospaced
  one they were measured with.

### Landed: one island per module, instead of a disc of dots

A single force pass over everything can only settle into a disc. Repulsion is
uniform, so the shape carries no information: the module structure — the thing
worth reading — disappears inside an evenly spaced circle of nodes.

`islandLayout` runs two passes. The first lays out the modules themselves as a
small graph, with an edge repeated per doubling of the crossings between two
modules, so modules that talk end up near each other. The second lays out each
module's own nodes, seeded on a golden-angle spiral from its centre — hubs
first — and anchored there while their edges decide where they sit inside it.

Four things had to be true for it to read as a graph rather than as blobs:

- **Islands are sized by the area their members need**, not by how many there
  are. Fifty file circles want far more room than fifty method dots, and an
  island too small for its contents settles as a packed disc no matter how the
  physics is tuned, because the inward pull and the outward pushing cancel.
- **The pull inward eases off as a module grows** (`ISLAND_GRAVITY` divided by
  `log2(count)`), so a thousand-node module spreads enough to show its own shape
  while a five-node one stays a unit.
- **Islands are pushed apart only until they stop overlapping.** The default
  spacing is tuned for single nodes; at island scale it left most of the canvas
  empty, and the fit then zoomed out until every island was a speck.
- **Node size comes down as a scene gets dense.** Sigma shrinks a node by
  `1 / sqrt(zoom)`, deliberately sublinear, so zooming out to fit a thousand
  nodes does not shrink them to a thousandth: past a few hundred their combined
  area exceeds the viewport and they must overlap whatever the layout does.
  Taking density out of the size instead is what lets the layout's spacing
  survive the fit.

The fit also had to stop framing hidden nodes: a filtered-out node is still in
the graph, and `structure only` left the visible files crowded into a corner
around the empty space where the functions were.

### Landed: a module no longer looks like a file

The tiers separated a file from a method, but a module aggregate and a file were
still the same big coloured circle, so the one thing the Overview is for — is
this a module or a file? — took a click to answer.

- **A module is pastel.** An aggregate is lifted toward white past every file's
  brightness and painted above them, so it reads as the level above rather than
  as another node of the same kind.
- **Importance inside the file tier is saturation, not whiteness.** A file's
  colour is pushed up in saturation and lightness in proportion to
  `sqrt(degree / peak degree)`: the handful of files a module actually turns on
  are the vivid ones. Mixing toward white was tried first and reads as washed
  out — the opposite of important.

That leaves three readable levels: pastel and largest is a module, vivid is a
file worth opening, dim is code inside one.

### Definition of done

Inherited verbatim from P0.3 in [capability coverage](capability-coverage.md),
plus: every number in the 3A table above is re-measured and published for the
new implementation, on the same fixtures, before the gate is called closed.
