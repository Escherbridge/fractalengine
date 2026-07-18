---
type: Implementation Plan
title: "Implementation Plan: UX Interaction Hardening"
tags: [ux_interaction_hardening_20260718]
resource: ./spec.md
---

# Implementation Plan: UX Interaction Hardening

## Overview

Four independent surfaces ordered structural-first: the toolbar/context work
(FR-2) reshapes where panels live, so it lands before control hardening
touches those panels. Damping and highlighting are parallel-safe. All fe-ui
changes pass the ui_ux.md pre-merge checklist; full workspace sweep runs ONCE
at the end per the standing test-execution policy. In-app verification is
user-gated (established convention for UX tracks).

## Phase 1: Toolbar as context menu (FR-2)

Goal: TOOL_DEFS drives both tool activation and sidebar-region context.

- [ ] Task: Extend `ToolDef` with a sidebar-context discriminant + add the core-inspector entry (icon, shortcut); extend the exhaustive-coverage and unique-key tests first (TDD)
- [ ] Task: Sidebar renders from the active toolbar context; inspector becomes a peer context instead of the hardcoded default (TDD: context→panel mapping table test)
- [ ] Task: Context persistence semantics — active context survives selection changes; tool-switch mid-edit routes through the FR-1 cancel path (TDD)
- [ ] Verification: every sidebar panel reachable from exactly one icon; shortcut hint line regenerates correctly [checkpoint]

## Phase 2: GPX/path control hardening (FR-1)

Goal: interrupted edits never leave ghost geometry or stale state.

- [ ] Task: Inventory in-flight editor state (pen, point-drag, place modes) + write failing tests for Escape / tool-switch / petal-switch interruption at each stage (TDD red)
- [ ] Task: Cancel-safe teardown for every in-flight operation; extend the staged-Escape ladder to cover tool-switch and petal-switch (TDD green)
- [ ] Task: Drag-handle forgiveness — hit-area padding on point handles reusing the narrow-phase picking prior art; keyboard nudge/delete parity on the selected point (TDD)
- [ ] Task: Undo-safe point manipulation — point edits batch into single undo units (TDD)
- [ ] Verification: interruption matrix (3 interrupts × each mode) green; no orphaned entities after any row [checkpoint]

## Phase 3: Gimbal smoothing (FR-3)

Goal: damped, frame-rate-independent drags that land exactly on target.

- [ ] Task: Extract drag-target math into a pure damping module (dt-based critically-damped interpolation); unit tests for convergence, no-overshoot, dt-independence (TDD)
- [ ] Task: Wire damping into `gimbal_interaction.rs` drag application; axis-constrained drags stay exactly on-axis; raw-mode constant for tests (TDD)
- [ ] Verification: simulated drag at 30/60/240 Hz dt sequences lands identical final transform [checkpoint]

## Phase 4: Selection highlighting (FR-4)

Goal: at-a-glance selection legibility for both authorities, calm palette.

- [ ] Task: Spike — material-side (fe-renderer) vs overlay-side (fe-ui gizmo) highlight; record decision + add selection-highlight constant to theme.rs if absent (respect §1 tier ownership)
- [ ] Task: Selected-object highlight with smooth ease in/out, driven by `NodeManager.selected`; hover pre-highlight subtler than selection (TDD on transition/state math, headless)
- [ ] Task: Editing-track highlight driven by `PathEditorState.editing_track_id`, visually distinct from object selection; both may coexist (TDD)
- [ ] Verification: both authorities highlighted simultaneously and distinguishably; no alarm-tier colors used [checkpoint]

## Phase 5: Close-out

- [ ] Task: Single end-of-track workspace sweep (test/clippy/fmt) per standing directive
- [ ] Task: ui_ux.md pre-merge checklist pass recorded; in-app verification handed to the user (user-gated)
- [ ] Task: Retro + archive per track-per-feature workflow
