# fe-ui/src/panels — the egui shell

The Inspector Data tab delegates object history, event-time measurements and enforced schema editing
to `../object_data`. Its state is an Inspector read model; historical results never apply to current
scene entities. See `../object_data/AGENTS.md` for scopes, bounds, request routing and retained drafts.

Sidebar descendant clicks restore their complete verse/fractal ancestry through NavigationManager;
expanded rows can belong to a different parent than the current cursor. See `../AGENTS.md` §navigation-ancestry.

## 2026-07-26 Brush and numeric hardening

Topbar **Options** routes to `RightSidebarSection::Options`, which owns all active-tool controls.
Selecting Brush opens the contextual Options section with mutable radius/strength/op controls.
`ToolPanelState` and `SculptToolState` sanitize relevant `f32`s before widgets and use practical finite ranges.
Circle/Rect/Polygon are labeled report-only; Brush is the live paint shape.

The Paths edit card exposes legacy color/width as border style, an alpha-aware
fill color, and a closed-zone toggle. It also owns session-local snap settings
(enabled, grid step meters, anchor radius meters); viewport interaction applies
them on Pen append and single-point release. Border width, both snap values,
point Y, and Settings render distance are repaired and capped before egui sees
them; the full-panel numeric smoke seeds NaN/Inf across these surfaces.
All `PathEditorState` numeric repair (point Y, grid step, anchor radius, and
border width) runs once at the `gardener_console` shell boundary rather than
inside the conditionally visible GIS Paths tab, so hidden state is already
finite when that tab later constructs its numeric widgets.

**Shell layout moved to area managers** (`ui_shell_architecture_20260724`
Phase 2). The topbar/left/right render bodies now live in `crate::ui_shell`
(`topbar`, `left_sidebar`, `right_sidebar`); `gardener_console` just orders the
manager calls. See `fe-ui/src/ui_shell/AGENTS.md` for the manager topology, the
`active_section` precedence rule, and the per-section render-fn seam.

- `mod.rs` — `gardener_console()`, the single entry point called from
  `plugin::gardener_ui_system`. Owns overall layout order (topbar → status bar →
  left sidebar → right sidebar → viewport → dialogs → toast), the still-floating
  windows, and the toast overlay itself. The topbar/left/right rendering is
  delegated to `ui_shell` managers. **No left tool-inspector panel** as of
  Phase 5 (FR-8, RATIFIED Q-1) — see §tool-inspector below.

## §identity-banner (F6)

