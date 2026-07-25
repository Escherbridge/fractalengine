---
type: Track Retro
title: Retro — tool_inspector_ux (superseded by ui_shell_architecture)
tags: [retro, ux, tool_inspector_ux_20260719]
timestamp: 2026-07-24T00:00:00Z
resource: ./metadata.json
---

# Retro — tool_inspector_ux_20260719

Archived 2026-07-24, status **superseded** (by `ui_shell_architecture_20260724`).

## 1. What shipped before supersession

- **Phase 1 delivered @ `f223bfa`**: the left per-tool SidePanel
  (`fe-ui/src/panels/tool_inspector.rs`) — active tool as a legible UI mode,
  pure `panel_descriptor`/`gimbal_affordance_label`/`selection_summary`/
  `mode_button_fill` helpers (unit-tested), luminance-based active-mode
  emphasis (ui_ux.md §1). The metadata's 2026-07-19 "UNTRACKED/uncommitted"
  validation note was **stale** — this is committed.
- **FR-7 gimbal "grabbable wherever shown"** also delivered @ `f223bfa`
  (path_gimbal_drag ungated + marker-body-yield preserving FR-2/FR-1a).

## 2. Why superseded

**User verdict 2026-07-24 (live test): the always-open tool-descriptions
sidebar wastes real estate.** The panel-as-permanent-fixture model is replaced
by a **tooltip model** for tool descriptions. Phases 2–6 (per-tool
Use/Settings bodies, snapping/constraint/highlight pure models,
`tool_panel.rs` migration, keyboard-parity close-out) are **subsumed by
`ui_shell_architecture_20260724`** — its per-area tab managers +
right-sidebar inspector own that surface now.

## 3. Migration flag (load-bearing)

**pen_curve_tool's read-only corner-settings affordance currently lives in
`tool_inspector.rs`** (read-only by construction; the editable default lives
in `tool_panel.rs`). It **must migrate with the shell work** when
ui_shell_architecture retires/reshapes the left panel — do not drop it.

## 4. Carried-forward

- The two-selection-authority split and the no-fe-ui→fe-terrain-dep
  constraint carry into the shell track unchanged.
- The pure per-tool descriptor helpers in tool_inspector.rs are reusable as
  tooltip content sources.
