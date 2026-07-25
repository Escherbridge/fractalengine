---
type: Track Retro
title: Retro — ux_interaction_hardening (FR-5 delivered; FR-1..4 absorbed by ui_shell_architecture)
tags: [retro, ux, ux_interaction_hardening_20260718]
timestamp: 2026-07-24T00:00:00Z
resource: ./metadata.json
---

# Retro — ux_interaction_hardening_20260718

Archived 2026-07-24, status **superseded** (by `ui_shell_architecture_20260724`).

## 1. What shipped

- **FR-5 camera hardening delivered @ `320ebfe`**: `scaled_min_distance`,
  `min_pitch_above_ground`, `height_at`, `clamp_above_ground` in
  `fe-renderer/src/camera.rs`, all with green pinned tests (documented in
  `fe-renderer/src/AGENTS.md`). Close-up planning work can no longer dive the
  camera below terrain; the camera_focus_clip near=0.01 contract is kept.

## 2. What was absorbed (not dropped)

The remaining four FRs are **ALL ABSORBED into
`ui_shell_architecture_20260724`**:

- **FR-1** — cancel-safe / forgiving / undo-safe path-point edits on the
  shipped path editor.
- **FR-2** — top-bar-as-context-selector for the inspector/left-sidebar
  region (its per-area tab managers are the successor design).
- **FR-3** — damped, frame-rate-independent gimbal drags — **explicitly
  absorbed into the shell track's pointer-ops manager scope, not dropped**.
- **FR-4** — calm, smooth selection highlighting for paths AND objects (the
  two selection authorities rendered distinguishably, per ui_ux.md §5).

## 3. Verification evidence

- `320ebfe` (FR-5 + tests); CI green @ `f5d9673`. The 2026-07-19 validation
  pass confirmed FR-1..4 had zero implementation evidence — nothing is lost
  by superseding; the unbuilt scope moves wholesale.

## 4. Carried-forward constraints

- The two-selection-concept split stays codified (ui_ux.md §5) — highlight
  both authorities distinctly, never unify storage.
- `TOOL_DEFS` remains the single-source tool table for whatever the shell
  track builds on the top bar.
