# fe-ui/src/actions — UiAction queue, split by domain

New UiAction variants: `RevealSection { slug }` (idempotent reveal, handler routes
through RightSidebarState via the ToolStateParams bundle) and `TerrainProposalSelect { id }`.

## §notices — severity queue + humanized DB-error copy (polish_lane_20261031 W1-S1)

`UiManager`'s single toast slot became a severity-tagged queue (OD-1):
`notice.rs` (crate root, NOT under `actions/`) owns the `Severity`/`Notice`
types plus the pure `humanize_db_error` transform; `UiManager` (this file)
owns the queue, the error-history ring, hold/dismiss and the persistent
condition chips.

- **Priority over recency.** `active_notice`/`active_toast` pick the
  highest-`Severity` live notice, tie-breaking on the most recent
  `spawned_at` — NOT simply the most recent push. This is why a `show_toast`
  (Info, 2.5s) fired after a `show_error` (Error, 7s) never bumps the error
  off the single on-screen slot before its 7s are up. `show_toast`'s and
  `active_toast`'s signatures are unchanged on purpose: `panels/mod.rs`
  (Lane B frozen through W1/W2) calls both directly, and W3-S1 is the one
  that replaces the renderer to use `active_notice`/severity colors/hold/✕.
- **Queue cap 4, evict `Info` first.** `evict_for_new_notice` never drops an
  `Error` unless the queue is somehow all-`Error`. `error_history` (ring of
  20, newest-pushed first) only ever receives `Error`-severity notices —
  it's the status bar's "recent errors" popover source, not a general log.
- **Hold-while-hovered.** `held_notice: Option<u64>` is a single id, not a
  per-notice flag on `Notice` itself, because only one notice renders at a
  time (the renderer calls `hold_notice`/`dismiss_notice` with the id
  `active_notice` just handed it). `hold_notice` on an id no longer queued
  is a no-op rather than resurrecting it.
- **`humanize_db_error` keeps the raw string in the log, not the notice.**
  For a recognized `"<prefix>: permission denied: <reason>"` shape it
  returns a role-denial sentence; for anything else it returns a short
  `"Something went wrong: {snippet}"` and the FULL raw string stays only in
  the `bevy::log::error!`/`warn!` line the caller (`db_results/query.rs`'s
  `handle_error`, W1-S2) keeps alongside the `show_error` call — the notice
  itself is never the only place the real error text survives.
- **`send_or_toast`** replaces a bare `tx.send(cmd).is_err()` + `warn!` pair
  with one call that also tells the user ("Couldn't {what} — not connected")
  instead of failing silently on a closed channel (AGENTS.md anti-pattern
  6). `actions/mod.rs` itself has no remaining bare `.send(..).ok()` sites
  as of this wave (re-grepped at edit time — the AGENTS anti-pattern-6 audit
  (X-17) target list names other files: `sidebar.rs`, `create_entity.rs`,
  `gltf_import.rs`, `join.rs`, `entity_settings.rs`, `inspector.rs` — those
  slices call `send_or_toast` directly).
- **`JoinRequestState`** (`pending`/`error`) is the single source of truth
  the Join dialog (W1-S4) and the `JoinVerseFailed`/`VerseJoined` result
  arms (W1-S2, `db_results/hierarchy.rs`) share, instead of each inferring
  the other's state from `ActiveDialog::JoinDialog`'s own fields.
- **D-12 (Edit Path).** `UiAction::PathSelectTrack`'s handler now also
  activates the Pen tool (`ToolState::activate`, which does the one-shot
  Options reveal itself) and toasts "Path open in Options" — previously it
  only loaded the track's points, so right-clicking a path and choosing
  "Edit Path" silently did nothing visible. The misleading "Tools panel"
  toast strings at the `context_menu.rs` call site are a separate fix
  (W4-S4) — see the DOC NOTE below.
