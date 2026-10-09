---
type: Design Note
track: mission_prodready_continuation_20261008
title: "M6 (F15/F16) — gis-tile-etl region map (recon 2026-10-09)"
timestamp: 2026-10-09T07:10:00Z
---

# M6 — GIS hexon examples: implementation map

Recon output (sonnet explore, 2026-10-09) for F15/A28 + F16/A29. Repo:
`C:\Users\atooz\Programming\fractalengine-workspace\gis-tile-etl`.

## Load-bearing facts

- **New regions are config-only.** Any XYZ endpoint becomes a source via a
  `[[sources]]` TOML block (`gis-tile-etl/AGENTS.md:1-7`); `{z}{x}{y}` tokens
  substitute by name so `{z}/{y}/{x}` orderings work (`src/source.rs:22-27`).
  One shipped config: `configs/us-regions.toml` (2 sources × 6 US regions).
- **Elevation is already global + keyless**: `aws-terrarium`
  (`s3.amazonaws.com/elevation-tiles-prod/terrarium/...`, Terrarium encoding).
  Only a global **imagery** source is missing (USGS imagery is US-only).
- **No API-key mechanism exists** (no auth-header field on `SourceConfig`,
  no env lookup) — candidate sources must be keyless; OpenTopography/NASADEM
  would need code. Stick to keyless XYZ.
- CLI: `plan` (no network) / `run <config> [--region --max-zoom] [--dry-run]`
  (resumable cache, per-source rate limiter, 404=Missing never retried) /
  `verify dist/<slug>.hexon`. CI-scale: ~1-2° bbox at z6-8 = seconds.
- Provenance convention: license attribution free-text + manifest description
  (no structured per-source schema field in fe-format) — follow it, don't
  invent schema.
- **F16 chain already works end-to-end**: `cargo run -p fe-terrain --example
  install_sample_hexons -- <file-or-dir>` → `HexonStore::install_tileset`
  (validates, persists, loads); relay tile plane at
  `GET /api/v1/tiles/{elevation|satellite}/{tileset_id}/{z}/{x}/{y}.*`
  (petal-bound authz, `fe-api/src/terrain.rs:325-418`). Scale-bar fields are
  backfilled on install for archives that lack them
  (`fe-terrain/src/tiles/store.rs:50-61`) — A29's scale-bar acceptance needs
  no ETL-side changes.
- `sample-hexons/` convention exists in the app repo; mirror
  `sample-hexons/alpine-demo-terrain/README.md` for provenance.

## DEC-C8 (2026-10-09) — source + regions

- New imagery source: **ESRI World Imagery**
  (`https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}`)
  — keyless, global, same ArcGIS `{z}/{y}/{x}` pattern already unit-tested.
  ToS is NOT public domain → `license_type = "attribution"` with an honest
  attribution string (unlike USGS's `free`).
- Regions (CI-scale bboxes, final bounds implementer's choice): **Zurich/
  Alps (Switzerland)** — echoes the vetted alpine-demo anchor 47.3769,8.5417 —
  and **Mount Fuji (Japan)**. Both non-US (A28 needs ≥1; we ship 2).
  Larger "full" bounds documented as operator-run commands, not executed.
- If network is unavailable at execution time: produce configs + documented
  smoke commands, mark A28/A29 pending-with-reason (mission skill rule).
