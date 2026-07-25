# fe-ui/src/panels — the egui shell

- `mod.rs` — `gardener_console()`, the single entry point called from
  `plugin::gardener_ui_system`. Owns overall layout order (toolbar → status
  bar → sidebar → inspector/portal-toolbar → viewport → dialogs → toast) and
  the toast overlay itself.
- `toolbar.rs` — top toolbar + the `Tool` enum (viewport transform tool).
  `Tool` is reachable crate-internally as `crate::panels::toolbar::Tool`
  (not re-exported further — see root `AGENTS.md` §compat).
- `status_bar.rs` — bottom status bar (online/peer indicators, active verse).
- `sidebar.rs` — left verse/fractal/petal/node tree, drag-reorder, space
  overview.
- `inspector.rs` — right inspector panel: entity/transform/**Portal
  URLs**/properties/schema/API-access tabs. The "Portal URLs" section is
  the browser-integration seam — see root `AGENTS.md` §portal before
  touching `inspector_url_meta_section`.
- `asset_card.rs` — Properties-tab "Asset" card (name + Download button).
  Queues `UiAction::DownloadNodeAsset`; fe-ui does no file I/O — see root
  `AGENTS.md` §asset-download for the `PendingAssetOps`/`AssetDownloadStatus`
  integration contract the main binary must implement.
- `annotation_card.rs` — Properties-tab "Annotation" card
  (`gis.annotation.title`/`body`/`color`). Reuses `SetNodeProperty`/
  `DeleteNodeProperty` exactly like the Custom Properties section below it;
  Save-click action emission is a pure `annotation_save_actions` helper; the
  color field pairs a hex `TextEdit` with an interactive
  `egui::widgets::color_picker::color_edit_button_srgb` swatch
  (`rgb_to_hex`/`parse_hex_color` round-trip). See root `AGENTS.md`
  §gis-query-ui.
- `query_tab.rs` — inspector "Query" tab (ad-hoc SurrealQL).
- `portal_toolbar.rs` — replaces the inspector panel while a portal webview
  is open (back/url/close row).
- `gis_panel.rs` — GIS Query panel: an independent floating window (toggled
  from the toolbar, not part of `ActiveDialog`), tab-strip'd into Query /
  Annotations / Layers (`GisPanelTab`, mirrors `inspector.rs`'s tab bar).
  Query tab: three query modes (annotated / property filter / bbox), a
  results list that reuses the sidebar's click-to-select + camera-focus
  mechanism, and the embedded `gpx_import_card`. Annotations tab: every
  annotated node in the active petal (reuses the Annotated query flow +
  `render_results`, plus a Refresh button). Layers tab: embeds
  `layer_manager_card`. State lives in `crate::gis::GisPanelState`; see root
  `AGENTS.md` §gis-query-ui.
- `layer_manager_card.rs` — visibility/opacity toggles for the active
  petal's terrain layer stack, round-tripped through `SetPetalTerrain`, plus
  the Mesh/Splats/Hybrid view-mode selector (`render_view_mode_row`,
  additive `"view_mode"` terrain JSON field). See root `AGENTS.md`
  §gis-query-ui.
- `gpx_import_card.rs` — GIS panel's GPX import button (rfd file picker) +
  persistent status row, queues `UiAction::GpxImportFile`. See root
  `AGENTS.md` §gpx-import.

## §egress-card

- `egress_card.rs` — GIS panel **Export** tab: the "Copy for BI" card
  (`analytics_egress_20260714` Phase 4). Renders one-click-copy rows for the
  egress SQL, the `/api/v1/query` POST endpoint, the
  `export.parquet`/`export.csv` GET URLs, and a DuckDB
  `read_parquet('<export-url>')` snippet, plus a "Shareable link" section.
  **All string building is pure and unit-tested** in
  `crate::gis::egress_strings` (SQL builders per source, RFC-3986 percent
  encoding, URL/snippet/curl assembly) — the card must never inline-format
  these strings. Copy goes through egui's builtin `ctx.copy_text` (no
  clipboard dep) + the shared toast.
