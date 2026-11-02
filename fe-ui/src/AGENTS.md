# fe-ui — module map and design rationale

`fe-ui` is the egui-based control surface for FractalEngine's GUI binary:
sidebar hierarchy, inspector, dialogs, the embedded webview ("portal"), and
terrain/hexon map management. This doc replaces verbose inline comments —
see each module's own doc-comment for the "what"; this file is the "why".

## §qa-settings-isolation

With explicit `FE_QA_STORAGE_ROOT`, settings resolve only to
`<root>/config/settings.ron`; malformed roots disable persistence rather than
falling back to user configuration. The existing settings restart journey
uses pure path inputs to verify QA precedence without global environment
mutation. GUI startup validates the same root before loading any settings.

## §brush-tool-and-numeric-boundary

`Tool::Brush` converts the meter-valued radius/target/delta through the active
petal's sanitized `world_scale` at press, then samples Y=0 ray hits at half the
converted radius. Release queues one bounded `SculptBrushStroke`; its handler
builds one swept-corridor polygon and performs one terrain-document write plus
one endpoint-node creation per completed stroke.
Escape, right-click, tool/petal change, or a stranded release cancels without
replay.

Path Tools, Terrain Tools, stamp-editor, and sculpt `f32` buffers are sanitized
before egui widget construction. Non-finite values recover to defaults and
finite values clamp to practical caps. Brush cursor and commit share the same
sanitized radius. Missing petal/map is a warned, visible no-op.

## §file-dialogs

`file_dialogs` is the Strategy boundary for GLTF/GPX import, Hexon install, and asset/GPX export.
`UiManager.file_dialogs` owns the selected provider. Normal input-disabled builds use native rfd;
synthetic-input hosts fail closed unless an isolated QA root and an actual loopback listener enable
the queued provider. Missing or wrong-kind answers produce typed errors through the normal notice/status path.
Answers have a bounded active-verse lease and batches validate completely before mutating the queue.

QA paths reject traversal, UNC/device prefixes, drive-relative paths, ADS, redirected ancestors and Windows
reserved device components, including extension/superscript aliases. See Microsoft's
[filename rules](https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file) and
[console device names](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilea).
Open inputs require existing files; save outputs require an existing confined parent. `DialogPath` owns the
selected file/ancestry guards through its consuming operation. Windows handles prevent replacement of inputs
while an import reads them. Unix directory handles do not provide the same rename exclusion.

Export callers use `DialogPath::write_bytes` or `copy_from`, never a check followed by a plain path reopen.
QA output streams into an owner-only sibling and atomically replaces the destination entry through the shared
identity file helper; a leaf changed into a link cannot redirect the write. Native exports preserve native semantics.

## §navigation-ancestry

Global tree rows and viewport cards enter descendants through
`NavigationManager::navigate_to_fractal_in_verse` or `navigate_to_petal_in_fractal`.
These replace the full ancestor chain together, including names, even when a different verse
or only a verse is active. Leaving a fractal clears its petal. Startup selects a populated
`(verse, fractal, petal)` tuple before updating the cursor, so scenes and replicas use the same verse.
The relative navigation methods remain for callers already operating within the active parent.

Initial hierarchy auto-selection is allowed only while NavigationManager's startup
gate is armed and no Verse is selected. Explicit navigate/back methods suppress it,
including Back before startup data arrives. Read-only hydration calls the separate
suppression method without changing navigation intent or any async intent revision.
Both hierarchy fallback branches use this gate; repeated reloads cannot re-enter
a Verse after exit. The latch field remains public only for existing struct-update
callers across crates; transitions belong to the manager methods.


`active_scope` returns no scope for an empty or incomplete cursor; callers must never downgrade
an incomplete descendant selection into a broader verse query. The role resolver clears its cache
when the scope changes or ancestry is invalid, warns once per invalid interval, and issues no request
for invalid navigation. Role replies apply only when their returned scope still equals the active scope.
These are fail-closed UI affordances; database/API authorization remains authoritative.

## Module map

| Module | Owns |
| --- | --- |
| `plugin` | `GardenerConsolePlugin`, `UiSet` ordering, the `EguiPrimaryContextPass` entry system, cross-crate re-export shims (see §compat), UI-only resources that don't belong to a specific domain manager (`SidebarState`, `ToolState`, `InspectorFormState`, `LocalUserRole`, `CameraFocusTarget`, `ViewportCursorWorld`, `ViewportRect`, `SpawnedNodeMarker`). `ToolState::activate` is the SOLE writer of `active_tool` (re-press toggles back to Select; one-shot Options reveal at activation edge only). |
| `visibility` | hierarchy_visibility_groups_20260808: pure resolver (`resolve.rs` — solo lens > tri-state override > ancestor chain > ANY-hidden-group), `VisibilityState`, `sync_node_visibility` (spawn-then-hide apply + D-13 force-deselect), the shared picker rule `pickable()`, `cache.rs` (persisted `view.state`/`view.groups`) and `groups.rs` (persisted Group registry + verse/fractal/petal eyes). See §visibility. |
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
| `verse_manager` | `VerseManager` (in-memory verse/fractal/petal/node tree) + `DbResult` draining, GLTF/fallback-sign spawning, petal-switch respawn. Also the residency-ledger primitive (`spawn::distance_ranked_allowance` + `spawn::ResidencyBudget`). See §residency-ledger. |
| `settings` | `AppSettings` (D-78 application settings Resource) + `TileSourceMode` + RON persistence (`load_settings_from`/`save_settings_to`/`sanitize`, debounced save) + live syncs (mesh budget, camera) + the 3-way `effective_render_distance(global, petal_column, session_override)` precedence + session-only `PetalRenderOverride`. See §app-settings. |
| `terrain_proposal_state` | `ProposalEditState` (Resource) + `ProposalRecord`/`ProposalOp` + pure `to_json`/`from_json`. The fe-ui-local mirror of the persisted proposal contract. See §terrain-proposal-editor. |
| `geometry` | fe-ui-local pure mirror of fe-terrain's ruler math (`polygon_area_m2`/`world_to_real_distance`/`bearing_deg`). See §geometry-mirror. |
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

## §camera-focus (`camera_focus_clip_20260716`)

`CameraFocusTarget.target: Option<(String, [f32; 3])>` carries `(node_id,
fallback position)`, not just a raw position. `plugin::apply_camera_focus`
resolves the live spawned entity's `GlobalTransform` for `node_id` first (same
`SpawnedNodeMarker` idiom as `node_manager::sidebar_sync`) and only falls back
to the cached position — this fixes the stale-origin-then-real-spot two-step
teleport that happened when a freshly created node's tree entry still held
`[0.0; 3]`. Producers: `panels::sidebar` (tree click) and
`panels::gis_panel::render_results` (GIS/Annotations row click).

## §actions

`UiAction` is a one-frame signal queue pushed during the egui render pass
(`EguiPrimaryContextPass`) and drained once per `Update` frame by
`process_ui_actions` (registered in `UiSet::ProcessActions`). Each match arm
delegates to a small domain function in `portal.rs` / `node_props.rs` /
`hexon.rs` / `query.rs` — this keeps `process_ui_actions` itself as a thin
dispatcher and lets the domain logic be unit-tested without a Bevy `App`
where possible (see §portal).

## Portal URL ownership and backend feedback

Both Inspector and Node Options validate a trimmed draft through actions::portal::validate_url
and queue one captured SaveNodeUrl payload. The dispatcher rechecks current Petal, actual tree
node and advisory Editor access, then retains one ULID request owner with exact scope, target,
selection and a30-second deadline. Sending shows Saving URL. A current NodeUrlUpdated receipt
owns only its success notice; a newer draft remains unsaved. Independent bounded per-node write
owners schedule a fresh typed ReadNodeUrl even after selection or notice timeout. Only an owned
current read updates the exact node cache; obsolete form ownership cannot discard a committed
change. A failed send never changes the tree.

The DB uses its own local identity, captured Editor authority and stored ancestry in the SAME
atomic op/model transaction. Node asset/Petal/tombstone, model URL and exclusive live reference
are fenced before the write. Missing, assetless, shared or foreign-Petal models refuse honestly.
The returned value comes from the committed UPDATE RETURN AFTER. This preserves the existing
model.external_url storage contract: no per-node URL schema, URL history replay or unsupported
model-row sync publication is implied.

Open remains an explicit separate action. Tauri implements Back/Forward through history and
Reload through location.reload using the existing pinned wry evaluate_script API. The toolbar
shows backend errors and a loading indicator; only a URL-bound page-finished event can end that
indicator, never an unbound legacy completion. The measured remaining panel rectangle places
the native overlay below all toolbar/status controls. Selection changes close the portal with
an advisory. Open externally is a separate validated system-browser launch request; a successful
spawn says launch requested and cannot prove that a page loaded. Unsupported builds explain
embedded browsing is unavailable and never auto-launch an external application.

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

## §inspector — clipping, full-height Query tab, scoped templates (2026-10-25 in-app findings F1-F3)

