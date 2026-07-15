---
type: Track Spec
title: Pen Auto-Create Track — first Pen click with no track creates one
tags: [feature, ui, gpx, pen, usability, pen_autocreate_track_20260713]
timestamp: 2026-07-13T00:00:00Z
resource: ./metadata.json
---

# Specification: Pen Auto-Create Track

**Track ID:** `pen_autocreate_track_20260713`
**Crates:** `fe-ui`

## Vision / Why (user, 2026-07-13)

The Pen tool silently does nothing unless a track is already selected/being
edited. User report: "pen does not seem to create pointer events" — verified
cause: `handle_path_point_interaction` early-returns when
`PathEditorState.editing_track_id` is `None` (`path_point_interaction.rs`
~line 161). Pressing P and clicking the viewport with no track in edit mode is
a silent no-op — the requirement to first create/select a track in the Paths
tab is invisible. User later confirmed: "now only works with a track
selected." **Decision: auto-create a track on the first Pen click when none is
being edited** (fluid drawing, matches normal drawing-tool behavior).

This is a usability gap exposed by (not caused by) `input_router_20260713`.

## Functional Requirements

- **FR-1:** When the Pen tool is active and a left-click lands in the viewport
  with `editing_track_id == None`, auto-create a new track instead of no-op:
  queue `UiAction::PathCreateTrack { petal_id, name }` (default name, e.g.
  `"Path N"` or a timestamp-free incrementing name) for the active petal, and
  **stash the intended first-point world position** as pending.
- **FR-2:** Track creation is async — the new track's `node_id` returns later
  via `DbResult::NodeCreated { id, petal_id, .. }`
  (`fe-ui/src/verse_manager/db_results.rs:141`). On that event, if a pending
  first-point exists for a just-auto-created track: call
  `PathEditorState::start_editing(new_id)` and flush the pending point as
  `UiAction::PathAppendPoint { track_node_id: new_id, position }`. Guard so this
  only fires for the pen-auto-create case, not every `NodeCreated` (e.g. a
  `pending_pen_first_point: Option<[f32;3]>` flag on `PathEditorState` plus a
  marker that the last create was pen-initiated).
- **FR-3:** After the first auto-created point lands, subsequent Pen clicks
  append normally (editing_track_id is now `Some`) — no special-casing beyond
  the first click. The state machine returns to the existing path.
- **FR-4:** Auto-create must target the **active petal**. Source the petal id
  the same way the Paths tab create-button does (it has a `petal_id` in scope;
  see `render_track_list` in `path_editor_card.rs` and the `PathCreateTrack`
  dispatch in `actions/mod.rs:398`). If no active petal, fall back to the
  existing no-op (can't create a track with nowhere to put it) — optionally a
  one-line hint.
- **FR-5:** Remove the temporary `PEN-DIAG` `eprintln!` diagnostics added to
  `path_point_interaction.rs` during this investigation.

## Relevant Files

- `fe-ui/src/node_manager/path_point_interaction.rs` — the pen-append branch
  (~line 268-283); the `editing_track_id == None` early-return (~line 161);
  the temp `PEN-DIAG` eprintlns to remove.
- `fe-ui/src/gis/mod.rs` — `PathEditorState` (add `pending_pen_first_point`);
  `start_editing` (~line 165).
- `fe-ui/src/verse_manager/db_results.rs` — `DbResult::NodeCreated` handler
  (~line 141): where the new track's node_id arrives; flush the pending point
  here (or in a dedicated system reading NodeCreated).
- `fe-ui/src/actions/mod.rs` — `UiAction::PathCreateTrack` (~92, 398),
  `PathAppendPoint` (~96, 411).
- `fe-ui/src/actions/path.rs` — `create_track` (~45), `append_point` (~68).

## Constraints

- Bevy 0.18, `default-features = false`. Never `rustfmt`. No concurrent cargo.
  fe-ui builds `-j1`. No quarantine contact (`fe-api/*`,
  `fe-database/src/lib.rs`).
- The append cannot be synchronous — the node_id doesn't exist until the DB
  round-trips. FR-2's deferred-flush is mandatory; do not try to fabricate a
  node_id.
