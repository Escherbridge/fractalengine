---
type: Track Spec
title: Blender-Like Tool Inspector — Active Tool as an Explicit UI Mode
description: Re-home the scattered viewport tool surface into a Blender-style tool-mode switcher (top) plus a left per-tool inspector panel, so the ACTIVE tool becomes a legible UI MODE that reshapes how Select/Move/Rotate/Scale/Pen behave, surfaces each mode's affordances (axis constraints, snapping, element highlighting, pivot/orientation), and makes "which tool is active / is the gimbal grabbable" obvious — without merging selection storage, without an fe-terrain dependency, egui-only
tags: [feature, editor, ui, tool-inspector, blender-like, tool_inspector_ux_20260719, pending]
timestamp: 2026-07-19T00:00:00Z
resource: ./metadata.json
---

# Specification: Blender-Like Tool Inspector

**Track ID:** `tool_inspector_ux_20260719`
**Priority:** P1 UX (user-directed 2026-07-19)
**Crates:** `fe-ui` (tool-mode model, left inspector panel, per-tool settings,
element-highlight model, tool_panel migration, gimbal-legibility reconciliation).
No new crate dependency — see NFR-1.

Cross-links: [`../../product.md`](../../product.md),
[`../../tech-stack.md`](../../tech-stack.md),
[`../../code_styleguides/ui_ux.md`](../../code_styleguides/ui_ux.md),
[`../terrain_editor_overhaul_20260718/spec.md`](../terrain_editor_overhaul_20260718/spec.md)
(this track consumes its `SelectionKind` read-model + `Operation` dispatch).

## Overview

Verbatim user ask (2026-07-19):

> "A Blender-like tool-inspector that makes the ACTIVE TOOL an explicit UI MODE:
> tool icons along the TOP (adapt the existing Select/Move/Rotate/Scale/Pen row),
> a LEFT sidebar with tabbed panels that open PER-TOOL — each tool gets a panel
> for actually USING the tool plus its settings (element highlighting, snapping,
> axis constraints, pivot/orientation, spacing). The active tool becomes a mode
> that restricts/enables actions and reshapes how the core tools behave, so users
> stop being confused about when the gimbal is / isn't interactive."

Today the tool surface is scattered and the "which tool is active" state is weak:
the top toolbar owns a five-button tool row, a separate floating "Tools" window
owns path-asset stamping + pen curves/shapes + a terrain-tools toggle, and the
gimbal is drawn-but-sometimes-not-draggable depending on the active tool. This
track re-homes all of that into one **mode-driven** shape: the top row is the
**mode switcher**, and a **left per-tool inspector** shows exactly the affordances
+ settings of the mode you are in. It is a UX/presentation reorganization on top
of machinery that already exists (`Tool` enum, `ToolState`, the `SelectionKind`
read-model, the `Operation` dispatch, the gimbal) — it adds legibility, it does
not rebuild the editor core.

### Ground truth (2026-07-19 exploration sweep)

- **Tool enum + single source of truth.** `Tool { Select, Move, Rotate, Scale,
  Pen }` (`fe-ui/src/panels/toolbar.rs:14-22`); `ToolState { active_tool }`
  (`fe-ui/src/plugin.rs:159-163`). `TOOL_DEFS` (`toolbar.rs:36-77`) is the ONE
  source for the toolbar buttons, the keyboard bindings, and the viewport hint
  line (`shortcut_hint_line`, `toolbar.rs:80-88`) — they cannot drift by design,
  and shortcuts read it directly (`node_manager/shortcuts.rs:27-32`). The mode
  switcher must preserve this single source.
- **Active state reads weakly.** The active tool button fills with
  `BG_BUTTON_ACTIVE` — a saturated blue that `ui_ux.md §1` explicitly flags as a
  violation-to-migrate (`theme.rs:9`): active/emphasis should read via *luminance*,
  not hue. So "which mode am I in" is currently a small hue shift, not a legible
  mode indicator.
- **The scattered surface = `tool_panel.rs`.** A floating "Tools" window
  (`fe-ui/src/panels/tool_panel.rs`) hosting: a Path-Asset stamp section
  (`render_path_asset_section`, emits `UiAction::PathAssetApply` against
  `PathEditorState.editing_track_id`), a Pen section (curve mode / tension /
  samples / circle-ellipse-rectangle shapes, `render_pen_section:500-669`), and a
  Terrain-Tools reachability toggle (`render_terrain_tools_section:676-699`). Its
  pure helpers (`installed_assets`, `filter_assets`, `build_descriptor`) are
  unit-tested and must survive the move intact. This window is the redesign target.