- **F1 root cause.** egui 0.33's default text-wrap mode for a plain
  `ui.horizontal(...)` row (and for an `egui::Grid` cell) is `Extend`, which
  sets `wrap.max_width = f32::INFINITY` — any `ui.label`/`Label` without an
  explicit `.wrap()`/`.truncate()` renders at its full natural width and can
  blow the panel out (this is distinct from `.wrap()`, which multi-line wraps
  and was already used correctly for the API-Access curl/scope boxes before
  this fix, just not with ellipsis truncation). `panels/widgets.rs::
  clipped_value`/`clipped_value_copyable` force `.truncate()` (single line +
  ellipsis) plus a hover tooltip with the full value, and the copyable variant
  adds click-to-copy with a toast. Swept onto every unclipped long-value
  `Label` across Properties/API-Access/Query (Node ID, API-Access Scope +
  Node ID, generated-token Client DID + curl command, Active-Tokens label +
  client DID, Query tab's scope hint). `inspector_entity_section`'s Node ID
  moved out of its `Grid` to match the existing `copy_value_box` convention
  (§widgets) — a Grid cell ignores `set_max_width` too.
- **F2 full-height Query tab.** `ui_shell::right_sidebar::section_chrome`
  wraps every right-sidebar section (Inspector included) in one
  `egui::ScrollArea::vertical().auto_shrink([false, false])`. In egui 0.33
  that ScrollArea's content `Ui` is bounded to the visible viewport, not
  grown to "infinite" for the scroll axis (`content_max_size` only goes
  infinite on a dead code path — see `egui::containers::scroll_area`), so
  `ui.available_height()` called inside `query_tab.rs` after laying out the
  editor/templates/buttons top-down *is* the real remaining panel height.
  The Result `ScrollArea` now sizes to that (floored at `RESULT_MIN_HEIGHT`)
  instead of a fixed `max_height(300.0)`, and renders unconditionally (an
  empty-state message before any query runs) so the tab fills the panel from
  the first frame rather than only after a result exists.
- **F3 scoped templates.** Real table/key shapes are in
  `fe-database/src/schema.rs`: `node` is keyed by `node_id` and carries a
  `petal_id` column (no scope field), `iot_reading` is keyed by `node_id`.
  `query_tab::query_templates` offers "This node" / "This node's device
  data" (when a node is selected) and always offers a petal-scoped "All
  nodes in this petal" (or an unscoped fallback with no active petal) —
  one click fills the SQL buffer, Run is still a separate step. Values are
  interpolated as SQL string literals (`sql_literal` escapes `'`/`\`)
  because `actions::query::submit` (`fe-ui/src/actions/query.rs`) always
  sends `RawQuery` with an **empty `vars` map** — the GUI tab has no
  bind-variable plumbing, so `$id`-style binds silently never resolve. The
  "Templates scoped to: <petal>" row is deliberately NOT phrased as an
  enforced filter: `UiAction::SubmitQuery`'s `scope` field is dropped
  (`scope: _`) at the `actions/mod.rs` dispatch arm, so a hand-edited query
  is never scope-checked server-side (that gap is a logged follow-up, not
  fixed here). The hint text was also corrected: `fe-database/src/handlers/
  raw_query.rs`'s `BLOCKED_KEYWORDS` blocks `RETURN` too, so "Only SELECT and
  RETURN statements are allowed" was actively wrong — it now reads "Only a
  single SELECT statement is allowed," matching the handler's own error.

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

Portal save and browser feedback now use the owned receipt contract documented above.

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
- **Result routing — by request id (residual CLOSED 2026-10-11,
  FEUI-QUERY-REPLY-ROUTING).** Every GUI `RawQuery` sender (GIS panel
  `actions/gis.rs::submit_query`, Paths list `actions/path.rs::query_tracks`,
  Query tab `actions/query.rs::submit`) mints `next_ui_query_id(kind)`
  (`ui-{kind}-{n}`, a process-wide counter — the hyphens make it impossible
  to collide with an API ULID), stores it in its own buffer
  (`GisPanelState.query_request_id`, `PathEditorState.tracks_request_id`,
  `InspectorFormState.query_request_id`) and sends it as `correlation_id`.
  The DB thread echoes it on `QueryResult` and answers a failure with
  `QueryFailed`; `db_results/query.rs::route_query_reply` lands the reply
  ONLY in the buffer holding that id, then clears the id + pending flag. A
  stale id (the user re-submitted, minting a new one) or an API waiter's
  ULID claims nothing and falls through to `try_deliver`. A bare
  `DbResult::Error` no longer touches any query buffer (it only clears a
  stranded pen create). The old flag-priority claim (GIS > Paths > Query)
  survives only for a legacy `correlation_id: None` result, which no GUI
  sender emits.
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
`SetPetalTerrain` sends during a drag.

**Unified overlay registry (VIS-PH5, D-11 / ui_ux §7).** The card renders
ONLY from `gis::overlay_registry(terrain_json, &VisibilityState,
&AppSettings) -> Vec<OverlayEntry { key, name, visible, opacity }>` — one row
widget for every overlay: the four terrain layers (`satellite`, `terrain`,
`gpx_track`, `geojson_overlay`; only when the petal has a map), the active
petal's Groups (registry order), then the scale bar and metric grid
(`AppSettings.show_scale_bar`/`show_world_grid` — the same fields the
Settings section edits, so there is still ONE state per overlay, not two
toggles). A checkbox dispatches `gis::apply_overlay_toggle(key, visible,
petal_id, groups)`: terrain layer → `GisSetLayer`, Group → whole-registry
`SetGroupRegistry`, ruler overlay → `SetAppOverlay` (applied by the reducer
via `apply_app_overlay`). It takes the petal id and current registry because
both persisted writes need them (a deviation from the planned two-argument
shape). GPX/GeoJSON rows are LIVE: an empty-source `gpx_track` /
`geojson_overlay` layer is a CATEGORY layer every ribbon/overlay binds to
(fe-terrain `src/AGENTS.md` §petal_binding) — the old inert "Coming soon"
rows are gone.

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
one panel query id per GIS panel keeps the routing simple (see the
id-routing note above). Rows show a color
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
   `PathEditorState.tracks_request_id` claims the correlated
   `DbResult::QueryResult`/`QueryFailed` reply in
   `verse_manager/db_results/query.rs` — routed by id, independent of the GIS
   panel and the inspector's Query tab (see the id-routing note above).
3. **Point-list editing reads back persisted `gpx_points` on track select.**
   Track-row clicks in `path_editor_card::render_track_list` push
   `UiAction::PathSelectTrack { track_node_id }` (selection bypasses the
   panel's own `db_sender`, so it must route through the action pipeline
   rather than calling `PathEditorState::start_editing` directly — mirrors
   why every other Paths op is a `UiAction`). The handler
   (`actions::path::select_track`) calls `start_editing` (clears the local
   buffer), sets `PathEditorState.points_pending = true`, and sends
   `DbCommand::GetNodeProperties { node_id: track_node_id }`. The reply lands
   in `verse_manager::db_results`' `DbResult::NodePropertiesLoaded` arm,
   gated on `path_state.editing_track_id.as_deref() == Some(node_id)` AND
   `points_pending` — a **separate claimant from the inspector's
   `is_for_selected_node` guard** on the same (uncorrelated, broadcast)
   result type, since the inspector may have its own `GetNodeProperties` in
   flight concurrently. On match, `properties.get("gpx_points")` is decoded
   via `gis::query::decode_gpx_points` (mirrors the bridge's
   `json_to_route_points` in `fractalengine/src/gpx_bridge.rs`: JSON array
   of `[x, y, z, time_seconds]`, best-effort-skipping malformed/short
   entries) into `PathEditorState.points`, then `points_pending` clears.
   After this, further edits within the session still build up purely via
   queued `PathOp`s (`actions::path::append_point`/`remove_point`/etc.) —
   only the *initial* load on track-select is a DB round-trip.
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
8. **Pen auto-create + deferred first-point flush** (correlation-id matched;
   `pen_autocreate_track_20260713` + HIGH-1/HIGH-2 hardening). The Pen tool no
   longer requires a track to be pre-selected. A Pen gesture with
   `editing_track_id == None` (`node_manager/path_point_interaction.rs`, see
   `node_manager/AGENTS.md` §pen-tool; resolved at gesture Release since
   `pen_curve_tool_20260722` FR-4) generates a **fe-ui-side correlation id**
   (`gis::next_pen_correlation_id`, a monotonic `pen-track:{n}` counter — no
   `rand`/`uuid` dep), stashes `PathEditorState.pending_pen_create` (that id +
   the gesture's FULL first-anchor payload: the world position PLUS
   `handle_in`/`handle_out`/`corner`/`smoothness` — the FR-4 must-fix, so a
   press-drag that STARTS a track keeps its curve), and queues
   `UiAction::PathCreateTrack { petal_id,
   "New Path", correlation_id: Some(id) }` for `NavigationManager.active_petal_id`.
   The append **cannot** be synchronous because the track's `node_id` doesn't
   exist until the `CreateTrack` round-trips.

   The id threads end-to-end: `UiAction::PathCreateTrack` → `PathOp::CreateTrack
   { correlation_id }` → the gpx bridge's `drain_path_ops`, which — when the op
   carries an id — reuses it verbatim on `DbCommand::CreateNode { correlation_id }`
   (only the manual "New Path" button leaves it `None`, for which the bridge
   generates its own id, unchanged). The DB echoes that id back on
   `DbResult::NodeCreated { correlation_id }`. `verse_manager::db_results`' arm
   flushes the pen point **only when the echoed id matches** the pending create
   (`take_pending_pen_create_if(cid)`), then `start_editing(new_id)` + pushes the
   first append — `PathAppendSmoothPoint` when the stash carries handles, else
   the legacy `PathAppendPoint`. Because the match is on the id — **not** a content
   heuristic (`!has_asset && in_active_petal`) — a concurrent foreign create (a
   GPX-import track/waypoint node, the create-entity dialog) carries a different
   id (or `None`) and can **never** hijack the flush (HIGH-1, the old heuristic's
   bug). **HIGH-2:** `PathCreateTrack` failure surfaces as `DbResult::Error`
   (never `NodeCreated`), and `Error` carries only a message — no node/correlation
   id — so `db_results` clears `pending_pen_create` best-effort on **any** `Error`
   while a pen create is pending (with a `warn!`); otherwise a failed create would
   strand the pending state and leave the pen permanently dead. The next Pen click
   simply starts a fresh auto-create.
9. **Per-track style controls persist on release, not per-frame** (MEDIUM-2,
   `track_styling_20260713`). `path_editor_card::render_style_controls` binds the
   border/fill color pickers, border-width slider, closed-zone toggle, and
   visibility checkbox to
   `PathEditorState.edited_track_style`. A drag fires egui `.changed()` every
   frame, and each `PathSetStyle` → `SetNodeProperty` → refetch →
   despawn/respawn/ribbon-rebuild round-trip in the gpx bridge is expensive
   (dozens of full mesh rebuilds per drag). So `edited_track_style` is still
   mutated live every frame (the widget shows the value immediately), but the
   returned persist signal only fires on the **settle**: the slider on
   `drag_stopped()` (or a non-drag `.changed()` for keyboard/step); the color on
   a `.changed()` observed while the primary pointer is released (the button
   response can't see the popup's internal slider drag, so `drag_stopped()` is
   unusable there); the checkbox immediately (a single click, no drag churn).
   Live visual feedback is preserved; each changed field writes once at release.
   Absent `gis.track.closed` / `gis.track.fill.color` properties default to
   `false` / transparent, so legacy tracks remain visually unchanged.
   The adjacent snap controls are session-local editor settings: anchor-first
   within a meter radius, then XZ grid, with Y preserved. The viewport converts
   both distances through the active petal's `world_scale`; Pen append and
   single-point release consume them, while whole-track transforms do not.
10. **Corner settings card** (`pen_curve_tool_20260722` FR-6 + ratified Q5).
   The per-anchor corner-settings CALL SITE moved to the Options section
   (tool_options Pen arm); pure math + tests stay in path_editor_card.rs.
   `path_editor_card::render_corner_settings` is the "Corner settings —
   vertex N" sub-card in the edit view, gated on `selected_point`: a
   Corner/Smooth/Symmetric toggle + a smoothness slider `0.0..=1.0` (unitless
   per `ui_ux.md §2`; handle lengths shown in m). It follows item 9's
   deferred-push idiom: live-edits the row (deriving collinear handles via
   `node_manager::curve::derive_symmetric_handles` each changed frame for
   instant marker echo) and persists once on settle as `PathSetAnchorHandles`;
   the toggle persists `PathSetAnchorCorner` via the pure
   `corner_toggle_outcome`. **The toggle is NON-destructive** (post-review
   hardening): an anchor whose handles are already symmetric-collinear
   reclassifies only — a same-kind re-click is a full no-op and a
   Smooth↔Symmetric switch never touches hand-pulled handles; the re-derive
   fires ONLY from the broken/asymmetric grey-out state (at the geometry
   READBACK, never the stored scalar — the explicit Q5 re-enable route) or on
   a handle-less anchor (at the `max(stored, 0.5)` floor, so the handles
   exist and the classification survives the Q6 reload normalization). When
   nothing is derivable (no neighbors) the kind stays/reverts to Corner —
   never persist a handle-less Smooth that reload would silently demote; the
   same coherence rule (`zero_handle_kind_revert`) reverts the kind, locally
   AND persisted, when the slider is settled at exactly 0. **Q5 grey-out:**
   the pure `smoothness_slider_enabled` predicate (both handles `None`, or
   `handle_in ≈ −handle_out` within epsilon) gates the slider —
   manually-broken handles show the pure `gis::smoothness_readback`
   (`|handle_out| / (k·min_neighbor_gap)` clamped 0..1) as a disabled value +
   hint instead; the slider never silently overwrites manual handles. The
   ENABLED slider seeds from that same readback whenever the anchor HAS
   handles (`slider_seed`) — the stored scalar (only trusted for handle-less
   anchors) can be stale, and every commit path now also stores the readback
   (`node_manager/AGENTS.md` §pen-tool). Authority B by construction: the
   card mutates `PathEditorState.points` + pushes `UiAction`s;
   `render_edit_view` still receives no `NodeManager`. The read-only twin
   (inspector `anchor_readout`) and the tool-level default
   (`pen_new_anchor_kind`) are documented in `panels/AGENTS.md`
   §tool-inspector / §tool-panel.

## §data-icons — type icons on three surfaces (`data_icons_20260713`)

"Icons for the data": path points and single-point track nodes read as bare
spheres and panels list plain text, with no type legibility. Three surfaces,
each independent:

- **Panel row glyphs (FR-1, `panels/path_editor_card.rs`).** Track rows and
  point rows get a recolorable geometric glyph prepended. egui recolors plain
  `\u{25xx}`/`\u{27xx}` codepoints reliably (color emoji do NOT — they render
  in their own palette), so `type_glyph`/`GLYPH_*` are single-scalar geometric
  shapes: `\u{29BF}` (track), `\u{25CF}`/`\u{25CB}` (timed/untimed point),
  `\u{25C6}` (waypoint), tinted by `theme::ICON_TRACK`/`ICON_POINT`/
  `ICON_WAYPOINT`. `type_glyph(gpx_type)` is the pure `gpx_type → glyph` map,
  `pub(crate)` and shared with FR-3; its module is `pub(crate)` only so
  `viewport_labels` can reuse it. Mirrors the `sidebar.rs:306` `◆`/`●`
  precedent.
- **3-D billboard markers (FR-2).** A `Billboard` marker component
  (`plugin.rs`, `pub`, constructable from both fe-ui and
  `fractalengine::gpx_bridge`) + `node_manager::billboard::billboard_face_camera`
  — a standalone per-frame system that copies the `OrbitCameraController`
  camera's world rotation onto every `Billboard` `Transform`, so a flat
  `Rectangle` icon quad stays parallel to the camera plane (a quad lies in
  local XY, +Z normal; matching the camera's +Z points the face at the viewer).
  Path-point markers (`path_point_interaction::sync_path_point_markers`) and
  single-point track nodes (`gpx_bridge::spawn_single_point_node`) spawn a
  double-sided (`cull_mode: None`) unlit quad instead of a sphere. **Picking is
  preserved**: the single-point node keeps its `Mesh3d`, so it still yields an
  `Aabb` for §glb-mesh-picking. The quad's local AABB is thin in Z, but
  billboarding keeps it presented head-on, so the ray/slab test always crosses
  it cleanly. Rotation is orientation-only, so the system runs outside the
  selection `.chain()`.
- **3-D floating labels (FR-3, `viewport_labels.rs`).** An egui screen-space
  overlay — NOT in-world text meshes (`bevy_text`/`Text2d` are not enabled).
  `draw_viewport_point_labels` runs in `EguiPrimaryContextPass` after
  `gardener_ui_system` (so it reads the same-frame `ViewportRect`), projects
  each edited-track point via `Camera::world_to_viewport` (viewport coords ==
  egui screen coords for the fullscreen CentralPanel, same basis the gimbal
  uses), and paints a translucent `glyph + index` label via `layer_painter` in
  `theme::TEXT_VIEWPORT_HINT`. Gated to `editing_track_id.is_some()` so labels
  only appear while drawing a path, and clipped to `ViewportRect` so a label
  that projects under a side panel is dropped.

## §logging

Convention (issue #16, clippy-quality track): ECS/Bevy code (all of `fe-ui`,
`fe-webview/src/plugin.rs`) logs via `bevy::log::{debug,info,warn,error}` so
output routes through Bevy's `LogPlugin`. Non-ECS code (webview backends, DB /
sync / network threads) logs via `tracing::` directly. Do not mix the two
within a single module; silent `.ok()` swallowing of errors that deserve a log
line should become a `warn!`/`error!` when touched.

## §mesh-budget — app-wide mesh-instance watchdog (oom_guardrails_20260717)

A 2026-07-17 crash queued ~15.7M mesh-instance slots in one render phase
(2.75 GB MeshUniform buffer > the 2 GiB `max_*_buffer_binding_size` limit),
panicking bevy_pbr's bind-group creation and then OOMing the host. Bevy's
per-phase instance buffers are grow-only and wholly bound, so the only real
guard is keeping the main-world instance count bounded BEFORE render
extraction — post-panic remediation is impossible.

Guard architecture (in order of defense):
1. **Per-path caps** (primary): `MAX_STAMPS` (4096/track) +
   `MAX_STAMPS_PER_PETAL` (65,536/pass) in `verse_manager/path_asset_*`,
   `MAX_PETAL_NODES` (10,000) in `verse_manager/spawn.rs` (enforced by both
   `petal_respawn` and the `HierarchyLoaded` handler), `MAX_POINT_MARKERS`
   (4096) in `node_manager/path_point_interaction.rs`. All saturate + `warn!`.
2. **`MeshInstanceBudget` watchdog** (backstop, `plugin.rs`): counts
   `Query<(), With<Mesh3d>>` in `Last` (archetypal filter — O(archetypes)).
   Ceiling `MAX_MESH_INSTANCES = 2,000,000`: with the worst realistic view
   multiplier (main + 4 shadow cascades = 5 slots/entity) that is 10M slots,
   under the ~12.2M/phase hard limit (2 GiB / 176 B per MeshUniform), and caps
   the CPU-side input Vec (~96 B/entity) at ~192 MB. On crossing: `error!` +
   toast (HMI rule: no silent failure) and `exceeded = true`, which every
   fe-ui spawn path checks before spawning more scene entities. The gate
   clears automatically when the count drops (petal switch, deletes).
3. Render-degrade switches (NoIndirectDrawing, CPU preprocessing) do NOT
   avoid the panic (same 2 GiB entire-binding limit on the CPU path) — they
   were deliberately not applied.

The `HierarchyLoaded` handler additionally skips nodes that already have a
live `SpawnedNodeMarker` entity (stamp instances excluded via
`Without<PathAssetInstance>`): every API `GetHierarchy` poll re-broadcasts
`HierarchyLoaded`, and before this guard each event duplicated the entire
active petal's scene — the amplification behind the crash.

## §app-settings — the D-78 application settings surface

`settings::AppSettings` (Resource) is the single home for user-tunable knobs
(D-78, ratified 2026-07-18). It resurrects the `render_distance_lod_20260407`
design. Fields: `render_distance` (global soft-residency horizon, default 500;
`0` halts spawning), `mesh_budget_ceiling` (seeded from
`plugin::MAX_MESH_INSTANCES`), `entity_cap` / `stamp_ceiling` (soft per-petal
caps, defaults mirror the hard caps), `tile_mode`, `camera_sensitivity` /
`camera_zoom_speed` / `camera_easing`, the ruler toggles `show_scale_bar`
(default on) / `show_world_grid` (default off), and `relay` (P2P relay spec).
The `const` HARD caps stay as the never-crash backstops; the settings fields are
soft budgets routed through them (`.min(HARD_CAP)` at each spawn site, and
`sanitize` clamps to the same caps).

- **Persistence (PAS-P4B).** `settings_path()` = `dirs::config_dir()/fractalengine/settings.ron`
  (`%APPDATA%\fractalengine\settings.ron` on Windows). `load_settings_from`:
  missing → defaults; partial → `#[serde(default)]` fills the gaps; unparseable
  RON or non-UTF-8 bytes → defaults + a warning, and the bad file is renamed to
  `settings.ron.corrupt-<unix-secs>` so it is kept for inspection and the next
  save cannot overwrite it; any other read error (e.g. permissions) → defaults
  + a warning with the file left in place. Every load ends in `sanitize`. `save_settings_to` is
  atomic (write `settings.ron.tmp`, then rename over).
- **Who writes the disk.** Only a host that inserts `SettingsPersistence { path }`
  (the GUI binary, after `load_startup_settings()`) gets saves; tests and other
  hosts never touch the user's config dir. `save_app_settings_debounced` uses
  `SettingsSaveDebounce`: the first observed value is the loaded state (never
  re-saved), every value change restarts a `SAVE_DEBOUNCE_SECS` (1 s) timer, and
  a value that returns to the saved state cancels the pending save — so dragging
  a slider writes once, after the drag settles. Comparison is by value
  (`PartialEq`): the Settings section takes `&mut AppSettings` every frame, so
  `is_changed()` would always be true. A failed save warns and is not retried
  until the next change. A change made less than 1 s before quitting is lost
  (no save-on-exit hook).
- **`sanitize`** clamps: `render_distance` finite `0..=RENDER_DISTANCE_MAX`
  (50 000; non-finite → default), ceilings `1..=` their hard caps, camera
  multipliers `CAMERA_MULTIPLIER_RANGE` (0.1–10), `camera_easing` 0–1, and an
  unparseable `relay` → `"default"`. The Settings section calls it every frame
  before drawing (so a bad value never renders).
- **Live consumers.** `sync_app_settings_to_mesh_budget` mirrors
  `mesh_budget_ceiling` into `MeshInstanceBudget.ceiling`;
  `sync_app_settings_to_camera` sets `OrbitCameraController.sensitivity` /
  `zoom_speed` to `BASE_CAMERA_* × multiplier` (the base consts mirror
  `OrbitCameraController::default()`, pinned by a test). `entity_cap` is the
  residency ledger's live cap (§residency-ledger); `stamp_ceiling` applies on the
  next restamp.
  `tile_mode` is applied by the main binary's `terrain_bridge::apply_global_tile_mode`
  (only `Offline` overrides the petal's mode; fractalengine `src/AGENTS.md`
  §tile-mode). `relay` is read once at startup by fe-sync (`FE_SYNC_RELAY` env
  wins; fe-sync `src/AGENTS.md` §relay-health); its text field stores only a
  spec `RelayConfig::parse` accepts and says "Applies after restart".
- **Knobs not exposed (ui_ux §6 — no faked settings).** `camera_easing` is
  persisted but has no consumer (the controller's `easing_rate` is a rate, not
  the 0–1 factor), so it has no widget.
- **Per-petal render distance (p2p_asset_streaming S6, closes the prior open
  decision).** `petal.render_distance` is now a durable, replicated column
  (fe-database `src/AGENTS.md` §handlers "Petal render distance";
  `fe-runtime::messages::{Set,Get}PetalRenderDistance` /
  `PetalRenderDistanceLoaded`), carried on `PetalHierarchyData`/`PetalEntry`.
  `actions::hexon::manifest_save` now sends `SetPetalRenderDistance`
  (`CallerAuth::Local`) instead of touching the session override — a manifest
  save is a durable, shared setting. `PetalRenderOverride { petal_id,
  render_distance }` stays session-only, for a viewer on a weak machine: set by
  `actions::hexon::set_render_override` (`UiAction::PetalSetRenderOverride`),
  reset by `clear_petal_render_override_on_nav_change` on petal change.
  Precedence, highest first: session override → petal column → global
  (`effective_render_distance(global, petal_column, session_override)`, each
  candidate finite+positive or skipped, the winner clamped to
  `RENDER_DISTANCE_MAX` — the same ceiling `sanitize` puts on the global knob,
  and the schema `ASSERT` puts on the column); `ResidencyBudget::render_distance_for(petal_id,
  petal_column)` (verse_manager/spawn.rs) is what every spawner reads, the
  petal_column argument coming from `VerseManager::find_petal(id).render_distance`.
  A peer's `ReplicatedRowApplied { table: "petal", .. }` for the active petal
  re-requests `GetPetalRenderDistance` (`db_results/mod.rs`) so a peer's change
  lands without a full hierarchy reload.
- **Ruler toggles cross a crate boundary.** fe-ui has no fe-terrain
  dependency, so the main binary's `terrain_bridge::sync_ruler_settings` mirrors
  `show_scale_bar`/`show_world_grid` into fe-terrain's `RulerSettings`. Keep the
  names: they are persisted fields and VIS-PH5's overlay registry rows.
- **The Settings surface** is the right-sidebar Settings section
  (`ui_shell/right_sidebar.rs::render_settings_section`), which reads/writes
  `AppSettings` directly; there is no Settings dialog or toggle action.

## §terrain-map — Hexon Manager, P2P tileset downloads (`p2p_asset_streaming_20260718` S4)

`terrain_map/` owns which tileset(s) the active petal uses (`PetalMapState`),
the hexon registry op queue the main binary drains (`PendingHexonOps`), and
the sync-thread event drain that feeds the Hexon Manager's Installed /
Available / Downloads tabs (`dto.rs`, `events.rs`, `mod.rs`).

- **`TilesetDownloads` is independent of the dialog.** Before S4,
  `download_progress` lived inside `ActiveDialog::HexonManager`, so closing
  the Maps section (which tears down that carrier —
  `ui_shell/right_sidebar.rs::clear_maps_carrier`) silently stalled any
  in-flight download: `drain_tileset_events`'s old ping-pong (`ChunkReceived`
  → send the next `RequestChunk`) only ran `if let ActiveDialog::HexonManager`.
  `TilesetDownloads` (a top-level resource) now holds the
  `DownloadProgressMap` for the whole session; `dialogs::hexon_manager::render_hexon_manager`
  takes it as a read-only `&DownloadProgressMap` param instead of owning it.
  `fe-ui/src/actions/mod.rs::ToolStateParams` and
  `plugin.rs::MiscUiParams` both carry it (param-ceiling bundles, see their
  own doc comments) — `start_download`/`cancel_download` write it,
  `drain_tileset_events` is its other writer, everything else reads.
- **No more chunk ping-pong.** Since fe-sync S3 wired
  `SyncCommand::DownloadTileset` to drive meta resolution AND every bundle
  fetch on the sync thread, `start_download` sends exactly one
  `DownloadTileset` and `drain_tileset_events` only updates progress —
  `TilesetMetaReceived` sets the chunk-count estimate (or, with an empty
  `root_hash` — fe-sync's honest failure reply — sets `Failed` with the
  reason from `meta_json.error`), `ChunkReceived` bumps
  counters, `TilesetDownloadComplete` pushes
  `HexonOp::AssembleDownload { tileset_id, index_path, bundle_paths }` for the
  main binary. `TilesetDownloadResumed` (emitted at sync-thread startup from
  the persisted ledger, S3) seeds/updates the same tracker so a download that
  completed before a restart still shows as resolved in the Downloads tab.
  `Complete` is set by the host (`terrain_bridge::drain_hexon_assemble_results`)
  once the install succeeds — fe-ui never sees the install itself.
- **Assembly/install never happens in fe-ui.** fe-ui has no fe-terrain
  dependency (root `AGENTS.md` §petal-map) — `HexonOp::AssembleDownload` is
  just a queued request; `fractalengine::terrain_bridge::drain_hexon_ops`
  verifies the bundle files' hashes against the index (again — defense in
  depth, S2's `TilesetTransfer` already verified each on receipt),
  assembles + installs via `fe_terrain::tiles::assemble_bundles`, and refreshes
  the dialog's installed list, on a background thread (never the Bevy main
  thread — NFR-4/S4 invariant).
