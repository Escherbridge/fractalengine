---
type: Track Spec
title: UI Shell Architecture — Manager Split, Right-Sidebar Tool Surfaces, and P0 In-App Fixes
description: Restructure the fe-ui rendering shell into explicit managers (one pointer/cursor-operations manager; one tab manager each for topbar, left sidebar, right sidebar; one modal manager for tooltips/toasts/dialogs), migrate every floating tool window (Tools, Terrain Tools, Proposal Report) into a toggle-revealed right-sidebar inspector, replace the always-open tool-description left panel with tooltips, and land two P0 fixes first — the terrain-tools app crash (panic in gardener_ui_system mid-egui-pass) and the in-app inability to select/manipulate existing path points (viewport→edit-mode bridge starved by a render-gated track-list load)
tags: [chore, ui_shell_architecture_20260724, pending]
timestamp: 2026-07-24T00:00:00Z
resource: ./metadata.json
---

# Specification: UI Shell Architecture + P0 In-App Fixes

**Track ID:** `ui_shell_architecture_20260724`
**Type:** chore (architecture refactor) with embedded P0 bug fixes
**Priority:** P0 (crash + unusable pen/point editing) then P1 (manager split, window migration)
**Crates:** `fe-ui` only (no new dependencies — NFR-2)

Cross-links: [`../../product.md`](../../product.md),
[`../../tech-stack.md`](../../tech-stack.md),
[`../../code_styleguides/ui_ux.md`](../../code_styleguides/ui_ux.md) (§5 is SACRED — see NFR-1),
[`../terrain_editor_overhaul_20260718/spec.md`](../terrain_editor_overhaul_20260718/spec.md),
[`../tool_inspector_ux_20260719/spec.md`](../tool_inspector_ux_20260719/spec.md),
[`../ux_interaction_hardening_20260718/spec.md`](../ux_interaction_hardening_20260718/spec.md),
[`../pen_curve_tool_20260722/spec.md`](../pen_curve_tool_20260722/spec.md).

## Overview

Verbatim user directives (2026-07-24, from live in-app testing):

1. > "I still cannot select and manipulate existing points and use the tool"
2. > "The tool descriptions for select move etc is in a sidebar thats always open
   > when it could just be a small tool tip it serves no value and takes up a lot
   > of realestate."
3. > "The terrain tools crash the app"
4. > "lets ensure we are using the right software architecture coupling for the UI
   > rendering system: 1 manager for pointer cursor operations; 1 manager for UI
   > tab interactions for each core area — topbar, side bar left, side bar right;
   > 1 modal manager for rendering things like tooltips; the current tool managers
   > should be rendered in the right sidebar inspector instead of floating —
   > terrain tools, path tools etc should be revealed in the right sidebar when
   > toggled on instead of the various floating controls."

This track does two things, strictly in order:

- **Phase 0 (P0):** fix the two in-app blockers — the terrain-tools crash
  (directive 3) and the unreachable point/handle manipulation (directive 1) —
  with root-cause findings + regression tests, landable independently of
  everything below.
- **Then (P1):** restructure the UI shell per directive 4 into four explicit
  manager seams (pointer-ops, per-area tabs, modal), migrate the floating tool
  windows into the right sidebar (directive 4), and replace the always-open
  tool-description panel with tooltips (directive 2).

### Ground truth (2026-07-24 code sweep)

**The shell today is one monolithic egui pass.** `gardener_ui_system`
(`fe-ui/src/plugin.rs:562-623`, registered in `EguiPrimaryContextPass` at
`plugin.rs:472`) calls `panels::gardener_console` (`fe-ui/src/panels/mod.rs:51-191`),
which renders — in one function body — the top toolbar, status bar, left hierarchy
sidebar, the tool-inspector left panel, right inspector/portal toolbar, the
CentralPanel viewport, nine floating dialogs, the GIS "Data" window, the floating
"Tools" window, the floating "Terrain Tools" window, the "Proposal Report"
window, and the toast overlay. A panic anywhere in that body unwinds
mid-egui-pass and takes the whole app down (see the crash cascade below).

**Panel/window inventory** (from `egui::Window::new` / `SidePanel` /
`TopBottomPanel` call sites):

