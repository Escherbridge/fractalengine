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
menu. Classifier now resolves all four extra target kinds: `SetCornerSmooth`
(corner-kind cycle → PathSetAnchorCorner), `DeletePoint` (PathRemovePoint),
`EditRegionParams` (TerrainProposalSelect + RevealSection "terrain"), and
`TerrainProposal` Delete via two-step confirm to TerrainProposalDelete;
`render_context_menu` takes `path_state`.

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
- The classifier (`node_manager::context_pick`) produces `Node`, `Stamp`,
  `PathVertex`, `PathHandle`, `PathSegment`, `TerrainProposal` (palette
  overlay), `EarthworkRegion` (committed Brush/shape region) and `Empty`.
  Ground overlays route Delete through `proposal_delete_action`: a palette
  proposal → `TerrainProposalDelete` (drops its record); an earthwork region →
  `SculptDeleteRegion` (drops the record AND tombstones its endpoint node via
  `actions::terrain_proposal::handle_delete_region`). A region's menu is
  `ReportVolume`/`CopyApi`/`Delete` — no `EditRegionParams`, since regions are
  not in the palette mirror. Region `CopyApi`/`ReportVolume` render
  disabled-with-hint: the target carries no `node_id` yet (N-8, never silent).

## §settings

The floating Settings dialog is gone; Settings is the right-sidebar section
`ui_shell/right_sidebar.rs::render_settings_section` (see `ui_shell/AGENTS.md` §right).

## §map-terminology

User-facing artifact name is "map" ("Map Manager", "Search maps...", "Set
petal map"); "hexon" is reserved for the package/file format in
publish/import contexts ("Install from file..." + `.hexon` filter). Internal
identifiers (`hexon_id`, DTO names) unchanged (baked terminology decision,
ux hardening batch 2026-07-17).
