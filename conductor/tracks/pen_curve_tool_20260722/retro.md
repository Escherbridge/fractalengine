---
type: Track Retro
title: "Pen Curve Tool Phase 7 Close-Out — Phases 1-7 LANDED + PUSHED"
tags: [pen_curve_tool_20260722, retro, phase-7, bezier, curve-tool]
timestamp: 2026-07-24T00:00:00Z
resource: ./spec.md
---

# Phase 7 Close-Out Retro: Pen Curve Tool

## What Shipped

**Phases 1-7 COMPLETE, ALL LANDED + PUSHED to origin/main** (commits 09e539b feat + ab08b68 conductor + 190188f/4d718a1/b12b646):

- **Phase 1:** Per-anchor bezier data model (`handle_in`/`handle_out: Option<[f32;3]>` + `corner: CornerKind` + `smoothness: f32`) on `PathPointRow` and render twin `TimestampedRoutePoint`; extended to `TimestampedRoutePoint` fe-terrain-local mirror.
- **Phase 2:** Anchor-aware `flatten_route` in `fe-terrain/src/mesh/curve.rs` (de Casteljau duplicate, ~15 lines); wired into `render_gpx_tracks` + `TrackPickShape` — all-corner tracks byte-identical legacy mesh.
- **Phase 3:** `PathOp` + `UiAction` variants `AppendSmoothPoint` / `SetAnchorHandles` / `SetAnchorCorner`; bridge `advance_path_edits` + `persist_and_render_points` round-trip (append smooth, set handles, reload, handles survive).
- **Phase 4:** Pen click-vs-drag gesture via `PointerPhase::phase()` + `PenHandleDrag` resource; restructured press path from press-time append to release-time decision: threshold ~0.15 m ⇒ corner (today's append) vs above ⇒ symmetric smooth (no Alt) or combination anchor (Alt: `handle_out`=final, `handle_in`=frozen-at-Alt-press or None); threaded first-anchor handles through `PendingPenCreate` + `NodeCreated` echo.
- **Phase 5:** Handle editing via `HitTarget::PathHandle` + `Operation::MoveHandle`; handle-marker spawning + along-ray pick (priority handle > vertex > gimbal); collinear/symmetry enforcement on drag (Symmetric keeps opposite equal+opposite, Smooth frees length, Alt breaks to Corner).
- **Phase 6:** Corner settings UI in `path_editor_card.rs::render_edit_view`: Corner / Smooth / Symmetric toggle + smoothness `0.0..=1.0` slider (auto-derives collinear handles from neighbor tangent, length = smoothness·k·min-gap); read-only affordance in `tool_inspector.rs`; editable Pen default `pen_new_anchor_kind` in `tool_panel.rs`; deferred-push via `render_style_controls` idiom.
- **Phase 7:** Directory-level docs in `fe-ui/src/node_manager/AGENTS.md` (pen-tool §), `fe-terrain/src/mesh/AGENTS.md` (curve §), `fractalengine/src/AGENTS.md` (gpx_points 4/12-slot encoding).

## 14-Finding Adversarial Fix Pass

Post-implementation review swarm identified 14 confirmed defects; all fixed + cluster-verified before push:

- **Stored smoothness readback:** Geometry readback |handle_out|/(k·min_gap) at EVERY commit site (pen release, handle drag, slider toggle) — no silent stale reads.
- **Corner toggle non-destructive:** Reclassify-only on symmetric handles; re-derive from broken state at readback or handle-less at 0.5 floor.
- **Gesture cancellation:** Escape/track-switch/petal-change mid-drag drops the anchor instead of auto-creating spurious track.
- **Same-frame press+release:** Resolves as Release, not stranded-drag hover.
- **Bridge drain_queued_appends:** Hoists appends past in-place ops, stops at RemovePoint — fixes silent append stranding under seed-dedup.

## Ratified Decisions (spec §Ratified decisions, 2026-07-24)

- **Q2 — Smoothness:** Auto-derived collinear handle LENGTH (no arc geometry; fillet radius deferred).
- **Q5 — Slider readback:** GREY OUT on manual asymmetry (handles break symmetry → slider disabled + readback hint; re-enable via corner-type toggle re-derive).
- **Q7 — Pick priority:** Handle-marker > vertex-marker > gimbal-arm (along-ray pick + `PICK_RADIUS=0.7`; `MARKER_BODY_RADIUS=0.7` yield untouched).
- **Q8 — Alt-drag placing:** Full-Illustrator combination anchor: `handle_out`=final drag vec; `handle_in`=frozen symmetric value when Alt first pressed (`None` if Alt held from start); classification `Corner`. No retroactive mutation of prior anchor.
- **CI stance:** Track latest stable rustc; no lint-job pin. Run `cargo +<stable> clippy --workspace --all-targets -- -D warnings` locally before push.

## Verification State

**FULL-WORKSPACE SWEEP GREEN, CLIPPY CLEAN, FMT CLEAN:**
- 1801 tests / 0 fail / 77 suites.
- `clippy -D warnings` clean against rustc 1.97.1.
- `cargo fmt --check` clean.

**NOT IN-APP VERIFIED** — in-app re-verify is user-gated per `conductor/workflow.md`. User in-app test 2026-07-24 found selection/manipulation unreachable (app-wide root cause, NOT pen-specific — routed to `ui_shell_architecture_20260724` track, FR-2 landed there, terrain crash un-root-caused).

## Follow-Ups Routed Out

- **Selection/manipulation crash** → `ui_shell_architecture_20260724` track (FR-2 already landed; terrain-tools crash still un-root-caused).
- **In-app re-verify + archival** → user-gated.

## Lessons & Gotchas

- **Toolchain drift:** Local rustc 1.94 vs CI floating stable 1.97.1. Always run `cargo +<CI-stable> clippy/fmt` before declaring done; CI will catch latent pre-existing warnings if local toolchain lags.
- **Run completion sweep on FINAL tree:** A fmt failure slipped because a sweep ran before a later cherry-pick merged into the tree. Run the full test/clippy/fmt sweep ONCE at the very end, after all cherry-picks/merges land.