- **Seeding (`HexonToggleSeeding`/startup)** sends `SetSeeding` into
  `PendingHexonOps` as before; the main binary additionally bundles the
  tileset (`fe_terrain::tiles::bundle_for_seeding`, cached under
  `<hexon store>/bundles/<id>/`, reused when the source hash still matches)
  and sends `SyncCommand::PublishTileset` — see
  `fractalengine/src/AGENTS.md` §tileset-p2p for the full host-side flow,
  shared verbatim by the headless relay's startup seeding.

## §residency-ledger — the D-74 node + stamp-group pointer-set (p2p_asset_streaming S8)

`verse_manager/residency_ledger.rs` owns `ResidencyLedger`, the per-frame
pointer-set of what the active petal keeps resident: `admitted_nodes`
(node ids) and `admitted_stamp_groups` (path-asset track ids). It is the
node/stamp half of D-74; fe-terrain's `TileBundleDemand` (S7) is the tile half.
It replaces the old "spawn everything up to a hard cap at petal entry, never
despawn for distance" behaviour.

- **Placement.** `update_residency_ledger` runs in the `VerseManagerPlugin` chain
  right after `apply_lifecycle_events` + `apply_db_results` and before
  `respawn_on_petal_change`. It sees this frame's tree and runs ahead of every
  spawner, which is how D-74's "first system" is read (§8 assumption 3: first
  ahead of the spawners, not first in the frame).