- Query-context sources are **panel-local** (`EgressCardState` on
  `GisPanelState.egress`): Petal (active petal), Node (id buffer filled only
  by an explicit "Use viewport selection" click — never implicitly from
  `NodeManager.selected`, per project memory track-selection-two-concepts),
  and Bbox (reuses the Query tab's bbox buffers).
- Shareable link: fe-ui has **no HTTP-client seam** to fe-api, so minting is
  a displayed, copyable `curl` command against the Phase-3 contract
  (`POST /api/v1/query/share`, body `sql`+`format`+`ttl_secs`); wiring a real
  in-app mint call is a recorded follow-up on the track. The bbox SQL filters
  on `position.coordinates[0|1]` (the `[x, z]` GeoJSON point contract from
  `gis::query::extract_xz`); validate against the live guard/DB in Phase 6.
- Route/URL shapes implement the plan contract (Phases 2–3), not fe-api's
  current code — endpoints are being built concurrently to the same contract.

## §tool-panel

- `tool_panel.rs` — Tools panel: an independent floating window (like
  `gis_panel`, not part of `ActiveDialog`) that hosts the hexon-path-asset
  stamping controls. State lives in `ToolPanelState` (`open`,
  `selected_hexon_ref`, `asset_filter`, `spacing_mode: SpacingMode`,
  `spacing_value`, `count_value`, `tangent_align`); the panel-local
  `SpacingMode` (`FixedSpacing | FixedCount`) maps to
  `fe_sdk::path_asset::SpacingMode` via `to_sdk()` at emit time (panel state
  stays SDK-free). The "Path Asset" section renders a real **asset picker**
  plus the repetition/pattern controls (spacing-mode radio, spacing/count
  `DragValue`, tangent-align checkbox) and the **"Stamp along path"** button.

  **Asset picker (FR-1b, `path_asset_picker_20260713`).** The picker
  (`render_asset_picker`) lists already-installed, re-stampable models sourced
  from `VerseManager` — every node with a set `asset_path`, collected by the
  pure `installed_assets(verse_mgr)` helper (dedup by `asset_path`, name-falls-
  back-to-path, case-insensitive name sort) and filtered by `filter_assets`
  against the `asset_filter` buffer. Row click sets
  `selected_hexon_ref = Some(asset_path)`. This source is deliberately
  **quarantine-free**: an installed node's `blob://{hash}.glb` already exists,
  so the picker needs no `DbCommand::ImportGltf` (that dispatch lives in the
  quarantined `fe-database/src/lib.rs`). Browsing-and-ingesting a brand-new
  `.glb` (FR-1a) is deferred, gated on that quarantine lift. A collapsible
  "Or paste a blob:// path" fallback retains manual entry for power users; the
  list is the primary UX.

  The stamp target is the track being edited in the Paths tab
  (`PathEditorState.editing_track_id`). The panel now **names** that target
  ("Stamping onto: <name>", resolved from `PathEditorState.tracks` by
  `track_display_name`, id fallback) so multi-track editors know what Stamp
  hits (FR-4.1). On click the panel emits
  `UiAction::PathAssetApply { track_node_id, descriptor }` (built by
  `build_descriptor`), which `actions::process_ui_actions` routes to
  `node_props::set` → `SetNodeProperty(path_asset, ...)`. The
  `verse_manager::path_asset_reconcile` system then stamps the model along the
  track's `gpx_points` (see `fe-ui/src/verse_manager/AGENTS.md`
  §path-asset-stamp). The button is disabled until both a track is selected
  and an asset reference is set. **The emit path is unchanged by the picker** —
  the picker's only job is to populate `selected_hexon_ref`.

  `render_tool_panel` takes `ui_mgr: &mut UiManager` (to queue the action),
  `path_state: &PathEditorState` (edit target + track names), and
  `verse_mgr: &VerseManager` (picker source). All three are already
  `gardener_console` parameters, so **no `gardener_console` signature change** —
  only the internal `render_tool_panel` call was widened.

  **Reachability (FR-4.2).** `ToolPanelState.open` is toggled by a
  **"🔧 Tools"** button in `top_toolbar` (right cluster, beside Data/Maps);
  `top_toolbar` gained a trailing `tool_panel: &mut ToolPanelState` param for
  it. Previously `.open` had no UI toggle (a discoverability gap) — the panel
  was only openable via code/default.

  The panel also hosts a **Pen** section (curve mode radio, sensitivity/tension
  slider, "Smooth path", and shape buttons) whose buttons queue pen actions
  into `ToolPanelState.pending_actions` (drained in `process_ui_actions`, since
  the panel has no `ui_mgr` for those). The pen curve/shape math + action
  contract live in `fe-ui/src/node_manager/AGENTS.md` §pen-tool (phase 2).
  `pen_curve_tool_20260722` (FR-4/NFR-6) adds the **"New anchor"**
  Corner/Smooth/Symmetric picker bound to `ToolPanelState.pen_new_anchor_kind`
  — the anchor kind a plain (below-threshold) Pen click places, consumed by
  the Release decision (§pen-tool's bezier subsection). It lives HERE, not in
  `tool_inspector.rs`, because this panel owns `&mut ToolPanelState` while the
  tool inspector is read-only by construction (§tool-inspector); a pure
  default needs no `UiAction`.
  `gardener_console` retains its trailing
  `tool_panel: &mut crate::panels::tool_panel::ToolPanelState` param from the
  shell pass (the caller `plugin.rs` still registers `ToolPanelState`).

## §widgets

- `widgets.rs` — shared, reusable, egui-only panel widgets (no Bevy queries).
  `copy_value_box(ui, ui_mgr, display, copy_value, toast, left, right)` renders a
  read-only, width-capped, copyable value box: the frame never exceeds
  `ui.available_width()` and the value wraps in monospace, so an arbitrarily
  large value can't push the panel wider. `left`/`right` draw caller header
  content (a label on the left; extra right-aligned controls beside the copy
  button). `copy_row(label, value)` is the labelled convenience (display == copy
  == value) the **egress card reuses** (it no longer inlines its own copy row).
  `elide(s, max_chars)` truncates (char-boundary-safe) with an ellipsis for the
  *display* only — the copy button always carries the full value.
- **Must be rendered OUTSIDE an `egui::Grid` cell.** A Grid cell ignores
  `set_max_width`, which is exactly what let a giant value blow the panel out.

## §inspector-units

`inspector_units_width_20260716`. Two fixes in `inspector.rs`:

- **FR-1 width-stable property values.** The Custom Properties section renders
  each value via `widgets::copy_value_box` (read-only, wrapped, width-capped,
  copy button, `elide`d display) instead of an unbounded non-wrapping `ui.label`
  inside a 3-col Grid. The key label + delete button + Add Property flows are
  unchanged; only the value cell and its container changed. Result: the panel
  stays at its 260px default regardless of value size.
- **FR-2 real-unit transform inputs.** Position shows/edits **meters**, rotation
  **degrees**, and (when the selection has a pickable AABB) asset **Size** in
  meters; a raw Scale-multiplier row remains as the fallback when no AABB is
  available. Rotation was *already* degrees-side in `inspector.rot`
  (`node_manager::inspector_sync` converts radians→degrees on fill,
  `actions::transform` converts back on Apply) — FR-2 only added the `°` label.
- **Where conversions live.** Pure helpers in `inspector.rs`
  (`world_to_meters`/`meters_to_world`, `size_to_scale`/`scale_to_size`,
  `sane_scale`) are unit-tested. `real_m = world / world_scale`;
  `world = m * world_scale`; a degenerate `world_scale` (≤0 / non-finite) is
  treated as `1.0` so fields read as raw units, never NaN.
  `world_scale` is `PetalMapState.world_scale` (world units per meter).
- **Size ↔ scale.** `size_m_i = base_extent_i * scale_i / world_scale`;
  back-computed on edit as `scale_i = size_m_i * world_scale / base_extent_i`
  (per-axis, independent), guarded to `None` when `base_extent_i ≈ 0`.
  `base_extents` are the selected node's combined child-AABB extents expressed in
  the root's LOCAL frame (extents at root scale/rotation = identity → stable,
  rotation-invariant), computed by `combined_local_extents`.