`status_bar.rs` renders `plugin::IdentityBanner.message` as a persistent,
click-to-dismiss warning segment — deliberately NOT `UiManager`'s toast
(`actions/mod.rs::show_toast`), which auto-fades after 2s and would be easy
to miss for a warning the user needs to act on ("identity could not be
loaded, running ephemeral"). It mirrors the existing persistent
guard-error segment (`ModalManagerState::last_error`, Q-5/FR-7) in shape —
unguarded `status_bar` call, session-sticky — but adds an explicit dismiss
since this is advisory, not a disabled panel. The GUI binary
(`fractalengine/src/main.rs`) sets the message once at startup via
`app.insert_resource`; `GardenerConsolePlugin::build` only `init_resource`s
a `None` default for any other harness that builds this plugin without that
step. See `fractalengine/src/AGENTS.md` §identity-tiers for when the message
is set.
- `toolbar.rs` — the `Tool` enum + the single-source tool data: `TOOL_DEFS`,
  `shortcut_hint_line`, `active_tool_hint`/`stash_active_tool`,
  `mode_button_fill`, and (Phase 5, FR-8) `tool_tooltip_text` — the rich
  hover-tooltip content joined from `TOOL_DEFS` + `tool_inspector::panel_
  descriptor`. The top-toolbar RENDER body lives in `ui_shell::topbar` (FR-4)
  and calls these. `Tool` is reachable crate-internally as
  `crate::panels::toolbar::Tool` (not re-exported further — see root
  `AGENTS.md` §compat).
- `status_bar.rs` — bottom status bar (online/peer indicators, active verse). Dim "N hidden" chip when nodes are hidden by visibility groups, absent at zero.
  Also (polish W1): one dim "⚠ {text}" chip per `UiManager::conditions()`
  entry (X-05; the writers arrive with W3-S2's watchdog), and a right-aligned
  error segment (X-01) showing `UiManager::latest_unacknowledged_error()`
  while it is under 60 s old (`ERROR_SEGMENT_SECONDS`, egui clock — the same
  `NoticeClock` value every notice is stamped with). Clicking it opens an
  `egui::Popup` listing the ring (cap 20): each humanized line, its raw
  `detail` underneath, a Copy (copies the raw text) and a Dismiss on the
  newest, which clears the segment. It iterates the ring in place — no
  per-frame copy. The segment is drawn LAST: its right-to-left layout takes
  the rest of the row, so anything after it (the FR-7 guard error, the F6
  identity banner) was pushed off-screen.
- `sidebar.rs` — left verse/fractal/petal/node tree, drag-reorder, space
  overview. Eye toggles at all 4 tree levels (CollapsingState::show_header for
  the right-aligned gutter, same persistent-id scheme), muted hidden rows, active-petal
  eye disabled (D4), tri-state override cycle with hidden-reason tooltips.
  VisibilityState threaded via gardener_console. Node-row eye clicks persist via
  `UiAction::SetNodeView`; verse/fractal/petal eye clicks persist via
  `UiAction::SetScopeHidden` (`push_scope_eye`); active-petal node rows carry
  the D-8 right-click menu (Isolate / Hide others / Show all / "Add selection
  to group…" — the last selects the row and reveals the right-sidebar Groups
  section, `fe-ui/src/AGENTS.md` §visibility). A collapsed "Groups" section
  under the tree lists the active petal's Groups with eye + member count
  (read + toggle only, D-10) and a "Manage Groups…" reveal.
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
- `layer_manager_card.rs` — renders ONLY the unified overlay registry
  (`gis::overlay_registry`: terrain layers incl. live GPX/GeoJSON category
  layers, visibility Groups, scale bar, metric grid — one row widget, VIS-PH5),
  terrain-layer toggles round-tripped through `SetPetalTerrain`, plus
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

## §tool-options — active-tool options dispatcher

`tool_options.rs` (`ui_shell::right_sidebar::render_tool_options_section`) is the single active-tool options dispatcher (one arm per Tool: Selection/Transform/Pen/Brush/Measure), reuse-don't-rewrite rule. Each arm renders live controls (mutable numeric radius/strength/op + toggles) OR options_hints (static advisory text for tools with no live controls), mutually exclusive per tool. The `active_petal_id` param threads through for geometry conversions (world_scale). Shape-region Apply wiring routes through `SculptShapeRegion`; region delete is produced by the viewport right-click menu (`dialogs/AGENTS.md` §context-menu → `SculptDeleteRegion`). The Measure arm (hexon_scale FR-9) renders the Tape/Area picker, the readout rows as `copy_row` boxes, and Undo/Clear/Close polygon; it mutates `MeasureState` only through `node_manager::measure::reduce` (see `node_manager/AGENTS.md` §measure-tool), with the scale from `measure_world_scale` (unscaled reads "world units").

## §tool-panel

- `tool_panel.rs` — Path-asset stamping helpers: pure `installed_assets()`/`filter_assets()`/`build_descriptor` and the "Path Asset" section body (asset picker + repetition/pattern controls + stamp button) now render via `ui_shell::right_sidebar::render_path_tools_section`. State lives in `ToolPanelState` (`selected_hexon_ref`, `asset_filter`, `spacing_mode: SpacingMode`, `spacing_value`, `count_value`, `tangent_align`); the panel-local `SpacingMode` (`FixedSpacing | FixedCount`) maps to `fe_sdk::path_asset::SpacingMode` via `to_sdk()` at emit time (panel state stays SDK-free).

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

  **Reachability (right-sidebar, Phase 2).** The Path Asset section is now
  part of the right-sidebar layout managed by `ui_shell::right_sidebar` (no
  separate `.open` flag); the section renders when `active_section` is
  appropriately set. See `fe-ui/src/ui_shell/AGENTS.md` §right_sidebar for
  section precedence rules.

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
- **Role gating (X-02, OD-3).** `role_gated(ui, allowed, reason, widget) ->
  Response` wraps `ui.add_enabled` + `on_disabled_hover_text`: a disallowed
  control renders disabled with a hover explanation rather than hidden, so it
  stays discoverable. `role_denial_reason(role, needed, verse_name) -> String`
  builds that explanation ("`<needed>` role needed — you are `<role>`[ in
  `<verse>`]"); the role word is `crate::notice::role_label`, the same one
  the denial notices use (unresolved, X-03, reads "—"; `RoleLevel::None`
  reads "Visitor"). This is advisory UI only
  — the real guard is the DB thread's `fe-policy` gate; never trust a
  client-side role to authorize anything. To gate a composite widget (e.g. a
  `confirm_button`), wrap it in `ui.add_enabled_ui(allowed, ..)` and put
  `on_disabled_hover_text(role_denial_reason(..))` on the scope's response —
  see `dialogs/hexon_manager.rs`'s Remove.
- **Two-step destructive confirm (X-06).** `confirm_button(ui, id_salt, label,
  confirm_label, name) -> bool` extracts the inspector token-revoke pattern
  (`inspector.rs`'s Revoke/Confirm Revoke) into one reusable widget: first
  click arms (shows `label`), a second click within 4s on `confirm_label`
  fires; the arm disarms on timeout or on any click elsewhere this frame.
  State lives in egui temp memory keyed by `id_salt` — no extra struct field
  needed per call site. Every new single-click-destroys site should call
  this instead of hand-rolling the pending-flag dance (see
  `fe-ui/src/dialogs/AGENTS.md` §destructive-confirm, which this widget is
  now the canonical implementation of).
- **Cross-module reach.** `widgets` is `pub(crate) mod widgets;` in
  `panels/mod.rs` (W1 gate) so `dialogs/` can reuse `role_gated`,
  `confirm_button`, `role_denial_reason`, `clipped_value`, `world_to_meters`
  and friends; it stays crate-private, not `pub`.

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

FR-5/FR-6 (`terrain_editor_overhaul_20260718`) + D-78 (`p2p_asset_streaming_20260718` FR-7), added by ultrapilot worker w4a. Section = 8-mode palette + controls + proposal list; report shows selected proposal stats.

- `terrain_tools_panel.rs` — terrain proposal palette helpers: 8 mode buttons (`TerrainToolMode` — Raise/Lower/Flatten/Ramp/Slope/Pad/Cut/Fill, panel-local enum converted via `to_proposal_op()` into `ProposalOp`), footprint-radius/target-height/delta controls, plus select/delete list of `ProposalEditState.proposals` (select sets `ProposalEditState.selected`). NFR-1: never writes `TerrainHeightField` — only builds `UiAction` payloads. Footprint is 2-D `[f32; 2]` (Y dropped from `node_manager::curve::circle`, matching JSON contract).
- `proposal_report_panel.rs` — computed report for the selected proposal: extent (m), area (m²), slope (%), bearing (°) via pure, unit-tested `compute_report` from `fe_ui::geometry::{world_to_real_distance, polygon_area_m2, bearing_deg}`. **R-4 (terrain_proposal_delta_viz_20261025 S6): no volume figure is computed or shown here any more** — the old `area × |delta|` single-proposal line AND the multi-region "Earthwork totals" rollup (`cut_fill_totals`/`Earthwork`/`CutFillTotals`, all removed) are gone, not relabeled; true cut/fill now lives exclusively in `volume_report_panel.rs` (§volume-report below), hosted in the same section. `earthwork_regions_from_terrain`/`EarthworkRegionSummary` are `pub(crate)` (now carry `id`) solely because `volume_report_panel::resolve_volume_selection` reuses them to look a selected region id up by footprint. `render_controls_and_emit` takes `world_scale` and renders metres via geometry::world_to_meters/meters_to_world with the "no map scale — showing world units" fallback chip. **PROPOSAL_ONLY_DISCLOSURE** ("Proposal only — does not modify terrain.") is single-sourced. `circle_footprint`/`rect_footprint` pure helpers feed tool_options' Apply (and, via `shape_apply_footprint` → `terrain_preview::shape_commit_footprint_world`, the preview and `volume_report_panel`'s drawn-footprint fallback). **NFR-4 honesty:** unset/`<=0` `world_scale` uses `scale = 1.0` ("world units", never mislabeled) + "no map scale" chip; slope/bearing are scale-invariant (computed from raw extent), reported regardless.
- **Reconciled shapes (w4b is source of truth).** `ProposalRecord` is
  `{ id: String, op: ProposalOp, footprint: Vec<[f32; 2]>, target_height:
  Option<f32>, delta: Option<f32> }` and `ProposalEditState.selected:
  Option<String>` — 2-D footprint + `Option` height/delta, matching the JSON
  contract and `fe_terrain::TerrainProposal`. The two panels convert at the
  seams: the palette drops Y to build `[f32; 2]` and wraps `Some(..)`; the
  report lifts `[x, z]` back to `[x, 0, z]` for the `[f64; 3]` geometry helpers
  and treats a `None` delta as `0.0`.

## §volume-report (terrain_proposal_delta_viz_20261025 S6, FR-6/7/8, R-4)

`volume_report_panel.rs` — the "Volume report" block, hosted directly below `proposal_report_panel::render_report_body` inside the SAME renamed section (`ui_shell::right_sidebar::render_proposal_report_section`; `RightSidebarSection::ProposalReport`'s human label is now "Volume report" — slug stays `"report"`, D11 stability). Replaces the removed `area × |delta|` approximation with the TRUE composite cut/fill from fe-terrain's real integrator.

- **Selection, priority order** (`resolve_volume_selection`, pure): an explicit `ProposalEditState.selected` id, resolved first against the palette mirror (`ProposalRecord`), then against Brush-created regions (`proposal_report_panel::earthwork_regions_from_terrain`) — falling back to the live sculpt-shape draft via `terrain_preview::shape_commit_footprint_world` (the Apply button's `tool_options::shape_apply_footprint` converted meters→world exactly as `handle_shape_region` commits it — S7 fix: it used to send the meters footprint, so on any scaled petal the report integrated, and listed nodes inside, the wrong area) only when nothing is explicitly selected and the Brush tool is active. This is a deliberately scoped v1: there is no dedicated "draw a selection shape" tool distinct from the sculpt edit draft (that would need a new `Tool`/UI affordance outside this slice's owned files) — selecting a volume to report and arming a sculpt edit currently share the same draft, by design.
- **Request/response seam** (`VolumeReportState` Resource + `sync_volume_report_request`/`drain_volume_report_result` systems, registered `PostUpdate` in `plugin.rs`, mirroring `terrain_preview.rs`'s idiom exactly): on a selection/petal change — or a `DbResult::PetalTerrainLoaded` for the active petal (S7 fix: a commit/delete under an UNCHANGED selection left the old figure up indefinitely) — mints a correlation id and writes `fe_renderer::terrain_overlay::VolumeReportRequest`; `drain_volume_report_result` stores the matching `VolumeReportResult` only (wrong/stale correlation ids are dropped — fe-terrain's `answer_volume_report_requests`, S5, is the other end of this seam). fe-ui never depends on fe-terrain (NFR-5) — both message types are fe-renderer's seam.
- **R-4 the regression guard**: `VolumeDisplay` (`resolve_volume_display`) has exactly three states — `Unselected`, `Pending` (request in flight, no true figure yet), `Ready` (true figure). There is NO fourth "approximate" state; the panel either shows the real number or says "Computing…" — it never falls back to an area×delta guess. See `selected_region_report_shows_true_cut_fill_not_area_times_delta`.
- **FR-8 node list**: `nodes_in_footprint` is pure over `VerseManager::find_petal`'s result (no DB round-trip, AGENTS anti-pattern #3). `node_kind` is a best-effort classifier (asset/portal/earthwork/node) from the fields `NodeEntry` exposes today — there is no generic `node_kind` property bag on the in-memory tree, so "node" is the honest fallback for a stamp or path point, never a guess dressed up as certain.
- **FR-9 Copy as TSV**: `volume_report_to_tsv` is pure (totals + per-item rows + contained-node rows); the egui side is a plain `ui.ctx().copy_text(...)` button (not `widgets::copy_row`, which would inline the whole multi-line blob into the panel).

## §tool-inspector — active-tool helpers (`tool_inspector.rs`, no left panel)

`tool_inspector_ux_20260719` → `ui_shell_architecture_20260724` Phase 5
(FR-8, RATIFIED Q-1): the always-open left tool-inspector `SidePanel` is
**REMOVED**. `tool_inspector.rs` is now a **pure-helper-only module** (no egui
paint, no render fn) — its content moved to two consumers:

- **Tooltips carry title/description/Use-guidance.** Each topbar mode button
  (`ui_shell::topbar::render_topbar`, driven by `panels::toolbar::TOOL_DEFS`)
  shows a rich hover tooltip built by `toolbar::tool_tooltip_text`,
  which joins `TOOL_DEFS` (shortcut) with `tool_inspector::panel_descriptor`
  (title/subtitle/Use zone) so button/shortcut/tooltip can never drift.
  `tool_tooltip_text` IS wired (called from topbar). Test:
  `toolbar::tests::tool_tooltip_text_covers_every_tool_with_title_shortcut_description_and_use_zone`.
- **Live readouts moved to the right-sidebar Tool section.** `selection_summary`,
  `gimbal_affordance_label`, and `anchor_readout` are now called from
  `ui_shell::right_sidebar::render_tool_section`. It projects the selection via
  `node_manager::project_selection` exactly as the old panel did, guards a stale
  index via `fresh_path_selection`, and shows the gimbal-active affordance
  exactly when a gimbal is drawn.
- **Pure + total, still unit-tested.** `panel_descriptor(tool)` (title/subtitle/
  Use/options_hints labels per tool), `gimbal_affordance_label(tool, kind)`,
  `selection_summary`, `anchor_readout`, and `fresh_path_selection` are pure;
  all their tests survive unchanged in `tool_inspector.rs`. `mode_button_fill`
  is no longer duplicated here — `toolbar::mode_button_fill` is the sole
  source (topbar button fill + right-sidebar section rail).
- **Gimbal affordance (FR-7).** `gimbal_affordance_label` mirrors the ratified
  "grab it wherever it's shown" rule (see `node_manager/AGENTS.md §dispatch`): a
  vertex/segment selection shows the affordance in EVERY tool; an entity / whole
  track only in the transform tools. Keep it in lockstep with the draw/interact
  logic in `gimbal_interaction.rs` — it is the textual mirror of what is drawn.
- **Read-only anchor readout (`pen_curve_tool_20260722` FR-6/NFR-6).** The pure
  `anchor_readout(kind, points)` renders the selected path vertex's corner kind
  + smoothness ("Anchor #3: Smooth, smoothness 0.50" — unitless 0..1) under the
  selection summary. STRICTLY a readout: its new host, `render_tool_section`,
  takes `node_mgr`/`path_state` as shared refs only (no `&mut` path/pen state)
  — read-only-by-construction, same contract as the retired panel. The
  EDITABLE corner settings live in the Paths card (`fe-ui/src/AGENTS.md`
  §path-editor item 10) + the Pen default in `tool_panel.rs` (§tool-panel).

All panel-rendering submodules are `pub(crate)` — nothing outside `fe-ui`
should render sub-panels directly; go through `gardener_console`.

## Inspector Portal URL validation

The Portal URL control reuses `actions::portal::compute_save_url` as its inline
validation boundary, including empty-to-clear normalization and the backend's
`fe_webview::security::is_url_allowed` policy. Invalid drafts remain editable,
show an inline reason, and cannot queue SaveUrl or OpenPortal. Saving requires
the active Editor role and a selected node. The action handler still validates
again when draining the action. Opening a URL no longer logs its raw contents.
The existing admin config-URL buffer is unchanged by this validation slice.

## Inspector API access request ownership

Inspector owns one vertical body scroll below its fixed selection/tab header.
The shared sidebar chrome still supplies the fixed section title and rail, but
hands Inspector an unscrolled slot. Tabs therefore do not inherit a long tab
body's offset or depend on a forced offset reset when switching. The body keeps
the existing right_section_scroll_Inspector salt; property-table horizontal and
bounded-editor salts remain unchanged. Query, Data and property draft ownership
and the private tab lifecycle are unchanged. Native body offsets can still clamp
when a different tab has shorter content.

The generated token uses a clipped single-line native password preview. It is
masked by default, with native explicit Reveal semantics. Token-card actions use
layout_policy::action for at least 24 logical points and the shared 20-physical
backstop; client Copy is allocated before its clipped public DID. Clipboard
payloads still come from the stored token/client and full curl formatter, never
the clipped preview. This layout change does not establish the sole cause of
the retained QA15 scroll refusal or replace integrated pointer/A8 verification.
The existing Inspector journey renders the actual sidebar chrome at 260/320/640
points with a long token and a genuinely scrolling token list. It checks complete
native tab/action rectangles against hit and clip rectangles, bounded preview
height, exact client/token/curl clipboard payloads, then wheel-scrolls the body
and clicks Properties and API at their fixed header positions. Selection, Query
text and the Add Property draft remain intact; leaving API clears its secret.
Only the known native SidePanel state seeds the requested test width; widget
facts and tab IDs come from same-pass observed responses. These new assertions
are authored source coverage and remain unexecuted in this lane.

`InspectorTokenRequest` binds one mint to the current selected node and scope,
a process-unique nonce, requested role/client and a monotonic 30-second timeout.
Generate is disabled while pending; request inputs are retained on failure.
Selection, scope, access loss or leaving the API tab clears pending ownership
and generated secrets. A timeout is explicitly unconfirmed, since a token may
have been created; late replies are discarded and the list can be refreshed.
Backend failures render safe static text rather than echoing arbitrary payloads.
Tab transitions use InspectorFormState::set_active_tab; the underlying tab is
private so programmatic openers cannot skip exit cleanup. Target synchronization
also clears secrets outside API Access, and submission and result acceptance
check the actual current tab. Returning to API never restores a canceled nonce.

`tokens::handle_api_token_minted` receives the actual current node/scope/role
from the result dispatcher and synchronizes that target before acceptance.
Inspector-owned mint replies and RequestFailed values are consumed even when
late, before pending API or generic-feedback routing. Legacy uncorrelated mints
populate only a matching Entity Settings dialog, not the Inspector. Existing
token-list metadata replies retain their legacy protocol; this change adds
nonce ownership to minting, not a new token-list protocol.

`RuntimeApiEndpoint` is mirrored from the actual bound listener notification.
There is no guessed port or environment-derived fallback in Inspector curl.
The example is an authenticated GET of the selected node's properties, with
encoded path segments and shell quoting. Tokens default to native password
masking; the curl preview uses a placeholder until explicit Reveal. Copy Token
and Copy curl are explicit actions, copy the exact value and show a safe toast.
Names and notices never include token contents. Expiry uses the already locked
chrono version for RFC3339 parsing, human relative text and full-time hover.

Existing Inspector, token-result and runtime-correlation tests are extended in
place, including invalid URL preservation/no SaveUrl, no-selection API text,
nonce duplication/failure/timeout/target fences, masked rendering and the
nondefault 8882 endpoint example. The same journey calls the report's actual
Properties opener for the same selected node/scope, checks immediate secret and
reveal cleanup, then rejects its pending reply before and after returning to API.
The existing token-result test also exercises this path through the real result
handler without a token-list refresh for the rejected reply. Readbacks are model fixtures and no tests run
in this source-only lane; integrated compilation, DB/UI/A8 proof remain pending.

## Selected report incomplete state

Volume reports distinguish pending computation from incomplete terrain coverage.
Incomplete results show unavailable text and actual sampled/required counts;
they expose no cut/fill/net or TSV copy control. A matching later complete result
replaces that state without editing the footprint. Result adoption requires the
latest correlation and matching current/state/result petal identities. Unbound
or disabled terrain clears the selection report and rejects late replies.

## Typed property values and local edit drafts

`property_value::PropertyValueKind` is the Inspector's JSON parsing boundary.
Strings preserve the exact input, including whitespace and empty strings.
Numbers use JSON grammar and must have a finite numeric representation.
Booleans accept only lowercase `true`/`false`, with surrounding whitespace
ignored. Objects and arrays require the selected root shape and retain nested
JSON. Existing null values remain null. The legacy datetime/geometry/reference/
address selector entries remain string-backed labels; this parser does not
claim format validation or backend field-definition enforcement for them.

`InspectorFormState::property_editor` holds only form drafts, keyed by the
NodeManager-derived node id and property key. It does not own selection or
mutate the loaded property bag. The Inspector resets these drafts, Add buffers,
type defaults and inline errors when its node scope changes or is cleared.
Deleting a property clears its draft after authoritative readback removes it.
An existing-property editor fixes its JSON kind from the stored value; a dirty
draft cannot overwrite a newly changed stored value without reopening.

Add/Save parse before queueing the existing `SetNodeProperty` UI action.
Invalid input retains the draft and selection and produces an inline error.
Adding a duplicate key is refused in favor of Edit. A submitted valid value
remains visibly unconfirmed until the existing property-readback path contains
that value; the UI never inserts a speculative value into `node_properties`.
After 30 seconds without confirmation, the draft remains with an error and
Refresh stored values can request the existing load action. Local payloads
remain editable after the pending write expires, even if that refresh is
still loading; Add/Save wait for loaded stored values. Cancel discards
only an unsent draft and is disabled while a write is awaiting confirmation.

The property grid supersedes the earlier vertical property cards (and the
historical copy-box presentation in §inspector-units). `property_grid` owns
only column geometry, compact previews and exact clipboard formatting;
the parser and draft model remain shared with Add/Save. Key, Type, Value and
Actions columns fill the available body width. Column minima reserve readable
values and the armed Confirm/Cancel pair; below that width a horizontal scroll
keeps the Inspector's viewport boundary intact. Row actions remain text buttons
and use at least 24 points or 20 physical pixels, whichever is larger.

Column and action slots allocate their exact rectangles before rendering child
controls. Child content cannot enlarge the table's measured width. Edit and
Copy retain their own slots; the destructive-action slot always reserves the
full Confirm/Cancel pair and uses a vertical parent for the canonical helper's
internal horizontal row. Its width includes the actual square-button minimum,
with whole-point rounding. Arming and canceling therefore do not change the
column origins or require a new horizontal scroll position. Child clipping
uses those same slot rectangles, preserving actual leaf-response observations.

Expanded editors and read-only JSON details use the table viewport's horizontal
range captured before its horizontal ScrollArea. The shared `Columns::expanded`
scope places the existing content at that visible x range and the current row y;
native scope allocation contributes its actual vertical height. The already
allocated full-width table still owns horizontal content size. This keeps the
Editing header, value input, status and Save/Cancel in view at either scroll
edge without forcing the scroll offset or introducing a second editor/draft.

Scalar previews are single-line ellipses with bounded hover text. Newlines and
tabs are escaped for display, empty strings are explicit, and objects/arrays
show field/item counts. Compound values expand into a bounded read-only JSON
view; Edit expands the existing typed editor under its own row. Copy always
uses the full stored string verbatim or compact JSON for other kinds, never a
preview or unsaved draft. There is no silent display-to-clipboard coercion.

Every row scopes widget IDs by node id and key. Edit/Copy/Delete remain visible
while editing; Delete uses the canonical two-step confirmation helper with
node/key identity and contextual action names. Canceling deletion or editing
does not change the loaded value. Native `ResponseExt` names and actual-response
observations remain on the leaf controls, including confirm/cancel. Editors
stay inside the shared layout-policy scroll seam; pending, dirty, conflict,
timeout and authoritative-readback rules above remain unchanged. Add Property
stays after the table, with its existing accessible names. This presentation
does not add history, replay, charts, or field-definition enforcement.

The existing Inspector transform test is extended as
`inspector_transform_and_typed_property_editing_journey`: original transform
assertions plus typed Add, invalid-input/no-write behavior, the actual GUI
command's `correlation_id: None`, draft/readback confirmation, stale-value
refusal, timeout, deletion and node-scope reset. The same journey now also
authors three-width table/target observations, exact clipboard output and
real egui Delete arm/cancel/confirm plus Edit/cancel interactions. No new test
function is added. At 260/320/640 points it positions the native scroll container
at the right edge, then compares content width, horizontal offset and Edit/Copy
positions before/after arming and cancellation. It checks the complete action
rectangles against their actual hit rectangles, child clips and table clip,
including the full Confirm/Cancel pair; a control count alone is insufficient.
At those same widths it opens Edit from the right edge, checks the full header,
input, status and footer against actual hit/clip geometry, verifies real editor
height with unchanged table width/offset, and cancels a dirty draft without a
write. Header/status observations are their actual native label responses.
Only this declared native container ID is used for scroll setup; widget facts
come from the actual same-pass responses. Its persistence readback is an explicit model simulation;
these authored assertions have not been executed in the source-only lane and
do not prove DB persistence or backend schema enforcement. Integrated
compilation, execution and live UI readability/a11y verification remain required.

## Hierarchy rows and local appearance

`tree_entry::Target` carries the clicked row's kind, stable ID and ancestry.
`tree_entry::row` owns the shared fixed icon/name/visibility/menu layout and
routes typed presentation commands. Menus never borrow the Inspector's current
target. Rename and Options reuse Node Options or Entity Settings; Node Options
retains its explicit subtree-delete confirmation. Parent Delete now routes to
Entity Settings' existing two-step confirmation, backed by the reviewed stored-
Manager authorization and atomic physical subtree cascade. It does not invent
P2P tombstones or ordinary history coverage for structural deletion.
Copy ID copies the clicked ID. Menu editing controls require the exact active
scope for which LocalUserRole is resolved: nodes use their owning petal;
hierarchy rows require that entry with no downstream scope. The opened dialog
then obtains its own target-authorized stored details and role. Every viewport
and tree opener uses the same entity_details factory; cached names are titles,
not editable stored-value guesses.

The existing NavigationManager remains the hierarchy cursor and NodeManager
remains the node-selection authority. A current hierarchy name is a no-op;
other names navigate with full ancestry, while only native chevrons collapse.
Selected rows have a strong fill and selected button semantics; active ancestors
use a quieter path fill. Visibility remains an independent 24-point gutter.
Horizontal scrolling keeps row controls reachable at narrow widths, with a
220-point tree content floor and 12-point indentation. Inactive-petal node names
enter their complete petal ancestry and request bounded deferred selection.

`tree_appearance` draws the appearance dialog inside the existing ActiveDialog
mutual-exclusion family. Its actual circular hue/saturation wheel and numeric
Hue/Saturation/Brightness controls sit alongside the named vector-icon palette;
hex input uses strict #RRGGBB. The preview and Reset alter only the draft until
Save; Cancel/Escape discard it. An invalid hex or deleted target cannot save.
Vector symbols retain an outline for very dark/light selected colors, without
depending on font glyph coverage. These are per-device display preferences in
AppSettings, keyed by hierarchy kind plus stable entry ID, not replicated scene
properties. Existing debounced settings persistence and QA-root isolation apply.
No custom color replaces selection or visibility state.

The sidebar no longer hosts Reset Database or the old Space count grid. Moving
the reset action to the campaign's application Danger Zone remains a separate
shell task; this slice does not claim that rehoming is complete.
## Selected terrain volume and material

The true-volume block is the primary report. Footprint-only geometry is secondary and never labels arbitrary proposals as earth. Selected standalone fill/cut/net and declared material precede combined terrain totals; TSV preserves operation/material with delimiter-safe text.

## Changed-space intersections

The Volume report shows selected standalone figures before combined terrain totals, then object impacts against
the combined changed surface within that footprint. Object movement, mesh arrival, visibility and material changes
advance a scene revision; a matching request cannot adopt old-revision object data. Unresolved geometry and omitted
objects remain visible. GLB/rotated bounds are explicitly potential intersections; sampled overlap is not exact mesh
volume. Select/open-properties returns a stable node ID through the current-Petal picker and reveals Inspector.
Viewer access gates the report in the same UI pass. The old XZ-origin list remains only a bounded, explicitly
non-intersection TSV section. All TSV names, kinds, operations and material fields escape tab/CR/LF delimiters.


Query tab requires an open petal for Run and Ctrl/Cmd+Enter, labels the petal
by name and offers its full ID on hover. There is no unscoped template fallback.
The existing captured scope/request ID travels through ScopedQuery; stored DB
Viewer authority and source substitution are the actual guard. Results
correlation/invalidation and current-petal selection are unchanged. The hint
lists only petal-filtered desktop sources.

## §feedback-rendering

The last overlay uses active_notice rather than active_toast, so severity and
notice ID reach the renderer. Errors use normal-size text, reserve a named
Dismiss action, and call hold_notice on pointer containment. Dismiss retains
the history entry. The existing numeric render smoke also exercises real pointer
hover and press/release through expiry, priority and dismissal, alongside the
queue's existing journeys.

The status bar receives LocalUserRole and optional injected NodeIdentity through
gardener_console. Role text is bounded/truncated with full verse and capability
details on hover; connection status uses neutral shared tokens. See ../AGENTS.md
§feedback-status for contrast pairs and condition ownership.

## Terrain report target picker

The Volume report includes a read-only target list for the active, loaded Petal:
palette proposals and actual material-bearing Brush regions share the existing
`ProposalEditState.selected` authority. The Viewer-gated report section hosts
the picker, which shows at most 128 named, selected-state controls in a bounded
scroll area. Names include target kind and ID; long visible IDs truncate with
full-ID hover text. Every click queues the same validated `OpenTerrainReport`
action used by the viewport menu. Draft fallback and volume/material/intersection
math are unchanged. Existing report and context-menu journeys cover the shared
handler and stale-map/Petal/role refusals; source authoring alone does not prove
compiled or live behavior.

## Object-impact report scrolling

Object-impact rows share the report section's existing outer scroll container.
The scene snapshot already bounds this list to128 objects. An additional vertical
viewport could retain its own offset while the outer report moved, hiding an
object's name and method above its visible material and volume. Keeping each row
in the surrounding flow makes its identity, method, material, volume and selection
action move together. The target picker retains its separate bounded list.

The existing TSV journey also renders five actual impact rows at1024×768,
1280×720 and1920×1080. It seeds a legacy inner-scroll offset, checks the actual
painted glyph bounds against their clip rectangles, verifies the first row's
sequential fields at two outer offsets, clicks its selection action, and scrolls
to the last row. This source extension does not establish execution or live QA.


## Chrome counts, hierarchy targets and copy receipts

The footer derives Petal and object counts from the loaded VerseManager tree at
its exact navigation scope. Library/Verse/Fractal/Petal labels identify that
scope; invalid ancestry shows unavailable rather than a stale Dashboard total.
Objects include hidden stored nodes and are not mislabeled as loaded 3D models.
Counting sums existing Vec lengths without a DB query or per-node allocation.

Hierarchy CollapsingState keeps its existing IDs and behavior. The tree scope
sets both indent and icon width to the shared 24-point target minimum (with the
20-physical-pixel backstop), which enlarges the native chevron hit rectangle.
Names still truncate with full hover; horizontal scrolling preserves deep rows.
The role summary explicitly disables label selection, so its hover metadata is
not advertised as a click/drag action. Real footer buttons reserve target space.

widgets::copy_with_feedback is the explicit copy-and-notice seam used by the
value widgets, hierarchy Copy ID and error details. It queues the original value
through egui's normal output; the host's synthetic-input clipboard suppression
remains authoritative. It never reads or writes the host clipboard directly.


### Reserved footer actions and diagnostic overflow

Footer peer, Status, identity-warning and error-history controls are allocated
first on the right by Sides::shrink_left with fixed bounded action widths. The
left side contains only clipped/truncated online, role and content metadata.
Long conditions or quarantined-panel messages cannot consume action space.
Status shows an active-condition count and opens complete wrapped conditions,
hidden counts, quarantine details and scoped content totals. Error history stays
reachable after the latest error ages out or is acknowledged. Identity warnings
open their full message and a separate Dismiss action. Popup IDs remain stable
when the optional identity control appears or disappears; its dismissal closes
that popup. Typed footer actions apply after immutable rendering, retaining
notice history and unrelated conditions.

The same reservation rule applies to copy_value_box headers: copy controls and
trailing actions allocate before the informational title. Clickable value rows
and copy icons reserve the shared minimum target height/width; ordinary labels
are unchanged. Copies still use the egui output seam, including context-owned
footer copies, so synthetic input never bypasses A8 suppression.

## ?transform-snapping

transform_options renders the real Move/Rotate/Scale constraints from ToolState.snap: one axis, grid increment with current map units, angle increment, uniform/per-axis scale and scale increment. There are no Pivot, selection-filter or highlight-style promises. The descriptor keeps only real gesture instructions. The Options dispatcher passes its existing active-Petal scale and the same settings consumed by the gimbal; rendering never starts a transform or persists one.
