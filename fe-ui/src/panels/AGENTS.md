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

## §tool-panel

- `tool_panel.rs` — Tools panel: an independent floating window (like
  `gis_panel`, not part of `ActiveDialog`) that will host hexon-path-asset
  controls. State lives in `ToolPanelState` (`open`, `selected_hexon_ref`,
  `spacing_mode: SpacingMode`, `spacing_value`, `count_value`,
  `tangent_align`); `SpacingMode` is `FixedSpacing | FixedCount`. This pass
  ships the shell + a "Path Asset" section with the repetition/pattern
  controls (spacing-mode radio, spacing/count `DragValue`, tangent-align
  checkbox) rendering over a "(hexon picker — coming soon)" placeholder, plus
  a stubbed "Terrain Tools" section. The hexon picker (reusing
  `hexon_manager.rs`'s list/select UX) and the `UiAction` wiring that turns
  these fields into a stamped path asset are deferred to the stamp unit.
  `gardener_console`'s signature grew a trailing
  `tool_panel: &mut crate::panels::tool_panel::ToolPanelState` param — the
  caller (`plugin.rs`) must register the `ToolPanelState` resource and pass
  it through.

All panel-rendering submodules are `pub(crate)` — nothing outside `fe-ui`
should render sub-panels directly; go through `gardener_console`.