- **Wiring (no `gardener_console` signature change).** `sync_inspector_units`
  (registered in `plugin.rs`, `UiSet::PostSelection`) mirrors `inspector_sync`'s
  cadence (selection change / `Changed<Transform>` / world-scale change) and
  refills the meter buffers `InspectorFormState.pos_m` / `size_m` plus
  `base_extents` / `world_scale`. The panel edits those meter buffers and
  **live-writes** the converted world/scale values into the existing
  `inspector.pos` / `inspector.scale` world buffers, so the Apply action path
  (`actions::transform`) still receives world units / radians — it is untouched.

## §terrain-tools

FR-5/FR-6 (`terrain_editor_overhaul_20260718`) + D-78
(`p2p_asset_streaming_20260718` FR-7), added by ultrapilot worker w4a.

- `terrain_tools_panel.rs` — the Cities-Skylines-style non-destructive
  terrain proposal palette: 8 mode buttons (`tool_panel::TerrainToolMode` —
  Raise/Lower/Flatten/Ramp/Slope/Pad/Cut/Fill, a panel-local enum mirroring
  the `SpacingMode`→SDK idiom, converted via `to_proposal_op()` into the
  cross-worker `terrain_proposal_state::ProposalOp`), footprint-radius/
  target-height/delta controls, and an "Add proposal" button emitting
  `UiAction::TerrainProposalAdd`. Footprint is a circle generated by
  `node_manager::curve::circle` **at the origin** — same "append at origin,
  drag to reposition" convention as the Pen section's shapes; dragging a
  placed proposal is a follow-up once FR-1/FR-3 (typed-selection gimbal)
  lands elsewhere in this track. Below the palette, a select/delete list of
  `ProposalEditState.proposals` (`UiAction::TerrainProposalDelete`) — select
  sets `ProposalEditState.selected`, read by `proposal_report_panel`.
  NFR-1: this panel never writes `TerrainHeightField`/the tileset — it only
  ever builds `UiAction` payloads. The palette builds a 2-D `[f32; 2]` footprint
  by dropping Y from `curve::circle`'s XZ points, matching the JSON contract.
