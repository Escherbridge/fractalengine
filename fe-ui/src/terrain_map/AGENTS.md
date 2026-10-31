# fe-ui/src/terrain_map — petal map state + hexon registry queue

- `mod.rs` — `PetalMapState` (which tileset(s) the active petal uses + its
  `world_scale`), `HexonOp`/`PendingHexonOps` (registry ops queued for the main
  binary, since `fe-ui` has no `TilesetRegistry` access itself),
  `load_petal_terrain_on_nav_change` (requests terrain config on petal switch,
  resets scale to 1.0 pending load, also resets the proposal mirror via reset_for_petal_switch),
  `tileset_to_terrain_json` (builds the `fe-terrain` `TerrainConfig`-shaped JSON blob,
  now including `world_scale`), and `sync_camera_scale_from_petal_map` (mirrors
  `PetalMapState.world_scale` into fe-renderer's `CameraScaleSettings` so the
  camera adapts live and on restart; no-op when the renderer resource is absent).
  `PetalMapState.terrain_json: Option<serde_json::Value>` (added for the GIS
  Layer Manager, see root `AGENTS.md` §gis-query-ui) holds the raw,
  last-loaded terrain doc verbatim — not just the derived `tileset_ids`/
  `world_scale` fields — so `crate::gis`/`actions::gis` can mutate a single
  layer's `visible`/`opacity` and round-trip via `SetPetalTerrain` without
  clobbering unrelated fields. Populated in `db_results.rs` on
  `DbResult::PetalTerrainLoaded`, reset to `None` on petal switch in
  `load_petal_terrain_on_nav_change`.

### Petal-map truth (M-03)

`PetalMapState.pending_set: Option<PendingMapSet>` tracks one in-flight,
GUI-correlated `SetPetalTerrain` for the active petal. `actions::hexon::
set_petal_map`/`set_petal_map_scale` mint a fresh id via
`actions::query::next_ui_query_id("petalmap")`, snapshot the pre-send
`tileset_ids`/`world_scale` into `pending_set` right alongside their
existing optimistic update, and send `correlation_id: Some(id)` instead of
`None` — a GUI-originated denial now comes back as the typed
`DbResult::PetalTerrainSetFailed` (the DB handler already branches on
`Some`/`None`; this just opts the GUI into the `Some` arm) rather than the
generic, un-routable `DbResult::Error`.

`verse_manager/db_results/terrain.rs::handle_petal_terrain_set_failed`
(called on that result) checks the echoed id against `pending_set`: a match
rolls `tileset_ids`/`world_scale` back to the captured snapshot, clears
`pending_set`, and raises `UiManager::show_error` with
`notice::humanize_db_error`; a non-matching id (a different in-flight set,
or a stale echo) is ignored outright — it must never roll back a change it
didn't cause. `handle_petal_terrain_loaded`'s new `correlation_id: Option<&str>`
parameter clears a *matching* `pending_set` on the confirming echo, checked
before the active-petal gate so the row can't get stuck "Applying…" forever
just because the user navigated away mid-write.

`dialogs/hexon_manager.rs`'s Installed tab renders one card per map (M-01;
a 7-column `Grid` doesn't fit a 440 px side panel) and shows "Applying…" in
place of "Active map ✓" while `pending_set.is_some()` for that petal. Set/
Clear/Remove are `role_gated` (X-02) and Remove is a `confirm_button`
two-step (M-04) — both from `panels::widgets` (owned by the sibling W1-S4
slice; see that slice's AGENTS.md entry).

### Terrain scale controls

The Hexon Manager's Installed tab (`dialogs/hexon_manager.rs::render_scale_controls`)
shows scale presets (1:1 / 1:10 / 1:100 / 1:1000) + a log slider **when the
active petal has an assigned map**. The slider writes `PetalMapState.world_scale`
directly for a live camera preview (mirrored by `sync_camera_scale_from_petal_map`)
and emits `UiAction::PetalSetMapScale` on release / preset click, which rebuilds
the petal terrain JSON with the new scale and persists via `SetPetalTerrain`. The
round-trip (`PetalTerrainLoaded` → bridge → `apply_terrain_assignments`) respawns
chunks at the new scale — no restart. **fe-ui never depends on fe-terrain**: the
JSON is built with `serde_json` and the scale is a plain `f64` field (boundary
rule). Persistence across restarts comes free — `world_scale` lives in the stored
petal terrain JSON and is parsed back in `db_results` on `PetalTerrainLoaded`.
- `dto.rs` — Hexon Manager dialog DTOs: `InstalledTilesetDto`,
  `AvailableTilesetDto`, `DownloadStatus`/`DownloadProgress`,
  `StorageInfoDto`, `HexonManagerTab`.
- `manifest.rs` — `PetalManifest`/`ManifestHexonEntry` (the petal hexon
  manifest schema, serde round-tripped through `petal.hexon_manifest`).
- `events.rs` — `drain_tileset_events`, folds `fe_sync::SyncEvent` tileset
  advertisement/download-progress events into the Hexon Manager dialog
  state.

`InstalledTilesetDto`, `PendingHexonOps`, `HexonOp`, `StorageInfoDto`, and
`PetalMapState` are re-exported at `fe_ui::plugin::*` for `fractalengine`
compat — see root `AGENTS.md` §compat.
