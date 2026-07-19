---
type: Track Spec
title: UX Interaction Hardening — GPX controls, toolbar-as-context-menu, gimbal smoothing, selection highlighting, camera ground-clamp
description: User-directed 2026-07-18 slate hardening path editing controls, turning the top bar into the context selector for the inspector/left-sidebar region (core inspector gets an icon), smoothing gimbal drags, adding calm smooth selection highlighting for paths and objects, and hardening camera controls (terrain ground-clamp, scale-aware close-range defaults, easing)
tags: [feature, ux_interaction_hardening_20260718, pending]
timestamp: 2026-07-18T00:00:00Z
resource: ./metadata.json
---

# Specification: UX Interaction Hardening

**Track ID:** `ux_interaction_hardening_20260718`
**Priority:** P0 UX — user-driven, 2026-07-18
**Crates:** `fe-ui` (primary), `fe-renderer` (highlight rendering if
material-side; camera controller), `fe-terrain` (terrain height provider for
the camera clamp)

## Overview

Verbatim user ask (2026-07-18): *"hardening the GPX controls and updating the
tool placement; the top bar can act as a context menu for what shows in the
inspector left side bar area with the core inspector getting an icon as well;
we need clean gimbal interactions and better smoothing and highlighting for
track selection and honestly object selection in general."*

Second verbatim user ask (2026-07-18, same day): *"we need better camera
controls and sensible distance defaults to allow for the upclose project
planning work — right now the camera often clips below the terrain which would
be nice to avoid."*

Five surfaces, one theme: the shipped 2026-07 interaction layer works but feels
rough — controls forgive nothing, tool placement is ad-hoc, gimbal drags are
raw, selection state is legible only to people who already know it, and the
camera dives through the ground during close-up planning.

**Styleguide is binding:** `conductor/code_styleguides/ui_ux.md` — §1 (calm
chrome, color = state; the four status tiers own their meanings — selection
highlight is NOT an alarm tier), §5 (interaction semantics incl. the codified
selection-authority rule), §6 (feedback tiers), §7 (selection highlight is part
of the default viewport; every new overlay is gated or ephemeral).

**Known constraint (do not "fix" in passing):** viewport selection
(`NodeManager.selected`) and Paths-tab selection
(`PathEditorState.editing_track_id`) are **deliberately distinct** authorities
(ui_ux.md §5, codified-now). This track highlights both — visually
distinguishable — and never silently bridges them.

## Functional Requirements

- **FR-1 — GPX/path control hardening.** Robust editing controls on the
  shipped path editor (`fe-ui/src/panels/path_editor_card.rs`,
  `fe-ui/src/node_manager/path_*.rs`), building on the staged-Escape and
  petal-switch-reset behavior already landed. Concretely: every path-edit
  operation is cancel-safe (Escape semantics consistent across pen/edit/place
  modes per the staged ladder), no orphaned in-flight state on petal switch or
  tool switch mid-drag, undo-safe point manipulation, drag handles with
  forgiving hit areas (narrow-phase picking prior art from 906d9cc), and
  keyboard parity for nudge/delete on the selected point. Acceptance: an
  interrupted edit (Escape, tool switch, petal switch) never leaves ghost
  geometry or stale editor state.
- **FR-2 — Toolbar as context menu for the inspector region.** The top-bar
  tool buttons become the context selector for what renders in the
  inspector/left-sidebar region: activating a tool swaps the sidebar to that
  tool's panel, and the **core inspector gets its own toolbar icon** as a
  first-class context. Extend `TOOL_DEFS` in `fe-ui/src/panels/toolbar.rs` —
  it stays the single-source table (tool, icon, key, sidebar context); the
  existing exhaustive-coverage/unique-key tests extend with it. Sidebar
  (`fe-ui/src/panels/sidebar.rs`) renders from the active context; inspector
  (`fe-ui/src/panels/inspector.rs`) becomes one context among peers rather
  than the hardcoded default. Shortcut hints stay generated from `TOOL_DEFS`.
  Acceptance: every sidebar-region panel is reachable from exactly one toolbar
  icon; no panel renders outside its context.
- **FR-3 — Clean gimbal interactions.** Smoothing/damping on gimbal drags
  (`fe-ui/src/node_manager/gimbal_interaction.rs`): critically-damped (or
  exponential) interpolation toward the drag target instead of raw per-frame
  deltas, frame-rate independent (dt-based), with axis-constrained drags
  staying exactly on-axis. No overshoot, no residual drift after release; a
  toggle/constant keeps raw mode reachable for tests. Acceptance: gimbal drag
  at any frame rate lands the node exactly at the release target.
