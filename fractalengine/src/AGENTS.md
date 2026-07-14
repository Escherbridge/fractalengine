# fractalengine (GUI binary) — main wiring rationale

`src/main.rs` boots the desktop GUI: it spins up the DB / network / sync / API
threads, builds the Bevy `App` (DefaultPlugins + egui + renderer + fe-ui +
terrain + webview), and bridges the background threads to Bevy resources.
`terrain_bridge.rs` and `asset_bridge.rs` are the per-frame drain systems that
turn fe-ui's queued ops into real side effects (the UI crate has no DB / blob /
filesystem access by design).

## §durability

SurrealKV's disk-flush cadence is controlled by the `SURREAL_DATASTORE_SYNC_DATA`
environment variable (read by surrealdb-core at datastore open). Valid values:
`never` | `every` | a duration string `>100ms`. Both binaries default it to
`every` **only when unset**, so an operator can still override it, and set it at
the very top of `main` — before the DB thread opens the datastore.

Note the earlier `SURREAL_SYNC_DATA="true"` block was a no-op: that variable
name is not read by surrealdb-core, and `"true"` is not a valid value for the
real `SURREAL_DATASTORE_SYNC_DATA` knob (it would reject at startup), so setting
the correct name to `every` is the actual durability fix.

## §assets

Two asset paths converge here:

1. **API asset endpoints.** `fe_api::ApiConfig.blob_store` receives a clone of
   the real `FsBlobStore` handle (`blob_store_for_api`). Without it every asset
   endpoint returns 503 on the GUI binary even though the DB reader is wired
   (see `fe-api/AGENTS.md` §assets). One content-addressed store is shared by
   the DB thread, the sync thread, Bevy's `blob://` asset source, the API
   thread, and the download bridge.

2. **UI-initiated downloads.** The inspector's asset card pushes
   `UiAction::DownloadNodeAsset { node_id }`, which fe-ui drains into
   `asset_ops::PendingAssetOps` (fe-ui never touches the blob store or the
   filesystem). `asset_bridge::drain_asset_ops` runs each frame and, per op:
   - `resolve_asset` resolves the node in `VerseManager`, reads its
     `asset_path` (`blob://{hash}.{ext}`), parses the hex hash, and asks the
     `BlobStoreHandle` for the on-disk path — the path is **never** built from
     user input, only from the content-address store keyed by the parsed hash
     (same guarantee as `BlobAssetReader`). A node with `has_asset == true` but
     no cached `asset_path` gets a distinct, clearer error than a node with no
     asset at all (previously both collapsed to the confusing "node has no
     asset" message even when `has_asset` was true — see 2026-07-11 fix).
   - on success, `prompt_and_copy` opens a native `rfd` save dialog (suggested
     filename = sanitized node name + real asset extension, default dir
     `dirs::download_dir()`) and copies the blob to the user's chosen
     destination on confirm. **The dialog lives bridge-side, not in fe-ui** —
     only the bridge has the resolved node name + extension needed to build a
     sane suggested filename, and `UiAction::DownloadNodeAsset` /
     `asset_ops::AssetOp::Download` only carry `node_id` (kept unchanged
     deliberately: threading a `dest` field through would require editing
     `fe-ui/src/actions/mod.rs`'s `UiAction` enum + dispatch match arm, which
     is out of scope for a bridge-only change). User-cancelled dialog is a
     silent no-op: `PendingAssetOps` still drains the op, but `status` is left
     untouched (no toast, no error).
   - writes the outcome (`saved_path` or `error`) into fe-ui's
     `AssetDownloadStatus` resource. A fe-ui system surfaces it as a toast, and
     (as of the 2026-07-11 fix) the Asset card also renders it as a **persistent
     status row** scoped to the currently-selected node, so success/failure
     doesn't disappear with the toast.

   **INTEGRATION_REQUEST (2026-07-11, asset_download_fix track):**
   `asset_card_section`'s signature grew a `status: &AssetDownloadStatus`
   param (fe-ui/src/panels/asset_card.rs). Wiring it in requires a resource
   that currently isn't threaded to the inspector render call chain at all —
   `right_inspector` (inspector.rs), its caller in `panels/mod.rs`, and
   ultimately the Bevy system in `fe-ui/src/plugin.rs` that calls
   `panels::mod`'s entry point (which needs a new `Res<AssetDownloadStatus>`
   system param) all need one argument/param added each. All three files are
   outside this track's owned-file set (`plugin.rs` explicitly belongs to
   another worker), so this wiring is left for the coordinator/owning worker
   to apply — it's a mechanical thread-through, not a design decision.

