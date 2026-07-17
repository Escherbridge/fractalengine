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

## §map-terminology

User-facing artifact name is "map" ("Map Manager", "Search maps...", "Set
petal map"); "hexon" is reserved for the package/file format in
publish/import contexts ("Install from file..." + `.hexon` filter). Internal
identifiers (`hexon_id`, DTO names) unchanged (baked terminology decision,
ux hardening batch 2026-07-17).
