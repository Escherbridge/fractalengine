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
  exercises.
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
  `AGENTS.md` §asset-download.
- `gpx.rs` — GPX track import request; only pushes onto
  `crate::gpx_ops::PendingGpxOps`, mirroring `asset.rs` exactly. See root
  `AGENTS.md` §gpx-import.

`process_ui_actions` stays a thin per-variant dispatcher; keep new actions'
actual logic in the matching domain file rather than growing the match arms
in `mod.rs`.