- **Left panel is the hierarchy tree.** `sidebar::left_sidebar`
  (`panels/sidebar.rs:14-46`) is the verse/fractal/petal/node tree, and it
  **auto-collapses whenever the right panel is open** (`panels/mod.rs:94-96`) —
  i.e. it hides exactly when a node is selected. A Move/Rotate/Scale inspector is
  most useful *when something is selected*, so the tool inspector must be exempt
  from that auto-collapse (see Open Questions).
- **Selection read-model already exists (consume, don't rebuild).** `SelectionKind`
  / `SelectionState` / `project_selection` (`fe-ui/src/node_manager/selection.rs`)
  is a per-frame **projection** over the two authorities — a facade, NOT a storage
  merge (codified `ui_ux.md §5`). It already models `Empty | Node | PathTrack |
  PathVertex | PathSegment | Stamp | TerrainProposal`. The inspector reads it.
- **Gimbal is drawn-but-not-draggable under Select/Pen (the confusion).**
  `draw_gimbal_system` already draws a Move gimbal for path selections even under
  Select/Pen, mapping those to an effective Move tool
  (`gimbal_interaction.rs:342-368`). But the *interaction* systems still early-out
  under Select (`:35`) and Select|Pen (`:170`), so the drawn handle is not
  grabbable there. That draw-vs-interact split is precisely the "when is the
  gimbal interactive?" confusion the ratified decision below resolves.
- **No fe-ui → fe-terrain dependency, by precedent.** Terrain/path-tool knobs
  cross via local mirror enums, not imports: `SpacingMode` mirrors
  `fe_sdk::path_asset::SpacingMode` (`tool_panel.rs:70-85`), `TerrainToolMode`
  mirrors `terrain_proposal_state::ProposalOp` (`tool_panel.rs:92-146`). The
  inspector follows the same rule.

### Ratified decision (bake into the spec)

**The gimbal is grabbable wherever it is shown.** Path vertex / segment / track
gimbals are drawn AND draggable in **every** tool where they appear (not just
Move). The tool inspector's job is to make the active mode + its affordances (e.g.
"gimbal active") **legible**, NOT to hide the gimbal. Per-tool panels surface the
current mode's affordances explicitly (Move shows axis constraints + snapping;
Select shows selection filters + highlighting), so the user always knows what the
current mode enables.

## Functional Requirements

- **FR-1 — Active tool as an explicit UI MODE (mode switcher).** Adapt the
  existing `TOOL_DEFS` top row into a legible **mode switcher**: the active mode
  reads via luminance emphasis (`ui_ux.md §1`), not the current saturated-blue
  fill, so "which mode am I in" is unmistakable. Introduce a thin `ToolMode`
  concept (wrapping the active `Tool`) that the inspector + affordance surfacing
  key on. The `TOOL_DEFS` single source of truth (buttons/shortcuts/hint) is
  preserved. *Priority: P1.* *Acceptance:* exactly one mode ever reads active;
  switching by click or by `S/G/R/X/P` updates both the toolbar emphasis and the
  left inspector body; active emphasis resolves to a theme *luminance* token, and
  no new saturated hue is introduced for a normal/active chrome state.