## §gpx

`gpx_pipeline_20260711` track — the import→persist→render→serve chain for
GPX files. `gpx_bridge.rs` is the persist step; render (spawning
`GpxTrackLine` entities + populating `TrackRouteMap`) and the `gpx_track`
layer wiring are **out of scope here** (per `metadata.json`, that's W-B's
`petal_binding` territory) and remain unwired after this pass — see
"Residuals" below.

**Current state of the pieces (as found, before this pass):**

- `fe-terrain/src/gpx/{parser,stats,convert,export}.rs` — GPX 1.0/1.1
  parsing (`parse_gpx_bytes` → `GpxData`), stats (`compute_stats` →
  `TrackStats` with `total_distance_m`, `elevation_gain_m`,
  `elevation_loss_m`, `duration: Option<f64>`, `avg_speed_kmh`,
  `max_speed_kmh`, `bounding_box: BoundingBox`), and a **fe-terrain-local**
  `DbCommand` struct (`convert.rs`, fields `petal_id, name, position,
  parent_node_id, properties`) used by `gpx_to_scene_commands`. This local
  struct is a dead end for any real persistence path: it is NOT
  `fe_runtime::messages::DbCommand` and nothing translates its
  `parent_node_id`/`properties` fields into the real command set (see next
  bullet) — `gpx_bridge.rs` does its own mapping instead of reusing it.
- `fe-api/src/gpx.rs`'s `import_gpx` HTTP handler calls
  `gpx_to_scene_commands` and then creates nodes via the *real*
  `fe_runtime::messages::DbCommand::CreateNode { petal_id, name, position,
  correlation_id }` (FR-4: `correlation_id: Option<String>`, `None` here)
  — which has no `properties` or `parent_node_id` field. The handler only
  ever forwards `cmd.name`/`cmd.position`; `cmd.properties` and
  `cmd.parent_node_id` are silently dropped. **This means the shipped HTTP
  import endpoint has never actually written `gpx_type`/stats properties or
  any hierarchy** — confirmed by reading `create_node_handler`
  (`fe-database/src/handlers/crud.rs`): it never populates `properties`, and
  the `node` schema has no `parent_node_id` column at all (grep across
  `fe-database/src` returns zero hits outside the DTO-shape code in
  `fe-api/src/gpx.rs`'s export path, which reads a field that was never
  written). `gpx_bridge.rs` fixes the properties gap for the UI-driven path
  via `SetNodeProperty`; the missing schema column is a residual (below).
- `fe-terrain/src/terrain_plugin.rs`'s `GpxTrackLine` component
  (`{ track_node_id: String }`) + `render_gpx_tracks` system + `TrackRouteMap`
  resource (`fe-terrain/src/iot/animation.rs`, keyed by `track_node_id`,
  values carry **timestamped** route points) are fully implemented but
  **nothing in the codebase ever spawns a `GpxTrackLine` entity or populates
  `TrackRouteMap`** (grep for `GpxTrackLine` outside `terrain_plugin.rs` /
  `petal_binding.rs` returns nothing). Wiring that spawn (reading
  `gpx_type == "track"` nodes for the active petal, and populating
  `TrackRouteMap` from per-point data with timestamps for the animation) is
  the FR-3 render half of this track and is **not done by this worker**.