- **Ranking (B2).** Weighted distance = `d × (1 − 0.5·max(0, cosθ))`, with θ
  the angle to camera-forward (the single `OrbitCameraController`'s
  `GlobalTransform`). A node **enters** at weighted ≤ r and **exits** at
  weighted > 1.5 r. Weighted ≤ raw, so turning the camera can never evict
  anything inside raw 1.5 r. A forward node at raw 1.8 r is admitted (weighted
  0.9 r) and stays until its weighted distance passes 1.5 r. It can drop when
  the camera turns away, which is allowed because it is outside r. (Final
  review 2026-10-14: the exit used to be "raw > 1.5 r and weighted > r", which
  gave a node straight ahead NO band — it entered and left at the same raw 2 r,
  so camera jitter there flapped its scene; test
  `a_node_straight_ahead_keeps_a_hysteresis_band`.) Candidates sort by tier, then weighted
  distance, then tree order. Tier 0 is the selected node, tier 1 is inside raw
  r, tier 2 is the rest. So when the cap (`entity_cap.min(MAX_PETAL_NODES)`)
  saturates, a band node never displaces one inside r.
- **Inputs.** r = `ResidencyBudget::render_distance_for` (S6 precedence:
  session override, then petal column, then global) × the GPU-pressure scale.
  - `mesh_budget.exceeded` sets `admit_new = false`. Nothing new is admitted
    and nothing still in the ring is evicted (prevention, not recovery, NFR-1).
  - The selected node (`NodeManager`) is always admitted.
  - With no orbit camera (headless apps, tests without one), every distance is
    `0`, which is the pre-ledger batch behaviour.
  - A non-finite camera freezes the ledger for that frame.
  - Pure `select_admitted` treats NaN or negative input as "admit nothing", and
    so does an r that is not positive and finite. r = 0 stays the
    "render nothing" kill switch.