- `proposal_report_panel.rs` — FR-6 report window for the selected proposal:
  extent (m), area (m²), volume (m³), slope (%), bearing (°), computed by the
  pure, unit-tested `compute_report` from `fe_ui::geometry::{world_to_real_distance,
  polygon_area_m2, bearing_deg}` (the fe-ui-local mirror of
  `fe_terrain::ruler` — same signatures). **NFR-4 honesty:** `world_scale`
  unset/`<=0` runs the identical math with `scale = 1.0` (definitionally
  "world units", never mislabeled meters) and shows a "no map scale" chip —
  mirrors `fe_terrain::ruler_plugin::draw_unscaled_chip`. Slope/bearing are
  unit-invariant ratios/angles computed from the **raw** footprint extent
  (not the scale-divided `extent_x`), so they're reported regardless of
  scale state.
- `tool_panel.rs`'s "Terrain Tools" section is now just a reachability
  toggle (`ToolPanelState.terrain_tools_open`) for the standalone
  `terrain_tools_panel` window — kept separate so the palette can hold a
  `ProposalEditState` handle without widening `render_tool_panel`'s own
  signature (contrast with §tool-panel's picker, which reused existing
  `gardener_console` params).
- **`gardener_console` signature change (wired).** Both new panels need
  `proposal_state: &mut terrain_proposal_state::ProposalEditState` and
  `app_settings: &mut settings::AppSettings` (also used by the D-78 Settings
  dialog, see `dialogs/AGENTS.md` §settings), added as trailing params.
  `plugin.rs::gardener_ui_system` supplies them via the `MiscUiParams` bundle
  (both resources are `init_resource`'d there).
- **Reconciled shapes (w4b is source of truth).** `ProposalRecord` is
  `{ id: String, op: ProposalOp, footprint: Vec<[f32; 2]>, target_height:
  Option<f32>, delta: Option<f32> }` and `ProposalEditState.selected:
  Option<String>` — 2-D footprint + `Option` height/delta, matching the JSON
  contract and `fe_terrain::TerrainProposal`. The two panels convert at the
  seams: the palette drops Y to build `[f32; 2]` and wraps `Some(..)`; the
  report lifts `[x, z]` back to `[x, 0, z]` for the `[f64; 3]` geometry helpers
  and treats a `None` delta as `0.0`.

## §tool-inspector — active tool as a legible UI MODE (`tool_inspector.rs`)

`tool_inspector_ux_20260719` Phase 1. A left `SidePanel` (`tool_inspector`)
rendered by `gardener_console` right after `left_sidebar`, so it sits inner of
the hierarchy tree. Unlike the hierarchy sidebar (which auto-collapses when the
right inspector opens, `mod.rs`), it uses plain `.show()` and is **always
visible** — a transform inspector is most useful exactly when a node is selected
and the tree has collapsed.

- **No signature change.** The panel reads only state already threaded into
  `gardener_console` (`ToolState`, `NodeManager`, `PathEditorState`); it projects
  the selection itself via `node_manager::project_selection` (re-exported for
  this) rather than taking a new `SelectionState` param.
- **Pure core, thin paint.** `panel_descriptor(tool)` (title + Use/Settings zone
  labels per tool), `gimbal_affordance_label(tool, kind)`, `selection_summary`,
  and `mode_button_fill(active)` are all pure + unit-tested; the egui body just
  lays them out. Phase-2+ fills the Use/Settings zones with real controls and
  folds in `tool_panel.rs` (see the track's `plan.md`).
- **Mode legibility (FR-1).** The top toolbar's active-tool button now fills via
  `mode_button_fill` — a brighter NEUTRAL (`theme::BG_MODE_ACTIVE`), i.e. a
  LUMINANCE emphasis, not the old saturated-blue `BG_BUTTON_ACTIVE` hue shift
  (`code_styleguides/ui_ux.md §1`).
- **Gimbal affordance (FR-7).** `gimbal_affordance_label` mirrors the ratified
  "grab it wherever it's shown" rule (see `node_manager/AGENTS.md §dispatch`): a
  vertex/segment selection shows the affordance in EVERY tool; an entity / whole
  track only in the transform tools. Keep it in lockstep with the draw/interact
  logic in `gimbal_interaction.rs` — it is the textual mirror of what is drawn.
- **Read-only anchor readout (`pen_curve_tool_20260722` FR-6/NFR-6).** The pure
  `anchor_readout(kind, points)` renders the selected path vertex's corner kind
  + smoothness ("Anchor #3: Smooth, smoothness 0.50" — unitless 0..1) under the
  selection summary. STRICTLY a readout: the panel signature is unchanged (all
  params still `&`), and the EDITABLE corner settings live in the Paths card
  (`fe-ui/src/AGENTS.md` §path-editor item 10) + the Pen default in
  `tool_panel.rs` (§tool-panel). Adding any `&mut` here would break the
  read-only-by-construction contract this panel exists to keep.

All panel-rendering submodules are `pub(crate)` — nothing outside `fe-ui`
should render sub-panels directly; go through `gardener_console`.