| Surface | Kind | File |
|---|---|---|
| Toolbar (mode switcher + window toggles) | top panel | `panels/toolbar.rs:110` |
| Status bar | bottom panel | `panels/status_bar.rs:18` |
| Hierarchy tree | left `SidePanel("sidebar")` | `panels/sidebar.rs:25` |
| Tool inspector (per-tool descriptions) | left `SidePanel("tool_inspector")` | `panels/tool_inspector.rs:185` |
| Node inspector | right `SidePanel("inspector")` | `panels/inspector.rs:37` |
| Portal toolbar | right `SidePanel("portal_toolbar")` | `panels/portal_toolbar.rs:10` |
| "Tools" (path-asset stamp + pen + terrain toggle) | floating window | `panels/tool_panel.rs:220` |
| "Terrain Tools" (8-mode palette + proposals) | floating window | `panels/terrain_tools_panel.rs:42` |
| "Proposal Report" | floating window | `panels/proposal_report_panel.rs:45` |
| "Data — Query, Layers & Export" (GIS) | floating window | `panels/gis_panel.rs:46` |
| 9 dialogs (Map Manager, Settings, Node Options, …) | floating windows (`ActiveDialog` set) | `dialogs/*.rs` |
| Toast overlay | `egui::Area` | `panels/mod.rs:202` |

**Pointer-operation surfaces already share two pure seams** the pointer manager
formalizes rather than reinvents: the first-claim-wins `ClickArbiter`
(`node_manager/router.rs:34-124`) and the object-aware
`resolve_operation(tool, kind, hit) -> Operation` truth table
(`node_manager/dispatch.rs:149-194`, with `HitTarget`/`Operation` enums covering
node/vertex/handle/segment/stamp/proposal/gimbal hits). The consumer systems are
`viewport_pick.rs`, `path_point_interaction.rs`, `path_handle_interaction.rs`,
`path_segment_interaction.rs`, `path_gimbal_drag.rs`, and
`gimbal_interaction.rs`.

**The left auto-collapse rule is a per-frame overwrite.**
`panels/mod.rs:97-99` sets `sidebar.open = !right_panel_open` every frame —
user intent about the left sidebar cannot survive a selection. The left tab
manager takes ownership of this rule.

### P0 root-cause findings

#### Directive 1 — cannot select/manipulate existing points (CONFIRMED in code)

The viewport→edit-mode bridge exists but is starved of data:

- `open_track_on_select` (`node_manager/viewport_pick.rs:92-144`) correctly
  pushes `UiAction::PathSelectTrack` when the viewport selection lands on a
  known track — the sanctioned cross-authority coordination (queued action, no
  storage merge).
- Its gate `track_to_open` (`viewport_pick.rs:163-168`) requires the clicked
  node id to appear in `path_state.tracks` — the Paths-tab track list.
- That list is populated ONLY via a lazy load that fires while the "Data —
  Query, Layers & Export" window's Paths tab is rendering
  (`panels/gis_panel.rs:108-110` → `actions/path.rs:15-27` `request_tracks` →
  `verse_manager/db_results/query.rs:21,60` → `actions/path.rs:401`).
- Consequence: in a fresh session where the user never opens the Data window,
  `path_state.tracks` is empty, the bridge silently no-ops, `editing_track_id`
  is never set, and every vertex/handle/segment interaction early-outs
  (`path_handle_interaction.rs:207,291,408`; `path_gimbal_drag.rs:192-193`;
  point markers spawn gated the same way). The freshly-landed pen-curve
  Phases 1-6 vertex/handle markers are therefore unreachable in-app despite
  being fully wired in code. The silent no-op also violates `ui_ux.md` §6
  ("every silent failure is a bug").

**Fix direction (FR-2):** eager-load the track list on active-petal change
(reuse the `request_tracks` idiom from `actions/path.rs:15-27`, driven from the
existing petal-change branch in `open_track_on_select` or a sibling system), so
the existing bridge works from direct viewport interaction. Authorities stay
split; the coordination remains the already-queued `PathSelectTrack` action.

#### Directive 3 — terrain tools crash (hypotheses; Phase 0 spike confirms)

