# morning-run-gpx

Demo `gpx_collection` hexon: one 30-point run along the Lake Zurich shore
(`terrain/tracks/morning_run.gpx`) with elevations and 30-second timestamps,
plus start/finish waypoints.

- Packed via `HexonArchive::export`; the GPX bytes are preserved verbatim in
  `terrain/tracks/` for lossless round-trip.
- `terrain/config.json` (a `fe_terrain::TerrainConfig`) IS packed into the
  zip and carries the petal terrain binding (origin at the track start,
  hybrid tile mode).
- The builder adds one `gpx_track` entry to `entries.json` with the BLAKE3
  hash of the GPX bytes.

Build: `cargo run -p fe-hexon --example build_sample_hexons`
