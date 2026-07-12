# fe-ui — module map and design rationale

`fe-ui` is the egui-based control surface for FractalEngine's GUI binary:
sidebar hierarchy, inspector, dialogs, the embedded webview ("portal"), and
terrain/hexon map management. This doc replaces verbose inline comments —
see each module's own doc-comment for the "what"; this file is the "why".

## Module map

| Module | Owns |
| --- | --- |
| `plugin` | `GardenerConsolePlugin`, `UiSet` ordering, the `EguiPrimaryContextPass` entry system, cross-crate re-export shims (see §compat), UI-only resources that don't belong to a specific domain manager (`SidebarState`, `ToolState`, `InspectorFormState`, `LocalUserRole`, `CameraFocusTarget`, `ViewportCursorWorld`, `ViewportRect`, `SpawnedNodeMarker`). |
| `actions` | `UiAction` (the one-frame action queue enum) + `UiManager` (queue + portal + active dialog + toast) + `process_ui_actions`, split into domain files (`portal`, `node_props`, `hexon`, `query`, `transform`). See §actions and §inspector-transform. |
| `portal` | `PortalState` + the pure webview-rect-sync math (`compute_portal_rect`). See §portal — this is the browser-integration seam. |
| `dialogs` | `ActiveDialog` (mutual-exclusion enum) + one render function per floating dialog/window. |
| `terrain_map` | Petal-map state (`PetalMapState`), the hexon registry op queue (`HexonOp`/`PendingHexonOps`), Hexon Manager DTOs, the petal manifest type, and tileset-event draining. |
| `asset_ops` | Node-asset download op queue (`AssetOp`/`PendingAssetOps`) + the result-status resource (`AssetDownloadStatus`) the main binary writes back to. See §asset-download. |
| `gis` | `GisPanelState` (GIS Query panel resource, incl. `GisPanelTab`) + pure query-building/row-parsing/bbox-filter/layer-JSON/view-mode logic backing the GIS Query, Annotations & Layer Manager panel. See §gis-query-ui. |
| `gpx_ops` | GPX track import op queue (`GpxOp`/`PendingGpxOps`) + the result-status resource (`GpxImportStatus`) the main binary writes back to. See §gpx-import. |
| `path_ops` | Path-editor op queue (`PathOp`/`PendingPathOps`) + the result-status resource (`PathEditStatus`) the main binary's GPX bridge writes back to. See §path-editor. |
| `panels` | The top-level egui shell (`gardener_console`) + one file per panel (toolbar, status bar, sidebar tree, inspector tabs, query tab, portal toolbar). |
| `node_manager` | `NodeManager` (single source of truth for selection) + gimbal interaction/hover/pick, viewport click-to-select, transform broadcast, inspector sync — one file per concern. |
| `verse_manager` | `VerseManager` (in-memory verse/fractal/petal/node tree) + `DbResult` draining, GLTF/fallback-sign spawning, petal-switch respawn. |
| `navigation_manager`, `viewport`, `atlas`, `gimbal`, `theme`, `role_chip` | Unchanged from before this decomposition; not god-files. |

This module tree is a **physical decomposition only** — no behavior changed
during the split. The architectural design (UiSet ordering, UiAction queue,
ActiveDialog enum, NodeManager-as-selection-source) was implemented earlier;
see `conductor/tracks/ui_manager_refactor_20260419/spec.md`.

## §compat — cross-crate re-export shims

`fractalengine/src/main.rs` and `fractalengine/src/terrain_bridge.rs` import
`fe_ui::plugin::{GardenerConsolePlugin, LocalUserRole, ActiveDialog, HexonOp,
InstalledTilesetDto, PendingHexonOps, StorageInfoDto, UiManager}` and cannot
be edited by this change. `plugin.rs` keeps these reachable via `pub use`
shims even though the types now live in `actions`, `dialogs`, and
`terrain_map`. `UiAction` and `PetalMapState` are also re-exported at the old
path defensively (not currently used externally, but were technically part
of the old public surface). When adding a new type that logically belongs in
one of the split modules, prefer importing it from its real home
(`crate::dialogs::X`, `crate::actions::X`, `crate::terrain_map::X`) in new
fe-ui code — only `plugin.rs` needs the compat re-export.

