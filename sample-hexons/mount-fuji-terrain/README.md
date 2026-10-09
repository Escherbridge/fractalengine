# mount-fuji-terrain

Real-world `terrain_tileset` hexon for Mount Fuji, Japan — the second of two
non-US regions shipped for F15/A28 (DEC-C8 ratified 2026-10-09: ship 2,
acceptance needs ≥1).

Produced by [`gis-tile-etl`](../../../gis-tile-etl), a separate standalone
repo (sibling of this workspace), not by the `fe-hexon` sample builder — so
unlike the other `sample-hexons/*` entries, there is no JSON source
definition here, only this provenance record.

## Provenance

- **Region**: "Japan — Mount Fuji"
- **Bounds** (WGS84 `[min_lat, min_lon, max_lat, max_lon]`): `[35.25, 138.6, 35.5, 138.9]`
- **Zoom**: 8–10
- **Elevation source**: AWS Open Data Terrain Tiles (Mapzen/Nextzen
  "terrarium" encoding), `s3.amazonaws.com/elevation-tiles-prod` — open,
  global, keyless. Derived from USGS 3DEP, SRTM, GMTED2010, ETOPO1 and
  others.
- **Satellite source**: Esri World Imagery,
  `server.arcgisonline.com/ArcGIS/rest/services/World_Imagery` — global,
  keyless, but **NOT public domain**: attribution required per
  [Esri's terms of use](https://www.esri.com/en-us/legal/terms/full-master-agreement).
  © Esri and its data providers (Maxar, Earthstar Geographics, USDA FSA,
  USGS, AeroGRID, IGN, GIS User Community).
- **License embedded in the archive**: `license_type = "attribution"`, full
  dual-source attribution string in `configs/intl-regions.toml`
  (`[sink] attribution`).
- **Measured output** (2026-10-09 run, zero 404s/failures/retries from
  either endpoint): 8 elevation + 8 satellite tiles, archive size
  840,808 bytes (~0.8 MB).
- **Produced by**:
  ```
  cd gis-tile-etl
  cargo run --release -- run configs/intl-regions.toml --region "Mount Fuji"
  ```
- **Publisher DID**: `did:key:z6MkGisTileEtl` (illustrative, unsigned —
  manifest signing is a v2 TODO in `gis-tile-etl`).

## Where the `.hexon` lives

Unlike the other `sample-hexons/*` samples (which build into this repo's
`sample-hexons/dist/`, gitignored, via `cargo run -p fe-hexon --example
build_sample_hexons`), this archive is built by the separate `gis-tile-etl`
repo and lands in **its own** gitignored output directory:

```
../../../gis-tile-etl/dist/japan-mount-fuji.hexon
```

To rebuild it (resumable — cached tiles are reused, nothing re-fetched on
a clean re-run):

```
cd gis-tile-etl
cargo run --release -- run configs/intl-regions.toml --region "Mount Fuji"
cargo run --release -- verify dist/japan-mount-fuji.hexon
```

## Installing into the engine

```
cargo run -p fe-terrain --example install_sample_hexons -- \
  ../gis-tile-etl/dist/japan-mount-fuji.hexon
```

`HexonStore::install_tileset` registers it under `hexon_id =
"tileset-japan-mount-fuji"`. `native_scale` / `ground_sample_distance_m` are
intentionally left unset by the ETL (mirrors
`fe-terrain/src/tiles/builder.rs`); `HexonStore::load_tileset` backfills
them on load via `backfill_scale_fields` — no ETL-side change needed for
scale-bar support.
