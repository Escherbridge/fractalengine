# fe-ui/src/terrain_map — petal map state + hexon registry queue

- `mod.rs` — `PetalMapState` (which tileset(s) the active petal uses + its
  `world_scale`), `HexonOp`/`PendingHexonOps` (registry ops queued for the main
  binary, since `fe-ui` has no `TilesetRegistry` access itself),
  `load_petal_terrain_on_nav_change` (requests terrain config on petal switch,
  resets scale to 1.0 pending load), `tileset_to_terrain_json` (builds the
  `fe-terrain` `TerrainConfig`-shaped JSON blob, now including `world_scale`),
  and `sync_camera_scale_from_petal_map` (mirrors `PetalMapState.world_scale`
  into fe-renderer's `CameraScaleSettings` so the camera adapts live and on
  restart; no-op when the renderer resource is absent).

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
