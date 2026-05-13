# Track: Crate Registry — Reusable Asset Packages, P2P Hosting, Marketplace

**Created:** 2026-05-08
**Status:** Draft
**Priority:** P1
**Depends on:** Open Crate Format (Phase 6.5), Headless Relay, Fractal Mesh (P2P replication)
**Blocks:** Community Marketplace, Premium Content

---

## Problem Statement

FractalEngine has content-addressed assets (BLAKE3 blobs) and a .fractal export format, but no mechanism for:

1. **Reusable content packages** — a skybox, material set, terrain tileset, or 3D model collection that multiple petals/verses can reference
2. **Community distribution** — creators publishing packages that others can discover and install
3. **P2P hosting** — packages hosted across peers and relays (no central server required)
4. **Monetization** — creators optionally paywalling packages (install-to-use model)
5. **Multi-format assets** — supporting GLB/GLTF, HDR/EXR skyboxes, PBR material sets, GPX collections, terrain tilesets, custom shaders

This track implements the **registry and distribution layer** on top of the Open Crate format (`.hexon`, spec: `docs/hexon-format-spec.md`). The format itself is shared with amp.SDK (Go) and plan.3D (Unity) — this track handles FractalEngine-specific install behaviors, P2P hosting, and marketplace semantics.

---

## Goals

1. **`.hexon` package format** — Open Crate spec v2.0.0, ZIP-based, self-describing, content-addressed, versionable
2. **Crate types** — first-class support for: Model, Material, Skybox, Terrain, GPX Collection, Sound, Script, Theme
3. **Publisher identity** — crates are signed by the publisher's DID (ed25519)
4. **P2P distribution** — any peer/relay can host crates; discovery via DHT + registry index
5. **Install semantics** — "installing" a crate links its assets into a petal without copying (lazy blob fetch)
6. **Paywall support** — optional access gate (encrypted blob keys, payment verification via external provider)
7. **Version management** — SemVer, dependency declaration, upgrade path
8. **Multi-format assets** — GLB, GLTF, HDR, EXR, KTX2, PNG, GPX, GeoJSON, WASM (shaders)
9. **Registry API** — REST endpoints for publish, search, install, list

## Non-Goals (this track)

- Payment processing (defer to external provider integration)
- DRM / client-side enforcement (trust model is signature-based)
- Crate dependency resolution (DAG solver — defer to v2)
- Custom shader execution sandbox (WASM runtime — defer)
- Automatic crate updates / hot-reload

---

## Architecture Overview

### Crate URI Scheme

```
hexon://{publisher_id}/{crate_id}[@{version}]
```

Example: `hexon://did:key:z6MkPublisher/alpine-terrain-pack@1.2.0`

Short form (latest version): `hexon://did:key:z6MkPublisher/alpine-terrain-pack`

amp.SDK-compatible: `hexon://plan-systems.org/plan.app.ui@3.0.1`

### `.hexon` Package Format (Open Crate v2.0.0)

A ZIP archive following the Open Crate spec (`docs/hexon-format-spec.md`):

```
.hexon ZIP:
├── manifest.json           # CrateManifest (identity, version, deps, permissions)
├── README.md               # Human-readable description (rendered in UI)
├── icon.png                # 256x256 crate icon
├── preview/                # Preview images/thumbnails
│   ├── thumb_01.png
│   └── thumb_02.png
├── assets/                 # Content-addressed blobs
│   ├── {blake3_hash_1}    # e.g., GLB model
│   ├── {blake3_hash_2}    # e.g., HDR skybox
│   └── ...
├── entries.json            # Asset entry catalog (type, path, metadata)
├── schema.json             # FieldDef definitions this crate provides
└── license.json            # License terms + access control config
```

### CrateManifest

```json
{
  "schema_version": "1.0.0",
  "crate_id": "alpine-terrain-pack",
  "publisher_did": "did:key:z6MkPublisher...",
  "publisher_name": "Alpine GIS Co.",
  "version": "1.2.0",
  "build_id": "260508-v1.2.0",
  "name": "Alpine Terrain Pack",
  "description": "High-resolution terrain tiles for the Swiss Alps (zoom 8-16)",
  "tags": ["terrain", "alps", "switzerland", "elevation", "satellite"],
  "crate_type": "terrain",
  "created_at": "2026-05-08T12:00:00Z",
  "updated_at": "2026-05-08T14:30:00Z",
  "approx_size_bytes": 524288000,
  "min_engine_version": "0.18.0",
  "homepage_url": "https://alpine-gis.example.com",
  "dependencies": [],
  "signature": "<ed25519 signature of manifest minus this field>"
}
```