- `fe-api/src/gis.rs`'s `GET .../gis/tracks` reads nodes via
  `properties.gpx_type = 'track'` and deserializes cached stats straight off
  flat property keys (`row_to_track`: `total_distance_m`,
  `elevation_gain_m`, `elevation_loss_m`, `duration_s`, `avg_speed_kmh`,
  `max_speed_kmh`, `bounding_box`). `GET .../gis/nodes` surfaces
  `gis.annotation.title/body/color` (flat dotted keys, per
  `fe-database/src/AGENTS.md` §gis) via `extract_annotation`. These two
  read shapes are the **contract** `gpx_bridge.rs` writes against.

**`gpx_bridge.rs` design (the persist step, FR-2):**

- Contract with fe-ui (mirrors `asset_ops` exactly, per spec): drains
  `fe_ui::gpx_ops::PendingGpxOps` (`GpxOp::ImportFile { petal_id, path:
  PathBuf }`), writes `fe_ui::gpx_ops::GpxImportStatus` for the UI to
  surface. **fe-ui's `gpx_ops` module did not exist when this file was
  written** (a parallel worker owns it) — the exact field names of
  `GpxImportStatus` (`petal_id: Option<String>, track_count: u32,
  waypoint_count: u32, error: Option<String>`, mirroring
  `asset_ops::AssetDownloadStatus`'s shape) are this worker's best guess and
  may need reconciling once the real module lands (see INTEGRATION_REQUEST
  below).
- **One track node per imported file.** All `<trk>` elements/segments in
  the file are merged into a single synthesized track node (`compute_stats`
  already aggregates across every track in the `GpxData`), positioned at
  the **first trackpoint** in document order (not the bounding-box center
  the HTTP endpoint uses — a petal with a real terrain origin needs the
  track's own position to align with that origin, not an arbitrary
  bbox-center local origin). Node properties: `gpx_type = "track"` plus the
  six stat keys + `bounding_box` object, written via one `SetNodeProperty`
  call per key (flat keys — `CreateNode` has no `properties` field, see
  above). If the file has zero trackpoints, no track node is created and
  every `<wpt>` is persisted as a standalone waypoint instead (see below).
- **Waypoints as the track's "children".** Every `<wpt>` in the file is
  persisted as its own node with `gpx_type = "waypoint"` and
  `gis.annotation.title` set to the waypoint's name (or `Waypoint {n}` if
  unnamed) — matching the reserved annotation-key contract in
  `fe-database/src/AGENTS.md` §gis exactly, so these waypoints also surface
  through `GET .../gis/nodes`. **There is no `parent_node_id` column on the
  `node` table and no DbCommand to set one** (see above), so "child" is
  approximated with a custom flat property `gpx_track_id` = the track
  node's ID, set once the track's `CreateNode` result resolves. This is the
  best available linkage under "existing commands only" — a real
  parent/child relation would need a new schema column + `DbCommand`
  variant, which is out of scope (`fe-database/src/lib.rs` is quarantined).
  Standalone waypoints (no track in the file) get no `gpx_track_id`.
- **Correlating `CreateNode`'s fire-and-forget result.** `DbCommandSender`
  is fire-and-forget and `DbResult` is a broadcast Bevy `Message` with no
  request ID — unlike `ApiCommand::DbRequest`'s `PendingApiRequests`
  correlation (oneshot channels), a plain ECS system has no such mechanism.
  `gpx_bridge.rs` correlates by content: a `PendingGpxImports` resource
  keeps a `HashMap<(petal_id, name), VecDeque<...>>` of "what to do when the
  next matching `DbResult::NodeCreated` arrives" (set the track's stat
  properties + create its waypoints; set a waypoint's type/title/track-link
  properties). The DB thread's dispatch loop is a single sequential
  `loop { rx.recv() ... await ... send_result() }` (`fe-database/src/lib.rs`)
  with **no per-command concurrency**, so results for a given caller's
  commands arrive in the same relative order they were sent — the
  `VecDeque` per key correctly disambiguates duplicate names (e.g. two
  waypoints both literally named "Camp") in FIFO order.