- **No per-frame allocation.** The ranked indices and candidates live in reused
  scratch `Vec`s. The admitted `HashSet<String>`s are rebuilt, and `node_epoch`
  / `stamp_epoch` bumped, only when membership actually changes. The ledger
  writes through `bypass_change_detection` and calls `set_changed` only on a
  transition. Consumers key on the epochs, never on `is_changed()` of a
  per-frame resource.
- **Consumers.**
  - `respawn_on_petal_change` spawns only admitted nodes at petal entry, with
    `MAX_PETAL_NODES` and the mesh gate as backstops.
  - `apply_node_residency` runs right after it. It spawns newly admitted nodes
    nearest-first (≤ `MAX_NODE_SPAWNS_PER_FRAME` = 256, carrying a backlog) and
    `try_despawn`s evicted ones, never recursively (anti-pattern #5).
    `kept_node_ids` is collected before any despawn is issued. It runs on an
    epoch change, on a backlog, or when a `ResidentNode` was spawned elsewhere
    this frame (`GltfImported`). That last trigger is a drift check, so a far
    node spawned outside the ledger is reconciled the same frame.
  - `materialize_cached_primitives` only freshly materialises admitted nodes.
  - `HierarchyLoaded` (`db_results/hierarchy.rs`) skips non-admitted nodes. An
    API `GetHierarchy` poll therefore can't spawn a petal's far nodes for one
    frame and then despawn them.
- **`ResidentNode` marker** (`spawn.rs`). `spawn_node_entity`,
  `spawn_primitive_entity` and `spawn_fallback_sign` tag the tree-node entities
  whose lifetime the ledger owns. Stamps (`PathAssetInstance`), GPX ribbons and
  single-point track icons also carry `SpawnedNodeMarker`, but they are not
  `ResidentNode`s and are never touched by `apply_node_residency`.
- **Stamp groups (§8 assumption 4).** A track's group is the residency unit.
  It is admitted when its nearest polyline point (raw distance, no forward
  weighting) is within r and evicted beyond 1.5 r. The admission bit is part of
  the `PathAssetApplied` gate, and `materialize_path_assets` also wakes on
  `stamp_epoch`. A group therefore restamps or tears down only on a residency
  transition, never per frame. When `take_stamp_budget` truncates, it drops the
  farthest stamps first (`stamps_to_keep`, nearest to the ledger's camera).
  Because survivors are then not a prefix, `StampTrackRenderData.stamp_indices`
  maps a pick-index hit back to the stamp index (`pick_stamp`, used by
  `node_manager::context_pick`). `stamp_is_orphaned` + the delete cascade are
  unchanged. Per-sample residency is the follow-up if in-app shows waste (plan
  §6).
- **B3 closed loop.** `fe_runtime::diag15m::RenderPressure` (one `Arc<AtomicU64>`
  in both worlds) carries the largest instance-buffer binding the render world
  needs (fe-runtime `src/AGENTS.md` §render-pressure).
  `radius_scale_for_pressure` maps it to a scale:
  - `1.0` below 1 GiB;
  - a linear ramp to `0.25` at 1.5 GiB, held there up to 1.75 GiB;
  - `0` at 1.75 GiB or more (admission halts), against the 2 GiB limit.

  `step_pressure_scale` applies drops at once but recovers at most 0.02 per
  update. Without that limit, eviction would lower the pressure, readmission
  would raise it again, and the two would oscillate. Under pressure the exit
  ring shrinks with the scale, floored at 0.25. That floor is why the halted
  state still sheds load beyond 0.375 r.
- **`distance_ranked_allowance`** (spawn.rs, pure) is now only the stamp budget
  taper (called at distance `0` → the full `stamp_ceiling`). Node residency is
  the ledger's.
- **Readout.** The Settings section shows
  "Resident: N nodes · M stamp groups · K tile bundles (X MiB)".
  `update_residency_readout` (ui_shell/right_sidebar.rs) copies ledger counts
  plus `fe_runtime::diag15m::ResidencyStats` into `RightSidebarState.residency`,
  writing only on change. The main binary mirrors the fe-terrain counters into
  `ResidencyStats` (`terrain_bridge::mirror_tile_residency_stats`), because fe-ui
  has no fe-terrain dependency.
- **Hard backstops kept.** `MAX_PETAL_NODES`, `MAX_STAMPS`,
  `MAX_STAMPS_PER_PETAL`, the 2M mesh watchdog and terrain `max_chunks` are
  untouched (decision record "Ratification" §3).
- **Decision 2.6.** FR-4 subsumes runtime_instance_guardrails FR-6. That
  track's "dense-petal camera-locality" acceptance is carried by
  `moving_camera_streams_nodes_in_and_out_with_hysteresis` plus a USER_GATED
  live run.

## §terrain-proposal-editor — analytics-first proposed overlays (FR-5)

`terrain_proposal_state` holds the NON-destructive terrain-editor state
(terrain_editor_overhaul FR-5): `ProposalEditState` (Resource) = the live
`Vec<ProposalRecord>` + `selected` + armed `active_op`. `ProposalRecord { id,
op, footprint, target_height, delta}` and `ProposalOp` (Raise…Fill) are the
**fe-ui-local mirror** of the persisted JSON contract — deliberately NOT
`fe_terrain::TerrainProposal` (fe-ui must not depend on fe-terrain). Serde field
names + the `snake_case` op tags match the contract exactly; `target_height`/
`delta` omit when `None`.

- **Persistence is additive (NFR-1).** `UiAction::TerrainProposalAdd`/`Delete`
  route through `actions::terrain_proposal`, which serializes the whole set
  (`terrain_proposal_state::to_json`) and embeds it under the petal terrain
  config's **`proposals`** key via the pure `embed_proposals` — preserving every
  other field (origin, layers, tileset uris, `world_scale`). Same "mutate one
  field of the stored terrain JSON, then round-trip via `SetPetalTerrain`" idiom
  as `actions::gis::set_layer`. The true heightfield / loaded tileset are never
  written. **P0 fix (ui_shell_architecture_20260724 Phase 0, H-C1):** a petal
  with no map yet used to get a bare `{"proposals": [...]}` skeleton, which
  fails to deserialize into `fe_terrain::config::TerrainConfig` (`enabled`/
  `origin`/`tile_source_url` have no `#[serde(default)]`) — confirmed
  non-fatal there (`terrain_config_from_petal_json` catches the `Err` and
  warns), but still a silent-failure surface (`ui_ux.md §6`). `embed_proposals`
  now seeds the complete baseline shape (`baseline_terrain_doc`, mirroring
  `terrain_map::tileset_to_terrain_json`'s fields, `enabled: false`) for any
  `None`/non-object base, so every terrain-JSON reader always sees a
  well-formed doc.
  **Hydration (`ui_semantics_unification_20260808` Phase 1).**
  `ProposalEditState.hydrated` gate: on petal switch,
  `load_petal_terrain_on_nav_change` (terrain_map/mod.rs) triggers
  `DbCommand::GetPetalTerrain`, and the `DbResult::PetalTerrainLoaded` handler
  (verse_manager/db_results/terrain.rs) rehydrates `ProposalEditState` via
  DescriptorCaches.proposal_state before rendering. The doc's `proposals`
  array holds TWO record shapes — palette `ProposalRecord`s and
  Brush/shape-tool earthwork regions tagged by a `material` key (see
  "Sculpt & earthwork regions" below) — so hydration parses back ONLY
  palette-shaped entries (any entry carrying `material` is filtered out
  BEFORE `ProposalOp` parsing); a region entry never enters the mirror, so it
  is never silently absorbed (stripping `material`) nor silently dropped for
  an op `ProposalOp` doesn't recognize (`level`/`smooth`).
  `reset_for_petal_switch` clears the state. `embed_proposals` reconciles
  differently depending on `hydrated`, and is precisely NOT a single
  "read-modify-write" for both cases:
  - **Hydrated** — a **shape-preserving merge**: palette-shaped doc entries
    are wholesale-replaced by the mirror's current records (so palette
    add/delete/edit all apply, matching "the mirror is authoritative"),
    while every material-tagged region entry passes through the doc
    byte-identical. No sequence of palette adds/deletes can remove or alter
    a region — this is the data-loss-guard invariant.
  - **Unhydrated** — a **tombstone-aware union**: every doc entry survives
    except ids in `ProposalEditState.pending_deletes` (ids deleted locally
    before this petal's mirror ever hydrated — without this exclusion, a
    pre-hydration delete of an already-persisted id is silently
    resurrected). An id present in the local records wins over the doc's
    copy for that id (handles a locally-minted `p{n}` id colliding with an
    already-persisted one); every other id is appended as new.
  The cut/fill report (proposal_report_panel) parses `Brush`/shape regions
  directly off the terrain doc via `earthwork_regions_from_terrain`, using the
  same `material`-key discriminator — disjoint from the palette mirror by
  construction post-hydration-filter, so totals never double-count a region
  that also happens to be visible to a hydrated mirror.
- **Ids** are minted by a monotonic `ProposalEditState` counter (`p{n}`, no
  `uuid`/`rand` dep — mirrors `gis::next_pen_correlation_id`). `replace_all`
  keeps the counter past any rehydrated `p{n}` so a load-then-add never collides.
- **The editor *panels* are w4a's.** `ProposalEditState` (state) + the
  add/delete/persist plumbing are owned here; the tool palette, brush UI, and
  report panel are w4a. Proposal overlay *meshes* must spawn through the same
  residency/mesh-budget gating (NFR-2) — no bypass.

### Sculpt & earthwork regions (sculpt_earthwork_regions_20260725, D-A8)

The sculpt tool **evolves this proposal path, does not fork it** (FR-6): a
defined-shape/brush earthwork edit is persisted as an *enriched* record in the
SAME `terrain.proposals` block (adds a `material` tag), so it round-trips through
`SetPetalTerrain` with no new config surface. `actions::terrain_proposal` owns:

- `SculptToolState` (Resource): armed `shape_mode` (Brush/Circle/Rect/Polygon) +
  `op` (Raise/Lower/Level/Smooth, fe-ui-local mirrors of
  `fe_terrain::sculpt::SculptOp` — no fe-terrain dep), `radius`/`strength`/
  `target_height`/`delta`, `material` (single-material `"earth"` default, Q-3),
  `region_draft` (polygon points). Brush controls remain meters in UI state;
  the viewport snapshot converts them to petal-local world units.
- Pure helpers: `region_json` (mirrors `EarthworkRegion`'s serde shape by
  contract), `embed_region` (additive append, same baseline guarantee as
  `embed_proposals`), `remove_region` (the **Q-2 revert** — dropping the record
  un-bakes the non-destructive overlay; the true heightfield was never written).
- Handlers `handle_{brush_stroke,shape_region,delete_region}` persist via
  `persist_doc`; a stroke appends all dabs with one document clone/write.
  `handle_shape_region` converts its `footprint`/`target_height`/`delta` from
  petal-local meters (N-1, the queueing panel's convention) to world units via
  `PetalMapState.world_scale` before persisting — mirroring
  `BrushSnapshot::from_state`'s conversion for the freeform stroke — so every
  persisted region entry is world units, matching Brush's own regions and
  every downstream reader (finding #11 fix, `ui_semantics_unification_20260808`).
- The **cut/fill report** is in `panels::proposal_report_panel`
  (`earthwork_kind` + `cut_fill_totals`): per-region fill/cut/net + totals across
  all regions (FR-5). The true separated cut+fill over relief is
  `fe_terrain::sculpt::cut_fill_volume` at bake.

**Commit line (LIVE, integration pass 2026-07-26):** `render_brush_controls` is the single copy,
hosted by the Options Brush arm (tool_options.rs). It configures `SculptToolState`; viewport press snapshots the active petal,
sanitized scale, and converted controls, and release directly queues one
fully-threaded `SculptBrushStroke`. Committed regions
also become addressable node rows (`EarthworkNodeMap`, `earthwork:{region_id}`
correlation, tombstone-on-delete) and receive bake-reported cut/fill volumes —
see `actions/AGENTS.md` §sculpt.

### Live pre-commit preview (terrain_proposal_delta_viz_20261025 S3, FR-3)

`terrain_preview::publish_proposal_preview` publishes the armed tool's draft
as `fe_renderer::terrain_overlay::ProposalPreview` (the S1b cross-crate seam
— fe-ui gains no fe-terrain dependency, NFR-5) BEFORE any
`TerrainProposalAdd`/`SculptShapeRegion`/`SculptBrushStroke` commits, so S2's
renderer can show the lattice ghost + cut/fill heatmap live. Four sources,
most-live wins:

1. **Brush stroke** — `node_manager::brush_interaction::BrushGesture::preview_snapshot`,
   a read-only accessor (the ONLY crate-visible window onto that module's
   otherwise-private gesture state; `brush_interaction` was widened from
   private to `pub(crate)` in `node_manager/mod.rs` for exactly this — the
   one supporting edit outside this slice's own file list). Its footprint is
   the SAME `stroke_corridor_footprint` the release commit uses — preview ==
   commit by construction (plan.md §5 tripwire).
2. **Sculpt shape draft** — a defined Circle/Rect/Polygon footprint while
   Brush is the active viewport tool (freeform Brush shape mode has nothing
   to preview until a stroke starts; that's source 1). Footprint =
   `shape_commit_footprint_world` (the Apply button's `shape_apply_footprint`
   → `meters_to_world`, exactly as `handle_shape_region` commits it), and NO
   strength (the committed shape record carries none → full strength).
3. **Armed palette op** — a circle at the origin (the same "append at
   origin" convention `terrain_tools_panel::render_controls_and_emit` uses),
   shown only while the TerrainTools section is the active right-sidebar
   section AND no proposal just committed this frame.
4. **Selected committed proposal**, readout-only fallback
   (`PreviewSource::SelectedProposal`: fe-terrain reports but spawns no
   duplicate mesh), so the Proposals list selection still drives a live
   cut/fill line when nothing is armed.

**Params mirror the committed record (S7).** Brush and shape previews send
the region RECORD's semantics, not the raw tool fields:
`actions::terrain_proposal::brush_region_fields` (shared with
`handle_brush_stroke`) folds strength into Raise/Lower `|delta|×strength`
and stores it as the Level/Smooth blend, then `region_record_preview_params`
maps the record back to `(target, delta, strength)` like fe-terrain's
`EarthworkRegion::effective_*`. Previewing raw `delta` at partial strength
showed twice the committed volume. The readout labels `wu³` + "no map
scale" when `ProposalPreviewReport.scaled` is false (NFR-4). An identical
draft is never re-inserted (compare-before-insert), so fe-terrain's idle
path sees an unchanged resource and allocates nothing.

**Units:** `ProposalPreview::target_m`/`delta_m` are REAL METERS (fe-terrain's
cut/fill math wants real units, NFR-4). `ToolPanelState`/`ProposalRecord`
values are WORLD units (unconverted, per `render_controls_and_emit`'s own
convention) and get `geometry::world_to_meters`'d at the merge boundary;
`SculptToolState` values are already petal-local meters (N-1) and pass
through unconverted — except the shape FOOTPRINT, which is converted to
world (S7 fix: it used to be sent in meters, mis-sizing/mis-placing the
preview and the Volume report's drawn selection on any scaled petal). The
footprint is WORLD units in all four sources — it's sampled against the (world-unit) height field, and
`cut_fill_volume`-family math applies its own `/scale`/`/scale²`.

**No preview is the resource's ABSENCE**, not a zeroed value — `commands.
remove_resource::<ProposalPreview>()`, never a default/empty struct — so S2
can't mistake a stale draft for a live one. Clears on petal switch (a
`Local<Option<String>>` baseline, same idiom as `respawn_on_petal_change`),
on disarm (TerrainTools section closed), and on commit (detected as
`ProposalEditState.proposals.len()` growing — the palette's own fields are
deliberately left armed post-Add, so that growth is the only "something just
committed" signal available here). Registered in `PostUpdate` (plugin.rs),
not `Update`, so it always observes the SAME frame's
`handle_brush_interaction`/`process_ui_actions` writes rather than lagging a
frame behind.

The live readout line (`PreviewReadout`, mirrored from
`ProposalPreviewReport` by `drain_preview_reports`) renders as a standalone
`egui::Area` overlay (`terrain_tools_panel::render_preview_readout_overlay`,
called from `plugin.rs`'s `gardener_ui_system`) rather than a widget spliced
into `render_terrain_tools_section`'s own call chain — that function lives in
`ui_shell/right_sidebar.rs`, owned by a later slice (S6) in this track.

## §geometry-mirror — fe-ui-local ruler math

`geometry` is a verbatim copy of fe-terrain's pure ruler formulas
(`polygon_area_m2` / `world_to_real_distance` / `bearing_deg`) plus a local
`sanitize_world_scale`. fe-ui must NOT depend on fe-terrain, so the pure
formulas are **copied, not imported** (same boundary rule as
`terrain_map::tileset_to_terrain_json` building `TerrainConfig` JSON by hand).
Kept byte-for-byte equivalent to `fe_terrain::ruler` — if either changes, sync
by hand. **Metrically honest:** an unusable `world_scale` (≤0/unset) sanitizes to
`1.0`, so the returned numbers are world units, not meters — callers surface the
"no map scale" chip (NFR-4) rather than fabricating meters.

## §visibility — hide/isolate that is real everywhere (hierarchy_visibility_groups)

**One resolver, one picker rule.** `resolve::effective_visibility` is the only
lattice; `visibility::pickable(node_id, nav, vis)` (built on
`active_ancestry(nav)`) is the only "may this be interacted with" rule. D-7
ratified SPAWN-THEN-HIDE, so hidden entities still exist in the ECS and the
picker filters ARE the hide mechanism — every surface calls `pickable`, none
re-derives it: viewport click (`viewport_pick`), right-click classification
(`context_pick`, incl. the ground-stamp tier), sidebar select (`sidebar_sync`,
refused with a toast), the path bridge (`pointer::open_track_on_select`:
a hidden track cannot open and an open one closes), path-segment clicks,
foreign-anchor snapping (`ForeignSnapSources`) and camera focus
(`plugin::apply_camera_focus`). Stamps are filtered on their OWN marker id
(`stamp_marker_id`) — the same id `sync_node_visibility` hides. GPX-HIDE-CASCADE
(`decisions_20261012_impl_20261015` W6-5): a stamp whose own override is
`Auto` follows its owning track's own `Hide` override or hidden-Group
membership (`resolve::node_hidden_reason` step 2.5, `HiddenReason::OwningPath`)
— a per-stamp `Show`/`Hide` override still wins outright (step 2 short-circuits
before the cascade runs). Living inside the ONE resolver means every consumer
gets the cascade for free with no extra wiring. Two things do NOT cascade
(A10): the track's own ANCESTOR chain (the stamp shares the track's petal, so
it is already hidden directly on the same grounds) and solo (membership is
never extended from a soloed track to its stamps). A promoted stamp's
persisted `view.state` (A11) still counts as its own override even though the
resolver is called with the SPAWNED marker id, not the promoted row's real
id — `VisibilityState::promoted_stamp_alias` (filled by
`verse_manager/db_results/properties.rs` wherever `hydrate_promoted_stamp`
runs) maps marker id -> promoted node id, and the resolver checks both keys
before falling through to the cascade. An entity
hidden OUTSIDE the lattice (a GPX ribbon whose map layer or `gis.track.visible`
style is off sets `Visibility::Hidden` itself) is not a ray-pick or snap target
either: `viewport_pick`, `context_pick` and `ForeignSnapSources` also require
`visibility::rendered(Option<&Visibility>)` (the VIS-PH5 layer checkboxes made
that path live; nothing invisible is clickable).

**Apply timing (`sync_node_visibility`).** The apply pass runs on an `epoch`
change, a petal switch, AND any frame with `Added<SpawnedNodeMarker>` — node
entities spawn late (GLTF loads, ribbons after tracks load, stamps, restart
hydration landing before the spawn), and an epoch-only gate would leave them
visible. It also re-asserts `Visibility::Hidden` on any entity in
`hidden_by_us` whose `Visibility` another writer changed back (a Layer
Manager toggle or ribbon rebuild writes `Inherited` on every layer-bound
entity). It never touches an entity it did not hide. Known gap (pre-existing):
restoring a lattice-hidden ribbon writes `Inherited`, which can un-hide a
ribbon whose map layer is off.

**Verbs.** H hides the selection, Shift+H shows all (active branch: Hide
overrides, petal/fractal/verse hidden sets, solo — `show_all_in`, one epoch),
Alt+H isolates the selection (`node_manager/shortcuts.rs`). A stamp instance
has no node row, so H on a stamp is a session-only override (no write, no
"not saved" toast) and Show all also clears the Hide overrides of the
branch's tracks' stamps (`stamp_track_in`) without persisting them; the
Groups section offers no membership buttons for a stamp. The sidebar node
row's right-click menu (D-8) offers Isolate / Hide others / Show all through
the same producers (`show_all_and_persist`, `hide_others_and_persist`). Esc
clears isolate on the `ClearIsolate` rung (node_manager §staged-escape).

**Persistence (Phase 3, `cache.rs`).** Overrides persist as node properties
`view.state` (`show`/`hide`; absent = Auto) and membership as `view.groups`,
written ONLY through the auth-carrying `DbCommand::SetNodeView` (D-15,
`CallerAuth::Local`, Editor+ in fe-database) — never raw `SetNodeProperty`.
Every persisted override goes through `UiAction::SetNodeView`
(`cache::set_node_view`: optimistic session write, then the command,
correlated `fe-ui:view:{node_id}`). Hydration is the third
`NodePropertiesLoaded`-fed cache idiom (`hydrate_from_properties`, next to the
primitive/path-asset caches) plus `LoadPetalViewState` on every petal entry
(`load_petal_view_state_on_nav_change`). Rules: a PRESENT key is applied, an
ABSENT key leaves the session value alone (it may be a write the DB refused);
the `NodeViewSet` echo is authoritative for both keys (absent = Auto/none),
whichever caller wrote it. A refused write keeps the session state and toasts
"Visibility not saved (you need Editor access)" (ui_ux §6 — never fake
persistence; `RequestFailed` matched by correlation prefix, not message
sniffing). Solo is never persisted and hydration never touches it.

**Group registry + scope eyes (Phase 4, `groups.rs`).** The active petal's
Groups (`VisibilityState.groups`, display order) mirror `petal.visibility_groups`
and the verse/fractal/petal eyes mirror `view_hidden` (D-1 persisted all
three levels — a session-only petal eye between two persisted ones would be
incoherent). Writes: `UiAction::SetGroupRegistry` (whole-registry replace;
pure transforms `with_group_added/visible/renamed/removed/moved`, `order` =
display index on the wire, new ids slugged from the name by `next_group_id`),
`UiAction::SetScopeHidden` (every sidebar scope eye, and Show all re-opens
the branch's closed eyes so a restart cannot re-hide them), and
`UiAction::SetNodeGroups` (membership = Phase 3's `SetNodeView { groups }`).
All are optimistic + `CallerAuth::Local`, correlated `fe-ui:groups:{petal}` /
`fe-ui:scope-eye:{id}`; a refusal keeps the session value and toasts "Groups
not saved (you need Editor access)". The Groups editor caps names at
`MAX_GROUP_NAME_CHARS`, the registry at `MAX_VISIBILITY_GROUPS` and per-node
membership at `MAX_VIEW_GROUPS` (fe-database's validation bounds), so the UI
never offers an edit the DB must refuse. Hydration: `load_petal_groups_on_nav_change`
clears the registry and sends `GetPetalVisibilityGroups` on every petal entry
(correlated `fe-ui:groups-load:{petal}` — a failed READ is never toasted as
"not saved");
`PetalVisibilityGroupsLoaded` (Get, Set echo, or an API write) replaces the
registry ONLY for the active petal; `HierarchyLoaded` REPLACES the three
hidden sets (`hydrate_scope_hidden`) — authoritative, so a refused session
eye lasts until the next hierarchy load. The active petal's eye cannot HIDE
it (D-4), but a petal persisted hidden and then entered keeps an enabled
"Show petal" eye (`panels/sidebar.rs::petal_eye`), otherwise its contents
would stay hidden behind a dead eye.

**Still session-only:** solo.

## Accessible control names and observations

Every new or reworked interactive control must retain a meaningful purpose
and contextual name and record its actual response for QA.
`accessibility::ResponseExt` is the adapter for rendered control responses.
Use `labelled` for a complete visible label, `named` for purpose-based names
(including contextual repeated-row actions), and `observed` when the widget
already supplies a complete native name. Pass `format_args!` for contextual
names: conversion happens inside the AccessKit builder, avoiding name
allocations when accessibility is inactive. Never include token, invite,
password, or other secret field contents in a name. Preserve native widget
roles, values, actions, and enabled state; painted clickable labels/buttons
use `button_named` to supply their action role through `WidgetInfo`.

Each adapter captures the actual same-pass response through the public
`ui_shell::layout_policy::record_response` seam. Keep observation at the
control construction site so its layer, clip, and hit rectangle remain
authoritative; do not reconstruct widget IDs or label container responses.
Shared copy and confirmation widgets accept the caller's purpose/target
context so their inner action responses can be named directly.

Source inventory and static checks do not establish exhaustive UI coverage;
make completion claims only after the integrated live accessibility audit.

`panels::widgets::confirm_button(ui, id_salt, label, confirm_label, name)`
is the single production confirmation API. Supply a contextual accessible
`name` (prefer `format_args!` for repeated rows); the helper names the arm,
confirm, and cancel responses directly. Existing tests call this same API.

## Local tree appearance settings

AppSettings.tree_appearance persists stable kind:id keys to a TreeIcon and
opaque RGB bytes. Absent entries use the hierarchy-kind default. Model types
live in settings.rs; temporary drafts and drawing live under tree_appearance.
These are local display preferences, not synced scene properties. Existing
settings debounce, restart format and QA-root isolation own persistence.
## Terrain object impacts

`scene_analysis::collect` runs after transform propagation, only while a volume selection exists, at most four
samples per second. It considers128 active-Petal tree entries; each physical subtree has a4096-entity traversal
budget. Global bounds use the existing gimbal helper after finite-data checks. Missing/unloaded/nonresident,
degenerate or over-budget geometry is unresolved; known assetless non-primitive placards are excluded. Hidden
objects remain physical candidates but cannot be selected through the report until visible. Actual resident
`PrimitiveNode` cubes with axis-aligned transforms use the solid-box method; a GLB carrying a primitive property
cannot acquire that label. Other shapes/rotations remain conservative bounds. Names are bounded to256 characters.

The material-only cache holds only current snapshot targets. A desktop-only64-ID projection binds the expected
canonical scope and current Petal, and the DB authorizes the actual local Viewer before and after reading.
Responses require the current nonce, scope, Petal and exact requested ID set. Scope change/revocation clears
pending state; old replies cannot hydrate it. Material/primitive changes invalidate the projection, while identical
property readbacks and transform-only commits do not cause needless material queries. Missing material is distinct
from unavailable or over4096-byte material; raw JSON type is retained in bounded previews. Requests use a monotonic
30-second deadline and bounded retry. No whole-property-bag cache or unscoped SQL is introduced.


## §feedback-status

The notice renderer uses theme::notice_fill and NOTICE_TEXT while UiManager
remains the queue, priority, expiry, hold and history authority. Only Errors
offer Dismiss and pin on hover; a held error stays until dismissal, matching
the existing queue contract. The renderer reserves space beside wrapped text
for the action. Info and Advisory retain their automatic lifetimes.

Role status belongs to the status bar. It shows the resolved current-scope
role plus verse name; absent resolution or no verse shows a dash, distinct
from resolved No access. Capability copy is advisory, never authorization.
The optional NodeIdentity resource comes from the binary and supplies only
the public DID on hover; harnesses without it report unavailable.

The mesh watchdog owns the mesh_budget condition key. It updates only when
count/ceiling changes while exceeded and clears on recovery. This ongoing
condition does not compete with completed-action notices. Existing rising-edge
logging and the spawn gate remain unchanged.

WCAG sRGB relative luminance gives TEXT_MUTED (145,145,165), TEXT_DIM
(155,155,175) and STATUS_NORMAL (155,165,175) at least 4.5:1 against panel,
status, dialog and button fills; muted/button is the tightest pair (~4.7:1).
NOTICE_TEXT exceeds 4.5:1 on all three notice fills at full opacity. Fade
transitions are transient; selected/disabled/overlay pairs outside this set
need separate checks when adopted. tab_fill and BG_PRIMARY provide neutral
shared emphasis for owned tab/action integrations. The existing render journey
contains contrast assertions; execution is pending the final integrated sweep.
## editor-undo

undo.rs owns the Command-pattern session stack for ordinary full transforms and
single path-point positions. At most 50 operations across Undo/Redo and 4 MiB
of encoded before/after values are retained. Full navigation ancestry changes
reset generation, pending ownership, projections and stacks; selection does not.
Restart starts empty. Undo appends ordinary immutable facts rather than deleting
history or materializing a historical preview.

One covered edit may be pending globally. Gimbal release captures AxisDrag before
clear; Inspector captures the full old Transform before assignment. The producer
sends one checked mutation. Matching identity, generation, target state and an
advanced persisted version are required before recording/transferring an entry.
First hydrated edits without a known version use atomic relevant-before CAS;
receipts establish the version afterward. No-op/cancel/refusal records nothing.

Every write identity is a restart-unique ui-undo-ULID. Timeout resolves its exact
immutable fact without retrying the mutation. Known refusal also reads current
authorized state before restoring display; an old optimistic before-state never
overrides peer state. Failed reconciliation stays blocked/indeterminate. A later
chord may retry only that read. Late prior-generation results cannot revive a
cleared stack. A conflict skips one requested item and clears redo.

Ctrl/Cmd+Z and Ctrl/Cmd+Shift+Z respect text focus, popups, close confirmation,
exclusive dialogs, live gestures and advisory Editor role; the DB rechecks stored
Editor authority in the transaction. Hidden/deleted/moved targets are not recreated.

Point gestures retain track/petal plus complete editor rows at press. The action
reserves before changing its buffer. The host reads authorized current state on
the DB channel and changes only xyz in the complete raw point JSON. Point order,
index and known metadata must still match. Timestamps, relative handles, corner
and smoothness are not rebuilt from f32 display rows. Route/mesh/editor rows and
path-stamp caches reconcile from matching confirmed/current projections. Queued
ordinary writes to that track wait for resolution. Segment/whole-path moves,
append/insert/remove/handles, arbitrary properties/schema, create/delete, Measure
and terrain remain outside Undo coverage.


Undo entries retain separate committed versions for their before and after
states. After a confirmed movement, only the next same-node/same-scope entry
with the exact prior version link and equivalent relevant state adopts the new
cursor. This permits consecutive same-object Undo/Redo without treating peer
versions as local predecessors. Recovery confirms the historical operation but
projects only its authorized current snapshot; an older receipt never replaces
a newer route. A differing recovery snapshot cannot rebase the next command.

Path bridge entry points check the actual NavigationManager at use, independently
of observer scheduling. Full verse/fractal/petal changes clear old reservations
and projections before a late read can enqueue a write. Typed read target identity
and petal must match the reservation; the DB still fences actual stored ancestry.
Navigation comparisons allocate only when the key changes. Authored absent time
and its existing stored zero encoding compare equivalently in display-row checks;
the complete raw timestamp value remains unchanged in the checked write.

## Inspector URL/API polish

The Portal URL section validates through actions::portal::compute_save_url and
retains invalid drafts without queueing writes. Inspector API generation owns
one scoped nonce until matching reply, failure or timeout; selection/scope/access
changes clear late secrets. See panels/AGENTS.md for the request/reveal/curl
contract. plugin.rs mirrors the confirmed RuntimeApiEndpoint through its
existing UiShellParams bundle; no default port is advertised as a listener.

Inspector tabs are private form state: read active_tab() and transition through
set_active_tab(), including programmatic openers. Leaving API Access clears
pending ownership, generated token/client and reveal state synchronously.

## Hierarchy browsers and target Options

viewport.rs shares one bounded card renderer across Verse/Fractal/Petal browsers. Options is always visible; it opens target-bound EntityDetails and never derives target permissions from the active navigation role. Creation remains advisory Manager-gated at the current parent and is authorized again by the DB. Stable scroll/grid IDs use hierarchy identity rather than mutable names/counts; per-card IDs preserve focus across list updates. Human ancestor buttons call NavigationManager methods and stop rendering the old branch immediately after a route change. Empty Verse browsing offers the existing Join flow. Counts name the actual hierarchy kind and peer counts remain reported observations.

Petal hints use a wrapped, backed, noninteractive Area anchored inside the viewport. Tool text reflects actual Pen/Brush/Measure behavior and the shared shortcut table. These changes do not alter picking, gimbal state or terrain calculations. Existing numeric render journey invokes the browser pointer/clip flow at three sizes; authored assertions are not execution evidence.


## Viewport help and floating Data

The console passes an explicit show_viewport_hints flag derived from the actual floating Data open
state. Petal tool hints also yield while UiManager has a dialog/context menu or egui owns a modal
layer. Data supplies its own point instructions and the topbar Help remains available. This gate
only skips the noninteractive hint Area; viewport picking, gesture admission, keyboard capture,
selection, and navigation use their existing paths. The existing native viewport journey paints
Pen help without an obstruction and checks its absence with Data, a dialog, and an egui modal.

## ?terminal-maintenance

Settings exposes terminal reset only when its trusted startup developer capability and the binary's request sender exist. Exact RESET plus the existing confirmation queues one local host request; it never supplies runtime readiness or sends the legacy reset command. The host stops producers before admitting the final DB operation.

`maintenance::clear_read_models` calls the navigation/node/tree/UI managers' reset methods and replaces form, history, query, token, path/stamp, proposal, download and residency buffers. This happens only after ordinary editor schedules have been removed. Old replies are drained without projection; no hierarchy reload or scene respawn occurs. Preferences and the guarded file-dialog provider are retained. The standalone terminal renderer owns subsequent Close/manual reopen behavior.

Stamp invalidation includes StampInteractionState, StampSelectionCatalog and
StampSelectionRequests as well as ToolPanelState's catalog mirror. The existing
native picker/Apply journey reuses its loaded properties and outstanding save,
queues a retained origin, calls the real invalidation, and checks late promotion
through registered readers plus late save refusal through the production receipt
handler. Queue reset is explicit source evidence; no-write alone does not prove
the private queue was empty. These assertions remain unexecuted until the sweep.


## Addressed invitation coordination

`invitations` owns nonblocking desktop issuance and join phases. Access retains one exact-target
Owner-issued invitation, with a recipient DID selected from known peers or entered explicitly.
See `invitations/AGENTS.md` for cross-thread ownership and credential lifetime boundaries.


The navigation replica lifecycle accepts only a one-use handoff from the ready addressed Join
coordinator, matched to the actual hierarchy namespace. This retains its confirmed live pipeline
on final navigation; ordinary verse changes retain their existing close/open behavior.


NavigationManager owns a monotonic intent revision alongside its cursor. Addressed admission
captures that revision to suppress late initial hierarchy navigation even after Cancel, until an
explicit navigation/back action begins a new cursor lifecycle. Call manager methods for intents.


Navigation replica submissions are nonblocking and retain only definitely-unsent transitions.
A superseded addressed handoff queues an exact native lease close before ordinary navigation
continues; mismatched handoffs are retained until accepted cleanup rather than dropped.


Terminal clear_read_models also replaces the separate ReplicaHandoff resource. Native shutdown
owns any remaining transport close; terminal invalidation only drops the obsolete UI lease and
cannot later reopen or adopt it. The existing native lifecycle journey covers this reset adjunct.