### Asset Entry Catalog (entries.json)

```json
{
  "entries": [
    {
      "entry_id": "skybox_sunset",
      "kind": "skybox",
      "asset_hash": "abc123...",
      "format": "hdr",
      "label": "Golden Hour Sunset",
      "tags": ["sunset", "warm", "outdoor"],
      "description": "360° HDR skybox captured at golden hour",
      "is_placeable": false,
      "is_private": false,
      "preview_image": "preview/thumb_01.png",
      "metadata": {
        "resolution": "4096x2048",
        "dynamic_range": "32-bit float"
      }
    },
    {
      "entry_id": "pbr_rock_granite",
      "kind": "material",
      "asset_hash": "def456...",
      "format": "material_bundle",
      "label": "Granite Rock PBR",
      "sub_assets": {
        "albedo": "hash_albedo",
        "normal": "hash_normal",
        "roughness": "hash_roughness",
        "ao": "hash_ao"
      }
    },
    {
      "entry_id": "mountain_cabin",
      "kind": "model",
      "asset_hash": "ghi789...",
      "format": "glb",
      "label": "Mountain Cabin",
      "is_placeable": true,
      "extents": [5.0, 3.5, 4.2],
      "center": [0.0, 1.75, 0.0]
    }
  ]
}
```

### Crate Types (CrateKind)

| Kind | Typical Contents | Install Behavior |
|------|-----------------|------------------|
| `model` | GLB/GLTF 3D models | Assets added to petal's model palette |
| `material` | PBR texture sets (albedo, normal, roughness, AO, metallic) | Materials registered in material library |
| `skybox` | HDR/EXR equirectangular or cubemap | Available in environment settings |
| `terrain` | Pre-cached tile sets, elevation data, satellite imagery | Configures petal TerrainConfig |
| `gpx_collection` | GPX files + waypoint icons | Imported as track node hierarchies |
| `sound` | Spatial audio files (OGG, WAV) | Available in audio library |
| `theme` | UI color schemes, font overrides | Applied to egui/webview styling |
| `script` | WASM modules (future) | Registered in script runtime |

### New Crate: `fe-hexon`

```
fe-hexon/
├── Cargo.toml
└── src/
    ├── lib.rs              # Public API + CrateRegistry resource
    ├── manifest.rs         # CrateManifest, AssetEntry, CrateKind, License
    ├── package.rs          # .hexon ZIP read/write (builds on `zip` crate)
    ├── registry.rs         # Local registry (installed crates index, SurrealDB)
    ├── resolver.rs         # Crate URI → blob fetch (local cache → peer → relay)
    ├── publisher.rs        # Build + sign + package workflow
    ├── signature.rs        # Ed25519 manifest signing + verification
    ├── access.rs           # License enforcement (free, attribution, paid stub)
    └── p2p/
        ├── mod.rs
        ├── announce.rs     # DHT announcement of hosted crates
        └── fetch.rs        # Fetch crate/assets from peers via iroh
```

### P2P Distribution Model

1. **Publish**: Creator builds `.hexon`, signs manifest with their DID, uploads to their relay (or any willing host)
2. **Announce**: Hosting relay announces `CrateAvailable { crate_uri, manifest_hash, hosting_peers }` on DHT
3. **Discover**: Clients search DHT by tags/name/publisher, receive manifest + peer list
4. **Install**: Client fetches manifest → verifies signature → resolves asset hashes → lazy-fetches blobs from nearest peer
5. **Use**: Installed crate's assets are referenced by hash in petal nodes (zero-copy linking)
6. **Replicate**: Any peer that installs a crate can optionally re-host it (seed model)

### Paywall Model (Future-Ready)

```json
// license.json
{
  "license_type": "paid",
  "price": { "amount": "9.99", "currency": "USD" },
  "payment_provider": "stripe",
  "payment_verification_url": "https://publisher.example.com/verify",
  "free_entries": ["preview/*"],
  "encrypted_key": "<encrypted blob decryption key>"
}
```