- **FR-2 — Left per-tool inspector panel (mode-driven).** A new left egui
  `SidePanel` (`panels/tool_inspector_panel.rs`) that renders the panel for the
  **active tool only**, with two zones per tool: a **Use** zone (the mode's action
  affordances) and a **Settings** zone (the mode's knobs). It coexists with the
  hierarchy sidebar and must remain visible in a transform/edit mode even when the
  right inspector is open (i.e. exempt from the `panels/mod.rs:94-96`
  auto-collapse). *Priority: P1.* *Acceptance:* switching modes swaps the
  inspector body; the panel names the active mode; a no-selection / nothing-to-do
  state renders a calm hint, never a blank panel (`ui_ux.md §7` — no invisible
  empty states).

- **FR-3 — Per-tool Use + Settings surfaces for Select / Move / Rotate / Scale /
  Pen.** Each mode's panel shows its enumerated controls:
  - **Select** — selection filters (which `SelectionKind` categories are pickable),
    element-highlight styling, and a live readout of the current `SelectionKind`.
  - **Move** — axis constraints (X/Y/Z lock), pivot/orientation, snapping (grid /
    vertex), and the `Ctrl`+drag vertical-constraint hint (`ui_ux.md §5` modifier
    table).
  - **Rotate** — rotation axis, pivot, and angle-snap presets (45° / 90° / custom,
    `ui_ux.md §8`).
  - **Scale** — uniform vs per-axis, pivot, snap increment.
  - **Pen** — curve mode (Polyline / Catmull / Bezier), tension, samples/segment,
    and shapes (circle / ellipse / rectangle) — migrated from `tool_panel.rs`.

  *Priority: P1.* *Acceptance:* each mode's panel shows its enumerated controls
  bound to a testable settings struct; every displayed number carries its unit
  (`m` / `°`, `ui_ux.md §2`).

- **FR-4 — Element highlighting + selection-filter model.** The Select panel
  exposes which `SelectionKind` categories are eligible for picking and how each is
  highlighted, driven off the existing `SelectionState` read-model. Highlighting
  stays **distinguishable per authority** — a node selection (`NodeManager.selected`)
  and a path selection (`PathEditorState`) never look identical and are never merged
  (`ui_ux.md §5`). *Priority: P2.* *Acceptance:* the panel displays the current
  selection kind; an eligibility predicate (`is_eligible(kind, mask)`) is pure and
  unit-tested; per-authority highlight styles are visually distinct and use theme
  luminance tokens (no saturated hue for a normal selection, `ui_ux.md §1`).

- **FR-5 — Snapping settings scaffold.** A `SnapSettings` model (grid snap on/off
  + increment in `m`, angle snap on/off + preset 45° / 90° / custom, vertex snap
  on/off) surfaced in the Move / Rotate / Scale / Pen panels, backed by pure
  helpers (`snap_to_grid`, `snap_angle`). Presets over bare numbers (`ui_ux.md §8`).
  This is a **scaffold**: the settings + math land and are unit-tested; wiring snap
  into every gesture is incremental (the measurement-first live readout is the
  `ui_ux.md §5` end target). *Priority: P2.* *Acceptance:* snap helpers round to
  fixtures; a disabled snap is the identity function; a zero/negative step is
  guarded; a preset populates the raw value while leaving it editable.

- **FR-6 — Axis constraints / pivot / orientation model.** A `TransformConstraints`
  model (axis-lock mask; pivot: median / individual / cursor; orientation: global /
  local) shared by the Move / Rotate / Scale panels, with a pure
  `apply_axis_lock(delta, mask)` helper. *Priority: P2.* *Acceptance:* the axis-lock
  mask zeroes the locked components of a delta (unit-tested); the all-unlocked mask
  is the identity; pivot/orientation selections persist per mode within a session.

- **FR-7 — Mode legibility + gimbal "grabbable wherever shown" (ratified).** The
  inspector surfaces the active mode's affordances explicitly — e.g. a
  "Gimbal active — drag handle to move" line when a transform gimbal is shown for
  the current selection — so users stop being confused about when the gimbal is
  interactive. Bake the ratified decision: **a drawn gimbal is a grabbable gimbal.**
  Reconcile the draw-vs-interact split — `draw_gimbal_system` already draws a Move
  gimbal for path selections under Select/Pen (`gimbal_interaction.rs:342-368`),
  but `handle_gimbal_interaction` still early-returns under Select (`:35`) and
  Select|Pen (`:170`) — by routing both through **one shared pure predicate** so
  wherever a path vertex/segment/track gimbal is drawn it is also draggable. The
  inspector must NOT hide the gimbal. *Priority: P1.* *Acceptance:* a shared
  predicate makes `gimbal_interactive(tool, kind) == gimbal_drawn(tool, kind)` for
  every `(tool, kind)` (unit-tested); dragging a path-vertex gimbal works under
  Select and Pen; the inspector shows a "gimbal active" affordance for the current
  mode when a gimbal is shown.

- **FR-8 — Migrate the scattered tool surface (`tool_panel.rs`) into the
  inspector.** Fold the floating "Tools" window's contents into the per-tool
  inspector: Pen curve/shape controls → the Pen panel; the Path-Asset stamp →
  a stamp affordance reachable from the relevant mode, still emitting
  `UiAction::PathAssetApply` against `PathEditorState.editing_track_id` (keep the
  tab-local target rule, `ui_ux.md §5`); the Terrain-Tools reachability toggle
  stays reachable (opens the existing `terrain_tools_panel`). Move the UI, keep the
  logic — `installed_assets` / `filter_assets` / `build_descriptor` and their tests
  are preserved. *Priority: P1.* *Acceptance:* no control is lost in the migration;
  `PathAssetApply` still targets the Paths-tab editing track; the old floating
  "Tools" window is removed or reduced to a redirect; the migrated pure-helper
  tests stay green.

- **FR-9 — Keyboard-shortcut parity.** The mode switcher stays wired to `TOOL_DEFS`
  + `shortcuts.rs` so `S/G/R/X/P` switch modes and the inspector body follows; the
  viewport hint line stays generated from `TOOL_DEFS`. No drift between button,
  shortcut, hint, and inspector; the `egui_wants_keyboard` gate is preserved.
  *Priority: P1.* *Acceptance:* each `TOOL_DEFS` key switches mode and swaps the
  inspector body; the existing `toolbar.rs` tests (`hint_line_covers_every_tool…`,
  `tool_defs_cover_all_tools…`) stay green.

## Non-Functional Requirements

- **NFR-1 — No fe-ui → fe-terrain dependency.** Any terrain/path-tool knob crosses
  via local mirror enums / the existing JSON contract, not a new `fe-terrain`
  import — precedent: `SpacingMode` and `TerrainToolMode` local mirrors
  (`tool_panel.rs:70-146`). The inspector opens the terrain panel by toggling an
  existing flag; it does not import `fe-terrain` types.
- **NFR-2 — Two-authority selection preserved.** The inspector reads the
  `SelectionState` read-model and the two authorities; it MUST NOT merge
  `NodeManager.selected` and `PathEditorState` storage (`ui_ux.md §5`). Paths-mode
  affordances key on `editing_track_id` only; no panel silently reads another
  context's selection.
- **NFR-3 — ISA-101 / HMI styleguide conformance.** Calm chrome + luminance active
  state (§1), a unit on every number (§2), two-step destructive confirm for any
  destructive control (§5), mode-gated overlays / calm default viewport (§7),
  presets over bare numbers (§8), and `path` / `map` / `Portal URL` terminology
  (§9). The `ui_ux.md` pre-merge checklist is the gate.
- **NFR-4 — egui-only.** Panels are `bevy_egui`; no new UI backend and no direct
  Bevy-UI nodes. egui paint is validated in-app; logic is validated by unit tests.
- **NFR-5 — Pure, testable helpers.** The tool→panel descriptor mapping, the
  settings structs, the snap math, the axis-lock application, the
  gimbal-interactive predicate, and the selection-eligibility predicate are all
  pure and unit-tested; the egui paint code stays thin (this repo unit-tests pure
  logic heavily and validates egui rendering in-app).
- **NFR-6 — Gimbal never hidden by the inspector; drawn == draggable.** The
  inspector's job is legibility, not suppression (ratified). The `TOOL_DEFS`
  keyboard single-source-of-truth is preserved.

## Out of scope

- The `SelectionKind` read-model, the `Operation` left-click dispatch table, and the
  terrain proposal system — owned by `terrain_editor_overhaul_20260718` (already
  landed). This track **consumes** `SelectionState` + the dispatch; it does not
  rebuild them.
- The gimbal drag math / axis-pick geometry itself (`gimbal_interaction.rs`) — this
  track only reconciles the draw-vs-interact **gate** (FR-7), not the drag math.
- Road/path placement input modes (straight/curve/freeform) — `road_builder_ux_20260716`.
- Measurement TOOLS (tape/area/bearing) + the proposal report panel —
  `hexon_scale_orchestration` Phase 5 + `terrain_editor_overhaul_20260718` FR-6.
- Scale-authority plumbing (unifying the `world_scale` accessor) —
  `map_scale_authority_20260716`; this track consumes `PetalMapState.world_scale`.
- Persisting per-tool settings across sessions into `AppSettings` (D-78) — the
  Settings window already exists; persistence here is a follow-up (see Open Q).
- A full theme migration of `BG_BUTTON_ACTIVE` — this track adds/uses a luminance
  active-state token where it touches the toolbar (`ui_ux.md §1` adopt-when-touched),
  not a repo-wide sweep.

## Open questions

- **Inspector placement vs the hierarchy sidebar.** A second inner-left `SidePanel`
  adjacent to the hierarchy tree, a tab inside the existing left sidebar, or a
  collapsible section? The auto-collapse-on-right-panel-open rule
  (`panels/mod.rs:94-96`) hides the whole left sidebar when a node is selected, yet
  a transform-mode inspector is most useful exactly then — so the inspector likely
  needs its own always-available panel. *Lean:* a dedicated left `SidePanel`
  exempt from the auto-collapse.
- **Switcher location.** Keep the mode switcher on the TOP toolbar (adapt
  `TOOL_DEFS` in place, as the ask states) or later add a Blender-style vertical
  left icon rail? *Lean:* top row for v1; vertical rail is a later refinement.
- **Select filters — behavioral or informational in v1?** Should the Select panel's
  element filter actually gate the pick systems, or only annotate the current
  selection kind? *Lean:* informational readout + filter model first, behavioral
  gating as a follow-up, to keep v1 non-destructive to picking.
- **Snapping depth for v1.** Settings + math only (scaffold) or wired into at least
  the Move gesture as a demonstrator? *Lean:* scaffold + Move-gesture wiring.
- **Per-tool settings persistence.** In-memory resource (reset per launch) vs
  persisted into `AppSettings` / D-78? *Lean:* in-memory v1; persistence a follow-up.