- **FR-4 — Selection highlighting, paths and objects.** Clear visual
  highlight for the selected path AND selected objects generally, with smooth
  in/out transitions (short ease, no pop): outline/rim or emissive-tint
  treatment reserving alarm colors per ui_ux.md §1 (selection ≠ warn/error
  tiers; pick from the selection-highlight slot in `theme.rs`, adding the
  constant if absent). The two selection authorities render distinguishably
  (viewport-selected node vs Paths-tab editing track), hover pre-highlight is
  subtler than selection, and highlight participates in the §7 default
  viewport set. Acceptance: a user can tell at a glance what is selected, in
  which context, without opening a panel.
- **FR-5 — Camera ground-clamp + scale-aware close-range defaults + easing.**
  (Evidence + design: `p2p_asset_streaming_20260718/memo.md` §C.) Four parts:
  1. *Terrain height provider.* fe-terrain retains each spawned chunk's final
     scaled elevation grid (downsampled ≤65×65) in a height-field resource
     (inserted in `spawn_chunk` at zero extra decode cost, removed on chunk
     despawn, cleared on assignment switch) exposing allocation-free bilinear
     `height_at(x, z) -> Option<f32>` in world units (O(1) per-tile sample
     over a bounded scan of resident tiles, ≤ terrain max_chunks). Crate boundary: fe-ui
     must not depend on fe-terrain, so the resource *type* lives renderer-side
     (fe-renderer), populated by fe-terrain — the proven CameraScaleSettings
     channel pattern, `Option<Res>` defensive and headless-safe. Rejected:
     per-frame sync tile fetch+decode (ms-scale, blocking), `Assets<Mesh>`
     vertex readback (VertexAttributeValues access gotcha), per-frame
     MeshRayCast (deliberately avoided per node_manager AGENTS.md).
  2. *Ground avoidance.* Applied once at the transform-recompute tail of
     `orbit_camera_system` (which runs after pan/fly/zoom mutate the
     controller, so every input path is covered): the orbit focus floors at
     the terrain surface (margin 0 — focusing a point ON the ground is
     legitimate), and the camera body keeps a `ground_margin(world_scale)`
     clearance primarily via a pitch clamp (`min_pitch_above_ground` —
     camera stays on the orbit sphere so zoom/pan speeds never diverge),
     with a transform-level raise as the steep-slope fallback. The margin is
     `2 m × world_scale` floored at 3× the fixed near plane, and the
     scale-aware min-distance floors at 2× near — near never scales, so both
     floors protect the near-plane-below-ground invariant. No height data ⇒
     no clamp (petals without terrain keep free flight).
  3. *Scale-aware close-range defaults.* `scaled_min_distance` mirroring the
     existing `scaled_max_distance` in `apply_camera_scale`;
     `controller.distance` clamped into `[scaled_min, scaled_max]` on scale
     change. **near = 0.01 and the pinned close-zoom behavior from
     `camera_focus_clip_20260716` are load-bearing — do not touch near, keep
     its pinned tests green.**
  4. *Camera easing.* Frame-rate-independent exponential decay toward
     `target_distance`/`target_focus` (inputs write targets; the system eases),
     no overshoot, raw-mode constant for tests — same damping doctrine as
     FR-3. The egui `is_using_pointer` gating and event draining stay exactly
     as-is.
  Acceptance: during close-up planning at any world scale the camera can no
  longer descend below visible terrain (+margin); zoom-in stops at a sensible
  scale-aware distance; motion eases without overshoot; all pinned camera
  tests stay green.

## Non-Functional Requirements

- **NFR-1** — No per-frame allocations in the smoothing/highlight hot paths;
  damping math unit-tested headless (no GPU).
- **NFR-2** — ui_ux.md pre-merge checklist passes for every fe-ui change.
- **NFR-3** — FR-5 height sampling allocates nothing per query: an O(1)
  bilinear sample per tile over a bounded scan of resident tiles (≤ terrain
  max_chunks = 256); the single-camera invariant (`query.single_mut()` sites)
  and `deactivate_foreign_cameras` are left undisturbed.

## Out of scope

- Unifying the two selection concepts (codified split, ui_ux.md §5).
- Road-builder input layer (owned by `road_builder_ux_20260716`).
- New measurement/graticule overlays (`hexon_scale_orchestration` Ph5–6).
- Multi-select / marquee selection (future track if the user asks).

## Open questions

- Whether sidebar contexts are exclusive or the inspector can pin alongside a
  tool panel (default: exclusive, per §7 calm defaults — user may override).
- Whether FR-4 highlight is material-side (fe-renderer) or overlay-side
  (fe-ui gizmo layer); decided by the Phase 4 spike.