- Free crates: all blobs are plaintext
- Paid crates: asset blobs are encrypted (ChaCha20-Poly1305); decryption key released after payment verification
- Verification is publisher-hosted (FractalEngine doesn't process payments)
- `free_entries` allow previews/thumbnails without purchase

### Database Tables (new)

```
crate_registry (id: crate_uri)
  - crate_uri: String
  - manifest_hash: String (BLAKE3 of manifest.json)
  - publisher_did: String
  - crate_type: String
  - version: String
  - name: String
  - tags: Array<String>
  - installed_at: Datetime
  - approx_size_bytes: i64
  - signature_valid: bool

crate_entry (id: entry_id)
  - entry_id: String
  - crate_uri: String
  - kind: String
  - asset_hash: String
  - format: String
  - label: String
  - metadata: Object FLEXIBLE
```

### API Endpoints

| Method | Path | RBAC | Description |
|--------|------|------|-------------|
| `POST` | `/api/v1/crates/publish` | Owner+ | Upload .hexon package |
| `GET` | `/api/v1/crates/search?q=&tags=&type=` | Viewer+ | Search available crates |
| `GET` | `/api/v1/crates/:uri` | Viewer+ | Get crate manifest + entries |
| `POST` | `/api/v1/crates/:uri/install` | Editor+ | Install crate into a petal |
| `DELETE` | `/api/v1/crates/:uri/uninstall` | Editor+ | Uninstall crate from petal |
| `GET` | `/api/v1/crates/installed` | Viewer+ | List installed crates |
| `GET` | `/api/v1/crates/:uri/entries` | Viewer+ | List entries in a crate |
| `GET` | `/api/v1/crates/:uri/entries/:id/asset` | Viewer+ | Fetch asset blob |

---

## Phases

### Phase 1: Package Format & Local Registry

- CrateManifest struct + serde (de)serialization
- AssetEntry catalog (entries.json)
- `.hexon` ZIP read/write (package.rs)
- Ed25519 manifest signing + verification
- Local registry (SurrealDB tables: crate_registry, crate_entry)
- Install workflow: unpack → verify → register entries → link blobs to blob store
- API: publish, install, list, search (local only)

### Phase 2: Multi-Format Asset Support

- Model assets: GLB/GLTF import into asset pipeline
- Skybox assets: HDR/EXR loading → Bevy environment map
- Material assets: PBR bundle (albedo + normal + roughness + AO) → StandardMaterial
- Terrain crate type: pre-cached tiles → TerrainConfig auto-setup
- GPX collection type: batch GPX import on install
- Asset preview generation (thumbnail rendering)

### Phase 3: P2P Distribution

- DHT crate announcement (fe-network integration)
- Peer-to-peer crate fetch via iroh (blob transfer)
- Manifest discovery (search across connected peers)
- Seeding: installed crates re-announced for others
- Relay hosting: relays auto-host published crates
- Bandwidth-aware fetch (prefer LAN peers over WAN)

### Phase 4: Marketplace & Access Control

- License enforcement (free vs paid stub)
- Encrypted blob support (ChaCha20-Poly1305)
- Payment verification endpoint integration
- Publisher profiles (name, avatar, homepage, crate count)
- Rating/review system (signed reviews stored in DHT)
- Crate update notifications

---

## Integration with Existing Systems

| System | Integration Point |
|--------|------------------|
| **fe-format** | .hexon scene archives can embed crate references (URI only, not full blobs) |
| **fe-entity-store** | Crate entries resolve to EntitySnapshots when placed in scene |
| **fe-terrain** | Terrain crate type auto-configures petal TerrainConfig |
| **fe-sync** | Crate blobs replicate via same iroh infrastructure |
| **fe-api** | REST endpoints for publish/install/search |
| **fe-database** | Registry tables, RBAC for install permissions |
| **Blob Store** | Crate assets stored in existing FsBlobStore (content-addressed) |
| **RBAC** | Publishing requires Owner+, installing requires Editor+ |

---

## Success Criteria

- [ ] Build a `.hexon` package from a directory of assets
- [ ] Sign + verify crate manifest with publisher DID
- [ ] Install a model crate → GLB assets available for placement in petal
- [ ] Install a skybox crate → HDR available in environment settings
- [ ] Install a terrain crate → petal gets terrain config + cached tiles
- [ ] Search for crates by tag/name across connected peers
- [ ] Fetch crate from a remote relay via iroh
- [ ] Uninstall removes registry entries (but doesn't delete shared blobs)
- [ ] .fractal export includes crate URIs; import resolves them on the target
