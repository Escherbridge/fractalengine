# alpine-demo-terrain

Demo `terrain` hexon: an offline tileset around Zurich (47.3769, 8.5417).

- 2x2 tile blocks at zooms 10 / 11 / 12 (12 elevation + 12 satellite tiles),
  anchored at the tile containing Zurich: z10 (536, 358), z11 (1072, 717),
  z12 (2145, 1434).
- Tiles are 64x64 and generated in memory by the builder — no binaries
  committed. Elevation uses Mapbox Terrain-RGB encoding of a gentle slope
  off a ~400 m base; satellite tiles are flat-color JPEGs.
- `tileset.json` holds the `TilesetMeta` fields; the builder overwrites the
  tile counts with actual generated counts.
- `terrain/config.json` is the petal-side install config (per-petal map
  selection): it is NOT packed into the zip by `export_tileset` — see
  `sample-hexons/README.md` design notes.

Build: `cargo run -p fe-hexon --example build_sample_hexons`
Consume: `fe_terrain::tiles::HexonTileSource::from_archive(bytes)`