One surface *narrowed* during the split: `Tool` (the viewport transform
tool enum) moved from `fe_ui::panels::Tool` to `fe_ui::panels::toolbar::Tool`,
and the `panels::{toolbar,sidebar,status_bar,inspector,query_tab}` submodules
are `pub(crate)` (crate-internal) rather than `pub`. Nothing outside fe-ui
referenced `fe_ui::panels::*` before this change (verified via
`grep -rn "fe_ui::" fractalengine/src fe-webview/src`), so this is safe.

## §actions

`UiAction` is a one-frame signal queue pushed during the egui render pass
(`EguiPrimaryContextPass`) and drained once per `Update` frame by
`process_ui_actions` (registered in `UiSet::ProcessActions`). Each match arm
delegates to a small domain function in `portal.rs` / `node_props.rs` /
`hexon.rs` / `query.rs` — this keeps `process_ui_actions` itself as a thin
dispatcher and lets the domain logic be unit-tested without a Bevy `App`
where possible (see §portal).

## §portal — the browser-integration seam

This is the highest-polish, most fragility-prone part of the crate: it's
where inspector form state crosses into `fe-webview`'s `BrowserCommand`
queue and the DB persistence layer. `actions::portal` intentionally splits
each action into a **pure decision function** (`compute_open_portal`,
`should_auto_close`, `compute_save_url`) that takes plain resource
references and returns an outcome/tuple, with the actual `MessageWriter`/
`Sender` I/O left in `process_ui_actions`. This makes the save/open/close
semantics unit-testable with plain `#[test]` functions (no `MinimalPlugins`
app needed) — see the `#[cfg(test)]` modules in `actions/portal.rs` and
`portal/mod.rs`.

**The complete save chain**, UI field → action → system → persistence:

1. `InspectorFormState.external_url` is a plain text buffer, edited directly
   by `panels::inspector::inspector_url_meta_section`. It is populated from
   `VerseManager` (not the DB) whenever selection changes
   (`node_manager::inspector_sync::sync_manager_to_inspector`).
2. Clicking **Save** pushes `UiAction::SaveUrl` — a bare signal with **no
   payload**. It does not carry `node_id` or the URL value; both are
   re-read from `NodeManager`/`InspectorFormState` at drain time.
3. `process_ui_actions` calls `actions::portal::compute_save_url(&node_mgr,
   &inspector)`. If nothing is selected, this returns `None` and the save is
   a **silent no-op** — no log, no toast. Whitespace-only URLs become
   `None` (clears the field); non-empty URLs are stored **as typed, not
   trimmed** — leading/trailing whitespace survives into the DB.
4. On `Some((node_id, url))`: `verse_mgr.update_node_url(...)` updates the
   in-memory tree immediately (optimistic local echo), then
   `db_sender.send(DbCommand::UpdateNodeUrl { .. })` is fired at the DB
   thread over an unbounded crossbeam channel. If the send fails, a single
   `bevy::log::warn!` fires and **nothing else happens** — no retry, no
   user-visible error, and the in-memory copy is already "saved" from the
   UI's perspective regardless of DB outcome.
5. Clicking **Open Portal** is a *separate* action (`UiAction::OpenPortal`)
   that independently re-reads `inspector.external_url` — saving does not
   automatically open the portal, and opening does not require a prior save.
6. `compute_open_portal` mirrors the same "no selection ⇒ silent no-op"
   shape: invalid URL syntax logs a warning, but a *valid* URL with nothing
   selected does nothing at all (not even a log line).
7. Every `Update` frame, before draining new actions, `should_auto_close`
   checks whether the currently-open portal's `opened_for_entity` still
   matches the live selection; if the selection changed or cleared, the
   portal force-closes (`BrowserCommand::Close`) — by design (FR-2), but it
   means selecting a *different* node while a portal is open silently kills
   the webview with no confirmation.

## §inspector-transform — Position/Rotation/Scale write-back

