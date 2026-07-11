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
- `query_tab.rs` — inspector "Query" tab (ad-hoc SurrealQL).
- `portal_toolbar.rs` — replaces the inspector panel while a portal webview
  is open (back/url/close row).

All panel-rendering submodules are `pub(crate)` — nothing outside `fe-ui`
should render sub-panels directly; go through `gardener_console`.
