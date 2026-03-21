# Technology Stack

## Language

- **Rust** (stable toolchain) — primary and only language for the FractalEngine binary

## 3D Engine

- **Bevy 0.18.x** — ECS-based 3D game engine
  - Built-in GLTF/GLB loader (`bevy_gltf`) — use embedded-texture GLBs to avoid hot-reload bug #18267
  - `bevy_egui 0.39` — in-game UI (only mature Bevy UI option)
  - `bevy_quinnet 0.20` — QUIC/ICE transport with NAT traversal for P2P connections
  - Note: Pin Bevy version; budget 1–2 engineer-weeks per quarterly upgrade cycle

## Database

- **SurrealDB 3.0.x** (embedded, in-process)
  - Backend: **SurrealKV** (pure Rust, no C++ build deps — do NOT use RocksDB)
  - `SURREAL_SYNC_DATA=true` — mandatory for crash safety (not the default)
  - Schema: strict types for core entities; FLEXIBLE for operator-defined extensions
  - RBAC enforced at the database layer via `DEFINE TABLE ... PERMISSIONS` — never in Bevy systems
  - Mutations written as an immutable op-log: `{ lamport_clock, node_id, op_type, payload, sig }`
  - Time-travel queries via SurrealKV VERSION clause (replaces hand-rolled event sourcing)

## P2P Networking

- **libp2p 0.56** — peer discovery only
  - Kademlia DHT for Node discovery and Petal metadata publication
  - mDNS for LAN discovery
  - Noise encryption + Yamux multiplexing + QUIC transport
  - Runs on a dedicated OS thread with its own Tokio runtime (isolated from Bevy's smol executor)

- **iroh** (iroh-blobs + iroh-gossip + iroh-docs) — P2P data layer
  - `iroh-blobs` — BLAKE3-verified content-addressed asset distribution (GLTF/GLB meshes, textures)
  - `iroh-gossip` — HyParView + PlumTree epidemic broadcast for real-time zone events
  - `iroh-docs` — eventually-consistent CRDT key-value store for Petal world-state sync on peer reconnect
  - iroh relay — stateless encrypted relay (Croquet TeaTime Reflector pattern) for NAT traversal fallback
  - Shares the network thread's Tokio runtime

## Thread Topology (Non-negotiable)

```
T1 (OS Main)    — Bevy ECS scheduler (smol executor) + wry WebView (event loop injection)
T2 (Network)    — libp2p Swarm + iroh endpoint (dedicated Tokio runtime)
T3 (Database)   — SurrealDB embedded (dedicated Tokio runtime)
T4-TN (IO Pool) — bevy_tasks AsyncComputeTaskPool (GLTF loading, asset I/O)
```

Cross-thread communication: typed `mpsc` channels only. Never call `block_on()` from a Bevy system. Never allow T2 and T3 Tokio runtimes to nest.

## Identity & Authentication

- **ed25519-dalek 2.2** — ed25519 keypair generation and signing
  - Always use `verify_strict()` (not `verify()`) to prevent weak-key forgery
  - Never enable `legacy_compatibility` feature
- **jsonwebtoken 10.3** — JWT minting and verification
  - JWT claims MUST include `sub: did:key:z6Mk<multibase_pub>` for W3C DID compatibility
  - JWT lifetime: 300 seconds maximum
- **keyring crate** — OS keychain storage for the Node's private key (never expose raw key material in UI)
- **RBAC**: `public` (hardcoded floor) → admin-defined custom named roles → `admin` (hardcoded ceiling)
- Session cache TTL: 60 seconds with mandatory SurrealDB re-validation
- Signed revocations broadcast via iroh-gossip within 5 seconds

## Embedded Browser

- **wry 0.54** — WebView embedding (maintained by Tauri organization)
  - Build a thin custom Bevy plugin — do NOT use `bevy_wry` (abandoned at Bevy 0.14)
  - V1 architecture: overlay window positioned per-frame via `Camera::world_to_viewport()` + `EventLoopProxy`
  - All JS↔Rust calls via a versioned typed command enum (typed IPC) — no raw eval
  - Mandatory WebView security rules:
    - Block all localhost and RFC 1918 addresses unconditionally
    - Enforce Content Security Policy headers
    - Display non-dismissible "External Website" trust bar on all overlays
    - No JavaScript-to-native bridge beyond the typed IPC enum
  - V2 target: offscreen render composited as wgpu texture (requires platform API stabilization)

## Asset Pipeline

- **Format**: GLTF/GLB only — no FBX, OBJ, or other formats accepted
  - Operators use Blender (free, open source) to convert from other formats
  - Always embed textures in the GLB binary (avoids Bevy hot-reload bug #18267)
- **Content addressing**: BLAKE3 hash of the final GLB bytes as the canonical asset ID
- **Distribution**: iroh-blobs for verified P2P asset transfer
- **Size limit**: 256 MB per asset (configurable per Node)
- **Ingestion**: `AssetIngester` trait with `GltfIngester` v1 implementation (stub `SplatIngester` for v2 Gaussian splatting)
- **Dead-reckoning**: `DeadReckoningPredictor` component on all moving Transforms — network updates only on threshold crossings (60–90% bandwidth reduction)

## Sync & Caching

- **Inter-Node sync**: iroh-docs range reconciliation on peer connect (delta only, not full state)
- **Local cache**: SurrealDB namespace per visited Petal; asset files cached by BLAKE3 hash
- **Cache limit**: 2 GB default, LRU eviction after 7 days inactivity (configurable)
- **Offline mode**: previously-visited Petals load from local replica without network
- **Lamport clock**: every SurrealDB mutation carries a Lamport clock for deterministic ordering

## Future-Proofing (Design for extensibility now)

| Abstraction | v1 Impl | v2 Target |
|---|---|---|
| `BrowserSurface` trait | wry overlay window | Bevy native compositor |
| `SceneSerializer` trait | JSON in SurrealDB | BSN scene format (Bevy 0.19+) |
| `AssetIngester` trait | GltfIngester | SplatIngester (Gaussian splat) |
| `ReplicationStore` trait | iroh-docs + file cache | SurrealDB live-query (2027) |
| DID identity | did:key from ed25519 keypair | Full VC wallet integration |

## Key Constraints

- **GLTF/GLB only** — no server-side format conversion; conversion is the uploader's responsibility
- **No Tokio in Bevy systems** — use `AsyncComputeTaskPool` for all async work from Bevy
- **RBAC at DB layer only** — Bevy systems receive only pre-authorised data from SurrealDB
- **All gossip payloads signed** — unsigned messages rejected at ingest
- **WebView Threat Model document required** before any WebView code is written
