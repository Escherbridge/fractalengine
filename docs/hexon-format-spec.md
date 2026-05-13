# Hexon Format Specification

**Version:** 1.0.0
**Status:** Draft
**Extension:** `.hexon`
**MIME Type:** `application/x-hexon+zip`
**Implementations:** FractalEngine (Rust/Bevy), amp.SDK (Go), plan.3D (Unity)

---

## Overview

**Hexon** is a universal, engine-agnostic container format for spatial computing content. Named after the hexon protein — the modular self-assembling subunit that forms viral capsids — a `.hexon` file is a self-contained building block that composes into larger spatial structures.

The metaphor extends:
- **Hexagonal tessellation** — hexons tile space efficiently (hex grids, honeycomb packing)
- **Self-assembly** — hexons compose into scenes, worlds, and verses without central coordination
- **Modularity** — each hexon carries everything needed to be useful in isolation

A `.hexon` file is a ZIP archive containing structured JSON manifests and content-addressed binary blobs. It serves as the universal package for scenes, 3D models, materials, skyboxes, terrain, GPX tracks, sounds, and any spatial asset type.

---

## Design Principles

1. **One format, many types.** A `.hexon` can contain a full 3D scene, a single skybox, a PBR material set, a terrain tileset, or a GPX track collection. The `hexon_type` field determines interpretation.

2. **Hierarchical addressing.** Content is addressed via a 3-level hierarchy (Node → Attribute → Item) using 128-bit UIDs, compatible with amp.SDK's tag system.

3. **Immutability.** Node logs are append-only. State is derived by replaying the log. This enables conflict-free distributed merge.

4. **Content addressing.** All binary assets are keyed by BLAKE3 hash. Integrity is self-evident. Deduplication is automatic.

5. **Engine neutrality.** The format describes WHAT (spatial data, transforms, properties) not HOW (rendering pipeline, shader model).

6. **P2P native.** Hexons are signed, hashable, and designed for decentralized hosting and discovery.

---

## Addressing System

### Hexon URI

```
hexon://{publisher}/{hexon_id}[@{version}][#{entry_id}]
```

- `publisher`: DID or domain (e.g., `did:key:z6MkABC`, `plan-systems.org`)
- `hexon_id`: Package name, chars `[A-Za-z0-9_.-]`
- `version`: Optional SemVer (e.g., `1.2.0`)
- `entry_id`: Optional asset entry reference (fragment)

Examples:
```
hexon://did:key:z6MkABC/alpine-terrain@1.2.0
hexon://plan-systems.org/plan.app.ui@3.0.1
hexon://themushroom.farm/land#mountain_cabin
```

### Spatial Address (amp-compatible)

Content within hexons is addressed via a 3-level tag hierarchy, each level a 128-bit UID:

```
NodeID / AttrID / ItemID [/ EditID]
```

| Level | FractalEngine Mapping | amp.SDK Mapping | Description |
|-------|----------------------|-----------------|-------------|
| **NodeID** | Petal | Node/Channel | Spatial partition / organizational unit |
| **AttrID** | Node | Attribute | Entity within the partition |
| **ItemID** | Property/Asset | Item | Granular data element |
| **EditID** | HLC Timestamp | Edit | Version/mutation identifier (newest-first) |

UID generation methods:
- **Random**: Cryptographic random (128-bit)
- **Time-based**: Unix timestamp + nanosecond entropy (sortable)
- **Hash-derived**: BLAKE2s of literal string, XOR-folded to 128-bit (deterministic, order-independent)
- **From ULID**: Direct mapping from existing ULIDs (FractalEngine node_id/petal_id)

Canonical serialization: Base32 (52 chars) or hex with `0x` prefix. Tag literals are dot-separated and order-independent (commutative addition via hash).

---

## Archive Structure

