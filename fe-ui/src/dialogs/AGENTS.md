# fe-ui/src/dialogs — floating dialogs + ActiveDialog

- `mod.rs` — `ActiveDialog` enum (mutual exclusion — at most one dialog open
  at a time), plus the small supporting types owned by its variants
  (`CreateKind`, `EntitySettingsType`, `SettingsTab`, `PeerRoleEntry`,
  `ApiTokenEntry`).
- One file per dialog: `context_menu.rs`, `create_entity.rs`,
  `gltf_import.rs`, `join.rs`, `peer_debug.rs`, `node_options.rs`,
  `entity_settings.rs` (General/Access/API tabs — the largest), and the
  Hexon Manager (`hexon_manager.rs`) / Petal Manifest (`petal_manifest.rs`)
  dialogs, which depend on `crate::terrain_map` for their DTOs.

Every render function early-returns via `let ActiveDialog::Variant { .. } =
ui_mgr.active_dialog else { return };` — this is the enforcement mechanism
for mutual exclusion; don't replace it with a separate `open: bool` flag.

## §destructive-confirm

Destructive buttons use a two-step inline confirm: first click flips a
pending flag, second click ("Confirm …") executes; "Cancel" clears the flag.
Canonical implementation: `entity_settings.rs` Delete (`pending_delete`
variant field). Node Options' Delete Node (`DbCommand::DeleteNode`, cascades
to child waypoints) and the sidebar's Reset Database follow it; the sidebar
and the inspector's token Revoke keep the pending flag in egui temp data
because their state lives outside `ActiveDialog`. New destructive actions
must adopt this convention — never a single-click irreversible send
(ux hardening batch 2026-07-17).

## §settings

D-78 (`p2p_asset_streaming_20260718` FR-7), added by ultrapilot worker w4a.

- `settings.rs` — `ActiveDialog::Settings` (stateless unit variant, same
  mutual-exclusion idiom as `PeerDebug`). Reads/writes
  `crate::settings::AppSettings` (w4b) **directly** — no `UiAction`
  round-trip, since there's no derived/queued side effect to route through
  the action queue (contrast with e.g. `PetalManifestSave`, which persists to
  the DB). First two live knobs: `render_distance` and
  `mesh_budget_ceiling` (the cheapest first knob per the decision record —
  `MeshInstanceBudget.ceiling` is already a runtime field). More knobs
  (stamp caps, tile source mode, camera, P2P relay/peer config) land as
  `AppSettings` grows further fields — this dialog just needs matching rows
  added.
- Reachability: a **"⚙ Settings"** button in `panels/toolbar.rs`'s right
  cluster (beside Data/Tools/Maps), pushing `UiAction::SettingsToggle`
  (cross-worker variant, w4b `actions/mod.rs`) rather than a direct
  `ui_mgr.open_dialog(ActiveDialog::Settings)` call — routed through the
  action queue so w4b's `process_ui_actions` can fold in future side effects
  (e.g. settings persistence) alongside the dialog toggle. TODO seam: the
  button compiles once `SettingsToggle` + its match arm land.
- `dialogs::settings_window` is called from `panels::gardener_console`
  alongside the other `render_*_dialog` calls — see `panels/AGENTS.md`
  §terrain-tools for the `gardener_console` signature change this and the
  terrain proposal panels required (`app_settings` param).

## §map-terminology

User-facing artifact name is "map" ("Map Manager", "Search maps...", "Set
petal map"); "hexon" is reserved for the package/file format in
publish/import contexts ("Install from file..." + `.hexon` filter). Internal
identifiers (`hexon_id`, DTO names) unchanged (baked terminology decision,
ux hardening batch 2026-07-17).
