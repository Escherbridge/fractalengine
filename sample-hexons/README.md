# sample-hexons

Sample/demo `.hexon` packages for terrain UX work and per-petal map selection.
Each subdirectory is the **source definition** of one sample package (JSON
manifests per `docs/hexon-format-spec.md`); the built `.hexon` zips land in
`sample-hexons/dist/` (gitignored — never commit built artifacts or binary
tiles).

## Samples

| Directory | `hexon_type` | Contents |
|---|---|---|
| `alpine-demo-terrain/` | `terrain` | Tiny offline tileset around Zurich (47.3769, 8.5417): 2x2 tile blocks at zooms 10–12, 64x64 programmatically generated tiles — flat-color JPEG "satellite" + Terrain-RGB PNG elevation encoding a gentle slope. |
| `morning-run-gpx/` | `gpx_collection` | One ~30-point GPX run along the Lake Zurich shore (`terrain/tracks/morning_run.gpx`) with elevations + timestamps, plus a petal terrain config. |
| `lakeside-scene/` | `scene` | 4 scene nodes (`entities/nodes.json`): two GPX waypoints, one node with a `hexon_ref` pointing at `alpine-demo-terrain`, one info node; `schema.json` carries the field defs used. |

## Building the .hexon zips

```
cargo run -p fe-hexon --example build_sample_hexons
```

This reads the source dirs here, generates the alpine tile PNGs/JPEGs in
memory (nothing binary is committed), packs each sample via the `fe-format`
archive APIs, and writes `sample-hexons/dist/<name>.hexon` with a summary.

Packing APIs used per sample:

- `alpine-demo-terrain` → `HexonArchive::export_tileset` (manifest +
  `terrain/tileset_meta.json` + `terrain/tiles/{z}/{x}/{y}.png` +
  `terrain/satellite/{z}/{x}/{y}.jpg`). Loadable offline through
  `fe_terrain::tiles::HexonTileSource::from_archive`.
- `morning-run-gpx` → `HexonArchive::export` (manifest + one `gpx_track`
  entry + `terrain/config.json` + `terrain/tracks/morning_run.gpx`,
  preserved byte-for-byte for lossless round-trip).
- `lakeside-scene` → `HexonArchive::export_scene` (manifest +
  `entities/nodes.json` + `entities/field_defs.json` + `schema.json`).

## Design notes (rationale lives here, not in inline comments)

- **Source JSON shapes.** `manifest.json` files deserialize directly into
  `fe_format::manifest::HexonManifest`; `alpine-demo-terrain/tileset.json`
  into `TilesetMeta`; `lakeside-scene/entities/nodes.json` into
  `Vec<fe_format::ExportNode>`; `schema.json` into `SchemaDefinition`.
  The builder fails loudly if a source file drifts from the spec types.
- **Tile generation.** Elevation tiles use Mapbox Terrain-RGB encoding
  (`elevation = -10000 + (R*65536 + G*256 + B) * 0.1`) with a gentle
  per-pixel slope off a ~400 m base; satellite tiles are flat-color 64x64
  JPEGs shaded per tile coordinate. Tile counts in `tileset.json` are
  authored values; the builder overwrites `tile_count` /
  `satellite_tile_count` with the actual generated counts.
- **`terrain/config.json` for the tileset sample.** `export_tileset`
  intentionally packs only tileset data (no `terrain/config.json` in the
  zip). The alpine `terrain/config.json` is the *petal-side install
  config* demonstrating per-petal map selection: it points
  `tileset_hexon_uris` at the built archive and sets
  `tile_source_mode: "offline"`. For the GPX sample the config IS packed
  (via `export`), since `gpx_collection` hexons carry their terrain
  binding.
- **Shared build logic.** The reusable builders live in
  `fe-hexon/tests/support/sample_hexons.rs`, included via `#[path]` by both
  the cargo example (`fe-hexon/examples/build_sample_hexons.rs`) and the
  integration test (`fe-hexon/tests/sample_hexons_test.rs`). They are not
  part of the fe-hexon lib because they need `fe-format`, `fe-terrain`, and
  `image`, which are dev-dependencies only — keeping the production
  dependency graph untouched.
- **Publisher DIDs** (`did:key:z6MkSample...`) are illustrative sample
  identities; the archives are unsigned (`signature` omitted), which the
  format allows.
