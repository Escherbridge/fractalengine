---
type: Tech Stack
title: FractalEngine Technology Stack
tags: [tech-stack, rust, bevy, surrealdb, iroh, analytics]
timestamp: 2026-07-17T00:00:00Z
resource: ./product.md
---

# Technology Stack

## Language

- **Rust** (stable toolchain) — primary and only language for all binaries

## Workspace layout (22 crates)

Derived from the root `Cargo.toml` members list (2026-07-17; `fe-auth` was
absorbed into `fe-database` in the 2026-07-17 consolidation).

| Layer | Crates |
|---|---|
| Binaries | `fractalengine` (GUI: DefaultPlugins + EguiPlugin), `fractalengine-relay` (headless `fe-relay`: MinimalPlugins + ScheduleRunnerPlugin) |
| Runtime / infra | `fe-runtime` (threads + channels), `fe-test-harness` (package name `fractalengine-test-harness`) |
| Data | `fe-database`, `fe-entity-store`, `fe-format` (HLC + op-log types), `fe-query` |
| Spatial | `fe-terrain`, `fe-hexon` (map package format), `fe-hexon-registry` (hosted registry HTTP service) |
| Network | `fe-network` (libp2p discovery), `fe-sync` (iroh data layer) |
| API | `fe-api` (axum gateway + MCP server) |
| Identity & auth | `fe-identity`, `fe-policy` (RBAC engine; `fe-database` re-exports `RoleLevel` and hosts the session cache) |
| Plugins | `fe-plugin`, `fe-sdk`, `fe-plugin-test` |
| UI / rendering | `fe-renderer`, `fe-ui`, `fe-webview` |

## 3D Engine

- **Bevy 0.18.x** — ECS-based 3D game engine
  - Built-in GLTF/GLB loader (`bevy_gltf`) — use embedded-texture GLBs to avoid hot-reload bug #18267
  - `bevy_egui 0.39` — in-game UI (only mature Bevy UI option)
  - Note: Pin Bevy version; budget 1–2 engineer-weeks per quarterly upgrade cycle

## Database

- **SurrealDB 3.0.x** (embedded, in-process)
  - Backend: **SurrealKV** (pure Rust, no C++ build deps — do NOT use RocksDB)
  - `SURREAL_SYNC_DATA=true` — mandatory for crash safety (not the default)
  - Schema: strict types for core entities; FLEXIBLE for operator-defined extensions
  - Mutations are written as an immutable op-log stamped with an **HLC**
    (hybrid logical clock: physical time + logical counter + node id) for
    deterministic causal ordering — `{ hlc, node_id, op_type, payload, sig }`;
    sigs carried verbatim (13 placeholder signing sites pending per-op
    ed25519, decision D5-1)
  - Time-travel queries via SurrealKV VERSION clause (replaces hand-rolled event sourcing)

## Thread Topology

Seven threads (see `fe-runtime/src/channels.rs`):

```
T1 — Bevy main (ECS scheduler + Tauri/wry portal event loop)
T2 — Network: libp2p Swarm (dedicated tokio runtime)
T3 — Database: SurrealDB embedded (dedicated tokio runtime)
T4 — Sync: iroh endpoint + gossip (dedicated tokio runtime)
T5 — Replication bridge
T6 — API gateway: axum (multi-thread tokio runtime)
T7 — Plugin host (dedicated tokio runtime)
```

Cross-thread communication: typed `crossbeam::channel::bounded(256)` channels
(`fe-runtime/src/channels.rs`). Never call `block_on()` from a Bevy system.
Runtimes never nest.

## P2P Networking

- **libp2p 0.56** (`fe-network`) — peer discovery only
  - Kademlia DHT for peer discovery and Petal metadata publication
  - mDNS for LAN discovery
  - Noise encryption + Yamux multiplexing + QUIC transport (`quic` feature)
- **iroh 0.35** (`fe-sync`, `fe-identity`) — P2P data layer
  - `iroh-gossip` — epidemic broadcast for real-time events + signed revocations (ported to 0.35)
  - `iroh-docs` — **TARGET** replication layer for petal world-state; currently
    mock-backed behind the VerseReplicator seam (p2p_mycelium_completion
    track); real Engine wiring may land directly against iroh 1.x
    (iroh_1_0_upgrade — hosted-relay wire protocol EOL 2026-12-31)
  - `iroh-quinn-proto 0.13` pinned for the BBR congestion controller — see `fe-sync/src/AGENTS.md` §congestion-control
  - iroh relay — encrypted relay for NAT traversal fallback
- **Blob distribution**: BLAKE3 content-addressed blob store (`fe-sync/src/blob_store.rs`)
  served over the iroh endpoint; map packages (hexons) distribute the same way

## Analytics & egress stack (primary feature)

