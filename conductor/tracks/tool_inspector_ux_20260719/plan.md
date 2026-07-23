---
type: Implementation Plan
title: "Implementation Plan: Blender-Like Tool Inspector — Active Tool as an Explicit UI Mode"
tags: [tool_inspector_ux_20260719]
resource: ./spec.md
---

# Implementation Plan: Blender-Like Tool Inspector

## Overview

Sequenced so the mode + left-inspector **scaffold** lands first (Phase 1, all pure
data-model + a thin panel shell), then the per-tool Use/Settings panels
(Phases 2–3), then the shared snapping / highlighting / constraint models
(Phase 4), then the `tool_panel.rs` migration + the ratified gimbal
"grabbable-wherever-shown" reconciliation (Phase 5), then keyboard parity +
close-out (Phase 6). This is a presentation reorganization over machinery that
already exists (`Tool`, `ToolState`, the `SelectionKind` read-model, the gimbal),
so most work is pure helpers with a thin egui shell. TDD throughout; a single
workspace sweep at the very end per standing policy
(`RUST_MIN_STACK=134217728 cargo test --workspace -j2`).

> **Status 2026-07-19 (implemented this session).** Phase 1 landed in
> `panels/tool_inspector.rs` (as `tool_inspector.rs`, not `_panel.rs`). The FR-7
> gimbal "grabbable wherever shown" reconciliation (Phase 5's gimbal task) landed
> EARLY as part of the in-app-regression bug-fix pass on the same day
> (`path_gimbal_drag.rs` ungated from Move-only; `dispatch.rs::resolve_gimbal`;
> `gimbal_interaction.rs::draw_gimbal_system` draws vertex/segment as Move arrows
> in every tool), so the inspector's `gimbal_affordance_label` (FR-7 legibility)
> shipped with Phase 1. Deferred to follow-up sessions: Phases 2–4 (per-tool
> Use/Settings bodies, snapping/constraint/highlight models) and the rest of
> Phase 5 (`tool_panel.rs` migration). The active tool is now a legible mode with
> a left per-tool panel; the zones are calm placeholders until Phase 2+.

## Phase 1: Tool-mode model + left inspector shell (scaffold) — DONE

Goal: the active tool is a legible mode with a left panel that swaps per tool,
all backed by pure, unit-tested helpers.

Tasks:
- [x] Task: pure `panel_descriptor(tool) -> ToolPanelDescriptor` mapping each of
      Select/Move/Rotate/Scale/Pen to a title + subtitle + Use/Settings zones
      (tested: all 5 tools have non-empty title/subtitle + both zones; titles
      distinct). `ToolInspectorState` deferred — the v1 panel is stateless (reads
      `ToolState` + the two selection authorities), so no new resource/signature
      change was needed.
- [x] Task: `panels/tool_inspector.rs` — a left `SidePanel` rendering the active
      tool's title + Use/Settings zones + a live `selection_summary` + the FR-7
      gimbal affordance, wired into `gardener_console` after `left_sidebar`
      (always visible, exempt from the right-panel auto-collapse); calm, never
      blank. egui paint kept thin.
- [x] Task: Active-mode legibility on the top toolbar — pure
      `mode_button_fill(active) -> Color32` returning a *luminance* fill
      (`theme::BG_MODE_ACTIVE`, a brighter neutral), applied in `top_toolbar`
      (tested: active resolves to strictly higher luminance than inactive).
- [x] Verification: covered by unit tests; in-app verification user-gated.

## Phase 2: Select + Move panels

Goal: the two most-used modes have real Use/Settings bodies bound to pure models.

Tasks:
- [ ] Task: `ToolSettings` root resource + `SelectSettings` (eligibility filter
      mask + highlight style) pure model; Select panel body reads `SelectionState`
      and shows the live `SelectionKind` + filter toggles (TDD: pure
      `is_eligible(kind, mask)` truth-table; filter mask defaults all-on)
- [ ] Task: `MoveSettings` (axis-lock mask + pivot + snap toggle) + Move panel body
      (X/Y/Z axis locks, `Ctrl`+drag vertical-constraint hint per `ui_ux.md §5`,
      snap toggle with `m`-suffixed increment) (TDD: axis-lock apply zeroes locked
      delta components; increment carries `m`)
- [ ] Verification: Select panel names the live selection kind + filters; Move
      panel shows axis locks + snapping; per-authority highlight stays distinct
      [checkpoint]

## Phase 3: Rotate + Scale + Pen panels

Goal: the remaining three modes, including folding the pen curve/shape controls out
of `tool_panel.rs`.

Tasks:
- [ ] Task: `RotateSettings` (axis + pivot + angle-snap preset) + Rotate panel with
      45°/90°/custom preset chips (`ui_ux.md §8`) and `°` units (TDD: a preset chip
      sets the raw angle value; custom stays editable)
- [ ] Task: `ScaleSettings` (uniform vs per-axis + pivot + snap increment) + Scale
      panel (TDD: uniform toggle mirrors one axis value across all axes; per-axis
      keeps them independent)
- [ ] Task: Pen panel — move curve-mode / tension / samples + shape controls from
      `tool_panel.rs::render_pen_section` into the Pen inspector zone, keeping the
      `curve::{ellipse,circle,rectangle}` calls + the same `UiAction` emission
      (TDD: shape point counts unchanged; the panel emits the same `UiAction`
      variants — `PathSmoothCurrent`, `PathAppendShape`)
- [ ] Verification: each transform/pen mode shows its enumerated controls with
      correct units + presets [checkpoint]

## Phase 4: Snapping + highlighting + constraint models (shared, pure)

Goal: the cross-cutting settings models the transform panels consume, all pure and
fixture-tested.

Tasks:
- [ ] Task: `SnapSettings` + pure `snap_to_grid(v, step)` / `snap_angle(deg, step)`
      helpers; disabled = identity (TDD: rounds to fixtures; identity when off;
      zero/negative step guarded)
- [ ] Task: `TransformConstraints` (axis mask, pivot enum, orientation enum) +
      `apply_axis_lock(delta, mask)`; shared by Move/Rotate/Scale settings (TDD:
      mask zeroes locked components; all-unlocked = identity)
- [ ] Task: Element-highlight model — per-authority highlight style tokens (theme
      luminance, `ui_ux.md §1`) + pure `highlight_for(kind) -> HighlightStyle` that
      keeps node vs path authorities visually distinct (`ui_ux.md §5`) (TDD:
      distinct styles per authority; no saturated hue token for a normal selection)
- [ ] Verification: snap / lock / highlight helpers pass fixtures; Move/Rotate/Scale
      consume the shared models [checkpoint]

## Phase 5: tool_panel.rs migration + gimbal "grabbable wherever shown"

Goal: retire the scattered floating "Tools" window and bake the ratified gimbal
decision.

Tasks:
- [ ] Task: Migrate the Path-Asset stamp section into a stamp affordance in the
      inspector, still emitting `UiAction::PathAssetApply` against
      `PathEditorState.editing_track_id` (keep the tab-local target rule,
      `ui_ux.md §5`); keep `installed_assets` / `filter_assets` / `build_descriptor`
      + their tests intact (TDD: migrated pure-helper tests stay green from their
      new home; `PathAssetApply` target unchanged)
- [ ] Task: Terrain-Tools reachability — the inspector exposes the "Open Terrain
      Tools" toggle (drives the existing `ToolPanelState.terrain_tools_open` that
      `terrain_tools_panel` reads); remove or redirect the old floating "Tools"
      window (TDD: the toggle flips the same flag the terrain panel reads)
