---
type: Track Retro
title: Retro — terrain_editor_overhaul (unified typed-selection editor + terrain proposals)
tags: [retro, ux, analytics, terrain_editor_overhaul_20260718]
timestamp: 2026-07-24T00:00:00Z
resource: ./metadata.json
---

# Retro — terrain_editor_overhaul_20260718

Archived 2026-07-24, status **done** — with one residual defect explicitly
transferred (see §3).

## 1. What shipped

- **FR-1..6 committed @ `320ebfe`**: `SelectionKind` typed read-model facade
  (`node_manager/dispatch.rs` + `selection.rs`) over the two codified selection
  authorities (NOT a merge, ui_ux.md §5); object-type-aware left-click op
  table; FR-3 gimbal-on-path and FR-4 path-delete cascade-stamp QA fixes;
  FR-5 NON-destructive PROPOSED terrain overlays
  (`fe-terrain/src/terrain_proposal.rs`, `layers/stack.rs`,
  `terrain_plugin.rs` — never writing `TerrainHeightField`, the analytics
  contract); FR-6 ruler.rs geometry math wired into the scale/geometry report.
- **The four in-app regression fixes landed @ `f223bfa`** — the metadata's
  2026-07-19 "LOCAL/uncommitted" validation note was **stale**; the fixes
  (GLB-select recursive-AABB-DFS, gimbal grab-wherever-shown +
  marker-body-yield, ribbon default 0.1, tool-inspector Phase 1 spillover) are
  committed.
- **CI green @ `f5d9673`** — the workspace sweep including this track's
  surface passes the full Lint (`clippy -D warnings`) + build + test matrix.

## 2. Verification evidence

- Commits: `320ebfe` (FR-1..6), `f223bfa` (regression fixes), CI success on
  `f5d9673` (run 30055712210).

## 3. DEFECT TRANSFERRED → ui_shell_architecture_20260724

**The residual acceptance gate (user in-app sign-off) ran 2026-07-24 and
FAILED: existing path points cannot be selected/manipulated from the
viewport.** Suspected selection-routing gap between viewport picks and the
Authority B (`PathEditorState`) edit mode — app-wide, not specific to this
track's dispatch table. **This defect is explicitly TRANSFERRED to
`ui_shell_architecture_20260724`**, which owns the viewport/selection routing
rework; it is also logged in
[ux_qa_review findings](../../ux_qa_review_20260714/findings.md) (2026-07-24
batch). Archiving here closes the build scope, not the transferred defect.

## 4. Carried-forward flags

- The two-authority selection split remains codified (ui_ux.md §5); the
  `SelectionKind` facade is the only sanctioned read surface over both.
- Terrain proposals are analytics-first PROPOSED overlays — any future
  "apply" pathway needs its own decision round before touching true terrain.
