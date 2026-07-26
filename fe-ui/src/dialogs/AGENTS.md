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
variant field). Node Options' Delete Node and the sidebar's Reset Database
follow it; the sidebar and the inspector's token Revoke keep the pending flag
in egui temp data because their state lives outside `ActiveDialog`. New
destructive actions must adopt this convention — never a single-click
irreversible send (ux hardening batch 2026-07-17).

Node Options' Delete Node now routes through the sync-safe spine path
(`UiAction::DeleteNode { cascade: true }` → `actions::node::handle_delete` →
`DbCommand::CascadeTombstoneNode { auth: CallerAuth::Local }`), NOT the legacy
raw `DbCommand::DeleteNode` drop — this is the real remove-a-node path that
fixes the "empty husk" bug (contextual_controls_20260725 FR-2). The confirm
copy comes from `ui_shell::modal::cascade_confirm_message` (Q-2: always
confirm on cascade). The descendant count shown is currently `0` (the flat UI
hierarchy can't resolve a node's subtree size); an authoritative count is a
T1/T5 query follow-up.

## §context-menu

`context_menu.rs` owns the object-aware right-click menu (D-A9; radial
deferred). Its heart is a **pure table** — `menu_for(hit: &HitTarget) ->
Vec<Verb>` — mapping the viewport hit classification (`node_manager::dispatch`,
the SAME enum left-click resolves) to the ordered `Verb` set valid for that
object (FR-1). It is total over every `HitTarget` and unit-tested exhaustively;
no verb appears for an object it can't act on. `TerrainCell` is treated like
empty ground (creation verbs only); `GimbalAxis` (a transform widget) yields no
menu.

Ratified per-object verbs (spec Q-1):

| Object (`HitTarget`) | Verbs |
|---|---|
| empty ground (`Empty`/`TerrainCell`) | Add Empty Node · Add GLTF Model |
| node (`Node`) | Edit Properties · Rename · Duplicate · Clear Properties · Copy API · Report · Delete |
| stamp (`Stamp`) | Promote to Node · Scale/Rotate · Slide Along Path · Copy API · Report · Delete |
| path (`PathSegment`) | Edit Path · Add Stamps · Copy API · Report · Delete |
| path point (`PathVertex`/`PathHandle`) | Corner/Smooth · Delete Point |
| earthwork region (`TerrainProposal`) | Edit Region · Report Volume · Copy API · Delete |

Verb → action map (`verb_action`, node-scoped verbs this track owns):
`Delete → UiAction::DeleteNode { cascade }`, `Duplicate → DuplicateNode`,
`ClearProperties → ClearNodeProperties`, `CopyApi → CopyApiString`,
`Report/ReportVolume → ReportObject`, `EditProperties → LoadNodeProperties`.
Path/stamp/region verbs return `None` (routed by their owning surfaces —
T2/T3). Handlers live in `actions/node.rs` + `actions/node_props.rs`.

**Clear Properties ≠ Delete** (the husk-bug distinction): Clear Properties
removes a node's reserved custom properties but KEEPS the node
(`node_props::handle_clear`); Delete tombstones it. Both coexist on the node
menu.

**FR-4 seam (T5 `endpoint_api_surface`).** `Copy API` / `Report` /
`Report Volume` are seam-gated (`verb_is_seam_gated`): they call
`crate::gis::egress_strings::{api_string_for, report_for}(node_id: &str) ->
Option<String>`. On `Some` the verb is enabled (the clipboard write is
render-side — only egui `ctx` can touch the clipboard, no clipboard dep); on
`None` the verb renders **disabled-with-an-explanatory-hint**, never silently
absent (ui_ux §6). Verbs light up automatically when T5's seam yields `Some`.

**Render coverage.** Node verbs are live now via the Node Options surface
(`node_options.rs`), which is object-scoped (carries the `node_id`). The
viewport right-click menu currently renders the empty-ground verbs because
`ActiveDialog::ContextMenu` carries only the cursor world position; object
menus light up through the same `menu_for`/`render_verb_button` machinery the
moment that variant carries a `HitTarget` (a one-line change gated on enriching
the `ContextMenu` variant + classifying the hit at the `viewport.rs` open site
— both central/non-T4 files).

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
