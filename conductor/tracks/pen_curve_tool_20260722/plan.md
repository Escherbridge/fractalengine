---
type: Implementation Plan
title: "Implementation Plan: Pen Curve Tool — Illustrator-Style Bezier + Corner Settings"
tags: [pen_curve_tool_20260722]
resource: ./spec.md
---

# Implementation Plan: Pen Curve Tool

## Overview

Sequenced data-model-up so nothing renders a half-built anchor: the per-anchor
bezier fields + wire format land first (Phase 1, pure + backward-compat, no
behavior change), then curve rendering reusing the existing de Casteljau
tessellation (Phase 2), then the ops/actions/bridge persistence (Phase 3), then the
Pen click-vs-drag gesture (Phase 4), then viewport handle editing (Phase 5), then
the **corner settings UI** — the user's headline ask — (Phase 6), then docs +
a single end-of-track sweep (Phase 7). TDD throughout; one workspace lint/test
sweep at the very end per standing policy
(`RUST_MIN_STACK=134217728 cargo test --workspace -j2`,
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`).

> **Provenance.** Design produced by the `pen-curve-tool-design` workflow
> (4 grounded code-readers → design → adversarial critique, run
> `wf_a7826e5e-6dc`). Critique verdict: **sound-with-fixes** — all 3 sacred
> invariants verified RESPECTED. The two critique **must-fixes** are folded into
> Phases 4 and 6 (marked ⚠). Key scope finding: the bezier→ribbon tessellation
> already exists (`node_manager/curve.rs`), so Phase 2 is small.

> **Status 2026-07-22.** Design-only. NOT implemented, NOT committed. Open
> decisions in `spec.md` await user ratification before Phase 1 starts.

## Phase 1: Anchor data model + wire format (pure, TDD-first)

Goal: per-anchor handles + corner classification travel end-to-end through the
positional data pipeline with legacy compatibility, no behavior change yet.

Tasks:
- [ ] Task: Add `CornerKind { #[default] Corner, Smooth, Symmetric }` +
      `handle_in`/`handle_out: Option<[f32;3]>` (relative meter offsets) +
      `corner`/`smoothness: f32` to `PathPointRow` (`fe-ui/src/gis/mod.rs:120-124`);
      update every constructor/test that builds a `PathPointRow`.
- [ ] Task: Mirror the fields onto `TimestampedRoutePoint`
      (`fe-terrain/src/iot/animation.rs:47-50`) with a fe-terrain-local corner enum;
      handle the `#[cfg(feature="render")]` twin.
- [ ] Task: Extend encoder `route_points_to_json` (`gpx_bridge.rs:242-251`) to emit
      the compact 4-slot form for handle-less corners and the 12-slot
      `[x,y,z,t,in3,out3,corner_code,smoothness]` form otherwise (never shorten an
      existing longer row; never emit an intermediate length).
- [ ] Task: Extend BOTH decoders (`json_to_route_points` `gpx_bridge.rs:256-275`,
      `decode_gpx_points` `query.rs:171-190`) to read slots 4-11 with
      `.get()`/`unwrap_or` defaults (Corner / None / 0.0), plus the partial-row
      normalization guard (Open Q 6).
- [ ] TDD: write failing tests first — legacy 4-slot decode ⇒ Corner/no-handle;
      12-slot bezier round-trip; mixed-length array; extend
      `fe-database/tests/gpx_path_persistence_test.rs:169-200`. [checkpoint]

## Phase 2: Curve rendering + pick (reuse existing tessellation)

Goal: curved segments render while straight paths stay byte-identical, all in the
raw-meter frame.

Tasks:
- [ ] Task: Add `flatten_route(points, samples_per_seg) -> Vec<[f32;3]>` in new
      `fe-terrain/src/mesh/curve.rs` (small anchor-aware de Casteljau, mirroring
      `node_manager/curve.rs::push_cubic:157`): cubic
      `[P_i, P_i+out_i, P_{i+1}+in_{i+1}, P_{i+1}]`; both-handles-`None` ⇒ straight
      passthrough (zero added points). Only subdivide handle-carrying segments.
- [ ] Task: Call `flatten_route` before the centroid-recenter + `track_mesh` in
      `render_gpx_tracks` (`terrain_plugin.rs:718-722`); `track_mesh`
      (`mesh/track.rs:48`) stays polyline-based, width stays meter-frame, Transform
      stays scale-1.
- [ ] Task: Build `TrackPickShape` / `track_pick_shape`
      (`path_segment_interaction.rs:26-33`, `gpx_bridge.rs:524-555`) from the SAME
      flattened polyline so clicks hit the visible curve.
- [ ] TDD: `flatten_route` — straight passthrough == identical polyline; single
      cubic sample count; symmetric handles; assert **no `world_scale` leakage**;
      assert an all-corner track meshes byte-identically. [checkpoint]

## Phase 3: Ops, actions & bridge persistence

Goal: the UI has a persisted path to append smooth anchors and mutate
handles/corner-type.

Tasks:
- [ ] Task: Add `PathOp` variants `AppendSmoothPoint` / `SetAnchorHandles` /
      `SetAnchorCorner` (`fe-ui/src/path_ops.rs:11-55`); keep `AppendPoint`/
      `MovePoint` unchanged (`MovePoint` rides relative handles for free).
- [ ] Task: Add matching `UiAction` variants + handlers beside `PathAppendPoint`
      (`actions/mod.rs:593`) and `append_point` (`actions/path.rs:94-109`):
      `append_smooth_point` pushes a full `PathPointRow` + queues the op.
- [ ] Task: Extend bridge `advance_path_edits` (`gpx_bridge.rs:1246`) +
      `persist_and_render_points` (`gpx_bridge.rs:1700`) to read-modify-write the
      new slots.
- [ ] TDD: bridge round-trip — append smooth, set handles, reload, assert handles
      survive; assert `MovePoint` moves handles with the anchor. [checkpoint]

## Phase 4: Pen click-vs-drag gesture

Goal: click = corner anchor; press-drag = symmetric smooth anchor; Alt-drag =
corner with an independent out-handle.

Tasks:
- [ ] Task: Wire `PointerPhase::phase()` (`router.rs:113`, currently dead code) into
      `handle_path_point_interaction`.
- [ ] Task: Add a `PenHandleDrag { anchor_pos, alt_held }` resource (mirror
      `PathPointDrag`, `path_point_interaction.rs:70-74`); **restructure** the
      live empty-ground branch (`:360-402`) from press-time append to
      Press(capture) / Hold(track out-handle on y=0) / Release(threshold decision).
- [ ] Task: Release below threshold (~0.15 m) ⇒ existing `append_point` (corner);
      above, no Alt ⇒ `PathAppendSmoothPoint` (symmetric `handle_out=drag`,
      `handle_in=-drag`); above + Alt ⇒ corner with out-handle only.
- [ ] Task ⚠ (critique must-fix 2): thread `handle_out`/`handle_in`/`corner_code`
      through `PendingPenCreate` + the deferred `NodeCreated` echo so a press-drag
      that STARTS a new track keeps its handles (else a new curved track silently
      starts sharp).
- [ ] Task: add `ToolPanelState.pen_new_anchor_kind` default (beside `pen_mode`,
      `tool_panel.rs:165`).
- [ ] TDD: pure unit tests on the threshold / symmetry / Alt decision (mirroring
      `path_gimbal_drag.rs:277-351`); assert first-anchor-of-new-track keeps
      handles; assert Select-yield + `PathPlace`-claim ordering unchanged.
      [checkpoint]

## Phase 5: Viewport handle editing

Goal: grab and drag an anchor's bezier handles with collinear/symmetry discipline.

Tasks:
- [ ] Task: Add `HitTarget::PathHandle { idx, side }` +
      `Operation::MoveHandle { idx, side, position }` to `dispatch.rs` (`:17-70`);
      resolve in `resolve_operation`/`resolve_gimbal` (`:134-176`). Leave the
      lone-vertex/segment "grab wherever shown" arms untouched.
- [ ] Task: Spawn handle-marker entities analogous to `PathPointMarker`
      (`path_point_interaction.rs:19`); pick via along-ray + `PICK_RADIUS`; claim
      AHEAD of vertex-body/gimbal (respect `MARKER_BODY_RADIUS`,
      `path_gimbal_drag.rs:33,218-229`).
- [ ] Task: On handle drag — Symmetric keeps the opposite handle equal+opposite,
      Smooth keeps direction/frees length, Alt breaks to Corner; commit via
      `SetAnchorHandles`/`SetAnchorCorner`.
- [ ] TDD: collinear enforcement + Alt-break transitions; handle-marker claim
      ordering vs the vertex-body yield. [checkpoint]

## Phase 6: Corner settings UI (Paths card) + inspector affordance

Goal: the user's headline ask — a per-anchor smoothness slider + corner-type
toggle, split-safe in Authority B.

Tasks:
- [ ] Task: In `path_editor_card.rs::render_edit_view` add a "Selected vertex"
      sub-card gated on `selected_point.is_some()` (below `:378-384`): a
      Corner / Smooth / Symmetric segmented toggle + a smoothness `0.0..=1.0` slider.
- [ ] Task: Slider auto-derives collinear symmetric handles from the neighbor
      tangent (`normalize(P_{i+1}−P_{i-1})`, length `= smoothness·k·min-gap`, `k≈1/3`,
      reusing `curve.rs:47-77` tension math); use the `render_style_controls`
      deferred-push idiom (`:559-576`) — live buffer edit, persist on release via
      `SetAnchorCorner`/`SetAnchorHandles`. Define the slider-vs-manual-handle
      readback rule (Open Q 5).
- [ ] Task ⚠ (critique must-fix 1): add the read-only per-anchor affordance to
      `tool_inspector.rs` with **NO** `&mut`/signature change, and put the EDITABLE
      Pen tool-level default (`pen_new_anchor_kind`) in `tool_panel.rs` (which owns
      `&mut ToolPanelState` and already renders editable pen widgets, `:503-582`) —
      NOT in `tool_inspector` (read-only by construction).
- [ ] TDD: verify all corner-settings mutations target `PathEditorState` + a queued
      `UiAction` and NEVER `NodeManager.selected` (split compliance,
      `ui_ux.md §5`); smoothness 0/1 produce sharp/round anchors. [checkpoint]

## Phase 7: Docs & format note + close-out

Goal: record the why/design in directory-level `AGENTS.md` (not verbose inline
comments) and land with one clean sweep.

Tasks:
- [ ] Task: Extend `fe-ui/src/node_manager/AGENTS.md` §pen-tool (anchor model,
      gesture split, dispatch handle variants) and note the read-only vs editable
      split for the Pen affordance.
- [ ] Task: Add a §curve section to `fe-terrain/src/mesh/AGENTS.md`
      (`flatten_route` + straight-passthrough + RDP note) and note handles on
      `TimestampedRoutePoint` in the iot doc.
- [ ] Task: Update the `fractalengine/src/AGENTS.md` `gpx_points` section documenting
      the 4/12-slot encoding + backward compat.
- [ ] Task: Leave only one-line pointers in code per repo convention; run the full
      workspace test/clippy/fmt sweep ONCE at the end; retro + archive per the
      track-per-feature workflow. [checkpoint]
