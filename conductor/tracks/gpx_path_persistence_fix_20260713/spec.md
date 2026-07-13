---
type: Track Spec
title: GPX Path Persistence Fix — Read-Back gpx_points on Re-Select
tags: [bugfix, gpx, ui, persistence, gpx_path_persistence_fix_20260713]
timestamp: 2026-07-13T00:00:00Z
resource: ./metadata.json
---

# Specification: GPX Path Persistence Fix

**Track ID:** `gpx_path_persistence_fix_20260713`
**Crates:** `fe-ui` (editor buffer + read-back wiring)
**Work unit:** W1 (ultrapilot, branch `up/w1-persist-20260713`)

## Problem (user, 2026-07-13)

GPX paths do not reload when you leave and return to a petal. The path
geometry IS persisted in the DB (the `gpx_points` JSON array on the track
node) but never read back into the editor buffer, so re-selecting a track
shows an empty point list.

## Root Cause (deliberate v1 limitation)

`PathEditorState::start_editing()` in `fe-ui/src/gis/mod.rs` calls
`self.points.clear()`. The editor buffer was write-only by design in v1
("no read-back of persisted gpx_points"). Track selection also bypasses the
action pipeline — it calls `start_editing` directly inside the egui panel,
where no `db_sender` is in scope — so the load cannot be issued from
`start_editing` itself.

## Functional Requirements

- **FR-1:** Re-selecting an existing track repopulates the editor point list
  from the persisted `gpx_points` property.
- **FR-2:** No new DbCommand/DbResult message types — reuse the existing
  `GetNodeProperties` → `NodePropertiesLoaded` round-trip.
- **FR-3:** The read-back must not be stomped by the inspector's own
  `GetNodeProperties` reply (broadcast, no correlation id) — gate on
  `editing_track_id` + a `points_pending` flag.

## Design

1. New `UiAction::PathSelectTrack { track_node_id }`; the track-row click in
   `path_editor_card.rs` emits it instead of calling `start_editing` directly.
2. Routed in `actions/mod.rs` (where `db_sender` exists): `start_editing`,
   set `points_pending = true`, send `GetNodeProperties`.
3. `db_results.rs` handles `NodePropertiesLoaded` gated on
   `editing_track_id == node_id && points_pending`: decode `gpx_points` into
   `PathEditorState.points`, clear the flag.
4. fe-ui-local decoder maps `[x,y,z,t]` → `PathPointRow` (no persistence I/O
   in fe-ui — local decode only).

## Verification

Create path → leave petal → return → points still shown. Full `fe-ui` test
suite green.