- [ ] Task: Gimbal draw-vs-interact reconciliation — extract a shared pure
      `gimbal_interactive(tool, kind)` matching the existing draw predicate, and
      ungate `handle_gimbal_interaction` + the hover pick under Select/Pen for path
      selections (`gimbal_interaction.rs:35,170`) so drawn == draggable (ratified)
      (TDD: `gimbal_interactive == gimbal_drawn` for every `(tool, kind)`;
      path-vertex drag reachable under Select/Pen)
- [ ] Task: Inspector "gimbal active" affordance — the active mode's panel shows a
      "Gimbal active — drag to move/rotate/scale" line when a gimbal is shown for
      the current selection (TDD: pure `gimbal_affordance_label(tool, kind) ->
      Option<&str>` returns `Some` only when a gimbal is shown)
- [ ] Verification: no control lost from `tool_panel`; dragging a path vertex works
      under Select + Pen; inspector shows the gimbal affordance; old "Tools" window
      gone [checkpoint]

## Phase 6: Keyboard-shortcut parity + close-out

Goal: prove no drift between switcher, shortcut, hint, and inspector; document and
land.

Tasks:
- [ ] Task: Confirm mode switch via `S/G/R/X/P` updates both `ToolState` and the
      inspector body, and the hint line stays generated from `TOOL_DEFS` (TDD:
      switching each key selects the matching mode; extend/keep the existing
      `toolbar.rs` tests — `hint_line_covers_every_tool…`, `tool_defs_cover_all…` —
      green)
- [ ] Task: Update `fe-ui/src/panels/AGENTS.md` (§tool-inspector) +
      `fe-ui/src/node_manager/AGENTS.md` (§gimbal grabbable-wherever-shown) per the
      directory-doc convention; note deferred follow-ups (behavioral pick filter,
      snap-into-gesture, per-tool settings persistence)
- [ ] Task: Single end-of-track workspace sweep
      (`RUST_MIN_STACK=134217728 cargo test --workspace -j2`,
      `cargo clippy -- -D warnings`, `cargo fmt --check`); retro + archive per the
      track-per-feature workflow [checkpoint]
