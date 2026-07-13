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

## §pen-tool — pen (phase 1 polyline + phase 2 curves/shapes)

`Tool::Pen` (`panels/toolbar.rs`, hotkey `P` via `shortcuts.rs`) is the
click-to-place tool for drawing a track's polyline. Phase 1 places straight-
segment control points; phase 2 (`curve.rs` + the Tools-panel Pen section)
resamples them into curves and generates shape rings.

- **Gating, not a separate system.** Pen behavior is folded into the
  existing `handle_path_point_interaction` (no new system/registration).
  The system takes `Res<ToolState>` and gates the "empty click on terrain →
  `PathAppendPoint`" branch on `tool.active_tool == Tool::Pen`. Marker
  drag-to-move and Shift/Alt+click-to-annotate stay ungated (they only fire
  when a marker is actually picked under the cursor), so switching to
  Select still lets you reposition/annotate existing points — only new-
  point placement requires the Pen tool.
- **No new action.** Reuses `UiAction::PathAppendPoint` unchanged; the pen
  tool only changes *when* a click is allowed to emit it.
- **UI entry point.** `panels/path_editor_card.rs`'s edit view no longer has
  an "Append from cursor" button (removed — it placed points anywhere the
  3-D cursor happened to be, independent of tool mode, which made accidental
  placement easy). It now shows a one-line hint pointing at the Pen tool.
- **Preview.** Placed points render via the existing `sync_path_point_markers`
  spawn/despawn (yellow `Sphere(0.35)`); the connecting polyline renders via
  `fe_terrain`'s `render_gpx_tracks` off the same `gpx_points`. No separate
  pen-preview mesh in phase 1.

### phase 2 — curves + shapes (`curve.rs`)

`curve.rs` is pure `[f32;3]` math (XZ plane, Y carried through), no Bevy
systems. `PenMode` (Polyline/CatmullRom/Bezier) + `resample()` turn placed
control points into a denser sampled polyline; `catmull_rom`'s `tension` is
the "sharp ↔ smooth" sensitivity (1.0 ≈ straight, 0.0 ≈ round). `ellipse`/
`circle`/`rectangle` generate closed point rings. The Tools-panel Pen section
(`panels/tool_panel.rs`) exposes a mode radio, tension slider, and shape
buttons; its buttons queue `UiAction::PathSmoothCurrent` / `PathAppendShape`
into `ToolPanelState.pending_actions`, drained in `process_ui_actions` (the
panel has no `ui_mgr` handle). Both actions re-express their result as the
existing `RemovePoint`/`AppendPoint` `PathOp`s, so no gpx-bridge change is
needed — smooth = resample-then-replace, shape = append the generated ring.

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
- **Interaction model.** Empty click on terrain (Y=0 plane) while `Tool::Pen`
  is active → queue `PathAppendPoint` (see §pen-tool). Plain click on a
  marker → begin a drag on that marker's current y-plane, regardless of
  active tool; release commits a single `PathMovePoint` (no remove+append
  index churn). Shift/Alt+click on a marker → open the inline
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
