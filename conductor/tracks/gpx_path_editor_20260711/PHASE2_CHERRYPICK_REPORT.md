---
type: report
track: gpx_path_editor_20260711
title: Phase-2 GPX path editor cherry-pick from archive
date: 2026-07-12
status: complete-pending-build
---

# Phase-2 GPX Path Editor Cherry-Pick Report

Grafted the reverted "phase-2" GPX path editor (cursor-driven point
place/drag/annotate) from `archive/splat-coverage-experiment-20260712`
(commits 9839094, 4be8ec4) onto the current hardened tree. Archive content
was read READ-ONLY via `git show`/`git diff` — no checkout/merge/cherry-pick.
Edit-only; NO cargo/build/test/check/clippy was run.

## Headline scoping decision

The archive's phase-2 diff bundled TWO features:

1. **FR-6/7/9 — cursor point editing** (click-to-place, drag-to-move,
   Shift/Alt+click annotate). This is the user's reported bug ("fails to
   place points for gpx on cursor"). **PORTED.**
2. **FR-10 — per-track line styling** (color / line_style / visibility).
   **DROPPED** — it depends on `fe_terrain::terrain_plugin::GpxTrackStyle`,
   which lives ONLY in the archived `terrain_plugin.rs` (on the EXCLUDE
   list). Current `render_gpx_tracks` is
   `Query<(Entity, &GpxTrackLine), Without<Mesh3d>>` — no style component.
   Porting FR-10 would require editing the excluded `terrain_plugin.rs` and
   reviving excluded ribbon/render code. Not the reported bug.

## Files CHANGED (my edits this session)

### NEW files
| File | Source | Notes |
| --- | --- | --- |
| `fe-ui/src/node_manager/path_point_interaction.rs` | archive wholesale, adapted | ~230 lines. Marker sync + place/drag/annotate. Condensed the archive's 5-line `//!` block to a terse pointer per styleguide; WHY moved to `node_manager/AGENTS.md §path-points`. No `unwrap()/expect()` in production path (only `map_or`/`unwrap_or` non-panicking fallbacks, kept as-is). |

### MODIFIED files (editor hunks only; Track-1 hardening preserved)
| File | Hunks taken | Hunks skipped |
| --- | --- | --- |
| `fe-ui/src/node_manager/mod.rs` | `mod path_point_interaction;`, `path_edit_capturing: bool` field, `init_resource::<PathPointDrag>()`, 2 systems in `.chain()` BEFORE `viewport_pick::handle_viewport_click` | — |
| `fe-ui/src/node_manager/viewport_pick.rs` | `if manager.path_edit_capturing { return; }` guard after `is_dragging()` | — |
| `fe-ui/src/node_manager/AGENTS.md` | new `§path-points` section + submodule entry (the WHY) | — |
| `fe-ui/src/path_ops.rs` | `PathOp::MovePoint` variant | — |
| `fe-ui/src/actions/mod.rs` | `UiAction::PathMovePoint` variant + dispatch arm | — |
| `fe-ui/src/actions/path.rs` | `move_point()` handler + 2 unit tests | — |
| `fe-ui/src/gis/mod.rs` | `PathEditorState` annotate buffers (`annotating_index`, `annotate_{title,body,color}_buf`) + `open_annotate_form`/`close_annotate_form`; `stop_editing` closes form | FR-10 style buffers (`style_color_buf`, `style_line_style`, `style_visible`); `GisResultRow.visible` field; style resets in `start_editing` |
| `fe-ui/src/panels/path_editor_card.rs` | Inline annotate form (`render_annotate_form`) replacing the v1 placeholder-title flow; hint label; remove-closes-form | FR-10 `track_style_section` call; per-row visibility checkbox; `to_toggle_visible`; `TRACK_VISIBLE_KEY` import |
| `fe-ui/src/panels/gis_panel.rs` | Auto-populate track list on first Paths view (editor-support) | FR-10 `visible: None` in `run_query` (not needed — `GisResultRow.visible` field dropped) |
| `fractalengine/src/gpx_bridge.rs` | `PendingPathRead::MovePoint` variant + `drain_path_ops` fast-path arm + `advance_path_edits` fallback arm | ALL FR-10: `GpxTrackStyle` import, `hex_to_linear_rgba`, `read_track_style`, `PendingStyleRefresh`, `refresh_track_style_on_change`, `style` param threading through `spawn_track_route`/`persist_and_render_points`/`advance_path_materialization`, 4 FR-10 tests |

### Files NOT changed (archive changed them; I intentionally did not)
- `fe-ui/src/panels/track_style_card.rs` — NOT created (FR-10 only).
- `fe-ui/src/panels/mod.rs` — `mod track_style_card;` NOT added (FR-10 only).
- `fe-ui/src/actions/node_props.rs` — `TRACK_COLOR/LINE_STYLE/VISIBLE_KEY` NOT added (FR-10 only).
- `fe-ui/src/gis/query.rs` — `track_visible` column NOT added (FR-10 only; `GisResultRow` unchanged so all construction sites stay valid).
- `fractalengine/src/main.rs` — NO change. The archive's +2 lines were the FR-10 `PendingStyleRefresh` init + `refresh_track_style_on_change` system. MovePoint flows through the already-registered `drain_path_ops`/`advance_path_edits`, so no new registration is needed.

## Reconciliation detail: gpx_bridge.rs vs Track 1