Crash cascade from the user's log: panic in
`fe_ui::plugin::gardener_ui_system` → panic in
`bevy_egui::run_egui_context_pass_loop_system` + ERROR "bevy_egui pass output
has not been prepared" → panic in `bevy_app::main_schedule::Main::run_main` →
exit 101. The second and third panics are consequences: `EguiPrimaryContextPass`
runs *inside* bevy_egui's pass-loop system, so any panel panic mid-pass leaves
`begin_pass` unbalanced and aborts the app. Ranked hypotheses for the root
panic (no `unwrap`/index found in the terrain panel bodies themselves —
`terrain_tools_panel.rs`, `proposal_report_panel.rs` are clean):

- **H-C1 (leading):** the "Add proposal" persist round-trip mutates shared
  frame state that a *downstream* panel in the same `gardener_console` body
  chokes on. `actions/terrain_proposal.rs:33-54` optimistically replaces
  `PetalMapState.terrain_json` with `embed_proposals(...)` — and on a petal
  with NO existing terrain config, `embed_proposals` (`terrain_proposal.rs:18-28`)
  produces a proposals-ONLY document (no `enabled`/`layers`/
  `tileset_hexon_uris`), a shape the terrain-json consumers (terrain-map
  loader, layer-manager card, GIS panel) may not expect. Repro variant to test
  first: terrain tools on a petal with no map installed.
- **H-C2:** the documented stale-index class — `selected_point`/
  `selected_segment` surviving a delete/reload and indexing `points[idx]` in a
  panel body. Most surfaces guard with `.get()` (e.g.
  `path_editor_card.rs:720-727`, `tool_inspector.rs` `anchor_readout`), but the
  hazard class is real (`tool_inspector.rs:151` comment) and the pen-curve
  Phases 3-6 code is fresh in the working tree.
- **H-C3:** an egui-internal invariant panic (window/Area/Grid state) in the
  "Terrain Tools"/"Proposal Report" windows — lowest probability; the captured
  backtrace rules this in or out immediately.

**Fix direction (FR-1):** reproduce with `RUST_BACKTRACE=1`, capture the real
panic payload, fix the root cause with a regression test — and, because the
cascade makes ANY panel panic fatal, FR-7 adds a panic guard at the panel-render
boundary so one broken panel can never again abort the app.

## Overlapping tracks — supersession and absorption

| Track | Relationship (explicit) |
|---|---|
| `tool_inspector_ux_20260719` (in_progress; Phase 1 landed = the left per-tool `SidePanel`) | **SUPERSEDED by this track for all remaining phases (2-6), and its Phase 1 output is REWORKED.** Directive 2 rejects the always-open left descriptions panel: the panel is removed; its `panel_descriptor` content becomes rich tooltips on the mode switcher (FR-8); its per-tool *Settings* ambition moves to the right-sidebar Tool section (FR-6); its `mode_button_fill` luminance work and the ratified "gimbal grabbable wherever shown" reconciliation (already landed) are KEPT. The pure helpers (`panel_descriptor`, `gimbal_affordance_label`, `selection_summary`, `anchor_readout`) and their tests survive as the tooltip/right-sidebar content source. Its deferred FR-4/5/6 models (select filters, `SnapSettings`, `TransformConstraints`) become carried-forward P2 backlog inside the right-sidebar Tool section, not a separate left panel. Orchestrator: mark that track `superseded` when this one lands. |
| `ux_interaction_hardening_20260718` (in_progress) | **Item (2) FR-2 "toolbar as context selector for a sidebar region" is ABSORBED** into the per-area tab managers (FR-4/FR-6) — same concept, now directed at the RIGHT sidebar per directive 4; do not implement it separately. **Item (4) FR-4 selection highlighting is CROSS-REFERENCED, not absorbed:** the pointer manager (FR-3) is where highlight triggers originate, but the highlight rendering work stays in that track. FR-1/FR-3/FR-5 of that track are untouched. |
| `terrain_editor_overhaul_20260718` | **FOUNDATION — consumed, not modified.** FR-1 `SelectionKind` facade (`node_manager/selection.rs`, read-only projection) and FR-2 object-aware dispatch (`node_manager/dispatch.rs`) are exactly the pure seams the pointer manager (FR-3) is built ON. |
| `pen_curve_tool_20260722` (Phases 1-6 in working tree) | **UNBLOCKED by this track's FR-2:** its vertex/handle manipulation is fully wired in code but unreachable in-app until the edit-mode bridge is fed. Phase 0 makes its FR-5 handle drags reachable. Its open interaction decisions (Phases 4-6 ratifications) remain that track's. |

