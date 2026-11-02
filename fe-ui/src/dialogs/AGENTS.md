# fe-ui/src/dialogs — floating dialogs + ActiveDialog

- `mod.rs` — `ActiveDialog` enum (mutual exclusion — at most one dialog open
  at a time), plus the small supporting types owned by its variants
  (`CreateKind`, `EntitySettingsType`, `SettingsTab`,
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
id_salt, label, confirm_label, accessible_name) -> bool`** — extracted from this exact
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
results are dropped). Pending counts show Counting children; Confirm Delete is
disabled until Some(count), including Some(0) for an actual leaf.

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


## Hierarchy mutation errors

EntitySettings rename, description and delete report channel failures through
the shared error notice after releasing the dialog borrow. A failed delete send
keeps its confirmation open. The database authorizes these hierarchy mutations
at the actual stored target with Manager; the navigation role is not authority
for a different target opened from the tree.

## Authoritative hierarchy details

entity_details::dialog is the single EntitySettings constructor for all viewport
and tree openers. Editable name/access/description buffers start unhydrated;
the cached tree name is only a title. The first render queues GetEntityDetails.
The database resolves local identity and the target's actual stored ancestry,
authorizes Viewer from a captured role snapshot, fences that snapshot in the
read transaction, then returns bounded selected fields and the target role.
A missing default-access value stays explicitly unset, never guessed public.
A missing optional fractal description is an empty editable description.

EntityDetailsState owns Queued/Loading/Ready/Failed request state, a monotonic
process-local nonce and a 30-second deadline. Replies must match the nonce,
outer/inner kind and ID, expected stored ancestry and canonical scope; obsolete,
closed/reopened, foreign-target and timed-out replies cannot hydrate buffers.
Refresh compares each draft field with its previous stored baseline. Clean
fields update; dirty fields remain. A changed stored value behind a dirty draft
marks a conflict and disables writes until the user discards that draft. An
incoming value equal to the current draft is a confirmed match, not a conflict.
Read failure retains buffers and blocks saves until a successful retry. A read
refresh disarms any existing delete confirmation. Rename/access/description
acknowledgements queue a new authoritative read for the matching open target.

Dialog editing uses the role returned for this target, not a navigation role
from another scope. Backend mutation checks remain authoritative. The guarded
Claim Ownership recovery control remains available for private legacy orphan
verses even if Viewer details admission fails. This does not bypass the existing
never-shared/orphaned-creator checks. Existing Access/API result handlers retain
their separate contracts; this read seam does not claim their requests gained
new correlation fields.

Existing journeys cover command/result target and nonce binding, real persisted
private access and descriptions, dirty refresh/conflict/discard, read errors and
timeout. No new test functions were introduced; integrated execution and live
UI settings/readback verification remain required.

## Target-bound destructive confirmations and requirements drafts

Node Options uses the shared confirm_button for Clear Properties. Its salt is
bound to a target-specific dialog generation; changing targets, closing or a
render-frame gap starts another generation, so an armed state cannot carry to
another object or reopened window. The existing count-aware delete path now
accepts Option<usize>: None is pending, Some(0) is a leaf, Some(N) names the
count. Node confirmation is disabled while pending and the emitted action is
also guarded. Context menus use the same pending-count rule for plain nodes;
stamps, proposals and whole-track verbs retain their distinct confirmation
semantics. This is UI safety; the existing backend remains the authority.

Petal Manifest is explicitly a local requirements draft in this build. Both
Close and title-X ask to discard a dirty draft; Keep editing preserves all
buffers, and explicit Discard closes without a write. The duplicate render-distance control is removed while its existing data fields remain compatible.
Save is disabled with an explanation, and Copy JSON reports copy success
without clearing dirty or claiming persistence. Existing/unknown hexon IDs are
kept. Supplied installed IDs become selectable choices; unavailable lists use
an explicitly labeled manual path and unknown availability, not an invented
installed/missing result. Requirements JSON is not described as JSON Schema.

W6 persistence requirement: the database schema has petal.hexon_manifest, but
there is no Get/Set manifest command/result, scoped authoritative readback,
load/apply UI lifecycle or requirements replication route. manifest_open still
constructs a default draft and supplies no installed-ID catalog. Before Save
can return, add reviewed target-scoped authorization, complete requirements
validation preserving unknown references, transactional storage/readback with
correlation and failure/conflict handling, real catalog feeding, and P2P
replication/hydration. Keep residency distance owned by Settings; requirements
Save must not write its hidden legacy render_distance field. The legacy save
action is rejected with a shared error notice and has no DB sender.

Context-menu Clear Properties opens Node Options rather than directly queuing
the clear action. The target-bound confirmation remains the only touched
clear entry point. Editing the requirements add-ID/type controls also marks
the dialog dirty before Add is pressed; Close/X therefore cannot silently
drop that input. Copy JSON includes table requirements only and explicitly
excludes unapplied Add fields, with a visible Add instruction and copy notice.

## Terrain volume report routing

Report Volume is independent of the API egress seam. Both palette proposals
and Brush regions queue `OpenTerrainReport` with the render pass's active Petal;
the action handler revalidates map ownership, target existence and Viewer access.
Copy API and ordinary Report / Query retain their existing egress behavior.

Classified ContextTarget metadata pins the hit's owning Petal for the menu's
lifetime. Navigation to another Petal closes the classified menu before any
verb renders. Report routing also compares the pinned owner with the current
Petal and emits the pinned owner in its typed action; it never rebinds an old
hit ID to a new Petal. The action handler independently refuses queued commands
after navigation, including a same-ID target in the destination.

## Petal requirements

requirements_state.rs owns one dialog's exact ULID-correlated load/save and raw
persisted CAS baseline. Initial loading disables editing. Only a matching
Petal/scope/request readback acknowledges a save; newer edits and unapplied Add
fields stay dirty. Timeout/failure disables another save until an explicit read.
A differing retained draft requires an explicit keep-draft/use-saved decision.
No mutation is retried automatically. Actual navigation is checked at read,
write and response use, and reopening mints unrelated IDs.

The dialog edits bounded declaration/catalog metadata, not content installation,
scene fallback/gating or residency policy. Settings remains the distance
authority. Explicit legacy metadata is kept in the raw baseline without exposing
it as an active setting. Copy/dirty-close remain available for a retained draft.

## Target Access controller

entity_access separates request ownership from rendering. Each local request binds a fresh ULID to EntityType/id/full stored scope. Late/foreign receipts are ignored; timeout is unconfirmed. Failed writes require an explicit read before another mutation. Role controls show effective inherited roles and confirm every assignment/revoke. Owner blocking and Manager own-grant revocation use distinct copy. UI hints are not backend authority.

EntitySettings generates invites/API credentials through this typed owner, not legacy uncorrelated list/mint/invite state. Secrets are masked by default, clear on leaving API Access and are rejected at receipt use if another tab is active, including programmatic transitions. Explicit copy emits the requested value and a nonsecret toast. Read examples derive from the confirmed listener: Petal uses its registered scoped nodes GET; Verse/Fractal use authenticated scoped query POST. No health-check bearer demo or guessed port is shown.

The unrendered InviteDialog variant is retired. Legacy invite completion never opens an invisible exclusive modal or retains its credential; it directs the user to scoped Access controls. Existing General details/draft readback remain authoritative. The dialog body has one bounded vertical viewport below the tabs.

Legacy EntitySettings invite/JWT/list buffers are removed. Their producer census is empty after switching the renderer to EntityAccess. Uncorrelated legacy mint/invite completions retain no secret and cannot update the new target state; correlated Inspector replies retain their existing exact namespace/tab/target guards and API fallback behavior.

Destructive/role confirmation salts include the exact successfully loaded Access request ID, in addition to peer/token values. Opening the same target again or refreshing its readback creates a new identity and disarms prior controls; the shared confirm widget does not otherwise scope its temporary state to the dialog. Backend tuple/authority fences remain independent of this UI confirmation boundary.

### Access role confirmation generation

Role assignment and revoke confirmation IDs include the exact successful Access readback request ID, matching targeted token revoke. A fresh readback therefore cannot inherit a four-second confirmation arm from an older snapshot, even when target, peer and draft are unchanged. The existing EntitySettings journey clicks the actual role buttons, refreshes through the controller without an intervening dismiss click, applies the new receipt, and verifies both controls return to their initial arm step.

## Current context targets and URL saves

Every context edit verb uses the shared advisory role gate and rechecks its owning active Petal,
existing node/track and live path editor identity. Path vertex/handle classification retains its
track ID. Point removal uses the shared timed confirmation salted by complete route content,
track, index and navigation, then captures PointEditOrigin for checked dispatch/DB compare-and-swap.
It has no immediate destructive verb-map action. The bounded menu is constrained to the viewport;
long object names truncate with hover details. Existing node cascade confirmation remains intact.

Node Options validates its URL through the same canonical validator as Inspector and queues
SaveNodeUrl after the borrow window. It never mutates the manager optimistically. Rename and URL
remain distinct transactions; the URL saved notice confirms only the URL transaction. Copy/report
continue through egui output plus shared feedback, retaining A8 synthetic clipboard suppression.

Context point ownership begins at vertex/handle classification, before any menu click. A dialog-owned PointEditOrigin sidecar is discarded on every dialog replacement. Both Corner/Smooth and removal refuse a route that changed since classification. Their queued intents carry that original origin and recheck it at dispatcher use. The existing menu journey reorders and changes points before the first real pointer click and asserts neither verb queues a mutation.

## Node Options Portal storage

Node Options uses the same confirmed storage explanation as Inspector. Known unsupported URL
storage disables its URL editor/action while the shared Save still permits authorized rename.
Missing unique metadata explains that explicit nonempty Save creates local Portal metadata;
opening or selecting the dialog does not create it.
