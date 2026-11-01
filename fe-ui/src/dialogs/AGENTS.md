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
**Canonical implementation (W1-S4): `panels::widgets::confirm_button(ui,
id_salt, label, confirm_label) -> bool`** — extracted from this exact
`entity_settings.rs`/inspector-Revoke shape, now also timing out after 4s and
disarming on a click elsewhere. New destructive actions call this instead of
re-deriving the pending-flag dance; existing per-dialog variant-field flags
(`entity_settings.rs`'s `pending_delete`, Node Options', the sidebar's) are
pre-existing and not retrofitted by this slice, but any NEW site should use
`confirm_button`. Never a single-click irreversible send (ux hardening batch
2026-07-17).

## §borrow-split-on-active_dialog (W1-S4)

Every render fn pattern-matches `ActiveDialog::Variant { ref mut field, .. }
= ui_mgr.active_dialog else { return };`, which borrows `ui_mgr.active_dialog`
— a single field — for as long as `field` is used. If the body also needs
`&mut ui_mgr` for something else (`send_or_toast`, `close_dialog`,
`ui_mgr.join_request`, a notice), that call must happen **after** the last
use of `field`, not inside the same `.show(ctx, |ui| { .. })` closure that
reads it — otherwise the borrow checker sees two live `&mut` paths through
`ui_mgr` at once and refuses to compile. The fix used in `join.rs`,
`gltf_import.rs` and `create_entity.rs`: read any other `ui_mgr` state needed
(e.g. `join_request.pending`) *before* the match; inside the closure, only
set local flags/owned values (`Option<DbCommand>`, `bool`); then act on
`ui_mgr` again only after `.show()` returns, once the field-borrow's last use
has passed.

## §dialog-requests — one in-flight shape for Join and Import

A dialog that must stay open until its DB reply lands keeps its round-trip in
an `actions::DialogRequestState` on `UiManager` (`join_request`,
`gltf_import_request`) — never in egui temp memory, because the reply arm in
`verse_manager/db_results` has no egui context to clear it with. Lifecycle:
the dialog `start(now)`s it only after `send_or_toast` succeeds; the reply arm
`finish()`es it (true only for a request this dialog sent) and closes the
dialog only while that dialog is still the active one; Cancel/close
`reset()`s it. A dialog whose failure arrives as an uncorrelated
`DbResult::Error` bounds its lock with `waiting_within(now, timeout)`.
Both dialogs bound it (30 s): even a typed reply never comes if the DB thread
died mid-request, and a lock without a bound leaves no Cancel and no close-X.

## §join-flow (D-05, W1-S4/W1-S2)

`join.rs` no longer closes the moment Join is clicked. It `start`s
`ui_mgr.join_request` (after `send_or_toast`) and keeps the window open with a
spinner; `join_request.error` (set only by
`verse_manager/db_results/hierarchy.rs`'s `JoinVerseFailed` arm) renders
inline. The window's native close-X is withheld while pending, same as
Cancel/Join/the invite text field, so an in-flight join cannot be
accidentally abandoned mid-air — only a `VerseJoined`/`JoinVerseFailed` reply
or an explicit Cancel while *not* pending ends it. The lock lifts after
`JOIN_LOCK_TIMEOUT_SECONDS` (30 s) if no reply arrives at all.

## §gltf-import-flow (D-06, W1-S4)

`gltf_import.rs::import_enabled(file_path, petal_id)` is the pure gate for
Import (both must be non-blank). A click no longer closes the dialog; it
`start`s `ui_mgr.gltf_import_request`, locks the fields and shows
"Importing…". `db_results/mod.rs::finish_gltf_import_request` (called from the
`GltfImported` arm) closes the dialog and toasts "Imported {name}". A failed
import replies with an uncorrelated `DbResult::Error("GLTF import failed:
…")`; `db_results/query.rs::handle_error` recognizes that prefix and, when
the dialog's request is in flight, finishes it with the humanized reason as
`gltf_import_request.error`, rendered inline so the user can fix the path and
retry at once. `IMPORT_LOCK_TIMEOUT_SECONDS` (30 s) is only the no-reply
safety net. The Position row reads in meters from the petal `world_scale`
that `panels/mod.rs` threads in.

## §create-entity-flow (D-07, L-06, W1-S4)

`create_entity.rs::build_create_command` is the single pure builder for all
four `DbCommand::Create*` variants (unit-tested table), sent by
`render_create_dialog` through `send_or_toast`. (The bare-send `apply_create`
wrapper had no callers and was deleted in the W1 advocate pass.) Create is
disabled on a blank name; the Name field autofocuses on open (nothing else in
this small dialog competes for focus); Enter submits exactly like clicking
Create. A node spawns at `spawn_position` — the camera focus target
`panels/mod.rs` threads in (L-06) — or the origin when there is none.
**Still open (D-07 "toast on result"):** a successful create has no toast; the
`VerseCreated`/`FractalCreated`/`PetalCreated`/`NodeCreated` arms in
`verse_manager/db_results/` would need a `DialogRequestState` like Import's to
tell a dialog-sent create from an API/pen one.

## §hexon-manager — Maps section cards (M-01, M-03, M-04, X-02)

`hexon_manager.rs` renders as the right-sidebar Maps section;
`ActiveDialog::HexonManager` is only its state carrier. The Installed tab is
one `egui::Frame` card per installed map (clipped region name + Seed toggle,
a zoom/tiles/size line, then the Map and Remove actions) rather than the old
7-column Grid, built from `panels::widgets::{clipped_value, role_gated,
role_denial_reason, confirm_button}`. While `PetalMapState.pending_set` is
`Some`, the active map's button reads "Applying…" instead of "Active map ✓"
(`terrain_map/AGENTS.md` §petal-map truth). Remove is a `confirm_button`
inside `add_enabled_ui(can_edit)`, so a Viewer sees the same button disabled
with the role reason. The carrier's `pending_remove` field is now dead weight:
`seed_hexon_manager()` in `ui_shell/right_sidebar.rs` still fills it, nothing
reads it (removal belongs to the W6 dead-code sweep).

## §claim-ownership (F6)

`entity_settings.rs`'s General tab (Verse only) shows "Claim Ownership"
whenever `local_role.role != Some(RoleLevel::Owner)` — not just when a
viewer badge is literally rendered, since the dialog has no separate
"orphaned" signal client-side. It follows §destructive-confirm's two-step
pattern (`pending_claim_ownership`, mirroring `pending_delete`) even though
the action isn't destructive, because it needs the same "explicit, not a
single click" property OF-4 requires. The real eligibility guard (never
shared, `did:key:`-shaped stale creator, no foreign role grant) lives
entirely in the DB handler (`fe-database/src/handlers/claim_orphaned_verse.rs`)
— a confirmed click on an ineligible verse sends `DbCommand::
ClaimOrphanedVerse` anyway and gets back a typed `OrphanedVerseClaimRefused`,
surfaced as a toast (`verse_manager/db_results/roles.rs::
handle_orphaned_verse_claim_refused`) rather than failing silently. On
success (`OrphanedVerseClaimed`), `handle_orphaned_verse_claimed` flips
`LocalUserRole.role` to `Owner` directly when the claimed verse is the
active one — no extra round-trip, since the claim's success already is
what the next `resolve_role` would say.

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