- **axum 0.8** HTTP gateway (`fe-api`, T6): authenticated, rate-limited (10 q/s/DID)
- `/api/v1/query` — injection-guarded **single-SELECT SQL endpoint** with
  query_guard cost / row / timeout limits
- GIS endpoints (GeoJSON-shaped): `/petals/:id/gis/nodes`, `/gis/tracks`
- **GeoParquet** export (arrow/parquet 54.x, WKB Point Z) behind the `parquet`
  feature; parquet/CSV download endpoints
- **DuckDB-compatible dialect translation** (`fe-query/duckdb_compat`) — the
  engine PowerBI / Python / spreadsheets speak
- **ed25519-signed share URLs** with scope + expiry
- **MCP server** hosted in `fe-api`/relay (6 tools, growing toward 20 — mcp_scene_primitives track)
- **CRS seam (critical)**: `node.position` stores **petal-local meters**;
  lat/lon↔local conversion lives at the API edge via the petal's terrain
  origin (`fe-query/src/AGENTS.md` §gis, §local-coords). Users think lat/lon;
  the store is local meters. Every egress path must respect this.

## Identity & Authentication

- **ed25519-dalek 2.2** — ed25519 keypair generation and signing
  - Always use `verify_strict()` (not `verify()`) to prevent weak-key forgery
  - Never enable `legacy_compatibility` feature
- **jsonwebtoken 10.3** — JWT minting and verification
  - JWT claims MUST include `sub: did:key:z6Mk<multibase_pub>` for W3C DID compatibility
  - JWT lifetime: 300 seconds maximum
- **keyring crate** — OS keychain storage for the operator's private key (never expose raw key material in UI)
- **RBAC**: fixed `RoleLevel` hierarchy **Owner > Manager > Editor > Viewer > None**
  (canonical home: `fe-policy`, re-exported by `fe-database`), resolved
  hierarchically over scope strings (`VERSE#v-FRACTAL#f-PETAL#p`).
  Enforcement: deny-by-default `Policy::evaluate` in `fe-policy` (since
  2026-07-15); the DB write path (`require_write_role`), fe-hexon
  install/uninstall, and the fe-sync write gate all route through it. Never
  enforce authorization in Bevy systems or UI code.
- Session cache: `fe_database::session_cache::SessionCache` (moved from the
  absorbed fe-auth crate) — 60-second TTL with mandatory re-validation;
  log-first revocation (see `fe-database/src/AGENTS.md` §session-cache)
- Signed revocations broadcast via iroh-gossip within 5 seconds

## WebView

- **Tauri portal architecture** (`fe-webview`, `backend-tauri` feature = tauri + wry 0.54 + winit)
  - The webview runs in a Tauri-managed portal aligned with the Bevy viewport; see the fe-webview AGENTS.md for the seam
  - The user-facing field for a portal's destination is the **Portal URL**
  - All JS↔Rust calls via a versioned typed command enum (typed IPC) — no raw eval
  - Mandatory security rules:
    - Block all localhost and RFC 1918 addresses unconditionally
    - Enforce Content Security Policy headers
    - Display non-dismissible "External Website" trust bar on all portals
    - No JavaScript-to-native bridge beyond the typed IPC enum

## Asset Pipeline

- **Format**: GLTF/GLB only — no FBX, OBJ, or other formats accepted
  - Operators use Blender (free, open source) to convert from other formats
  - Always embed textures in the GLB binary (avoids Bevy hot-reload bug #18267)
- **Content addressing**: BLAKE3 hash of the final GLB bytes as the canonical asset ID
- **Distribution**: content-addressed P2P transfer over the iroh endpoint
- **Size limit**: 256 MB per asset (configurable per operator)
- **Map packages**: hexon format v1.0.0 (`fe-hexon`) — tilesets, terrain, and
  path assets packaged for publish/import; hosted registry via `fe-hexon-registry`

## Sync & Caching

- **Inter-peer sync**: HLC-ordered op-log replication behind the
  VerseReplicator seam (currently mock-backed — see P2P Networking above)
- **Local cache**: SurrealDB namespace per visited Petal; asset files cached by BLAKE3 hash
- **Offline mode** (planned): previously-visited Petals load from local replica without network

## Key Constraints

- **GLTF/GLB only** — no server-side format conversion; conversion is the uploader's responsibility
- **No Tokio in Bevy systems** — use `AsyncComputeTaskPool` for async work from Bevy; typed crossbeam channels for cross-thread requests
- **Authorization through fe-policy only** — Bevy systems and UI receive pre-authorized data; never re-check roles in them
- **All gossip payloads signed** — unsigned messages rejected at ingest
- **CRS correctness at every egress path** — petal-local meters in the store, lat/lon at the edge, exports CRS-stamped
