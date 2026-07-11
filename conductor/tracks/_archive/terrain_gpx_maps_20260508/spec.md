# Track: Terrain & GPX — 3D Map Tiles, GPX Tracks, Elevation Mesh, Petal-Bound Terrain

**Created:** 2026-05-08
**Status:** Draft
**Priority:** P1
**Depends on:** Open Crate Format (Phase 6.5), Entity Data Layer Phase 6.1 (GIS validation), Viewport Foundation, Scene Graph Bridge
**Blocks:** IoT Path Tracking, Crate Registry (terrain crate type)

---

## Problem Statement

FractalEngine nodes currently exist in an abstract 3D space with no geographic context. For digital twin use cases — outdoor asset monitoring, IoT device tracking, GPX route visualization, and geographic scene composition — users need:

1. Real-world terrain rendered as 3D mesh with satellite imagery
2. GPX track import/export with 3D polyline visualization
3. Interactive waypoint placement on terrain
4. The ability to assign terrain configurations per-petal (each petal is its own "world")
5. IoT devices that follow GPX paths with real-time progress tracking
6. All terrain/GPX data persisted in the .hexon format for export/import

---

## Goals

1. **Single crate** (`fe-terrain`) handles GPX parsing, terrain tile fetching, elevation mesh generation, and map layer compositing
2. Terrain is **petal-scoped** — each petal can have its own terrain config (tile source, origin coordinates, zoom range)
3. GPX tracks import as node hierarchies (track → segments → waypoints) with full properties
4. **Round-trip fidelity** — GPX/terrain data survives .hexon export/import
5. **Overlay layers** — toggleable GPX tracks, GeoJSON boundaries, heatmaps, satellite imagery
6. **IoT path tracking** — external devices report position; system shows progress along a GPX route
7. **Offline-first** — terrain tiles cached to disk; works without network after first fetch
8. **Headless support** — relay can serve terrain data without rendering (feature-gated)

## Non-Goals (this track)

- Full GIS/cartography system (projections beyond WGS84/Mercator/local)
- Real-time weather overlays
- Indoor mapping / floor plans
- Vehicle routing / pathfinding algorithms
- Terrain editing / sculpting (deferred)

---

## Architecture Overview

### New Crate: `fe-terrain`

Workspace member depending on: `fe-runtime`, `fe-query` (GIS validation), `gpx` (georust), `image`, `reqwest`, `flat_projection`, `geojson`, `bevy` (mesh types, feature-gated behind "render").

**Modules:**

```
fe-terrain/
├── Cargo.toml
└── src/
    ├── lib.rs              # Plugin + public API
    ├── gpx/
    │   ├── mod.rs          # Re-exports
    │   ├── parser.rs       # GPX 1.0/1.1 parsing (wraps `gpx` crate)
    │   ├── stats.rs        # Track statistics (distance, elevation gain, speed)
    │   ├── convert.rs      # GPX → DbCommand pipeline (nodes + properties)
    │   └── export.rs       # Scene nodes → GPX XML export
    ├── tiles/
    │   ├── mod.rs
    │   ├── source.rs       # TileSource trait + XYZ/TMS implementations
    │   ├── elevation.rs    # Terrain-RGB / Terrarium decoding
    │   ├── cache.rs        # Disk LRU tile cache
    │   └── lod.rs          # Camera-driven LOD tile selection
    ├── mesh/
    │   ├── mod.rs
    │   ├── terrain.rs      # Elevation grid → Bevy Mesh
    │   ├── track.rs        # GPX polyline → tube/ribbon Mesh
    │   └── marker.rs       # Waypoint billboard/3D marker
    ├── layers/
    │   ├── mod.rs
    │   ├── stack.rs        # LayerStack resource (ordered, toggleable)
    │   ├── style.rs        # Track color modes (elevation, speed, time gradient)
    │   └── geojson.rs      # GeoJSON → overlay mesh
    ├── projection.rs       # WGS84 ↔ scene-local conversion (flat_projection)
    ├── config.rs           # TerrainConfig (per-petal), PetalTerrainBinding
    ├── iot/
    │   ├── mod.rs
    │   ├── path_tracker.rs # Device position → route progress
    │   └── animation.rs    # Playback animation along track
    └── format.rs           # .hexon integration (terrain/GPX serialization)
```

### Petal-Terrain Binding

Each petal can optionally have a `TerrainConfig` stored in its `properties`:

```json
{
  "terrain": {
    "enabled": true,
    "origin_lat": 47.3769,
    "origin_lon": 8.5417,
    "origin_ele": 408.0,
    "tile_source": "https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png",
    "elevation_source": "mapbox_terrain_rgb",
    "elevation_api_key_env": "MAPBOX_ACCESS_TOKEN",
    "max_zoom": 16,
    "min_zoom": 8,
    "cache_dir": "./tile_cache",
    "layers": [
      {"id": "satellite", "type": "satellite", "visible": true, "opacity": 1.0},
      {"id": "gpx_morning_run", "type": "gpx_track", "node_id": "track_001", "visible": true, "color_mode": "elevation_gradient"}
    ]
  }
}
```