## Functional Requirements

- **FR-1 (P0) — Terrain-tools crash fixed at root cause.** Reproduce the panic
  in `gardener_ui_system` (see hypotheses above), capture the payload, fix the
  root cause, and pin it with a regression test. A finding note (which
  hypothesis held, with the backtrace) is written into this track folder.
  *Acceptance:* the exact in-app repro (open Tools → Terrain Tools → use the
  palette / Add proposal, on petals both with and without an installed map) no
  longer crashes; a unit/regression test fails on the pre-fix code; no new
  `unwrap`/`expect`/unguarded indexing in the touched path.

- **FR-2 (P0) — Point selection/manipulation works from direct viewport
  interaction.** Clicking a path track in the viewport must enter Paths edit
  mode (Authority B) via the existing queued `UiAction::PathSelectTrack` bridge,
  after which vertex/handle/segment markers spawn and are pickable/draggable —
  making the pen-curve tool's manipulation reachable in-app. Root-cause fix:
  eager-load `path_state.tracks` on active-petal change (idiom:
  `actions/path.rs:15-27`), removing the render-gated dependency on the Data
  window (`gis_panel.rs:108-110`). The two authorities are NOT merged; the
  bridge stays a queued action; `track_to_open`'s buffer-clobber guard
  (`viewport_pick.rs:163-168`) is preserved. Entering/failing to enter edit
  mode must not be silent (`ui_ux.md` §6): entering edit mode gives visible
  feedback (e.g. the existing edited-track affordances), and the pre-fix silent
  no-op path gains a traced warning. *Acceptance:* fresh session → click a
  track in the viewport → edit mode active, vertex markers visible, dragging a
  vertex/handle works under every tool (per the ratified "grabbable wherever
  shown" rule); one track-list refresh request per petal change (unit-tested);
  `track_to_open` tests stay green.

- **FR-3 (P1) — Pointer/cursor-operations manager.** One explicit seam owns
  viewport pointer interpretation: the `ClickArbiter` (claim arbiter), a
  documented claim-priority order across the six consumer systems (handle >
  vertex > segment > gimbal-axis > node pick > empty, matching today's
  effective behavior), and the `resolve_operation` dispatch table. Deliverable
  is a consolidation, not a rewrite: a `node_manager/pointer` module (or
  hardened `router.rs`) that (a) exports the claim-priority as a pure, tested
  table, (b) hosts the cross-authority bridge (`open_track_on_select` moves
  here — coordination between `NodeManager.selected` and
  `PathEditorState.editing_track_id` lives in the pointer manager ONLY, as
  queued `UiAction`s), and (c) is the single place new pointer verbs register.
  *Acceptance:* every existing pointer system routes its click decision through
  the arbiter + dispatch (no bypass claims); claim-priority pure test covers
  every `HitTarget` pair; all existing router/dispatch/interaction tests stay
  green; behavior-preserving except the FR-2 fix.

- **FR-4 (P1) — Topbar tab manager.** The top toolbar becomes an explicit
  manager owning (a) the `TOOL_DEFS` mode switcher (single source of truth for
  buttons/shortcuts/hints/tooltips — preserved, per `toolbar.rs`) and (b) the
  window/section toggle buttons, which after FR-9 toggle right-sidebar sections
  instead of floating windows. *Acceptance:* `TOOL_DEFS` tests
  (`hint_line_covers_every_tool…`, `tool_defs_cover_all_tools…`) stay green;
  toggle state round-trips through the right-sidebar manager; no drift between
  button, shortcut, tooltip, and revealed section.

- **FR-5 (P1) — Left-sidebar tab manager.** Owns the hierarchy tree and the
  auto-collapse policy. The per-frame overwrite at `panels/mod.rs:97-99` is
  replaced by an explicit policy inside the manager (auto-collapse on
  right-panel-open remains the default behavior, but it becomes an owned,
  tested transition instead of a frame-stomp, so user intent can be respected
  later). *Acceptance:* pure `left_visibility(policy, right_open, user_intent)`
  helper unit-tested; existing behavior preserved by default.

- **FR-6 (P1) — Right-sidebar tab manager (the inspector region).** One right
  `SidePanel` manager with a compact toggle rail hosting sections:
  **Inspector** (the existing node inspector — default when a node is
  selected), **Tool** (active-tool settings — the re-homed
  tool-inspector content), **Path tools** (the Tools window's path-asset stamp
  + pen sections), **Terrain tools** (the 8-mode palette + proposal list), and
  **Proposal report**. Sections reveal on toggle (from the rail or the topbar);
  the portal toolbar continues to replace the region when the portal is open.
  Panel-local targets keep their authority rules: path/stamp sections keep
  keying ONLY on `PathEditorState.editing_track_id` (NFR-1). *Acceptance:*
  each section renders inside the right sidebar; toggling reveals/hides it;
  no section reads the other authority's selection; empty states are calm
  hints, never blank (`ui_ux.md` §7).

- **FR-7 (P1) — Modal manager: tooltips + transient overlays, panic-isolated
  panels.** One manager owns the transient layer: tooltips (FR-8), the toast
  overlay, the context menu, and the mutually-exclusive dialog set
  (`ActiveDialog`). It also owns the panel panic guard: every registered panel
  body renders through a `catch_unwind(AssertUnwindSafe(...))` boundary so a
  panicking panel is caught, logged via `tracing`, disabled for the session,
  and surfaced as a persistent status-bar error segment (`ui_ux.md` §6 tier) —
  the egui pass completes and the app survives. Panels must still be written
  panic-free (workflow rule); the guard is the blast-shield, not a license.
  *Acceptance:* a test panel that deliberately panics is caught, the frame
  completes, the panel is disabled with a visible error chip; guard adds no
  per-frame allocation when no panic occurs.

- **FR-8 (P1) — Tool descriptions become tooltips (directive 2).** The
  always-open left tool-inspector `SidePanel` (`tool_inspector.rs:185`) is
  removed. Its `panel_descriptor` content (title/subtitle/Use guidance) becomes
  rich hover tooltips on the topbar mode-switcher buttons — generated from the
  same single-source table so button/shortcut/tooltip cannot drift — plus the
  existing viewport hint line. The live selection/gimbal affordance readouts
  (`selection_summary`, `gimbal_affordance_label`, `anchor_readout`) move into
  the right-sidebar **Tool** section, keeping the 2026-07-19 legibility win
  without the standing real-estate cost. *Acceptance:* no always-open left tool
  panel remains; each mode button shows a tooltip naming the tool, its shortcut,
  and its one-line description; the pure helpers + tests survive relocation;
  reclaimed viewport width is visible in-app.

- **FR-9 (P1) — Floating tool windows migrate into the right sidebar with
  toggle reveal (directive 4).** The "Tools" window (`tool_panel.rs:220`), the
  "Terrain Tools" window (`terrain_tools_panel.rs:42`), and the "Proposal
  Report" window (`proposal_report_panel.rs:45`) are dissolved into FR-6
  sections; their toggles live on the topbar/rail. Move the UI, keep the logic:
  `installed_assets`/`filter_assets`/`build_descriptor`, the `TerrainToolMode`
  mirror + `to_proposal_op`, `compute_report`, and every `UiAction` emission
  (`PathAssetApply` against `editing_track_id`, `TerrainProposalAdd/Delete`)
  are preserved verbatim with their tests. Dialogs (Map Manager, Settings, …)
  and the GIS "Data" window stay floating in this track (see Out of scope +
  Open Q-4). *Acceptance:* zero control loss (checklist diff of every widget
  in the three windows); the old windows are gone; migrated pure-helper tests
  green from their new homes.

## Non-Functional Requirements

- **NFR-1 — Two-authority selection split is SACRED (`ui_ux.md` §5).**
  `NodeManager.selected` (viewport) and `PathEditorState.editing_track_id`
  (Paths tab) remain distinct storage. Managers COORDINATE (queued `UiAction`s
  in the pointer manager) and never merge; no path-editing surface reads or
  writes `NodeManager.selected`; the `SelectionKind` facade stays a read-only
  projection.
- **NFR-2 — No fe-ui → fe-terrain dependency.** Terrain knobs keep crossing via
  the local mirror enums (`TerrainToolMode`, `SpacingMode`) and the JSON
  contract. No new crate dependencies at all.
- **NFR-3 — No authorization in UI code; no `block_on` in Bevy systems.**
  Managers receive pre-authorized data; async stays on the channel seams.
- **NFR-4 — Single egui pass discipline (egui 0.39 / Bevy 0.18).** All manager
  rendering stays inside `EguiPrimaryContextPass` with one `begin_pass`/
  `end_pass` per frame. Because a mid-pass panic aborts the app (the FR-1
  cascade), panel bodies must be panic-free (no `unwrap`/`expect`/unguarded
  indexing — workflow checklist) AND rendered through the FR-7 guard.
- **NFR-5 — Geometry math in raw petal-local meters.** Wherever pointer math
  touches path points/handles, positions are raw petal-local meters — no
  `world_scale` multiplication (pen_curve invariant; the 2026-07-19 ribbon
  regression is the cautionary precedent). `world_scale` appears only in
  display formatting via the one conversion seam (`panels/widgets.rs`).
- **NFR-6 — Quality gates.** `cargo clippy -- -D warnings` on latest stable
  (CI's floating `rust-toolchain@stable` — see the 2026-07-23 toolchain-drift
  playbook), `cargo fmt --check`, workspace tests; single integrated sweep at
  the end of the batch, not per-fix loops.
- **NFR-7 — Docs convention.** Terse one-line doc comments; rationale and
  manager topology in directory `AGENTS.md` files (`panels/AGENTS.md`,
  `node_manager/AGENTS.md`, new `ui_shell` section), not inline blocks.
- **NFR-8 — `ui_ux.md` pre-merge checklist** applies to every touched surface
  (calm chrome/luminance §1, units §2, no silent failure §6, mode-gated
  overlays §7, path/map terminology §9).

## User Stories

- As a **world-builder**, when I click a path I imported, I can immediately
  grab its points and bezier handles in the viewport — no hidden prerequisite
  window — so the pen curve tool is actually usable.
  *Given* a fresh session with a petal containing a GPX path, *when* I click
  the path in the viewport, *then* edit mode activates with visible markers,
  *and* dragging a vertex or handle moves it under any active tool.
- As an **operator**, when a single panel misbehaves, the app keeps running
  and tells me which panel failed, instead of exiting with code 101.
- As a **user arranging my workspace**, tool surfaces live in one predictable
  place — the right sidebar, revealed when I toggle them — and tool
  descriptions cost me zero pixels until I hover a tool button.

## Technical Considerations

- **Managers are seams, not frameworks.** Each manager is a plain module with a
  small state resource + pure decision helpers + a thin egui render fn, called
  in order from a slimmed `gardener_console`. `gardener_ui_system` stays the
  single `EguiPrimaryContextPass` entry (its `SystemParam` bundles already
  group the state). No trait-object plugin registry unless a real third
  consumer appears.
- **Insertion points:** shell composition `panels/mod.rs:51-191`; system
  registration `plugin.rs:472-479`; auto-collapse `panels/mod.rs:97-99`;
  tool-inspector call site to remove `panels/mod.rs:107-109`; floating-window
  call sites to migrate `panels/mod.rs:160-185`; pointer bridge
  `viewport_pick.rs:92-168`; lazy track load to replace `gis_panel.rs:108-110`
  (+ `actions/path.rs:15-27` request idiom); claim arbiter `router.rs:34-124`;
  dispatch table `dispatch.rs:149-194`; right-panel home `inspector.rs:37`.
- **Right-sidebar capacity:** the inspector panel is already the widest fixed
  surface; sections render one-at-a-time via the rail (recommended, Open Q-2)
  to avoid unbounded vertical stacking.
- **`catch_unwind` in an egui pass** is safe at whole-window/section
  granularity: a caught panic before the window's `show` closure returns leaves
  the `Context` consistent enough to finish the pass (the failure mode being
  fixed is precisely the *uncaught* unwind). `AssertUnwindSafe` is justified
  because the guarded state is per-panel UI state that the guard then quarantines
  (panel disabled for the session). Verified by a deliberate-panic test.
- **Phase 0 lands first and alone** — it must be committable before any
  manager refactor starts, so the user gets a working editor immediately.

## Out of Scope

- Merging the two selection authorities (permanently out — NFR-1).
- The GIS "Data — Query, Layers & Export" window and the dialog set (Map
  Manager, Settings, …) — they stay floating in this track (Open Q-4 files the
  follow-up question).
- Selection highlighting rendering (`ux_interaction_hardening` FR-4), gimbal
  drag smoothing (its FR-3), and GPX control hardening (its FR-1).
- Pen-curve Phases 4-6 interaction decisions (that track's ratification queue).
- Behavioral select filters, snap-into-gesture wiring, per-tool settings
  persistence (carried-forward P2 backlog noted under the supersession table).
- Any fe-renderer/fe-terrain change; any new overlay-layer registry work
  (`ui_ux.md` §7 future-track).

## Open questions (ratify before build)

- **Q-1 — Fate of the left tool rail.** Remove the left tool-inspector panel
  entirely (tooltips + right-sidebar Tool section carry everything), or keep a
  slim icon-only vertical mode rail on the left (Blender-style) with zero text?
  *Recommended:* remove entirely — directive 2 says it "serves no value"; the
  topbar already switches modes. Ratify because the 2026-07-19 ask explicitly
  requested a left panel and this reverses it.
- **Q-2 — Right-sidebar section layout.** One-section-at-a-time tabs behind a
  compact icon rail (recommended — predictable width, calm), or stackable
  accordion sections (more simultaneously visible, more scroll)?
- **Q-3 — Edit-mode entry gesture.** After FR-2, a single viewport click on a
  track auto-enters path edit mode (today's intended behavior, finally
  working). Keep single-click auto-enter (recommended — fewest steps to the
  user's stated goal), or require an explicit second gesture (double-click /
  "Edit path" button) to avoid accidental edit sessions while arranging nodes?
- **Q-4 — Follow-up migration scope.** Should the GIS "Data" window (Query /
  Layers / Paths / Export tabs) and/or Map Manager also fold into the
  right-sidebar region in a successor track, or remain floating long-term?
  *Recommended:* keep floating for now — they are data/query surfaces, not
  tool palettes; revisit after living with the new shell.
- **Q-5 — Panic-guard failure UX.** When the FR-7 guard catches a panel panic:
  disable the panel for the session with a persistent status-bar error segment
  (recommended), or attempt automatic re-enable on the next petal switch?
  (Debug builds keep propagating the panic under `cfg(debug_assertions)` so
  developers still see crashes loudly — part of the recommendation.)

## Ratified decisions (2026-07-24)

User directive 2026-07-24: **"Ratify spec 5"** — accept all five recommended
answers verbatim and proceed. Normative for the phases they gate.

- **Q-1 → RATIFIED: remove the left tool-inspector panel entirely.** Tooltips
  (FR-8) + the right-sidebar **Tool** section carry all of its content; the
  topbar already switches modes. This reverses the 2026-07-19 "add a left
  panel" ask. Gates Phase 5.
- **Q-2 → RATIFIED: right sidebar is one-section-at-a-time behind a compact
  icon rail.** Predictable width, calm chrome; no stackable accordion. Gates
  Phase 2 (`RightSidebarSection` + rail) and Phase 4 (section reveal).
- **Q-3 → RATIFIED: keep single-click auto-enter path edit mode.** Fewest steps
  to the user's stated goal; no second gesture required. Gates the FR-2 edit
  entry (already landed) — no change needed.
- **Q-4 → RATIFIED: GIS "Data" window + Map Manager stay floating** in this
  track. They are data/query surfaces, not tool palettes; a successor track may
  revisit after living with the new shell. Gates Phase 4 scope boundary.
- **Q-5 → RATIFIED: panic-guard disables the panel for the session + persistent
  status-bar error segment** (`ui_ux.md` §6 Error tier). No auto re-enable on
  petal switch. Debug builds re-propagate under `cfg(debug_assertions)` so
  developers still see crashes loudly. Gates Phase 3 (FR-7 guard UX).