- **FR-4 correlation id (closes the authored-track vs. import race).** The
  original `(petal_id, name)` content match had a residual risk: an authored
  `PathOp::CreateTrack` and a same-named GPX import created in the same frame
  window both consumed the one `DbResult::NodeCreated` stream, so either could
  steal the other's result. `DbCommand::CreateNode` / `DbResult::NodeCreated`
  now carry an optional `correlation_id: Option<String>` echoed unchanged by
  the DB dispatch (`fe-database/src/lib.rs`). The authored-`CreateTrack` path
  (`drain_path_ops`) keys `PendingPathEdits::creates` by the op's
  `correlation_id`, so `advance_path_edits` matches by id, never by tuple.
  **The id source depends on the caller** (HIGH-1/HIGH-2): the `PathOp::CreateTrack`
  now carries `correlation_id: Option<String>` — the fe-ui **Pen auto-create**
  supplies its own (`gis::next_pen_correlation_id`, `pen-track:{n}`) so fe-ui's
  own deferred flush can match the echoed id (see `fe-ui/src/AGENTS.md`
  §path-editor); the manual "New Path" button leaves it `None`, for which the
  bridge generates a `next_authored_track_correlation_id` (`authored-track:{n}`,
  an atomic counter — no new crate dep) exactly as before. Either way the command
  goes out with a `Some(id)`. The import/annotate paths send `correlation_id:
  None`; `advance_gpx_imports` ignores any `Some(_)` result and
  `advance_path_edits`'s annotate branch only handles `None` — so the two streams
  are partitioned and can never cross-consume. Duplicate-name import waypoints
  still FIFO-disambiguate via `(petal_id, name)` as before (that path is not the
  racing one). The id is optional/additive: every non-track `CreateNode` sender
  passes `None` and keeps the legacy content correlation.
- **Projection (petal terrain origin).** Per spec, points project through
  the *petal's* terrain origin, not an arbitrary bbox-center. The only
  already-resident state carrying a resolved terrain origin is
  `fe_terrain::petal_binding::ActivePetalTerrain` (inserted by
  `TerrainPlugin`, tracks whichever petal is currently the *active*
  viewport petal). `gpx_bridge.rs` reads it directly (no new DB round trip)
  when `active_terrain.petal_id` matches the import's target `petal_id`; if
  it doesn't match (importing into a non-active petal, or no terrain
  configured yet) it falls back to the bounding-box-center `Projection`,
  same as `fe-api/src/gpx.rs`'s HTTP endpoint. A dedicated
  `DbCommand::GetPetalTerrain` round trip was considered and rejected: its
  `DbResult::PetalTerrainLoaded` is already consumed by
  `terrain_bridge::bridge_petal_terrain`, which unconditionally reassigns
  the *active* petal's terrain — reusing it here for a possibly-different
  target petal would hijack the viewport's active terrain as a side effect.

**FR-3 point-count materialization (`node_placement_z_axis_20260713`).**
`advance_path_materialization` maps a track's `gpx_points` length to a render
kind via the pure `materialization_kind(len)`: `0` → `None` (render nothing),
`1` → `Node` (a plain, visible, selectable node), `≥2` → `Line` (the
`GpxTrackLine` polyline). A single point spawns `spawn_single_point_node` — a
small unlit `Sphere` tagged with both `SinglePointTrackNode { track_node_id }`
(reconcile key, mirrors `GpxTrackLine`) and `fe_ui::plugin::SpawnedNodeMarker`
(so the glb-mesh-picking AABB test selects it; `Mesh3d` auto-supplies the
`Aabb`). `petal_id` for the marker is best-effort from `ActivePetalTerrain`
(same assumption as AnnotatePoint / `resolve_projection` — selection only needs
`node_id`). The `None`/`Node`/`Line` reconcile is factored into ONE helper,
`reconcile_track_render`, called by BOTH render paths so their spawn decisions
can never diverge:

- **Petal-load path** — `advance_path_materialization` calls it on each
  `NodePropertiesLoaded` with `force_line_redraw = false` (a matching line from
  earlier in the same batch is left alone); `NodeDeleted` despawns both a line
  and a single-point node for the id.
- **Live-edit path** — `persist_and_render_points` (driven by Pen append /
  remove / move / Ctrl-height, via `drain_path_ops` and `advance_path_edits`)
  calls it with `force_line_redraw = true` (an existing `GpxTrackLine` is
  despawned + respawned to force `render_gpx_tracks`, which only rebuilds meshes
  `Without<Mesh3d>`, to redraw). This is HIGH-1's fix: previously the live path
  went through `spawn_track_route` alone, which only handled the `Line`/`None`
  cases and never touched the single-point node — so 0→1 didn't render live,
  2→1 vanished the track, and 1→2 leaked a stale node. All four now reconcile
  live: 0→1 spawns the node, 1→0 despawns it, 2→1 tears the line down + spawns
  the node, 1→2 despawns the node before spawning the line (no duplicate).

Because `advance_path_edits` reconciles to the post-edit count while
`advance_path_materialization` may also see the same seed-read
`NodePropertiesLoaded` (pre-edit count, differing by ≤1), the shared helper's
`is_none()` spawn guards + `force_line_redraw = false` on the petal-load side
keep the double-processing idempotent (neither double-spawns; deferred
`Commands` from one aren't visible to the other in-frame, but the count-kind
boundaries never collide on a "spawn-absent" decision). To thread the reconcile
into the live path, `drain_path_ops` / `advance_path_edits` gained the
`single_nodes: Query<(Entity, &SinglePointTrackNode)>`, `ResMut<Assets<Mesh>>`,
`ResMut<Assets<StandardMaterial>>`, and (for `drain_path_ops`)
`Res<ActivePetalTerrain>` params the reconcile needs; all four path systems live
in one `Update` tuple so Bevy serializes them on the shared `Assets`/`TrackRouteMap`
`ResMut` access (a scheduling constraint, not a conflict). `spawn_track_route`
now assumes `points.len() >= 2` (its only callers — the `Line` arm of the helper
and the import path — both guarantee it). The user's "z-axis" height edits (FR-1)
persist through the unchanged `[x, y, z, time]` `gpx_points` encoding — no format
change (FR-2).

**§track-styling (track_styling_20260713).** Per-track color / thickness /
visibility, edited in the Paths tab, persisted as `gis.track.*` node props
(`gis.track.color` = `#rrggbbaa` hex, `gis.track.width` = number,
`gis.track.visible` = bool). `style_from_properties(&Value) -> TrackStyle` (pure,
unit-tested) parses them field-by-field, each falling back to its default on
missing/invalid input (FR-4, never panics). `advance_path_materialization` now
also threads `ResMut<TrackStyleMap>` + `Res<DbCommandSender>`:

- On a track's `NodePropertiesLoaded`, it refreshes `TrackStyleMap[node_id]`
  from the style props BEFORE reconciling. If the style actually changed and a
  `GpxTrackLine` already exists, it despawns that line so the reconcile
  respawns it `Without<Mesh3d>` and `render_gpx_tracks` rebuilds the ribbon with
  the new color/width (its build is gated on `Without<Mesh3d>` — same
  force-redraw mechanism the point-edit path uses).
- A live style edit lands as `NodePropertySet { key }` (no property bag), so a
  `gis.track.*` key triggers a `GetNodeProperties` re-read → the arm above fires
  with the fresh values. Non-style keys don't trigger the round trip.
- `NodeDeleted` also drops the `TrackStyleMap` entry.

