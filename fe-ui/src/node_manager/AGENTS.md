# fe-ui/src/node_manager — single source of truth for selection

- `mod.rs` — `NodeManager`/`NodeSelection`/`AxisDrag` types +
  `NodeManagerPlugin` (registers the `.chain()`-ed system order inside
  `UiSet::Selection`) + unit tests for the selection state machine.
- `shortcuts.rs` — keyboard tool-switch shortcuts + Escape-to-deselect.
- `sidebar_sync.rs` — resolves `NodeManager.pending_sidebar_select` (a
  node_id set by a sidebar click) into an ECS `Entity` + `select()` call.
- `gimbal_interaction.rs` — hover detection, axis pick → drag → commit, and
  the gizmo draw system. The largest submodule; keep pure geometry helpers
  (`pick_axis`, `segment_dist_2d`, etc.) private to this file.
- `viewport_pick.rs` — 3-D viewport click → nearest-node select/deselect.
- `path_point_interaction.rs` — path-point viewport editor (see §path-points).
- `inspector_sync.rs` — `NodeManager` → `InspectorFormState` display sync
  (transform strings + per-node URL/property load on selection change; also
  clears the Annotation card's title/body/color buffers here since the
  underlying property load is async — see root `AGENTS.md` §gis-query-ui).
- `transform_broadcast.rs` — commits a finished gimbal drag to
  `DbCommand::UpdateNodeTransform` + P2P sync, and applies inbound API
  transforms back onto the ECS.

System functions are `pub(super)` (visible to `mod.rs`'s plugin
registration only) — this module's public surface is just `NodeManager`
itself; nothing outside `fe-ui` should call the per-frame systems directly.

## §path-points — viewport path-point editor

`path_point_interaction.rs` is the "click-in-the-viewport to place / drag /
annotate a track's points" editor. It runs only while `PathEditorState`
has an `editing_track_id`. Design notes:

- **Why fe-ui owns the markers.** fe-terrain is not a dependency of
  `node_manager`, so the editor spawns/despawns its own `PathPointMarker`
  spheres straight into the ECS from `PathEditorState.points` rather than
  reusing any terrain-side ribbon geometry. `sync_path_point_markers` keeps
  one marker per point; it despawns-all + respawns whenever the point count
  changes (counts are small, so a full rebuild is cheaper than tracking
  per-index moves), and lets the drag system write live positions straight
  into each marker `Transform` between count changes.
- **Why `path_edit_capturing`.** The editor and `viewport_pick`/gimbal all
  want the same left-click. `handle_path_point_interaction` sets
  `NodeManager.path_edit_capturing = true` every frame a track is being
  edited; `viewport_pick::handle_viewport_click` early-returns on that flag
  (mirroring its existing `is_dragging()` guard) so node selection yields to
  point editing. The flag is cleared the frame editing stops.
- **Ordering.** Registered in `mod.rs`'s `.chain()` after the gimbal systems
  and BEFORE `viewport_pick::handle_viewport_click`, so the capture flag is
  set before node-pick reads it in the same frame.
- **Interaction model.** Empty click on terrain (Y=0 plane) → queue
  `PathAppendPoint`. Plain click on a marker → begin a drag on that marker's
  current y-plane; release commits a single `PathMovePoint` (no
  remove+append index churn). Shift/Alt+click on a marker → open the inline
  annotation form (`PathEditorState::open_annotate_form`, rendered by
  `panels/path_editor_card.rs`). All hit tests are the same manual
  along-ray + radius test `viewport_pick` uses (no Bevy picking backend).
- **Bridge path.** `PathMovePoint` → `PathOp::MovePoint` →
  `fractalengine::gpx_bridge`, which repositions the point in place through
  the same FR-5 `in_flight_points` authoritative-buffer path as
  Append/Remove (preserves timestamps, no read-modify-write race). The
  editor never persists `gpx_points` itself.

Per-track line styling (color/line-style/visibility, "FR-10") is NOT part
of this port — it depends on `fe_terrain::terrain_plugin::GpxTrackStyle`,
which lives outside the current tree's scope.