```
.hexon ZIP:
├── manifest.json           # Hexon identity, version, type, signature
├── entries.json            # Asset entry catalog (typed, indexed)
├── schema.json             # Property type definitions
├── license.json            # License terms + access control
├── README.md               # Human-readable description (optional)
├── icon.png                # 256x256 icon (optional)
├── preview/                # Preview images (optional)
│   └── *.png
├── assets/                 # Content-addressed blobs (BLAKE3 hex)
│   └── {hash}
├── entities/               # Scene-type hexons
│   ├── nodes.json          # Node snapshots + operation logs
│   └── field_defs.json     # Property definitions
└── terrain/                # Terrain/GPX hexons
    ├── config.json         # Terrain configuration
    ├── tracks/             # GPX files (lossless)
    │   └── *.gpx
    └── overlays/           # GeoJSON overlays
        └── *.geojson
```

---

## manifest.json

```json
{
  "schema_version": "1.0.0",
  "hexon_id": "alpine-terrain",
  "hexon_type": "terrain",
  "publisher_did": "did:key:z6MkABC...",
  "publisher_name": "Alpine GIS Co.",
  "version": "1.2.0",
  "build_id": "260508-v1.2.0",
  "name": "Alpine Terrain Pack",
  "description": "High-resolution terrain tiles for the Swiss Alps",
  "tags": ["terrain", "alps", "switzerland", "elevation"],
  "created_at": "2026-05-08T12:00:00Z",
  "updated_at": "2026-05-08T14:30:00Z",
  "source_peer_did": "did:key:z6MkExporter...",
  "approx_size_bytes": 524288000,
  "min_engine_version": "0.18.0",
  "homepage_url": "https://alpine-gis.example.com",
  "dependencies": [],
  "platforms": [],
  "address": {
    "node_id": "0x...",
    "attr_id": "0x..."
  },
  "signature": "<ed25519 base64 signature>"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `schema_version` | string | yes | Format version (`"1.0.0"`) |
| `hexon_id` | string | yes | Machine-readable ID `[A-Za-z0-9_.-]` |
| `hexon_type` | string | yes | One of the Hexon Types |
| `publisher_did` | string | yes | DID of publisher |
| `publisher_name` | string | no | Human-readable publisher name |
| `version` | string | yes | SemVer version |
| `build_id` | string | no | Build identifier (`yyMMdd-v{version}`) |
| `name` | string | yes | Display name |
| `description` | string | no | Short description |
| `tags` | string[] | no | Discovery tags |
| `created_at` | string | yes | RFC 3339 creation timestamp |
| `updated_at` | string | yes | RFC 3339 last build timestamp |
| `source_peer_did` | string | no | DID of exporting peer |
| `approx_size_bytes` | u64 | no | Approximate total size |
| `min_engine_version` | string | no | Minimum runtime version |
| `homepage_url` | string | no | Publisher homepage |
| `dependencies` | HexonDep[] | no | Required hexons (URI + version constraint) |
| `platforms` | Platform[] | no | Platform targeting |
| `address` | Address | no | Hierarchical tag address (amp-compatible UID) |
| `signature` | string | no | Ed25519 base64 signature |

### Hexon Types

| `hexon_type` | Description | Required Sections |
|--------------|-------------|-------------------|
| `scene` | Full 3D scene with nodes, logs, transforms | `entities/`, optionally `terrain/` |
| `model` | 3D model collection (GLB, GLTF) | `entries.json`, `assets/` |
| `material` | PBR material sets | `entries.json`, `assets/` |
| `skybox` | Environment maps (HDR, EXR, cubemap) | `entries.json`, `assets/` |
| `terrain` | Terrain tilesets, elevation data, satellite imagery | `terrain/`, `entries.json`, `assets/` |
| `gpx_collection` | GPX tracks, routes, waypoints | `terrain/tracks/`, optionally `entities/` |
| `surface` | Ground surfaces / terrain colliders | `entries.json`, `assets/` |
| `sound` | Spatial audio (OGG, WAV) | `entries.json`, `assets/` |
| `visual_layer` | 2D overlays, sprites | `entries.json`, `assets/` |
| `theme` | UI themes, colors, fonts | `entries.json`, `assets/` |
| `bundle` | Mixed collection | `entries.json`, `assets/` |

### Platform Targeting (amp-compatible)

```json
{
  "platforms": [
    { "platform": "windows", "min_version": "10.0" },
    { "platform": "macos", "min_version": "13.0" },
    { "platform": "linux" },
    { "platform": "android", "min_version": "12.0" },
    { "platform": "ios", "min_version": "16.0" }
  ]
}
```

---

## entries.json — Asset Entry Catalog

```json
{
  "entries": [
    {
      "entry_id": "skybox_sunset",
      "kind": "skybox",
      "asset_hash": "b3a7f2e1d4c5...",
      "format": "hdr",
      "label": "Golden Hour Sunset",
      "tags": ["sunset", "warm", "outdoor"],
      "description": "360 HDR skybox",
      "is_placeable": false,
      "is_private": false,
      "auto_scale": false,
      "center": [0.0, 0.0, 0.0],
      "extents": [0.0, 0.0, 0.0],
      "preview_image": "preview/thumb_01.png",
      "address": { "attr_id": "0x...", "item_id": "0x..." },
      "metadata": { "resolution": "4096x2048" }
    },
    {
      "entry_id": "pbr_rock_granite",
      "kind": "material",
      "asset_hash": "",
      "format": "material_bundle",
      "label": "Granite Rock PBR",
      "sub_assets": {
        "albedo": "hash_albedo",
        "normal": "hash_normal",
        "roughness": "hash_roughness",
        "ao": "hash_ao",
        "metallic": "hash_metallic",
        "displacement": "hash_disp"
      },
      "metadata": { "resolution": "2048x2048", "tiling": true }
    },
    {
      "entry_id": "mountain_cabin",
      "kind": "model",
      "asset_hash": "abc123def456...",
      "format": "glb",
      "label": "Mountain Cabin",
      "is_placeable": true,
      "center": [0.0, 1.75, 0.0],
      "extents": [5.0, 3.5, 4.2]
    }
  ]
}
```

### Entry Kinds (amp AssetKind-compatible + extensions)

| `kind` | Formats | Description |
|--------|---------|-------------|
| `model` | glb, gltf | 3D geometry + embedded materials |
| `texture` | png, jpg, ktx2, basis | 2D image texture |
| `sprite` | png, svg | 2D sprite/icon |
| `material` | material_bundle | PBR set (via `sub_assets`) |
| `skybox` | hdr, exr, ktx2 | Environment map |
| `surface` | mesh, heightmap | Ground/terrain collider |
| `visual_layer` | png, svg, geojson | 2D overlay |
| `visual_scope` | json | Visibility/LOD scope |
| `sound` | ogg, wav, mp3 | Audio |
| `gpx_track` | gpx | GPX track/route |
| `geojson` | geojson | GeoJSON features |
| `terrain_tileset` | tiles | Pre-cached map tiles |
| `script` | wasm | Runtime script (future) |

### Entry Fields

| Field | Type | Description |
|-------|------|-------------|
| `entry_id` | string | Unique within hexon (path-safe) |
| `kind` | string | Entry Kind (see above) |
| `asset_hash` | string | BLAKE3 hex of primary blob |
| `format` | string | File format |
| `label` | string | Display name |
| `tags` | string[] | Discovery tags |
| `description` | string | Short description |
| `is_placeable` | bool | Can be placed in scene via drag & drop |
| `is_private` | bool | Hidden from browse UI |
| `auto_scale` | bool | Auto-scale to scene units on placement |
| `center` | [f32; 3] | Bounding box center |
| `extents` | [f32; 3] | Bounding box half-extents |
| `preview_image` | string | Path to preview within ZIP |
| `sub_assets` | map | Named sub-blobs (PBR channels, etc.) |
| `address` | Address | amp-compatible tag address for this entry |
| `metadata` | object | Kind-specific freeform metadata |

---

## entities/nodes.json — Scene Nodes

Present in `hexon_type: "scene"` (or scene+terrain hybrid):

```json
[
  {
    "node_id": "01J5A8B2C3D4E5F6G7H8J9K0M1",
    "petal_id": "01J5A8B2C3D4E5F6G7H8J9K0M2",
    "name": "Sensor Alpha",
    "position": [47.3769, 0.5, 8.5417],
    "rotation": [0.0, 0.0, 0.0, 1.0],
    "scale": [1.0, 1.0, 1.0],
    "has_asset": true,
    "asset_path": "models/sensor.glb",
    "properties": {
      "gpx_type": "waypoint",
      "tracking_route_id": "track_001",
      "hexon_ref": "hexon://did:key:z6Mk.../sensor-pack@1.0.0#sensor_alpha",
      "device_id": "iot-sensor-42"
    },
    "node_log": [
      {
        "hlc_timestamp": 1715155200000,
        "op": "created",
        "source_did": "did:key:z6Mktest",
        "payload": { "position": [47.3769, 0.5, 8.5417] },
        "row_version": 1
      }
    ]
  }
]
```

### Node Log Operations

| `op` | Description |
|------|-------------|
| `created` | Node created |
| `transform_update` | Position/rotation/scale changed |
| `property_set` | Custom property set or updated |
| `property_deleted` | Custom property removed |
| `renamed` | Node renamed |
| `asset_attached` | Content-addressed asset linked |
| `asset_detached` | Asset reference removed |
| `hexon_installed` | Hexon assets linked to this node |
| `custom(string)` | Application-defined operation |

### Hexon References

Nodes reference assets from installed hexons via the `hexon_ref` property:

```
hexon://{publisher}/{hexon_id}@{version}#{entry_id}
```

This enables lazy resolution — the runtime resolves from installed hexons or fetches from peers.

---

## terrain/ — Terrain & GPX Data

Present in `hexon_type: "terrain"`, `"gpx_collection"`, or scene hexons with terrain bindings.

### terrain/config.json

```json
{
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
    {"id": "track_001", "type": "gpx_track", "node_id": "node_abc", "color_mode": "elevation_gradient"}
  ]
}
```

### terrain/tracks/*.gpx

Original GPX files preserved verbatim for lossless round-trip.

### terrain/overlays/*.geojson

GeoJSON FeatureCollection overlays.

---

## schema.json — Property Definitions

```json
{
  "field_defs": [
    {
      "key": "gpx_type",
      "property_type": "string",
      "description": "GPX element type: track, segment, trackpoint, waypoint, route"
    },
    {
      "key": "hexon_ref",
      "property_type": "hexon_ref",
      "description": "URI reference to a hexon entry"
    }
  ]
}
```

### Property Type System

| Type | JSON Representation | Description |
|------|---------------------|-------------|
| `string` | `"text"` | UTF-8 string |
| `number` | `42` or `3.14` | IEEE 754 float/int |
| `bool` | `true`/`false` | Boolean |
| `datetime` | `"2026-05-08T12:00:00Z"` | RFC 3339 |
| `geometry_point` | `{"type":"Point","coordinates":[lon,lat]}` | GeoJSON point |
| `geometry_polygon` | `{"type":"Polygon","coordinates":[...]}` | GeoJSON polygon |
| `blob_ref` | `"blake3:abc123..."` | Content hash → `assets/{hash}` |
| `hexon_ref` | `"hexon://pub/id@ver#entry"` | Reference to hexon entry |
| `address` | `"0x.../0x.../0x..."` | amp-compatible 3-level tag address |
| `array` | `[...]` | JSON array |
| `object` | `{...}` | JSON object |

---

## license.json

```json
{
  "license_type": "free",
  "spdx_id": "CC-BY-4.0",
  "attribution": "Alpine GIS Co.",
  "payment_provider": null,
  "payment_verification_url": null,
  "free_entries": ["*"],
  "encrypted_key": null
}
```

| `license_type` | Behavior |
|----------------|----------|
| `free` | All blobs plaintext, no restrictions |
| `attribution` | Free with attribution |
| `paid` | Asset blobs encrypted (ChaCha20-Poly1305); key released after payment |

---

## Signature & Integrity

### Manifest Signature

Ed25519 signature of manifest JSON with `signature` field zeroed:

1. Set `manifest.signature = ""`
2. Serialize to canonical JSON (sorted keys, no whitespace)
3. Sign with publisher's ed25519 key
4. Verify against `publisher_did` public key

### Asset Integrity

Every `asset_hash` is BLAKE3 hex of the blob at `assets/{hash}`. Verify on extract.

---

## Immutability Contract (Scene Hexons)

1. **Node logs are append-only.** Never modified or deleted. Merge by concatenating and sorting by `(hlc_timestamp, row_version)`.

2. **Properties are derived from logs.** The `properties` snapshot is a convenience; authoritative state comes from log replay.

3. **HLC timestamps** enable point-in-time queries and distributed merge without coordination.

4. **Peer provenance.** Every log entry carries `source_did` for trust and auditing.

---

## Forward Compatibility

- Parsers **MUST** ignore unknown JSON fields
- New `hexon_type` values are treated as opaque by older parsers
- New `op` variants use `custom(string)` until formalized
- ZIP may contain additional directories (skipped by older parsers)

---

## amp.SDK Compatibility

### Addressing Mapping

| amp.SDK | Hexon | Notes |
|---------|-------|-------|
| NodeID (128-bit UID) | `address.node_id` | Maps to petal/spatial partition |
| AttrID (128-bit UID) | `address.attr_id` | Maps to node/entity |
| ItemID (128-bit UID) | `address.item_id` | Maps to property/asset |
| EditID (128-bit UID) | HLC timestamp | Newest-first ordering |

### CrateManifest Mapping

| amp.SDK CrateInfo | Hexon manifest | Notes |
|-------------------|----------------|-------|
| `CrateURI` | `hexon_id` (within URI) | `{PublisherID}/{CrateID}` → `hexon://{pub}/{id}` |
| `MajorVersion/MinorVersion/BuildNumber` | `version` | SemVer string |
| `BuildID` | `build_id` | Convention: `yyMMdd-v{ver}` |
| `CrateName` | `name` | Display name |
| `PublisherName` | `publisher_name` | |
| `ShortDesc` | `description` | |
| `Tags` | `tags` | Array instead of comma-delimited |
| `HomeURL` | `homepage_url` | |
| `TimeCreated` | `created_at` | RFC 3339 |
| `TimeBuilt` | `updated_at` | RFC 3339 |
| `ApproxSize` | `approx_size_bytes` | |

