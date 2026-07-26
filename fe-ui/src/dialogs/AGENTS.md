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
confirm on cascade). The descendant count is authoritative end-to-end: arming
the confirm sends `DbCommand::CountNodeDescendants`; the
`DbResult::NodeDescendantCount` arm stashes it into the open dialog's
`descendant_count` field (Node Options or the context menu — stale/mismatched
results are dropped); the generic `cascade_confirm_message(0)` copy shows until
the count lands.

Node Options is also the node RENAME surface: its Name field diff
(`node_options::rename_request`, pure) sends `DbCommand::RenameNode { auth:
CallerAuth::Local }` on Save; the tree updates on `DbResult::NodeRenamed`
(never optimistically). Duplicate routes `UiAction::DuplicateNode` →
`DbCommand::DuplicateNode` (the legacy fake `CreateNode "{name} (copy)"` path
is gone; fe-database owns the copy semantics, replying `NodeCreated`).

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

**Render coverage (integration pass 2026-07-26).** `ActiveDialog::ContextMenu`
now carries `target: Option<ContextTarget>` — `{ hit: HitTarget, node_id:
Option<String>, stamp: Option<(track, index)> }` — filled by
`node_manager::context_pick::classify_context_menu` (same pick machinery as
left-click: ray/AABB + `TrackPickShape`, stamp ground fallback via
`StampRenderIndex`; a dim `…` placeholder renders for the ≤1 unclassified
frame). Wiring per verb:

- Node hit: header shows the node name; classify also selects the node
  (mirrors left-click), so Edit Properties' `LoadNodeProperties` result
  delivers to the inspector. Rename opens the prefilled Node Options dialog
  (`node_options_prefill` — Name field is the rename surface). Delete is a
  two-step IN-MENU confirm with the live descendant count (same
  `CountNodeDescendants`/`NodeDescendantCount` flow as Node Options).
- Stamp hit: header `Stamp {i} — selected` reads the live
  `StampInteractionState.selected()`; classify pushes `SelectStamp` (lazy
  promotion, idempotent). Promote/Delete/Copy API/Report gate on the LIVE
  promotion state (`render_gated_verb_button` — explicit hint, never silent);
  Scale/Rotate + Slide route to the Tools-sidebar per-stamp editor
  (`PathSelectTrack` + `ToolPanelState.stamp_edit_index`) with a toast.
- The classifier only produces `Node`/`Stamp`/`Empty` today —
  vertex/handle/segment/proposal menus render correctly if a future
  classifier yields those hits; their unwired verbs toast instead of
  silently no-oping (N-8).

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