- **P-01 (Open Portal).** `OpenPortalOutcome::NoSelection` now shows an
  Advisory ("No node selected") and `InvalidUrl` now shows an Error ("Not a
  valid URL — include https://") instead of each being a silent/log-only
  no-op.

**DOC NOTE** for `actions/portal.rs`'s owner (W4-S4, which owns that file):
`OpenPortalOutcome::NoSelection`'s doc comment ("Matches original behavior:
a silent no-op... flagged as a fragility rather than fixed here") is now
stale — W1-S1 fixed it at the `process_ui_actions` call site (P-01); the
comment should say the arm shows an Advisory now, or be removed.

## First-class Brush producer

The sculpt pending-action bridge is removed. The viewport queues fully-threaded
payloads directly. Level persists `target_height`; Raise/Lower persist
strength-weighted magnitude; Level and Smooth store strength compatibly in `delta`.
`handle_brush` refuses disabled terrain docs so invisible edits cannot persist.

- `mod.rs` — `UiAction` enum, `UiManager` resource (queue + portal + active
  dialog + severity-tagged notice queue, §notices), `process_ui_actions`
  dispatcher, `send_or_toast`.
- `portal.rs` — portal open/close/go-back + `SaveUrl`. Pure decision
  functions (`compute_open_portal`, `should_auto_close`, `compute_save_url`)
  are unit-tested directly; see `fe-ui/src/AGENTS.md` §portal for the full
  save/open chain this drives.

  §save-url-validation: `compute_save_url` enforces
  `fe_webview::security::is_url_allowed` *before persistence* (FR-1 of the
  `inspector_settings_20260419` track) — previously only `OpenPortal`
  validated, so a blocked URL (e.g. `http://192.168.1.1`) could be saved to
  SurrealDB and later opened by any path that trusted stored URLs. The
  outcome enum (`Persist`/`Blocked`/`NoSelection`) keeps the decision pure;
  the `mod.rs` dispatcher maps `Blocked` to a `warn!` (security event) plus a
  user-visible toast. Empty buffer still normalizes to `None` (clears the
  URL) and is not treated as a validation failure. The restart round-trip
  test (persist → DB-hydrated `VerseManager` → reload → still-validated)
  lives in `node_manager/inspector_sync.rs`, next to the reload path it
  exercises. `compute_save_url` persists the *trimmed* buffer so the
  inspector and Node Options (both labeled "Portal URL") normalize
  identically (ux hardening batch 2026-07-17).
- `node.rs` — empty-node creation at a world position (context-menu "Add
  Empty Node" → `CreateNodeAt` → `DbCommand::CreateNode`, targeting
  `nav.active_petal_id`; toast when no petal is active). Also the T4
  object-verb handlers: `handle_delete` (tombstone/cascade), `handle_duplicate`
  (→ `DbCommand::DuplicateNode`, replies `NodeCreated` — fe-database owns the
  copy semantics), `handle_rename` (→ `DbCommand::RenameNode`; empty names
  refused loudly, tree updates on the `NodeRenamed` result only),
  `handle_promote_stamp`, `handle_copy_api`/`handle_report` (T5 seam). All
  lifecycle sends carry `CallerAuth::Local` (N-5) and warn on channel-closed
  (N-8).
- `transform.rs` — inspector transform Apply. `apply` returns
  `Err(reason)` on a parse failure (naming the axis/field) and the
  dispatcher toasts "Transform not applied — …" so a bad field is never a
  silent no-op (ux hardening batch 2026-07-17); no-selection/despawned-entity
  remain silent `Ok`.
- `node_props.rs` — custom node property load/set/delete (`DbCommand`
  fire-and-forget with a `warn!` on channel-closed). Also owns the reserved
  `gis.annotation.{title,body,color}` key constants + pure
  `annotation_fields_from_properties`/`annotation_field_for_key` extractors
  used by the inspector's Annotation card and its
  `NodePropertyDeleted`-handling fix — see root `AGENTS.md` §gis-query-ui.
- `hexon.rs` — hexon/tileset install/remove/seed/download + petal-map
  set + petal-manifest save/open.
- `query.rs` — SurrealQL query submission.
- `gis.rs` — GIS query panel + layer manager + view-mode I/O: submits
  `DbCommand::RawQuery` built by `crate::gis::query`'s pure builders, and
  round-trips terrain layer edits (`set_layer`) and the splat view mode
  (`set_view_mode`) via `SetPetalTerrain` (mirrors
  `hexon.rs::set_petal_map_scale`'s mutate-then-persist idiom). State lives
  in `crate::gis::GisPanelState` (top-level module, not under `actions/`) —
  see root `AGENTS.md` §gis-query-ui for why the state/pure-logic and the
  I/O are split this way (mirrors `terrain_map`'s `PetalMapState` vs
  `hexon.rs` split).
- `asset.rs` — node asset download request; only pushes onto
  `crate::asset_ops::PendingAssetOps`, mirroring the `hexon.rs` pattern of
  queuing for the main binary rather than resolving in fe-ui. See root
  `AGENTS.md` §asset-download. Also homes the Wave-1 stamped-asset state +
  handlers (§stamped-assets).
- `gpx.rs` — GPX track import request; only pushes onto
  `crate::gpx_ops::PendingGpxOps`, mirroring `asset.rs` exactly. See root
  `AGENTS.md` §gpx-import.

`process_ui_actions` stays a thin per-variant dispatcher; keep new actions'
actual logic in the matching domain file rather than growing the match arms
in `mod.rs`.

## §stamped-assets — individual stamp select + overrides (stamped_asset_nodes_20260725 T2)

`StampInteractionState` (homed in `asset.rs`, registered in `plugin.rs`) is the
per-stamp authority — DISTINCT from `NodeManager.selected` and
`PathEditorState.editing_track_id` (N-3): a stamp selection is its own storage
until promotion yields a node id. It holds the selected stamp, a
pending-promotion marker, the `(track,index)→node_id` promotion map, and the
sparse per-stamp overrides.

- **FR-2 select → promote (Q-2).** `handle_select_stamp` (no `DbCommandSender` by
  design — N-5: selection is a pure state transition) records the selection and,
  on the FIRST individual address of an un-promoted stamp, sets
  `pending_promotion`. The UI drains it (`take_pending_promotion`) and queues
  `UiAction::PromoteStamp`, which routes to T1 `DbCommand::PromoteInstance{..,
  auth: CallerAuth::Local}` via the T4 `node::handle_promote_stamp` handler. The
  DB thread resolves the local role (N-5); the promotion is idempotent (T1) and
  guarded again by `is_promoted` (N-9: no store row until first address).
- **FR-3 overrides.** `handle_set_stamp_scale`/`handle_set_stamp_rotation`
  record a sparse override in-state (live gesture) and, once the promoted node id
  is known, persist it to a node property (`STAMP_SCALE_KEY`/`STAMP_ROTATION_KEY`
  /`STAMP_ARC_KEY` — a mirror-enum/JSON contract the materializer reads back;
  fe-ui must not depend on fe-terrain). Position is NEVER stored — it stays
  path-derived (no free translate). Arc-length "slide along path" (Q-1) is homed
  in `path.rs::handle_slide_stamp` (curve domain) and clamps to `[0, total]` of
  the edited track's points (N-3, meters — no `world_scale`, N-1).
- **FR-5 reflow.** `reflow_after_delete(track, deleted_index)` consumes T1's
  `LifecycleEvent::PathReflow`: it drops the deleted stamp's override and shifts
  same-track indices `> deleted_index` down by one so every survivor keeps its
  override under its new index; the selection/promotion markers shift too.
  Positions re-derive by re-running the sampler for the new count.

**Cross-boundary wiring (LANDED, Wave-1 integration pass):**

- **Promotion round-trip.** The `SelectStamp` dispatch arm (`mod.rs`) drains
  `take_pending_promotion()` and queues `UiAction::PromoteStamp`;
  `node::handle_promote_stamp` sends `PromoteInstance{petal_id, path_id,
  instance_index, auth: CallerAuth::Local}` with `petal_id` =
  `NavigationManager.active_petal_id` (missing petal → warn, never silent,
  N-8; already-promoted → no send, N-9). The echo lands in
  `verse_manager::db_results` (`DbResult::NodePromoted` arm) →
  `asset::handle_node_promoted`: `mark_promoted` then
  `flush_buffered_overrides` — each `Some` field of the buffered
  `StampOverride` becomes one `SetNodeProperty` under its matching
  `stamp.override.*` key (this is what wires `STAMP_ARC_KEY`).
- **Hydration on reload.** `asset::hydrate_promoted_stamp` runs in the
  `NodePropertiesLoaded` handler (`db_results/properties.rs`): a bag with
  `node_kind == "stamp"` re-binds `(path_id, instance_index) → node_id` and
  loads persisted `stamp.override.*` values into the overrides map (overwrite
  — the DB is the durable truth), so overrides survive restart. The identity
  keys are fe-database's promotion write contract.
- **Reflow dispatch.** `verse_manager::lifecycle_events` consumes
  `LifecycleEvent::PathReflow` and calls `reflow_after_delete` (see
  `verse_manager/AGENTS.md` §path-asset-materialization).
- **Materializer read view.** `overrides_for_track` (index-sorted) feeds
  override application + the applied-gate overrides fingerprint in
  `materialize_path_assets`. `selected()` is consumed by the right-click
  context menu (stamp header "— selected" marker; `dialogs/context_menu.rs`)
  — its dead-code marker is gone. Right-click on a stamp routes
  `UiAction::SelectStamp` through the same handler as left-select
  (idempotent, lazy promotion preserved).

## §sculpt — commit line + earthwork endpoint rows (sculpt_earthwork_regions T3 integration, 2026-07-26)

All in `terrain_proposal.rs`; region JSON mirrors `fe_terrain::sculpt::
EarthworkRegion` by contract (fe-ui must NOT depend on fe-terrain).

- **Commit line (was the missing seam).** The viewport Brush snapshots the
  active petal and converts sanitized meter controls through `world_scale` on
  press, then queues one bounded `SculptBrushStroke` on release. Its handler
  converts the sampled centerline into one finite swept-corridor polygon and
  performs one terrain-doc write. The whole-stroke centerline is reduced to at
  most 256 points by deterministic, interval-balanced maximum-deviation
  splitting before intersection validation and corridor building, preserving
  equal-amplitude turns and localized loops without an unbounded reducer.
  A loading map gets a distinct retry toast;
  a missing/disabled map keeps the assignment toast and persists nothing.
- **Endpoint rows (D-A8/N-10).** Each committed stroke/shape
  (`handle_brush_stroke` / `handle_shape_region`, gated on the
  `SetPetalTerrain` queue succeeding) also
  sends `CreateNode` at the footprint's vertex-mean centroid (petal-local
  world units) with `correlation_id = "earthwork:{region_id}"`.
  `db_results/nodes.rs` consumes the echo (pen-tool consume idiom: echoed-id
  match only): binds `EarthworkNodeMap` (region_id→node_id), consumes the
  stashed material, and writes the contract bag — literal fe-query keys
  `node_kind="earthwork_region"`, `material`, `region_id`, and zeroed
  `cut_volume_m3`/`fill_volume_m3`.
- **Volumes.** fe-terrain's bake publishes `fe_renderer::terrain_overlay::
  EarthworkVolumeReport`; `persist_earthwork_volumes` (registered in
  `plugin.rs`, message add is idempotent) maps region→node and sends
  `SetNodeProperty` ONLY when `(cut, fill)` differs from the last persisted
  pair (`EarthworkNodeMap` cache; bake re-fires per revision — the DB is not
  spammed). Unknown region → debug (node not created/hydrated yet; next
  revision re-fires). Hydration seeds the gate from the persisted values.
- **Delete.** `handle_delete_region` drops the record (Q-2 revert) AND
  tombstones the node (`TombstoneNode{auth: CallerAuth::Local}`, N-4/N-5)
  when the map knows it — the endpoint contract stays honest.
- **Hydration.** `hydrate_earthwork_region` runs in the `NodePropertiesLoaded`
  handler (mirrors `hydrate_promoted_stamp`): `node_kind=="earthwork_region"`
  re-binds region_id→node_id. Region ids are minted collision-safe against a
  reloaded doc (`mint_unused_region_id` — the counter restarts per session).
- **Brush cursor.** `sculpt_cursor::draw_sculpt_brush_ring` (registered in
  `plugin.rs`, PostSelection after the cursor system): immediate-mode `Gizmos`
  linestrip of `fe_renderer::terrain_overlay::brush_overlay_positions` at the
  viewport cursor, gated by the right sidebar's own `active_section ==
  TerrainTools`; missing height field → cursor-plane fallback, no warn spam.

## §dimension-callout — promote a Measure reading to an addressable node (HS-P6-CALLOUT, decision D-11 ratified)

All in `dimension_callout.rs`; the template is §sculpt's earthwork endpoint
row above — a committed measurement becomes an addressable node the same way
a committed brush stroke becomes an earthwork region node.

- **Promote (pure).** `callout_from_measure(&MeasureState) -> Option<
  CalloutGeometry>` accepts only a COMPLETE measurement: Tape with exactly 2
  anchors, or an Area that has been `Close`d with ≥3. Anything else (an
  in-progress Tape, an open Area) is `None` — the Options panel's "Save as
  callout" button is enabled exactly when this is `Some`. `callout_properties`
  turns the result into the property bag (`node_kind`, `dimension.mode`
  `"tape"`/`"area"`, `dimension.anchors` as `[f64;3]` petal-local meters,
  `dimension.closed`).
- **Commit line.** The Options panel computes `geometry`/`name` from the
  SAME same-frame `MeasureState` it is already displaying and queues
  `UiAction::SaveMeasureAsCallout` — nothing is re-derived from live state in
  `process_ui_actions`, so a later viewport click can't retroactively change
  what gets persisted. The handler (`save_measure_as_callout`) mints a
  session-local callout id (`c{n}`, no `uuid`/`rand` dep — mirrors
  `SculptToolState::next_region_id`), stashes the geometry, and sends
  `CreateNode` at the vertex-mean of the anchors (the tape midpoint = the
  area centroid formula, both are just "mean of the anchors") correlated
  `callout:{id}`.
- **Endpoint row.** `db_results/nodes.rs`'s `handle_node_created` calls
  `bind_created_callout` beside the earthwork bind (pen-tool consume idiom:
  echoed-id match only). It consumes the stashed geometry, records
  node_id→(petal_id, geometry) in `DimensionCalloutMap`, and writes the bag.
- **Delete.** `handle_node_deleted` calls `DimensionCalloutMap::forget` —
  no tombstone wiring yet (A12: non-atomic create is accepted, the plain node
  the crash leaves behind is deletable from the sidebar like any other node).
- **Hydration.** `hydrate_dimension_callout` runs in the
  `NodePropertiesLoaded` handler (mirrors `hydrate_earthwork_region`): a
  strictest-reader parse — `node_kind=="dimension_callout"`, a known mode, an
  anchor count that fits the mode, ≤256 anchors, all-finite `[f64;3]`
  numbers. Anything else is ignored (CO-1: a malformed peer-authored bag
  renders nothing rather than crashing the draw loop).
- **Visibility + drawing.** `visible_callouts(&DimensionCalloutMap, &nav,
  &vis) -> Vec<&CalloutGeometry>` filters to the active petal AND
  `crate::visibility::effective_visibility` (the one picker rule, never
  re-derived). `node_manager::draw_dimension_callouts` (registered
  unconditionally, outside the selection chain) draws the surviving set as
  gizmo line segments every frame. The in-world readout LABEL (A12: would be
  `readout(&MeasureState::from(geometry), measure_world_scale(..))`,
  recomputed per frame, never stored) is deferred — painting world-space text
  needs `bevy_egui`'s `EguiPrimaryContextPass` wiring
  (`viewport_labels.rs`'s pattern), which lives in `plugin.rs` and is outside
  this slice's file ownership; `callout_display_name` already computes the
  equivalent string for the node's display `name` at save time.
