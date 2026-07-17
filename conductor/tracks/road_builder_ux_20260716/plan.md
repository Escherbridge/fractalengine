---
type: Implementation Plan
title: "Implementation Plan: Road Builder UX — C:S-inspired path input layer"
tags: [road_builder_ux_20260716]
resource: ./spec.md
---

# Implementation Plan: Road Builder UX (input layer)

## Overview

Five phases, pure-math first (snap/geometry is the risk core and is fully
unit-testable without egui/Bevy), then the small independent gpx_bridge
provenance slice, then tool + routing + persistence wiring, then visuals/UX,
then hardening + docs + the single end-of-track workspace sweep.

Per-task TDD (red → green → refactor) on pure helpers and queue-level
assertions; **full workspace test/clippy/fmt sweep runs ONCE at the end of the
track** (Phase 5), per the 2026-07-16 directive — per-task runs stay scoped to
the new/changed test files only.

Crate boundaries (binding): `fe-ui` + `fractalengine/src/gpx_bridge.rs` only.
No fe-api / fe-runtime / fe-database / fe-terrain / fe-sdk edits (parallel
tracks `mcp_scene_primitives_20260716`, `map_scale_authority_20260716`).

## Phase 1: Pure snap & segment geometry core (`node_manager/road_snap.rs`)

Goal: every geometric decision the builder makes exists as a tested pure
function over `[f32; 3]` (XZ-plane math, Y passthrough), mirroring `curve.rs`.

Tasks:
- [ ] Task: Angle snapping — `snap_angle(anchor, cursor, reference_dir, increment_deg, enabled) -> SnapOutcome` quantizing direction to 45°/90° multiples relative to the previous-segment tangent or world axes, distance-preserving; returns snapped point + active-angle metadata for the indicator (FR-3, AC-3.1..3.3) (TDD: write test, implement, refactor)
- [ ] Task: Vertex/endpoint snapping — candidate collection from per-track polylines (shape matches `TrackPickShape` data), squared-distance radius rejection, endpoint-outranks-interior-vertex ordering, exact `[f32; 3]` coordinate reuse (FR-5, AC-5.1 coordinate math + AC-5.2) (TDD)
- [ ] Task: Guideline candidates + resolution — extension + perpendicular lines from within-radius endpoints, cursor→line projection, guide-intersection points; single `resolve_snap(...)` combinator implementing the total order vertex > intersection > guide > angle > raw (FR-4, AC-4.1..4.4) (TDD)
- [ ] Task: Freeform decimation — min-arc-spacing filter over a drag sample stream, first/last always kept (FR-1 freeform, AC-1.3) (TDD)
- [ ] Task: Segment expansion + length — straight/curved/freeform → committed point list (curved via existing `curve::bezier` 3-point quadratic path, chain-anchor exclusion so no duplicate start point) + pending-length helper (chord vs sampled arc length) (FR-1/FR-7, AC-1.2 expansion + AC-7.1) (TDD)
- [ ] Task: Chain bookkeeping — `ChainLog` of per-segment appended counts; `undo_plan()` returning high→low `RemovePoint` indices + restored anchor; Esc two-stage transition model (FR-2, AC-2.2 index math + AC-2.3) (TDD)
- [ ] Task: World-scale seam — `metric_scale()` helper over the sanitized `PetalMapState.world_scale` mirror (≤0/non-finite → fallback), meters/km vs world-unit degrade path; single swap point for the future `map_scale_authority` accessor (FR-7, AC-7.2) (TDD)
- [ ] Verification: `cargo test -p fe-ui road_snap` green; coverage spot-check on the new module (>80%); confirm zero egui/Bevy-ECS types in `road_snap.rs` [checkpoint marker]

## Phase 2: `path_kind` provenance (`fractalengine/src/gpx_bridge.rs`)

Goal: designed-vs-recorded provenance lands independently and early (it is the
analytics-facing contract other work may start querying).

Tasks:
- [ ] Task: `PATH_KIND_KEY = "path_kind"` + `"designed"` written on the authored `CreateTrack` arm (road builder, pen auto-create, manual "New Path" all flow through it) (FR-9, AC-9.1) (TDD: property-set builder test first)
- [ ] Task: `"recorded"` added to the GPX-import track draft property list (pure `GpxNodeDraft` mapping test) (FR-9, AC-9.2) (TDD)
- [ ] Task: Document absence⇒recorded default + the BI filter implication (`WHERE properties.path_kind == 'designed'`) in `fractalengine/src/AGENTS.md` §gpx; note fe-api import parity as out-of-scope-correct
- [ ] Verification: scoped `cargo test -p fractalengine gpx` green; grep confirms `gpx_type: "track"` writes unchanged [checkpoint marker]

## Phase 3: Tool, state machine, routing, persistence wiring

Goal: a working (invisible-ghost) road builder — clicks commit real tracks
through the existing PathOp seam.