The Transform section's Position/Rotation/Scale fields were historically
**display-only**: `node_manager::inspector_sync::sync_manager_to_inspector`
formats the selected entity's `Transform` into `inspector.pos/rot/scale`
every frame, but nothing ever read those buffers back. Editing a field
changed the text, not the node — no removed system, this write-back never
existed.

The fix follows the same `UiAction` pure-decision-function shape as §portal:

1. Losing focus on any field via Enter, or clicking the **Apply** button
   (`panels/inspector.rs::inspector_transform_section`), pushes
   `UiAction::ApplyNodeTransform` (no payload — it reads current buffer
   state at drain time, same as `SaveUrl`).
2. `process_ui_actions` (`UiSet::ProcessActions`) dispatches to
   `actions::transform::apply`, which parses the three `[String; 3]`
   buffers (rotation in degrees, matching the display format) via the pure
   `parse_inspector_transform`, writes the result onto the selected
   entity's `Transform`, and sets `NodeSelection::drag_committed = true`.
3. `UiSet::ProcessActions` runs *before* `UiSet::Selection`
   (`node_manager::NodeManagerPlugin`'s chain), so in the same frame:
   `sync_manager_to_inspector` re-reads the just-written `Transform` (a
   harmless round-trip), and `transform_broadcast::broadcast_transform`
   sees `drag_committed` and persists to DB + P2P — the exact same commit
   path the gimbal drag uses, so no second persistence path was added.
4. A field that fails to parse (non-numeric text) aborts the whole apply
   with a warning log rather than partially applying — see
   `actions/transform.rs::apply`.

## §asset-download — integration contract for the main binary

The inspector's "Asset" card (`panels::asset_card`) is UI-only: it reads
`NodeEntry.has_asset`/`asset_path` from `VerseManager` to enable/disable the
Download button and, on click, pushes `UiAction::DownloadNodeAsset { node_id
}`. `process_ui_actions` drains that into `asset_ops::PendingAssetOps`
exactly like `UiAction::Hexon*` actions drain into `terrain_map::PendingHexonOps`
— fe-ui never touches a `BlobStoreHandle` or writes files itself.

Integration contract for `fractalengine` (main binary, not owned by this
worker):
1. Each frame (or on a timer), drain `Res<PendingAssetOps>` (mirrors the
   existing `terrain_bridge.rs` pattern that drains `PendingHexonOps`),
   resolve `AssetOp::Download { node_id }` via the node's asset/blob
   reference and `BlobStoreHandle`, write the resolved bytes to disk.
2. Write the outcome into `ResMut<AssetDownloadStatus>`: `node_id` +
   `saved_path` on success, or `node_id` + `error` on failure. fe-ui exposes
   this resource for exactly this purpose; it is not read anywhere inside
   fe-ui yet (no auto-toast wiring was added, to keep this change minimal —
   a follow-up system reading `AssetDownloadStatus` and calling
   `UiManager::show_toast` is the natural next step, following the same
   shape as `HexonOpenStorageDir`'s OS-explorer reveal).

**Fragilities worth a dedicated e2e pass** (flagged, not fixed here — out of
scope for this decomposition task):
- Two silent no-op paths (`SaveUrl` and `OpenPortal` with no selection) with
  zero user feedback. If a user's click races a selection change by even one
  frame, the save/open is dropped with no visible signal.
- `UiAction::SaveUrl` carries no `node_id`/url payload — it trusts that
  `NodeManager.selected` and `InspectorFormState.external_url` are still
  valid at drain time, which is normally true (drain happens the same or
  next frame) but is a latent footgun if that assumption ever breaks (e.g. a
  future async dialog between click and drain).
- Three copies of "the URL": `InspectorFormState.external_url` (form buffer),
  `VerseManager`'s `NodeEntry.webpage_url` (in-memory tree), and the DB
  column. Steps 2-4 write only the first two synchronously; the DB write is
  fire-and-forget with no ack surfaced to the UI (`DbResult::NodePropertySet`
  has an equivalent round-trip for custom properties, but `UpdateNodeUrl`
  has no corresponding `DbResult` handled here to confirm the write landed).
- Whitespace is preserved (not trimmed) in the stored URL string; only the
  "is it empty" check trims, so `"  https://x  "` is stored with the
  padding intact and will need re-parsing/trimming on read.

## §gis-query-ui — Annotation editor, GIS Query panel, Layer Manager

Track `gis_query_ui_20260711`. Three pieces, all in `fe-ui` only (no new
crate deps — in particular no `fe-terrain`/`fe-query` dependency was added;
everything is `serde_json` + the existing `DbCommand`/`DbResult` surface).

**Annotation card** (`panels::annotation_card`). Edits three reserved,
flat, dotted-string node-property keys — `gis.annotation.title`,
`gis.annotation.body`, `gis.annotation.color` — through the *exact* Phase 5
custom-property path: `UiAction::SetNodeProperty`/`DeleteNodeProperty` →
`actions::node_props::set`/`delete` → `DbCommand::SetNodeProperty`/
`DeleteNodeProperty` (see `fe-database/src/handlers/entity_property.rs`'s
`properties[$key]` dynamic-key setter — the dotted key is one flat map key,
not a nested path). Saving an empty field pushes `DeleteNodeProperty`
instead of `SetNodeProperty` (clears the key). The three key constants and a
pure `annotation_fields_from_properties` extractor live in
`actions::node_props`; `InspectorFormState` gained three buffers
(`annotation_title_buf`/`_body_buf`/`_color_buf`) populated by
`db_results::apply_db_results` whenever `DbResult::NodePropertiesLoaded`/
`NodePropertyDeleted` land, and cleared by `node_manager::inspector_sync` on
every new selection (properties load asynchronously, unlike the synchronous
`external_url` sync, so they can't be populated at `just_selected` time).
**Shared-contract note:** `fe_query::gis` independently defines the same
three key strings for the data-layer side (petal_gis_endpoints track);
fe-ui deliberately duplicates the literals rather than depending on
`fe_query` for three constants — keep both in sync by hand if either ever
changes.

**GIS Query panel** (`panels::gis_panel`, state in top-level `crate::gis`,
I/O in `actions::gis`). An independent floating `egui::Window` (not part of
the mutual-exclusion `ActiveDialog` set — see `dialogs/AGENTS.md` — so it
can stay open alongside the inspector), toggled by the toolbar's "GIS"
button. Three modes:
- **Annotated** / **Property filter** — a single `SELECT` over
  `DbCommand::RawQuery` (the same mechanism `query_tab`'s ad-hoc SurrealQL
  box already uses), built by pure functions in `gis::query`
  (`annotation_query`/`property_filter_query`). All user-controlled values
  (petal_id, filter key/value) are passed as **bind vars** in the `vars`
  map — never string-formatted into the SQL — per fe-database's `RawQuery`
  security filter (single `SELECT`, no `;`, keyword blocklist; see
  `fe-database/src/lib.rs`). Only the compile-time-constant annotation-title
  key is inlined as a literal (safe: not user input).
- **Bbox** (local XZ plane around the camera) — **no DB round-trip**: an
  existing RawQuery precedent existed (used for the other two modes above),
  but node positions for the active petal are already resident in
  `VerseManager` (used for sidebar rendering/gimbal), so the bbox filter
  (`gis::bbox_contains`, pure) runs client-side over `verse_mgr.find_petal(..).nodes`
  directly — this is a deliberate design choice, not a fallback residual.
- **Result routing residual:** `DbResult::QueryResult`/`DbResult::Error`
  carry no request-id, so `db_results::apply_db_results` can't tell "this
  reply is for the GIS panel" vs "this reply is for the inspector's ad-hoc
  Query tab" apart from a `GisPanelState.query_pending` flag checked first
  (GIS claims the reply when pending, the Query tab is the fallback). If a
  user manages to have both an ad-hoc query and a GIS query in flight in
  the same frame window, one reply will go to the wrong buffer — accepted
  as a pre-existing architectural constraint of the untagged `RawQuery`
  channel, not something this track introduced.
- **Click-to-select:** reuses the exact sidebar mechanism — clicking a
  result row sets `NodeManager.pending_sidebar_select` (resolved to an
  `Entity` next frame by `node_manager::sidebar_sync`) and
  `CameraFocusTarget.target` (consumed by `plugin::apply_camera_focus`).
  No new selection/focus mechanism was invented.

**Layer Manager** (`panels::layer_manager_card`, embedded in the GIS
panel). `PetalMapState` gained a `terrain_json: Option<serde_json::Value>`
field — the raw, last-loaded petal terrain doc (previously only derived
fields like `tileset_ids`/`world_scale` were kept) — populated by
`db_results` on `DbResult::PetalTerrainLoaded` and cleared on petal switch.
Toggling a layer's visible/opacity mutates that stored doc in place via the
pure `gis::set_layer_field` (find-or-insert by `name`, update only the
`Some(..)` fields, preserve everything else) and round-trips through
`SetPetalTerrain` — the same "mutate one field of the stored JSON, then
persist" idiom as `actions::hexon::set_petal_map_scale`, but operating on
the actual persisted doc instead of rebuilding one from an
`InstalledTilesetDto` (more robust: doesn't require the tileset DTO to be
in hand, preserves origin/elevation/`world_scale`/any other layers
untouched). The opacity slider mirrors `hexon_manager::render_scale_controls`'s
world-scale idiom exactly: every `changed()` frame writes a **local-only**
preview into `petal_map.terrain_json` (so dragging feels live), while the
persisting `UiAction::GisSetLayer` only fires on `drag_stopped()` (or a
non-drag `changed()`, e.g. a discrete click) to avoid flooding
`SetPetalTerrain` sends during a drag. Only `"satellite"`/`"terrain"` are
currently mapped to a `LayerType` in `fe-terrain::petal_binding` (see
`fe-terrain/src/AGENTS.md` §petal_binding); GPX-track/GeoJSON-overlay
checkboxes are shown **disabled with a tooltip** explaining they're inert —
tracked as a residual per FR-3, not wired to any config since fe-ui must
not couple to fe-terrain's layer-name mapping.

### Round 2 additions (GIS Round-2 worker W-A)

**Annotation-save fix.** The Annotation card's Save button was landing
writes in the DB (proven by the Annotated query finding titled nodes) but
visibly reverting the just-typed field to blank in the same frame. Root
cause: a single Save click emits one `UiAction` per field — `SetNodeProperty`
for non-empty (post-trim) fields, `DeleteNodeProperty` for empty ones (an
annotation with, say, only a title set is the common case, so a Delete for
the still-empty body/color fields fires in the *same* batch as the title's
Set). `DbResult::NodePropertyDeleted`'s handler used to re-derive **all
three** Annotation buffers from `inspector.node_properties` — but that cache
can still be missing the sibling Set's value, because `NodePropertySet`'s
own refresh (`GetNodeProperties` → `NodePropertiesLoaded`) is a separate
async round-trip that usually resolves a frame or more *after* the Delete
result for the sibling field. The Delete handler was overwriting the
just-typed sibling buffer back to blank before its own confirmation arrived.
Fixed in `verse_manager/db_results.rs`: `NodePropertyDeleted` now clears only
the one buffer matching the deleted key (via `actions::node_props::annotation_field_for_key`)
instead of re-deriving all three. A secondary hardening: `NodePropertiesLoaded`/
`NodePropertySet`/`NodePropertyDeleted` are now gated on `node_id` still
matching `NodeManager.selected` (`db_results::is_for_selected_node`) — without
this, a stale in-flight property fetch (e.g. triggered by the old node's
post-Save refresh) could land after the user switched to a different node and
stomp that node's buffers. The Save button's action-emission itself was
extracted into a pure `panels::annotation_card::annotation_save_actions` for
direct unit testing.

**Color picker.** The Annotation card's hex swatch (previously a static
painted rect) is now `egui::widgets::color_picker::color_edit_button_srgb`
(egui 0.33 via bevy_egui 0.39), bound to a `[u8; 3]` derived from
`parse_hex_color`/formatted back via the new `annotation_card::rgb_to_hex`.
The hex `TextEdit` stays alongside it — the stored value is still a plain
hex string per the `gis.annotation.color` contract; only the widget changed.

**GIS panel tabs.** `panels::gis_panel` is now tab-strip'd (`GisPanelTab`:
Query / Annotations / Layers, mirroring `inspector.rs`'s tab-bar idiom) —
previously the Query section and Layer Manager were both always visible
stacked in one window. **Annotations tab** reuses the exact
`GisQueryMode::Annotated` query flow (`run_query`/`gis_state.results`) behind
a dedicated Refresh button rather than inventing a second results channel —
`DbResult::QueryResult`'s untagged-reply routing (see the residual note
above) is fragile enough already without a third claimant. Rows show a color
swatch (parsed from the query's new `annotation_color` column via
`GisResultRow.annotation_color`), title, and node name; click-to-select
reuses the same `pending_sidebar_select`/`CameraFocusTarget` mechanism as
Query-tab results. GPX-imported waypoints get `gis.annotation.title` set
from the waypoint name by the GPX bridge (another worker), so they surface
here automatically.

**Splat view mode.** Layers tab gained a Mesh/Splats/Hybrid selector
(`panels::layer_manager_card::render_view_mode_row`), persisted as an
additive `"view_mode": "mesh"|"splats"|"hybrid"` field on the petal terrain
JSON via `gis::set_view_mode_field`/`view_mode_from_terrain_json` (same
mutate-and-round-trip idiom as `set_layer_field`, new `UiAction::GisSetViewMode`
→ `actions::gis::set_view_mode`). The renderer side consuming this field is
owned by another track — the field name/values are a fixed contract, do not
rename.

## §gpx-import — integration contract for the main binary

`panels::gpx_import_card` (embedded in the GIS panel's Query tab) is
UI-only: an "Import GPX..." button opens an `rfd::FileDialog` filtered to
`.gpx` (same idiom as `dialogs::gltf_import`'s Browse button) and, on a
picked file, pushes `UiAction::GpxImportFile { petal_id, path }`.
`process_ui_actions` drains that into `gpx_ops::PendingGpxOps` exactly like
`UiAction::DownloadNodeAsset` drains into `asset_ops::PendingAssetOps` — fe-ui
never parses a GPX file or touches the DB itself.

Integration contract for `fractalengine` (main binary, not owned by this
worker; contract reconciled with the GPX-bridge worker mid-track):
1. Each frame (or on a timer), drain `Res<PendingGpxOps>` (mirrors the
   `asset_ops`/`terrain_bridge.rs` drain pattern), resolve
   `GpxOp::ImportFile { petal_id, path }` by parsing the GPX file and
   creating nodes/tracks for the petal. Imported waypoints get
   `gis.annotation.title` set from the waypoint name so they surface in the
   GIS panel's Annotations tab automatically — no fe-ui-side filtering needed.
2. Write the outcome into `ResMut<GpxImportStatus>`: `petal_id` +
   `track_count`/`waypoint_count` on success, or `petal_id` + `error` on
   failure. **Note the status shape is counts, not a single imported-track
   name** — a GPX file may contain multiple tracks/waypoints, and the
   success toast/status row read like "Imported N track(s), M waypoint(s)".
3. `gpx_ops::surface_gpx_import_status` (registered in `plugin.rs`) surfaces
   the outcome as a toast; `gpx_import_card::gpx_import_section` additionally
   renders a persistent status row gated on `petal_id` matching the active
   petal (mirrors `asset_card`'s FR-3 status row).

## §path-editor — GPX Path Editor (Paths tab)

Track `gpx_path_editor_20260711`, FR-1/FR-2. The GIS panel's fourth tab
(`GisPanelTab::Paths`) lets a user author, annotate, and export GPX paths as
first-class planning artifacts. fe-ui owns the `PathOp`/`PathEditStatus`
contract (per FR-2) — the main binary's GPX bridge
(`fractalengine/src/gpx_bridge.rs`, another worker) drains it and persists
`gpx_points` as a flat node property. Mirrors the `gpx_ops`/§gpx-import
pattern exactly (queue + status resource, no fe-ui-side I/O).

1. **`path_ops.rs`** (crate root, mirrors `gpx_ops.rs`): `PathOp` — one
   variant per intent (`CreateTrack`, `DeleteTrack`, `AppendPoint`,
   `RemovePoint`, `AnnotatePoint`, `ExportGpx`) — queued into
   `PendingPathOps` and drained by the bridge; `PathEditStatus` is the
   bridge's write-back result resource (`track_node_id` + `message` on
   success, `track_node_id` + `error` on failure).
   `path_ops::surface_path_edit_status` toasts the outcome, registered
   alongside `surface_gpx_import_status` in `plugin.rs`.
2. **Track listing** (`crate::gis::PathEditorState`, state module
   `gis/mod.rs`): track nodes are identified by a new reserved property key,
   `gis.track.name` (`actions::node_props::TRACK_NAME_KEY`) — set by the
   bridge on `CreateTrack`/import, mirroring how `gis.annotation.title`
   marks annotated nodes. `gis::query::track_query` builds the same
   "`SELECT` where `properties[key] != NONE`" shape as `annotation_query`;
   `actions::path::query_tracks` submits it via `DbCommand::RawQuery`, and
   `PathEditorState.tracks_pending` claims the untagged `DbResult::QueryResult`/
   `Error` reply in `verse_manager/db_results.rs` — a **third** claimant on
   top of `GisPanelState.query_pending`/the inspector's ad-hoc Query tab (see
   the existing residual note above); checked after `gis_panel.query_pending`,
   before the inspector fallback.
3. **Point-list editing is local-only in v1** — `PathEditorState.points`
   (`Vec<PathPointRow>`, `{ position, time_seconds }`) is populated purely by
   queuing `AppendPoint`/`RemovePoint` ops (`actions::path::append_point`/
   `remove_point`), never read back from the DB. Re-selecting an
   already-edited track (`PathEditorState::start_editing`) always starts
   from an empty local buffer — there is no `GetNodeProperties`-style
   round-trip to repopulate `gpx_points` client-side. This is an accepted v1
   gap (out of scope per the track spec's "no drag-gizmo... list-based
   move/remove suffices"): if a user navigates away from the Paths tab
   mid-edit and returns, their in-progress point list is gone (the queued
   ops already landed at the bridge, only the *UI's echo* is lost).
4. **Append from cursor** reuses `plugin::ViewportCursorWorld` (the same
   resource the context-menu GLB-import flow uses) — the button is disabled
   when `cursor_world.pos` is `None` (cursor not over the viewport / no
   terrain hit this frame).
5. **Annotate** reuses the exact `gis.annotation.*` property contract
   (`actions::node_props::ANNOTATION_TITLE_KEY`/etc.) via `PathOp::AnnotatePoint`
   — the bridge is expected to create a waypoint node at the point's stored
   position and set the three annotation properties, the same way GPX-import
   waypoints get `gis.annotation.title` (see §gpx-import). **v1 has no
   per-point annotation form** — `path_editor_card`'s Annotate button queues
   a placeholder `"Waypoint {index}"` title with empty body/color; a
   dedicated inline form (mirroring `annotation_card`'s title/body/color
   fields) is a natural follow-up, not added here to keep this pass to the
   spec's FR-1 list shape.
6. **Export** queues `PathOp::ExportGpx { track_node_id }` — per FR-5, the
   actual GPX 1.1 writer + rfd save dialog is fe-terrain's/the bridge's
   responsibility (pure writer in fe-terrain's gpx module); fe-ui only
   signals intent, same as every other `PathOp`.
7. **Contract note for the bridge worker:** if the bridge's independently
   authored types don't match `PathOp`/`PathEditStatus` field-for-field, this
   file (`path_ops.rs`) is the authoritative definition per FR-2 ("fe-ui does
   no persistence I/O" implies fe-ui also owns the intent vocabulary) — the
   bridge should conform to this shape rather than the reverse.
