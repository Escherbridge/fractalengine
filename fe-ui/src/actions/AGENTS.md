# fe-ui/src/actions — UiAction queue, split by domain

- `mod.rs` — `UiAction` enum, `UiManager` resource (queue + portal + active
  dialog + toast), `process_ui_actions` dispatcher.
- `portal.rs` — portal open/close/go-back + `SaveUrl`. Pure decision
  functions (`compute_open_portal`, `should_auto_close`, `compute_save_url`)
  are unit-tested directly; see `fe-ui/src/AGENTS.md` §portal for the full
  save/open chain this drives.
- `node_props.rs` — custom node property load/set/delete (`DbCommand`
  fire-and-forget with a `warn!` on channel-closed). Also owns the reserved
  `gis.annotation.{title,body,color}` key constants + a pure
  `annotation_fields_from_properties` extractor used by the inspector's
  Annotation card — see root `AGENTS.md` §gis-query-ui.
- `hexon.rs` — hexon/tileset install/remove/seed/download + petal-map
  set + petal-manifest save/open.
- `query.rs` — SurrealQL query submission.
- `gis.rs` — GIS query panel + layer manager I/O: submits
  `DbCommand::RawQuery` built by `crate::gis::query`'s pure builders, and
  round-trips terrain layer edits via `SetPetalTerrain` (mirrors
  `hexon.rs::set_petal_map_scale`'s mutate-then-persist idiom). State lives
  in `crate::gis::GisPanelState` (top-level module, not under `actions/`) —
  see root `AGENTS.md` §gis-query-ui for why the state/pure-logic and the
  I/O are split this way (mirrors `terrain_map`'s `PetalMapState` vs
  `hexon.rs` split).
- `asset.rs` — node asset download request; only pushes onto
  `crate::asset_ops::PendingAssetOps`, mirroring the `hexon.rs` pattern of
  queuing for the main binary rather than resolving in fe-ui. See root
  `AGENTS.md` §asset-download.

`process_ui_actions` stays a thin per-variant dispatcher; keep new actions'
actual logic in the matching domain file rather than growing the match arms
in `mod.rs`.