### GPX-to-Node Mapping

| GPX Element | FractalEngine Entity | Properties |
|-------------|---------------------|------------|
| `<trk>` | Node (parent) | `gpx_type: "track"`, `name`, `total_distance_m`, `elevation_gain_m` |
| `<trkseg>` | Node (child of track) | `gpx_type: "segment"`, `point_count` |
| `<trkpt>` | Node (child of segment) | `gpx_type: "trackpoint"`, `lat`, `lon`, `ele`, `time`, `hr`, `cad`, `power` |
| `<wpt>` | Node (standalone) | `gpx_type: "waypoint"`, `lat`, `lon`, `ele`, `name`, `desc`, `symbol` |
| `<rte>` | Node (parent) | `gpx_type: "route"`, `name` |

### IoT Path Tracking

An IoT device with `device_id` property can be assigned a `tracking_route_id` (node_id of a GPX track). The system:
1. Receives position updates via API (`PATCH /api/v1/nodes/:device_id/transform`)
2. Computes nearest point on route (snap-to-track)
3. Calculates `route_progress` (0.0 → 1.0) and `distance_remaining_m`
4. Emits `SceneChange::PropertyChanged` with updated tracking properties
5. Optional: generates alerts on deviation threshold

### .hexon Format Extension

```
.hexon ZIP (extended):
├── manifest.json           (existing)
├── entities/nodes.json     (existing — includes GPX nodes)
├── entities/field_defs.json
├── schema.json
├── assets/<hash>           (existing)
├── terrain/
│   ├── config.json         (TerrainConfig for the petal)
│   ├── tracks/             (original GPX files, preserved for lossless round-trip)
│   │   └── morning_run.gpx
│   └── overlays/           (GeoJSON overlay files)
│       └── boundaries.geojson
```

---

## Phases

### Phase 1: GPX Core (fe-terrain/src/gpx/)

- GPX 1.0/1.1 parsing via `gpx` crate
- Track statistics computation (Haversine distance, elevation gain/loss, speed)
- GPX → DbCommand conversion (creates node hierarchy)
- Scene nodes → GPX XML export (round-trip)
- API endpoints: `POST /api/v1/petals/:id/import/gpx`, `GET .../export/gpx`
- Projection module (WGS84 ↔ scene-local via `flat_projection`)
- Unit tests with embedded sample GPX

### Phase 2: Terrain Tiles & Elevation Mesh

- TileSource trait + XYZ/TMS implementations
- Terrain-RGB and Terrarium elevation decoding
- Disk LRU tile cache (configurable size, async fetch)
- Elevation grid → Bevy Mesh generation (feature-gated)
- Camera-driven LOD (zoom selection based on distance)
- Satellite imagery draping (StandardMaterial texture)
- TerrainPlugin for Bevy (spawns/despawns terrain chunks)

### Phase 3: Petal Binding & Layer Compositing

- TerrainConfig per-petal (stored in petal properties)
- LayerStack resource (ordered layers with visibility/opacity)
- GPX track rendering (3D polyline/tube mesh, color modes)
- GeoJSON overlay import + flat mesh draping
- Waypoint 3D markers (selectable via Selection System)
- API: `POST /api/v1/petals/:id/terrain/config` (set terrain config)
- .hexon format extension (terrain/ directory)

### Phase 4: IoT Path Tracking & Animation

- PathTracker system (snap device position to nearest route point)
- Route progress computation (cumulative distance ratio)
- Deviation detection + alerting (threshold-based)
- Track animation/playback (TrackAnimator component)
- Elevation profile API: `GET /api/v1/nodes/:track_id/elevation-profile`
- Heatmap overlay from point density

---

## Key Dependencies (Rust Crates)

| Crate | Version | Purpose |
|-------|---------|---------|
| `gpx` | 0.10 | GPX 1.0/1.1 parsing (georust) |
| `geojson` | 1.0 | GeoJSON FeatureCollection parsing |
| `flat_projection` | 0.4 | WGS84 → local Cartesian (pure Rust, ~500km) |
| `image` | 0.25 | PNG decode for terrain-RGB tiles |
| `reqwest` | 0.12 | Async tile fetching |
| `bevy` | 0.18 | Mesh generation (feature "render") |
| `blake3` | 1 | Content hash for cached tiles |

---

## Success Criteria

- [ ] Import a GPX file → nodes appear in 3D scene at correct geographic positions
- [ ] Terrain mesh renders under GPX track with correct elevation
- [ ] Toggle layers on/off via API/UI
- [ ] Export petal with terrain → .hexon ZIP includes terrain/ + GPX nodes
- [ ] Import .hexon with terrain → terrain renders correctly
- [ ] IoT device position updates show progress along GPX route
- [ ] Works offline after initial tile cache population
- [ ] Headless relay serves terrain data without Bevy rendering