**KEY FINDING:** Track 1's FR-4/FR-5/FR-6 rewrite is UNCOMMITTED in the
working tree — `git show HEAD:fractalengine/src/gpx_bridge.rs` has ZERO
`in_flight_points`. I reconciled against the WORKING-TREE version (the
hardened one), as instructed, NOT against HEAD. `git diff HEAD` on
gpx_bridge shows ~295 insertions = Track 1's uncommitted work (~240) plus
my MovePoint plumbing (~55). Verified: 15 `in_flight_points` additions in
the diff are Track 1's (pre-existing), 0 style/GpxTrackStyle references
added by me.

**MovePoint was fed INTO Track 1's `in_flight_points` path, not the
archive's older logic.** The archive's MovePoint handler assumed the
pre-FR-5 single-path structure (append/move done inline in
`advance_path_edits`). The current tree's Append/Remove fast-path through an
authoritative `in_flight_points` buffer in `drain_path_ops`. I mirrored the
current `RemovePoint` exactly:

- `drain_path_ops`: if `in_flight_points` has the track, mutate the point in
  place (`point.position = [... as f64]`, timestamp preserved), clone, call
  `persist_and_render_points`. Else issue `GetNodeProperties` and queue a
  `PendingPathRead::MovePoint` (fallback DB-read path).
- `advance_path_edits` (`NodePropertiesLoaded` arm): fallback handler mirrors
  the `RemovePoint` fallback — reads-queue cleanup, `points.get_mut(index)`
  reposition, `in_flight_points.insert` to keep the authoritative buffer in
  sync (so a later AppendPoint doesn't rebuild from stale state), then
  `persist_and_render_points`.
- `persist_and_render_points` signature was NOT changed (no `style` param) —
  the archive's style threading was FR-10 and dropped.

## Reconciliation detail: path_editor_card.rs vs Track 1

Track 1's change to this file is the "manual override, not the only sync
path" comment on the Refresh button (lines ~50-52). **Kept intact.** The
phase-2 layer added on top: replaced the v1 `PathAnnotatePoint`
placeholder-title flow with `open_annotate_form(index)` + an inline
title/body/color `render_annotate_form`; added the "Click terrain to add ·
drag markers to move · Shift/Alt+click a marker to annotate" hint; made
remove close the form if it targeted the removed point. Dropped the FR-10
`track_style_section` call and per-row visibility checkbox.

## Archive code deliberately dropped
- **All splat coverage-fill / terrain / ribbon** — never in scope; not touched.
- **All FR-10 line styling** — see headline decision (excluded terrain dep).
- **No `unwrap()/expect()` introduced.** The ported interaction file uses
  only non-panicking `map_or`/`unwrap_or`/`let-else`/`?`. `move_point`'s
  out-of-range case uses `bevy::log::warn!` (archive's own choice, kept).

## Wiring integrity verified (static, not compiled)
- `path_point_interaction.rs` imports all resolve in current tree:
  `ViewportRect`, `SpawnedNodeMarker`(n/a), `UiManager::push_action`,
  `PathEditorState`, `fe_renderer::camera::OrbitCameraController`.
- Resources registered: `PathEditorState` (plugin.rs:285), `ViewportRect`
  (plugin.rs:273), `UiManager`, and new `PathPointDrag` (node_manager mod.rs).
- `PathOp::MovePoint` field order matches the bridge match pattern.
- `points` is `let mut` in the `NodePropertiesLoaded` arm → `get_mut(index)` valid.
- 0 dangling FR-10 symbol references in any kept file (grep-verified).
- `track_style_card` referenced nowhere (grep-verified) — safe to not create.

## Quarantine / exclusion compliance
- Did NOT Edit/Write any of: `fe-api/*`, `fe-database/src/lib.rs`,
  `conductor/.conductor_session_log`, `.codex/`, `fe-terrain/src/splat/*`,
  `fe-terrain/src/terrain_plugin.rs`, `fe-terrain/src/lod_ring.rs`,
  `fe-terrain/src/gpx/*`, any `conductor/tracks/*`. These appear in
  `git status` only because the shared worktree already had unrelated
  uncommitted changes from parallel tracks BEFORE this session.

## UNSURE / flag for coordinator review before build
1. **FR-10 exclusion is a judgment call.** If the coordinator WANTS track
   styling too, `GpxTrackStyle` must first be added to the current
   `terrain_plugin.rs` (which is currently excluded) — then the dropped
   gpx_bridge/track_style_card/node_props/query hunks can be layered back. As
   scoped, styling is out; point placement (the reported bug) is in.
2. **Y=0 placement plane.** Empty-click placement uses the `y=0` world plane
   (archive behavior). If the active terrain's petal origin is not at y=0,
   placed points land on y=0, not on the terrain surface. Archive shipped it
   this way; no terrain raycast in the excluded-dep-free path. Acceptable for
   the bug fix, but worth a follow-up if surface-snapping is desired.
3. **System ordering across sets.** `handle_path_point_interaction` runs in
   `UiSet::Selection` and `push_action`s that are drained by
   `process_ui_actions` (a different system). This is the standard queue
   pattern used everywhere in fe-ui, so intent applies next drain — same as
   the existing "Append from cursor" button. No new ordering hazard, but
   noting it since the archive relied on the same implicit timing.
4. **gpx_bridge `git diff HEAD` looks huge (~295 lines)** because Track 1's
   FR-5 work is uncommitted in the working tree, not because I wrote 295
   lines. My net addition is ~55 lines (the MovePoint plumbing). Coordinator
   should confirm Track 1's uncommitted state is expected before committing.

CHERRYPICK_COMPLETE