fe-ui side: `UiAction::PathSetStyle { track_node_id, color?, width?, visible? }`
writes the changed keys via `SetNodeProperty` directly (mirrors
`PathAssetApply`; `actions::path::style_property_writes` is the pure, tested
`(key,value)` builder — only `Some` fields write, so an untouched control never
clobbers a stored value). The Paths-tab edit view seeds its controls from
`PathEditorState::edited_track_style` (a fe-ui-local `TrackStyleFields` mirror —
fe-ui must not depend on fe-terrain), populated at the same
`NodePropertiesLoaded` seam that seeds `points`.

**§track-picking (in-app fix).** A rendered track ribbon
(`fe_terrain::terrain_plugin::GpxTrackLine` + `Mesh3d`) wasn't viewport-selectable:
the fe-ui picker only iterates `SpawnedNodeMarker`, which `render_gpx_tracks`
can't attach (fe-terrain must not depend on fe-ui). `tag_track_lines_selectable`
(this crate — it sees both `GpxTrackLine` and `fe_ui::plugin::SpawnedNodeMarker`)
closes the gap: it queries ribbons `(With<Mesh3d>, Without<SpawnedNodeMarker>)`
and inserts a `SpawnedNodeMarker { node_id: track_node_id, petal_id }`, sourcing
`petal_id` from `ActivePetalTerrain` (the same pattern `spawn_single_point_node`
uses). Idempotent — the `Without<SpawnedNodeMarker>` guard means it tags each
ribbon once, the frame after `render_gpx_tracks` adds its mesh. Registered in
the same `Update` tuple as `advance_path_materialization`. The fe-ui side
(`viewport_pick::open_track_on_select`) then turns that selection into a Paths-tab
open — see `fe-ui/src/node_manager/AGENTS.md` §track-picking.

**INTEGRATION_REQUEST (gpx_pipeline_20260711, coordinator-owned `main.rs`):**
Register two new systems and one resource, mirroring the asset-bridge
wiring: `app.init_resource::<gpx_bridge::PendingGpxImports>();` and
`app.add_systems(Update, (gpx_bridge::drain_gpx_ops,
gpx_bridge::advance_gpx_imports));` — placed after `TerrainPlugin` is added
(needs `ActivePetalTerrain` to exist) and after fe-ui's `gpx_ops` module
lands (needs `PendingGpxOps`/`GpxImportStatus` to exist and be
`init_resource`'d, mirroring how `PendingAssetOps`/`AssetDownloadStatus` are
init'd in `fe_ui::plugin::GardenerConsolePlugin::build`). This binary cannot
compile until fe-ui's `gpx_ops` module exists — expected, per the coordinator's
final sweep.

**Residuals (not fixed by this pass, flagged for follow-up):**

1. No `parent_node_id` column on `node` — GPX waypoint "children" (and any
   future track/segment/trackpoint hierarchy) are property-linked
   (`gpx_track_id`) rather than a real FK. A future track should add the
   column + a `DbCommand::SetNodeParent`-style variant.
2. No per-trackpoint nodes are persisted (only the one track node's cached
   stats + standalone waypoint nodes) — `TrackRouteMap`'s
   `TimestampedRoutePoint` data (needed for `render_gpx_tracks`'s animated
   polyline) has no source yet. Whoever wires FR-3 rendering needs either a
   trackpoint-node persistence path added here, or a different route-data
   source (e.g. re-parsing the original GPX file referenced by the track
   node, if its source path/hash were cached).
3. `fe-api/src/gpx.rs`'s HTTP `import_gpx`/`export_gpx` endpoints are
   unaffected by this pass and remain incomplete in the ways described
   above (properties/hierarchy silently dropped on import; export reads a
   `parent_node_id` field that is never written). Out of this track's
   owned-file set (`fe-api/src/gpx.rs` is read-only here).
4. `GpxImportStatus`'s exact field names are this worker's assumption,
   pending reconciliation with whatever fe-ui's `gpx_ops` worker actually
   ships (see above).
