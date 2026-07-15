---
type: Track Spec
title: Path/Node Two-Way Binding Hardening — Single Source of Truth + DeleteNode
tags: [feature, gpx, database, ui, sync, architecture, path_node_binding_hardening_20260712]
timestamp: 2026-07-12T00:00:00Z
resource: ./metadata.json
---

# Specification: Path/Node Two-Way Binding Hardening

**Track ID:** `path_node_binding_hardening_20260712`
**Crates:** `fe-database` (DeleteNode + change feed), `fe-runtime`
(EntityType/messages), `fractalengine` (gpx_bridge), `fe-ui`
(verse_manager, panels, gis query)

## Problem (user, 2026-07-12)

The GPX path editor's persistence and delete are broken. Two confirmed bugs:

1. **Create/edit did not persist** — authored tracks silently failed to save.
2. **Delete left orphans** — deleting a track left the DB node, its
   `gis.track.name` property (so the track **reappears** in the Paths tab),
   its waypoint nodes, its **left-hand node-panel entry**, and the GIS cache
   all alive. Delete cascaded to almost nothing.

The user's requirement: **true 2-way data binding — a 1:1 map between live
path/track state, the persisted DB state, and the UI panels** (both the GIS
Paths tab AND the left-hand node/hierarchy panel), consistent with the query
engine.

## Root cause (architecture trace, 2026-07-12)

A track is a plain `node` row tagged by flat props (`gpx_type="track"`,
`gis.track.name`, `gpx_points` inline JSON). There are **three independent
"which tracks exist" answers with nothing keeping them in sync**:
DB rows (queried on `gis.track.name`), the left panel's in-memory
`VerseManager.verses[].nodes`, and render's `TrackRouteMap`+`GpxTrackLine`.
No single source of truth drives the others.

- **BUG 1:** writes are optimistic fire-and-forget (`db_tx.send().ok()`);
  `PathEditStatus` is set to success *before* the DB confirms
  (`gpx_bridge.rs:714`); and track-identity properties are set via a second
  async hop correlated by a **non-unique `(petal_id, name)`** key that
  `advance_path_edits` and `advance_gpx_imports` both consume from the same
  `NodeCreated` stream → silent drop on name collision → node exists but
  isn't a track. `AppendPoint`'s read-modify-write races and clobbers points.
- **BUG 2:** **no `DeleteNode` primitive exists** — `EntityType` is only
  `Verse|Fractal|Petal` (`messages.rs:83-87`); only `DeleteNodeProperty`
  exists. `DeleteTrack` (`gpx_bridge.rs:550-568`) clears two props + despawns
  the line, missing the node row, `gis.track.name` (→ reappears), waypoints,
  left panel, and GIS cache. The DB emits `SceneChange::NodeAdded/
  PropertyChanged` but no `NodeRemoved`, and the panels don't subscribe.

## Functional Requirements

- **FR-1 DeleteNode primitive (fe-database + fe-runtime):** add
  `DbCommand::DeleteNode { node_id }` and a `delete_node_handler` (mirror
  `create_node_handler`, `crud.rs:160`) that deletes the `node` row **and
  cascades** to child waypoints (`WHERE gpx_track_id = $node_id`), using the
  same `.check()` + matched-rows assertion as `entity_property.rs:47-49`
  (no silent no-op). Emit `DbResult::NodeDeleted { node_id }` and a new
  `SceneChange::NodeRemoved { node_id, petal_id }` (mirror `NodeAdded`,
  `lib.rs:328`).
- **FR-2 DeleteTrack uses it + full cascade (gpx_bridge):** rewrite
  `PathOp::DeleteTrack` (`gpx_bridge.rs:550`) to send `DeleteNode` instead of
  the two `DeleteNodeProperty` calls. On the `NodeDeleted` result, ensure
  every sink is reached: DB row (FR-1), `TrackRouteMap` entry, `GpxTrackLine`
  entity, waypoint entities, GIS Paths cache, and the left panel (FR-3).
- **FR-3 Left panel + Paths tab subscribe to node lifecycle:** add a
  `DbResult::NodeDeleted` arm in `verse_manager/db_results.rs` (next to
  `EntityDeleted`, `:268`) doing `petal.nodes.retain(|n| n.id != node_id)` —
  fixes the left-panel orphan generically for **all** node deletes, not just
  tracks. Make the Paths tab / GIS panel re-run `track_query` on any
  `NodeCreated`/`NodeDeleted`/track-property change (a "tracks dirty" flag in
  `apply_db_results` that `path_editor_card` drains) — remove the manual
  Refresh button as the *only* sync path (`path_editor_card.rs:50`).
- **FR-4 Confirmed persistence (BUG 1):** replace the `(petal_id, name)`
  correlation for authored track creation with a **request/correlation id**
  threaded through `CreateNode`/`NodeCreated` so `advance_path_edits` and
  `advance_gpx_imports` can't steal each other's results. Gate the success
  `PathEditStatus` on the actual `NodePropertySet` results (surfacing the
  `entity_property.rs:48` "matched no node" error) instead of setting success
  optimistically at `:714`.
- **FR-5 AppendPoint lost-update fix:** serialize the read-modify-write per
  `track_node_id` — keep an authoritative in-flight point list per track and
  apply appends to it, flushing one `SetNodeProperty` per settle, instead of
  re-issuing `GetNodeProperties` per op (kills the race at `:569-575`→`:720`).
- **FR-6 Single source of truth (projections):** treat the DB node row as the
  sole source of truth; `TrackRouteMap` + `GpxTrackLine` + panel caches are
  **projections rebuilt from `SceneChange`/`DbResult` events**. Generalize
  `advance_path_materialization` (`gpx_bridge.rs:911`, already rebuilds render
  state from DB on petal-load) so any node create/delete/property-change event
  drives render + panel updates through **one** path, not per-op hand-updates
  of a subset of sinks.

## Approach note (LIVE queries — optional, evaluate)

If SurrealDB LIVE queries are viable in the embedded SurrealKV setup, a
`LIVE SELECT … WHERE gis.track.name != NONE` feeding one "tracks changed"
event would collapse FR-3/FR-4 into a single subscription and is the most
robust long-term shape. But the event fan-out via the existing
`DbResult`/`SceneChange` crossbeam channels is the **lower-risk change that
fits the current 7-thread architecture** — default to that unless LIVE
queries prove clean.

## Out of scope

The reverted splat coverage-fill work (separate track
`splat_hexon_bake_20260712`); phase-2 viewport editing itself (cherry-picked
back from `archive/splat-coverage-experiment-20260712` *after* this hardening
lands — this track is the foundation it sits on); a full generic ORM/live-
binding layer for every node type beyond what FR-6's projection model needs.

## Verification

Unit tests: FR-1 DeleteNode cascade (node + child waypoints gone, matched-rows
assert fires on missing node). Integration/behavioral: FR-4 create→confirm
round-trip persists across an app restart; FR-2/FR-3 delete removes the track
from DB, Paths tab, left node panel, and viewport in one action with no
manual refresh. In-app (user): create a track → restart app → track still
there (persist). Delete a track → it vanishes from the Paths tab, the
left-hand node panel, and the viewport, and does NOT reappear on refresh (the
`gis.track.name` orphan is gone). These two are the exact failures the user
reported.

## Provenance

Architecture trace 2026-07-12 (conversation). Reverted phase-2 editor on
branch `archive/splat-coverage-experiment-20260712`
(cherry-pick after this lands, excluding the splat changes).