Tasks:
- [ ] Task: `Tool::Road` in `panels/toolbar.rs` + hotkey `B` in `shortcuts.rs` (egui `wants_keyboard_input` gating as-is); gimbal handler early-return extended to `Tool::Road` (mirrors Pen) (FR-8) (TDD: state-transition tests where pure)
- [ ] Task: `ClickPriority::RoadPlace` in `node_manager/router.rs`, ranked adjacent to `PathPlace`; arbiter tests extended (FR-8, AC-8.2) (TDD)
- [ ] Task: `RoadBuilderState` resource + pure placement state machine (Idle → AnchorSet → [ControlSet] → commit; freeform Dragging; Esc/Backspace transitions per `ChainLog`) — transitions as pure functions, resource is a thin holder (FR-1/FR-2, AC-2.1/AC-2.3) (TDD)
- [ ] Task: `road_builder_interaction.rs` system — arbiter claim, Y=0 ray-plane resolve, `resolve_snap` application, commit → existing `UiAction`s (`PathAppendPoint` / `PathAppendShape{points}` / `PathRemovePoint`); registered in the `node_manager` `.chain()` before `viewport_pick` per §input-router (FR-1/FR-10) (TDD on the pure commit-builder; system smoke via arbiter tests)
- [ ] Task: First-commit auto-create — `road-track:N` correlation ids, multi-point pending stash, deferred flush on `NodeCreated` echo + `DbResult::Error` cleanup in `verse_manager/db_results` (beside the pen's seam, not replacing it); default name "Road N" (FR-10, AC-10.2) (TDD)
- [ ] Task: Scripted queue-level acceptance tests — AC-1.1 (two clicks → CreateTrack + 2 AppendPoints), AC-1.2 (curve expansion counts), AC-2.1 (chain continues, no duplicate anchor), AC-2.2 (undo removes exactly the last segment), AC-5.1 (bitwise endpoint reuse), AC-9.3 (composition: designed provenance), AC-10.1 (existing op variants only) (TDD — these are the track's contract tests)
- [ ] Verification: scoped `cargo test -p fe-ui` for the new modules green; manual sanity in-app: place a 3-segment chain, restart app, track persists and ribbon renders [checkpoint marker]

## Phase 4: Ghost preview, guidelines, readout, panel UX

Goal: the C:S feel — everything the user sees during placement.

Tasks:
- [ ] Task: Gizmo ghost polyline (straight line / live-sampled Bezier / freeform trail) with valid/invalid coloring from a pure validity classifier (no petal, zero-length, non-finite) (FR-6, AC-6.1) (TDD on classifier)
- [ ] Task: Guideline + snap-indicator rendering — active guide lines, highlighted snap-target vertex, angle tick + degree label (FR-3/FR-4/FR-6 visuals)
- [ ] Task: Live metric length readout — cursor-tethered egui overlay + Tools-panel mirror, `format_distance_m` + `metric_scale` seam, world-unit degrade, chain running total in panel (FR-7) (TDD on formatting helpers)
- [ ] Task: Tools-panel Road section in `panels/tool_panel.rs` — compact mode buttons (Straight/Curved/Freeform), angle-snap toggle + 45°/90° increment, snap-to-paths + guidelines toggles, snap radius / curve samples / freeform spacing fields (FR-8) (TDD: pending-action/state plumbing tests per tool_panel conventions)
- [ ] Task: In-tool shortcuts — `1`/`2`/`3` modes, `A` angle-snap toggle, `Alt`-hold snap suspend, `Esc`/`Backspace` routing (egui-gated) (FR-2/FR-3/FR-8, AC-8.1) (TDD on the pure shortcut→state mapping)
- [ ] Verification: manual in-app pass (user-gated, AC-6.2): ghost coloring, guides appear/disappear with radius, snap indicators, metric readout vs known map distance, Alt suspend; screenshots for the retro [checkpoint marker]

## Phase 5: Hardening, docs, single workspace sweep

Goal: edge cases closed, "why" documented, one integrated green sweep.

Tasks:
- [ ] Task: Edge cases — petal switch mid-chain clears state; pending-create double-click guard; snap radius vs degenerate world_scale; large-track candidate cap sanity (NFR-2); no `unwrap()` in new prod paths audit
- [ ] Task: Docs — new §road-builder in `fe-ui/src/node_manager/AGENTS.md` (design rationale, snap resolution order, D-3 coordinate-sharing limitation feeding `procedural_roads`, router registration); pointer lines from new source files; `fe-ui/src/AGENTS.md` §path-editor note for the road pending-create seam
- [ ] Task: Full workspace sweep ONCE — `cargo test` + `cargo clippy -- -D warnings` + `cargo fmt --check` (RUST_MIN_STACK=64MB / -j4 if surrealdb-core recompiles); fix-forward until green
- [ ] Task: Track close-out — `metadata.json` status update, retro notes (incl. any OQ resolutions), flag `procedural_roads` follow-on inputs (junction inference needs, coordinate-equal endpoint contract)
- [ ] Verification: sweep evidence recorded; manual verification plan outcomes confirmed by user; checkpoint commit + git note per workflow.md [checkpoint marker]

## Dependency & sequencing notes

- Phase 1 and Phase 2 are independent — can interleave; Phase 3 depends on
  both; Phase 4 depends on 3; Phase 5 last.
- Soft dependency: `map_scale_authority_20260716` (parallel, not landed) —
  only the `metric_scale()` seam ever touches it; do NOT block on it.
- Landed foundations consumed as-is: `path_interaction_20260716`
  (`TrackPickShape`, segment measurement, `format_distance_m`),
  `gpx_stamp_persistence_20260716` (stamp persistence), pen auto-create
  correlation seam, `curve::bezier`.
