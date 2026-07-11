# fe-ui/src/terrain_map — petal map state + hexon registry queue

- `mod.rs` — `PetalMapState` (which tileset(s) the active petal uses),
  `HexonOp`/`PendingHexonOps` (registry ops queued for the main binary, since
  `fe-ui` has no `TilesetRegistry` access itself), `load_petal_terrain_on_nav_change`
  (requests terrain config on petal switch), `tileset_to_terrain_json`
  (builds the `fe-terrain` `TerrainConfig`-shaped JSON blob).
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
