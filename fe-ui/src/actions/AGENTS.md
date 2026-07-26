# fe-ui/src/actions — UiAction queue, split by domain

- `mod.rs` — `UiAction` enum, `UiManager` resource (queue + portal + active
  dialog + toast), `process_ui_actions` dispatcher.
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
  `nav.active_petal_id`; toast when no petal is active).
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

**Cross-boundary wiring (open, not owned by T2):** recording promoted node ids
into `StampInteractionState` on `DbResult::NodePromoted`, and dispatching
`reflow_after_delete` from a `PathReflow` observer, land in the db-results /
plugin systems (T6/T1 seam) — the handlers + state machine here are the leaf
logic those systems call.