### AssetEntry Mapping

| amp.SDK AssetEntry | Hexon entries[] | Notes |
|--------------------|----------------|-------|
| `Kind` | `kind` | Same values + extensions |
| `EntryURI` | `entry_id` | Path-safe chars |
| `Label` | `label` | |
| `Tags` | `tags` | |
| `IsPlaceable` | `is_placeable` | |
| `IsPrivate` | `is_private` | |
| `AutoScale` | `auto_scale` | |
| `CenterX/Y/Z` | `center` | `[f32; 3]` |
| `ExtentsX/Y/Z` | `extents` | `[f32; 3]` |

---

## API Endpoints (Reference Implementation)

| Method | Path | RBAC | Description |
|--------|------|------|-------------|
| `POST` | `/api/v1/hexons/publish` | Owner+ | Upload `.hexon` package |
| `GET` | `/api/v1/hexons/search?q=&tags=&type=` | Viewer+ | Search hexons |
| `GET` | `/api/v1/hexons/:uri` | Viewer+ | Get manifest |
| `GET` | `/api/v1/hexons/:uri/entries` | Viewer+ | List entries |
| `GET` | `/api/v1/hexons/:uri/entries/:id/asset` | Viewer+ | Fetch blob |
| `POST` | `/api/v1/hexons/:uri/install` | Editor+ | Install into petal |
| `DELETE` | `/api/v1/hexons/:uri/uninstall` | Editor+ | Uninstall |
| `GET` | `/api/v1/hexons/installed` | Viewer+ | List installed |
| `GET` | `/api/v1/petals/:id/export` | Viewer+ | Export petal as scene `.hexon` |
| `POST` | `/api/v1/petals/:id/import` | Editor+ | Import `.hexon` into petal |
| `POST` | `/api/v1/petals/:id/import/gpx` | Editor+ | Import GPX |
| `GET` | `/api/v1/petals/:id/export/gpx` | Viewer+ | Export GPX |
