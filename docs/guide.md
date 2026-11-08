# FractalEngine Developer Guide

Comprehensive guide for understanding, building, and extending FractalEngine.

---

## Table of Contents

- [1. What is FractalEngine?](#1-what-is-fractalengine)
- [2. Getting Started](#2-getting-started)
- [3. Architecture Deep Dive](#3-architecture-deep-dive)
- [4. Crate-by-Crate Reference](#4-crate-by-crate-reference)
- [5. Threading Model](#5-threading-model)
- [6. Message Types & Channel Protocol](#6-message-types--channel-protocol)
- [7. Identity & Cryptography](#7-identity--cryptography)
- [8. Database Layer](#8-database-layer)
- [9. Networking Layer](#9-networking-layer)
- [10. Asset Pipeline](#10-asset-pipeline)
- [11. WebView System](#11-webview-system)
- [12. Sync & Offline](#12-sync--offline)
- [13. Security Model](#13-security-model)
- [14. Development Workflow](#14-development-workflow)
- [15. Testing Strategy](#15-testing-strategy)
- [16. Extending FractalEngine](#16-extending-fractalengine)
- [17. Troubleshooting](#17-troubleshooting)

---

## 1. What is FractalEngine?

FractalEngine is a spatial analytics engine on top of a decentralized, peer-to-peer 3D digital-twin substrate. It ships as a single native binary that embeds:

- A **3D engine** (Bevy) for rendering interactive worlds
- An **embedded database** (SurrealDB) for persistence and RBAC
- A **P2P stack** — libp2p for discovery and iroh (blobs, gossip, docs) for data sync
- An **embedded browser** (wry) for in-world web content

No central server is required, and your data stays in a local embedded database. An operator creates a Verse, fills it with Fractals and Petals (3D spaces), and invites peers to join under role-based access control.

### Who is this for?

- **Analysts and operators** who want maps, terrain, GPX paths and IoT readings in one embedded database that PowerBI, a spreadsheet or DuckDB can query directly (see [bi-egress.md](bi-egress.md))
- **Small businesses** wanting self-hosted 3D collaborative spaces (showrooms, digital twins, meeting rooms)
- **Indie developers** building decentralized 3D experiences
- **Creative professionals** using Petals as interactive portfolios

### The Naming System

FractalEngine uses botanical/fractal metaphors. Content forms a four-level hierarchy, Verse > Fractal > Petal > Node:

| Entity | Description |
|---|---|
| **Verse** | The top-level container and P2P namespace; each Verse has its own replica |
| **Fractal** | Groups Petals under a Verse |
| **Petal** | A 3D world/space; one Petal is active at a time |
| **Node** | An interactive object in a Petal (a placed model, waypoint, path, ...) |
| **Asset** | A GLTF/GLB model, content-addressed by its BLAKE3 hash |
| **Role** | An RBAC assignment for a peer at verse, fractal or petal scope (Owner, Manager, Editor, Viewer) |
| **Room** / **Model** | Older constructs (a zone within a Petal and a placed object) that remain in the schema |

---

## 2. Getting Started

### Prerequisites

- **Rust 1.91+** (stable toolchain)
- **Git** for version control
- Platform-specific build deps (see [BUILDING.md](../BUILDING.md) and the [Bevy setup guide](https://bevyengine.org/learn/quick-start/getting-started/setup/))

### First-Time Setup

```bash
# Clone the repository
git clone https://github.com/Escherbridge/fractalengine.git
cd fractalengine

# Install Rust stable toolchain
rustup toolchain install stable
rustup component add rustfmt clippy

# Build the desktop app
cargo build -p fractalengine
```

### Running

The workspace has several binaries, so name the package:

```bash
# Default run
cargo run -p fractalengine

# With debug logging
RUST_LOG=debug cargo run -p fractalengine

# With per-crate log filtering
RUST_LOG=fe_network=trace,fe_database=debug,fe_runtime=info cargo run -p fractalengine

# Release mode
cargo run --release -p fractalengine
```

With `RUST_LOG` unset, only errors are logged.

On first launch, FractalEngine will:
1. Generate an Ed25519 keypair (the node identity)
2. Store the private key in your OS keychain (macOS Keychain, Windows Credential Manager, or Linux Secret Service); if the keychain is unavailable, it falls back to a key file under your per-user data folder
3. Create a SurrealDB data directory at `./data/fractalengine.db` (a packaged build keeps `data/` beside the binary)
4. Seed a starter Genesis Verse with a Genesis Fractal and Genesis Petal
5. Open the editor window

<!-- screenshot: first-launch-editor — The editor right after a first launch: the seeded Genesis Verse, Genesis Fractal and Genesis Petal in the hierarchy sidebar, an empty 3D viewport with the grid, and the top toolbar with the tool buttons. -->

### Build Variants

| Command | Use Case |
|---|---|
| `cargo build -p fractalengine` | Unoptimized debug build of the desktop app (fast iteration) |
| `cargo build --release -p fractalengine` | Optimized single binary (fat LTO) for deployment |
| `cargo build --release -p fractalengine-relay` | The headless relay (`fe-relay`) |
| `cargo build -p fe-database` | Build only the database crate |
| `cargo build -p fe-network` | Build only the networking crate |
| `cargo check -p fe-identity` | Type-check only the identity crate (the pre-commit gate) |

---

## 3. Architecture Deep Dive

### The Seven-Thread Model

The desktop app's runtime is split across seven long-lived threads, each with specific responsibilities (`fractalengine/src/main.rs` spawns them). The headless relay (`fe-relay`) runs T2 through T6 (network, database, sync, replication bridge and API) without a window and does not start the plugin host (T7).

**T1 — Main Thread (OS thread)**
- Bevy ECS scheduler (uses smol async executor, NOT Tokio)
- Embedded WebView overlay (`fe-webview`), driven from Bevy systems
- egui UI rendering
- Drain systems that consume messages from the other threads
- Asset loading via Bevy's `AsyncComputeTaskPool`

**T2 — Network Thread**
- Spawned as a dedicated OS thread with its own Tokio runtime
- Runs the libp2p Swarm (Kademlia DHT over QUIC) and bootstraps it at startup
- Receives `NetworkCommand` from T1, sends `NetworkEvent` back
- The iroh transport does not live here; it runs on T4

**T3 — Database Thread**
- Spawned as a dedicated OS thread with its own Tokio runtime
- Runs SurrealDB with SurrealKV backend
- Applies RBAC at the data layer through the fe-policy engine (deny-by-default), never in Bevy systems
- Receives `DbCommand` from T1 (and from the API and sync threads), sends `DbResult` back

**T4 — Sync Thread**
- Owns the iroh endpoint and the full P2P stack (iroh-blobs, iroh-gossip and iroh-docs behind one `Router`)
- Processes `SyncCommand`s from the rest of the app and emits `SyncEvent`s
- Hands inbound replicated rows to T3 as `DbCommand::ApplyReplicatedRow`; T3 stays the single database writer

**T5 — Replication Bridge**
- Forwards the database thread's replication events to T4 as `SyncCommand::WriteRowEntry`
- Never blocks: it uses `try_send` and counts dropped events when T4 falls behind

**T6 — API Gateway**
- Its own multi-thread Tokio runtime running axum on `127.0.0.1:8765` (REST, `/ws`, `/mcp`)
- Reaches the database through `ApiCommand` → `DbCommand` and matches replies by correlation

**T7 — Plugin Host**
- The `fe-plugin` thread with its own Tokio runtime, started by the desktop app's `PluginHostPlugin`
- Hosts Rhai and WASM plugins behind a scoped, role-checked bridge to the database; no plugin package loads automatically

### Why This Design?

- **No Tokio on T1**: Bevy uses smol internally. Mixing Tokio and smol on the same thread causes subtle bugs and panics.
- **Isolation**: Each worker thread owns its own Tokio runtime. No nested `block_on()` calls, no runtime conflicts.
- **Backpressure**: The core channels are bounded at 256. If a thread falls behind, senders either block or, on hot paths such as replication events and inbound rows, use `try_send` and count what they drop — memory never grows without bound.
- **Type safety**: All cross-thread messages are strongly-typed enums. No serialization overhead, no deserialization bugs.

### Data Flow Summary

```
User Input → Bevy System → Command enum → Channel → Worker Thread
Worker Thread → Result enum → Channel → Drain System → Bevy ECS State
```

> See [docs/diagrams.md](diagrams.md) for full visual diagrams of all flows.

---

## 4. Crate-by-Crate Reference

### `fractalengine` (binary)

The entry point. `main.rs` (~1000 lines) wires the whole application:
1. Resolve the packaged layout (`assets/` beside the binary means `data/` beside the binary) and install the panic log
2. Default `SURREAL_DATASTORE_SYNC_DATA` to `every` and initialize `tracing_subscriber` with the `RUST_LOG` env filter
3. Load the persisted settings, create `ChannelHandles` (the core channel pairs) and spawn T2 (network)
4. Open the blob store, load the node identity (OS keychain, then a key file, then a temporary identity), then spawn T3 (database), T4 (sync) and T5 (replication bridge)
5. Build the Bevy app with `BevyHandles`, install the plugin host (T7) and spawn T6 (API gateway)
6. Add the UI, renderer, terrain and WebView plugins and call `app.run()` (blocks until exit)

### `fe-runtime`

The Bevy application core and the shared message types:
- `app.rs` — `build_app()` constructs the Bevy `App` with all plugins, resources, and systems, including the systems that drain the channels
- `channels.rs` — `ChannelHandles` (4 core channel pairs, 8 endpoints) and `ApiChannels`
- `messages.rs` — `NetworkCommand`, `NetworkEvent`, `DbCommand`, `DbResult`, `ApiCommand` enums
- `screenshot.rs`, `vision.rs`, `input.rs` — the API-to-Bevy bridges for agent capture, camera read-back and input (see `fe-runtime/src/AGENTS.md`)

### `fe-identity`

Cryptographic identity:
- `keypair.rs` — `NodeKeypair` wraps ed25519-dalek `SigningKey`/`VerifyingKey`
- `jwt.rs` — `FractalClaims` struct, `mint_session_token()`, `verify_session_token()` (300s max lifetime)
- `api_token.rs` — API tokens (`ApiClaims`) for REST, WebSocket and MCP
- `did_key.rs` — W3C DID:key generation from ed25519 public key
- `keychain.rs` — OS keychain read/write via `keyring` crate, and the tiered `load_or_generate_keypair_durable`
- `secret_store/` — secret backends (OS keystore, environment for the relay, run-local)
- `resource.rs` — `NodeIdentity` Bevy resource

### `fe-database`

SurrealDB persistence:
- `lib.rs` — `spawn_db_thread_shared()` initializes SurrealDB and enters the command loop
- `schema.rs` — one `define_table!` call per table (38 tables)
- `rbac.rs`, `role_manager.rs`, `role_level.rs` — role resolution and checks
- `handlers/` — one handler module per family of `DbCommand`s
- `queries.rs` — Query builder functions
- `op_log.rs` — Hybrid-logical-clock and the append-only operation-log writer
- `admin.rs` — Admin utilities (clear, dump, reset)
- `types.rs` — `PetalId`, `NodeId`, `RoleId`, `OpLogEntry`
- `session_cache.rs` — Session cache (60-second TTL) + `revoke_session` (absorbed from fe-auth 2026-07-17)
- `canon_log/` — the canonical (signed) operation log; signed operations belong here, not in `op_log` (see [SECURITY.md](../SECURITY.md))

### `fe-network`

libp2p networking (T2). The iroh transport itself lives in `fe-sync`:
- `lib.rs` — `spawn_network_thread()` creates Tokio runtime and command loop
- `swarm.rs` — builds the libp2p Swarm (Kademlia over QUIC)
- `discovery.rs` — Kademlia bootstrap
- `gossip.rs`, `iroh_blobs.rs`, `iroh_docs.rs` — placeholders: a signed-message verification helper and stubs that only log

### `fe-renderer`

Asset pipeline and camera:
- `addressing.rs` — BLAKE3 content addressing
- `ingester.rs` — `AssetIngester` trait + `GltfIngester`
- `loader.rs` — Bevy GLTF/GLB loader integration
- `camera.rs` — `OrbitCameraController`
- `dead_reckoning.rs` — Transform prediction for bandwidth reduction

### `fe-webview`

Embedded browser:
- `plugin.rs` — `WebViewPlugin` Bevy plugin
- `backend.rs` — the `WebViewBackend` trait; `backends/` holds the Tauri (wry, default), Servo (feature-gated) and stub implementations
- `ipc.rs` — `BrowserCommand`/`BrowserEvent` typed IPC
- `overlay.rs` — the `BrowserSurface` trait and overlay configuration
- `petal_portal.rs` — selection-driven open and close of the portal
- `security.rs` — URL policy (http/https only; loopback and private ranges blocked), shared with `fe-runtime`

### `fe-sync`

The P2P stack and replication (T4):
- `sync_thread.rs` — owns the iroh endpoint and processes `SyncCommand`s
- `docs_engine.rs` — `DocsStack`: iroh-blobs, iroh-gossip and iroh-docs behind one `Router`
- `endpoint.rs`, `relay_config.rs` — the iroh endpoint wrapper; relay mode and health
- `replicator.rs` — `VerseReplicator` implementations (iroh-docs, an in-memory mock for offline and tests)
- `reconciliation.rs` — `reconcile_petal`, the older petal-level delta apply through the tombstone-honoring merge (nothing calls it yet; live inbound rows arrive as `DbCommand::ApplyReplicatedRow`)
- `sharding.rs`, `placement.rs`, `distributed_query.rs` — the timeseries shard fabric and distributed queries
- `blob_store.rs`, `blob_transport.rs`, `tileset_transfer.rs` — the content-addressed blob store and `.hexon` tileset transfer
- `replication.rs`, `cache.rs`, `offline.rs` — scaffolding (a config struct, a stub store, an asset-cache sketch and an offline-load stub) that no binary uses yet

### `fe-ui`

Editor interface:
- `plugin.rs` — `GardenerConsolePlugin` Bevy plugin
- `navigation_manager.rs`, `verse_manager/`, `node_manager/` — the three manager resources (active cursor, content tree, selection)
- `ui_shell/` — top bar, left and right sidebars, modal layer
- `panels/`, `dialogs/` — panels and floating windows (Inspector, Options, Settings, Maps, the Access tab)
- `settings.rs` — persisted application settings (`settings.ron`)
- `role_chip.rs` — Role badge display component

---

## 5. Threading Model

### Rules (Non-Negotiable)

| Rule | Rationale |
|---|---|
| No `tokio::runtime` on T1 | Bevy uses smol; Tokio conflicts cause panics |
| No `block_on()` in Bevy systems | Blocks the ECS scheduler; use channels instead |
| No nested Tokio runtimes | Each worker thread owns one; nesting panics |
| All cross-thread comm via typed channels | Type safety, no serialization, bounded backpressure |
| Async work from T1 via `AsyncComputeTaskPool` | Bevy's built-in task pool for I/O-bound work |

### Channel Handles

`ChannelHandles::new()` creates 4 channel pairs (8 endpoints):

| Channel | Direction | Type |
|---|---|---|
| `net_cmd` | T1 → T2 | `NetworkCommand` |
| `net_evt` | T2 → T1 | `NetworkEvent` |
| `db_cmd` | T1 → T3 | `DbCommand` |
| `db_res` | T3 → T1 | `DbResult` |

All four are `crossbeam::channel::bounded(256)` (`CHANNEL_BUFFER`). The API, sync and replication bridges use their own bounded channels.

### Event Draining

Bevy systems `drain_network_events()` and `drain_db_results()` run every frame. They call `try_recv()` in a loop to consume all pending messages, converting them into Bevy messages (events) or ECS state changes.

---

## 6. Message Types & Channel Protocol

### NetworkCommand (T1 → T2)

| Variant | Description |
|---|---|
| `Ping` | Health check |
| `Shutdown` | Graceful shutdown request |

### NetworkEvent (T2 → T1)

| Variant | Description |
|---|---|
| `Pong` | Response to Ping |
| `Started` | Network thread initialized |
| `Stopped` | Network thread shutting down |

### DbCommand (T1 → T3)

`DbCommand` is the database thread's whole command surface: hundreds of lines of variants in `fe-runtime/src/messages.rs`, sent by the UI, the API gateway and the sync thread. A few representative variants:

| Variant | Description |
|---|---|
| `Ping` / `Shutdown` | Health check / graceful shutdown request |
| `CreateVerse`, `CreateFractal`, `CreatePetal`, `CreateNode` | Create hierarchy entries |
| `ImportGltf` | Import a GLTF/GLB asset |
| `LoadHierarchy` | Load the full verse/fractal/petal/node tree |
| `AssignRole` / `RevokeRole` | Change a peer's role at a scope |
| `GetNodeProperties` | Read a node's custom properties |
| `ScopedQuery` | Run a user-authored read-only SELECT from the desktop (Query tab, path listing) |
| `RawQuery` | Run a trusted, pre-validated SELECT (built by the API gateway's query guard) |
| `SetPetalTerrain` | Bind a terrain config to a petal |
| `ApplyReplicatedRow` | Apply an inbound replicated row (sent by the sync thread) |

### DbResult (T3 → T1)

| Variant | Description |
|---|---|
| `Pong` | Response to Ping |
| `Started` | Database initialized and schema applied |
| `Stopped` | Database thread shutting down |
| `Error(String)` | Operation failed with reason |
| `VerseCreated`, `NodeCreated`, `GltfImported`, `HierarchyLoaded`, `NodePropertiesLoaded`, `QueryResult`, `PetalTerrainLoaded`, `ReplicatedRowApplied` | Replies and notifications for the commands above |

> `DbResult`s also arrive unsolicited (for example after an inbound replicated row), so API callers match replies by reply family and correlation id, never by arrival order. The enums grow with every feature; see [Adding a New Message Type](#adding-a-new-message-type).

---

## 7. Identity & Cryptography

### Key Generation

On first launch, `NodeKeypair::generate()` creates an Ed25519 keypair:
- Private key stored in the OS keychain via the `keyring` crate; if the keychain is unavailable it falls back to a key file under the per-user data directory, and only if both fail does the app run with a temporary identity (and show a banner)
- Public key used to derive a W3C DID:key identifier: `did:key:z6Mk<multibase_pub>`
- The DID is the node's canonical identity across the verses it joins

### JWT Sessions

- `mint_session_token()` / `verify_session_token()` in `fe-identity/src/jwt.rs`, signed and verified with Ed25519 (EdDSA)
- Claims: `sub` (DID), `petal_id`, `role_id`, `iat`, `exp` (max 300 seconds)
- API tokens for REST, WebSocket and MCP are separate (`ApiClaims` in `fe-identity/src/api_token.rs`): a scope and a maximum role bound what a token can reach
- Raw Ed25519 signature checks (share URLs, invitations, delegation proofs) use `verify_strict()` — never `verify()` (prevents weak-key forgery)

### Security Invariants

- Private keys never exposed in UI or logs
- `ed25519-dalek` `legacy_compatibility` feature never enabled
- Inbound replicated rows are attributed to their signed iroh-docs author and judged by deny-by-default RBAC on the database thread before they are applied
- Role revocations replicate as `revoked` role statements, and peers re-judge the demoted author's rows (`fe-sync/src/AGENTS.md` §rejudge-on-demotion)
- The `op_log` signature field is still a placeholder on the legacy write paths; do not rely on it for integrity (see [SECURITY.md](../SECURITY.md))

---

## 8. Database Layer

### SurrealDB Configuration

- **Version**: 3.0 (embedded, in-process)
- **Backend**: SurrealKV (pure Rust — no C++ deps, no RocksDB)
- **Crash safety**: `SURREAL_DATASTORE_SYNC_DATA` (the SurrealKV fsync cadence) defaults to `every`; both binaries set it when it is unset. `true` is not a valid value and makes startup fail
- **Namespace**: `fractalengine`
- **Database**: `fractalengine`
- **Data directory**: `./data/fractalengine.db`

### Schema

38 tables are defined in `fe-database/src/schema.rs`, one `define_table!` call each. The core ones:

| Table | Purpose | Key Fields |
|---|---|---|
| `verse` | Top-level container and P2P namespace | verse_id, name, created_by, namespace_id, default_access |
| `fractal` | Groups petals under a verse | fractal_id, verse_id, owner_did, name |
| `petal` | 3D worlds | petal_id, name, fractal_id, terrain, render_distance, visibility_groups |
| `node` | Objects in a petal | node_id, petal_id, asset_id, position (GeoJSON point), elevation, properties, tombstone |
| `asset` | Binary asset metadata | asset_id, content_hash, size_bytes, owner_did |
| `role` | Materialized role cache | peer_did, scope, role |
| `role_grant`, `role_statement_pending` | Admitted and deferred per-grantor role statements | statement_key, scope, peer_did, grantor_did, role, revoked, hlc |
| `op_log` | Append-only mutation log | lamport_clock, hlc_timestamp, node_id, op_type, payload, sig |

The rest cover membership (`verse_member`), IoT readings and device payloads (`iot_reading`, `device_format`, `device_payload`), property schemas (`field_def`), the hexon crate registry, the canonical log, and the older `room` and `model` tables.

### RBAC Model

- Roles are **Owner > Manager > Editor > Viewer > None**; `public` parses as Viewer
- Roles are assigned per scope (`VERSE#id-FRACTAL#id-PETAL#id`); a peer without a grant at a scope inherits from the enclosing one, and an Owner pin can carve a petal out of a wider Manager's authority
- Roles replicate as per-grantor statements; the `role` table is the materialized cache (see the README's Peer-to-Peer Sync section)
- Enforced on the database thread through the fe-policy engine (deny-by-default) — never in Bevy systems
- Session cache with 60-second TTL

### Op-Log

Every mutation writes an append-only entry to `op_log`:
- `lamport_clock` — the packed hybrid-logical-clock value that orders entries across peers; `hlc_timestamp` carries its formatted form
- `node_id` — author's DID
- `op_type` — the operation kind (an `OpType` variant such as `SetPetalTerrain`)
- `payload` — JSON mutation data
- `sig` — reserved Ed25519 signature field; the legacy write paths store a placeholder (all-zero) value, so do not rely on it for integrity (see [SECURITY.md](../SECURITY.md))

---

## 9. Networking Layer

### Discovery

| Method | Scope | Purpose |
|---|---|---|
| Kademlia DHT (libp2p, T2) | WAN | Peer discovery; bootstrapped at startup |
| Bootstrap addresses and join tickets | Any | Dial a known peer directly (`FE_SYNC_BOOTSTRAP`, invitation tickets) |
| iroh relay | WAN (fallback) | NAT traversal when direct connection fails (`FE_SYNC_RELAY`) |

There is no mDNS discovery. For a LAN-only swarm, set `FE_SYNC_RELAY=disabled` and dial peers with bootstrap addresses.

### Transport

- **QUIC** — UDP-based, multiplexed streams with built-in encryption, used by both the libp2p swarm and iroh; there is no separate Noise or Yamux layer
- iroh connections use explicit BBRv3 congestion control

### Data Distribution

| Layer | Protocol | Data Type | Pattern |
|---|---|---|---|
| iroh-blobs | Content-addressed | GLTF/GLB assets, scene bytes, `.hexon` tileset bundles | Request by BLAKE3 hash, verified delivery |
| iroh-gossip | Epidemic broadcast | Verse topic traffic (live writes, distributed queries) | HyParView + PlumTree |
| iroh-docs | CRDT sync | Verse rows (hierarchy, nodes, roles, readings) | Range reconciliation, delta-only |

---

## 10. Asset Pipeline

### Supported Formats

**GLTF/GLB only.** No FBX, OBJ, or other formats. Operators convert using Blender (free, open-source).

Always embed textures in GLB binary to avoid Bevy hot-reload bug #18267.

KTX2 and Basis textures (`KHR_texture_basisu`) are refused, from peers and from local imports, until a Bevy release picks up an upstream decompression fix; convert such textures to PNG or JPEG first (see [SECURITY.md](../SECURITY.md)).

### Ingestion Flow

1. A GLB arrives: the editor's import, `POST /api/v1/petals/{petal_id}/assets` (multipart) or MCP `upload_asset`
2. Validate the container and size (max 256 MiB over REST, 11 MiB decoded over MCP) and the per-owner upload quota (2 GiB by default)
3. Compute BLAKE3 hash — this becomes the canonical content id
4. Store the bytes in the content-addressed blob store (`data/blobs`)
5. Record an `asset` row (content hash, size, owner) and place a node that references it
6. Peers fetch the bytes by hash over iroh-blobs when the node row replicates, after the provider's admission check

### Content Addressing

Every asset is identified by its BLAKE3 hash. This provides:
- **Deduplication** — identical assets stored once
- **Integrity** — received bytes verified against requested hash
- **Cache coherence** — same hash = same content, always

### Dead-Reckoning

Moving entities use `DeadReckoningPredictor`:
- Predict position from velocity + acceleration each frame
- Send network updates only when prediction error exceeds threshold
- Reduces bandwidth by 60–90%

---

## 11. WebView System

### Architecture

FractalEngine embeds a browser through the `WebViewBackend` trait (`fe-webview/src/backend.rs`), chosen at compile time:
- **Tauri** (default, `backend-tauri`): a wry webview (wry is maintained by the Tauri organization). On Windows it is a borderless popup window owned by the Bevy window, because a child webview would be painted over by the renderer's swap chain
- **Servo** (feature-gated `backend-servo`) takes priority when enabled
- A stub backend when neither is compiled in

The UI writes the portal panel's rectangle every frame and the backend follows it; repositioning is skipped when the geometry has not changed.

### Browser Interaction Model

Selecting a node that carries a portal URL opens the portal; deselecting, `Esc` or despawning the node closes it. Editors and above set the URL in the Inspector's **Portal URL** field and save it with **Save**. The portal has two tabs:
1. **External URL tab** — admin-configured URL (product page, web app, video)
2. **Config tab** — model properties + per-setting role mapping

### Security

- **URL policy**: only http and https; loopback (localhost, 127.0.0.1) and private or link-local ranges, including all RFC 1918 ranges, are blocked unconditionally — at command dispatch and again in the webview's navigation handler, which catches redirects
- **CSP headers**: Content Security Policy enforced on all loaded pages
- **Trust bar**: non-dismissible "External Website" indicator on all overlays
- **Typed IPC only**: no raw `eval()` — all JS-Rust calls go through `BrowserCommand`/`BrowserEvent` enums
- See [docs/webview-threat-model.md](webview-threat-model.md) for the full threat model

---

## 12. Sync & Offline

### Replication Strategy

When you open a Verse, its replica (an iroh-docs document with its own gossip topic) syncs:
1. The replica subscribes to the verse's gossip topic and dials bootstrap peers
2. iroh-docs performs **range reconciliation** (delta only, not full state)
3. Each missing entry is applied on the database thread through the deny-by-default admission gate; the replica's current entries are replayed through the same path on every open
4. Assets and other content are fetched by BLAKE3 hash via iroh-blobs and stored locally

### Local Stores

- The replica store lives under `FE_P2P_DIR` (default `data/p2p`) and persists across restarts
- Blobs live in the content-addressed blob store (`data/blobs`, override with `FE_BLOB_DIR`)
- Everything a peer applies is written to the local SurrealDB, so content you have already received stays available without a network connection

### Cache Management

The cache-eviction knobs in `fe-sync` (`ReplicationConfig`: 2 GB, 7 days), the `AssetCache` and the offline-load helpers exist as scaffolding, but nothing in the desktop app or the relay calls them yet, so there is no visited-content cache limit, no eviction after 7 days of inactivity, and no "stale" marker on offline content today; the status bar shows only Online or Offline and the peer count. Unreferenced uploaded assets are reclaimed by the asset GC instead (`FE_ASSET_GC_INTERVAL_SECS`, `FE_ASSET_GC_GRACE_SECS`). Separately, terrain tile bytes live in a byte-bounded in-memory LRU and nodes beyond the render distance are despawned; neither is a visited-content cache.

### Offline Mode

Local edits are written to the local database and replica first, so you can keep working with no peers online. They reach other peers when a gossip neighbor appears or the replica is next opened, because set reconciliation runs on open and on a new neighbor.

If the iroh endpoint cannot bind, the store migration fails or the P2P stack cannot spawn, the sync thread degrades to an in-memory mock replicator and the app keeps running (loudly logged, never a crash).

---

## 13. Security Model

### Principle: Defense in Depth

Security is enforced at multiple layers:

| Layer | Mechanism |
|---|---|
| **Identity** | Ed25519 keypair + `verify_strict()` for raw signature checks |
| **Authentication** | JWT sessions (300s max, 60s cache TTL); scoped API tokens |
| **Authorization** | fe-policy engine on the database thread, deny-by-default (not Bevy systems) |
| **Transport** | QUIC (libp2p swarm and iroh) |
| **Data integrity** | BLAKE3 content addressing; iroh-docs entries attributed to their signed author (`op_log` signatures are placeholders) |
| **Replication** | Inbound rows judged on the database thread by the role their author held; role statements converge in any arrival order |
| **WebView** | URL policy + CSP + trust bar + typed IPC |
| **Revocation** | `revoked` role statements replicate; peers re-judge the demoted author's rows |

### Security Documentation

| Document | Purpose |
|---|---|
| [SECURITY.md](../SECURITY.md) | Reporting a vulnerability and known limitations |
| [security-checklist.md](security-checklist.md) | Pre-launch audit checklist |
| [webview-threat-model.md](webview-threat-model.md) | wry WebView threat analysis |
| [unwrap-audit.md](unwrap-audit.md) | `unwrap()`/`expect()` inventory |

---

## 14. Development Workflow

### Tests Ship With the Change

New behavior ships with tests that pin it; there is no numeric coverage gate (see `conductor/workflow.md`). Tests model real flows through public seams, and every test must be reachable by a batch filter (see [Testing Strategy](#15-testing-strategy)).

### Commit Format

```
<type>(<scope>): <description>
```

Types: `feat`, `fix`, `docs`, `style`, `refactor`, `test`, `chore`, `conductor`

Scope: the crate or area touched, for example `fe-runtime`, `fe-sync`, `db`, `api`, `p2p`, `qa`

### Quality Gates

Before any task is complete:

- [ ] `cargo check -p <touched crates>` passes (the pre-commit gate)
- [ ] The batches covering your change pass (`tools\qa\test_batches.ps1 -Batches <ids>`)
- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy -- -D warnings` passes
- [ ] No `unwrap()`/`expect()` in production paths
- [ ] No `block_on()` in Bevy systems
- [ ] RBAC checks only on the database thread (fe-policy)

---

## 15. Testing Strategy

### Unit Tests

- Every module has `#[cfg(test)]` blocks
- Use `tokio::test` for async code in `fe-network` and `fe-database`
- Mock external peers using in-process channels (no live network required)
- Test success AND failure paths for all auth/crypto/RBAC functions

### Integration Tests

- Located in `tests/` at crate root
- Full session handshake flow (connect → JWT → role assign → verify → revoke)
- RBAC enforcement on the database thread
- WebView IPC command dispatch
- Cross-crate P2P flows run in `fe-test-harness` (18 scenarios over real loopback iroh: `cargo run -p fractalengine-test-harness`)

### Security Tests

- Tampered message rejection for all signature verification
- WebView denylist coverage (localhost, 127.0.0.1, all RFC 1918 ranges)
- JWT expiry and revocation with synthetic clock advancement

### Running Tests

There is no numeric coverage target. Tests are grouped into usability-region batches (R1–R11, Harness, Misc, Slow) in [conductor/test-batches.md](../conductor/test-batches.md); each batch is a list of plain `cargo test -p <crate> ...` commands (a name filter or a `--test` file).

```powershell
tools\qa\test_batches.ps1 -Batches R1,R8 -ListOnly   # preview the cargo commands
tools\qa\test_batches.ps1 -Batches R1,R8             # run them, one at a time
```

Run tests on purpose, not after every edit: the pre-commit gate is `cargo check -p <crate>`. `cargo test --workspace` is retired because it links every test binary at once (one full sweep left 258 GB of test executables and debug symbols on disk); `-Batches All` runs the same tests one chunk at a time. See the "Test execution policy" in the root [AGENTS.md](../AGENTS.md).

---

## 16. Extending FractalEngine

### Adding a New Message Type

1. Add variant to enum in `fe-runtime/src/messages.rs`
2. Handle the new variant in the receiving thread's command loop
3. Add drain logic in `fe-runtime/src/app.rs`
4. Write tests for the new message flow

### Adding a New Database Table

1. Add a `define_table!` call in `fe-database/src/schema.rs` (syntax: `fe-database/src/AGENTS.md` §schema-macro)
2. Add type definitions in `fe-database/src/types.rs`
3. Add query functions in `fe-database/src/queries.rs`, or a handler under `fe-database/src/handlers/`
4. Authorize reads and writes in the handler through the fe-policy engine; tables carry no `PERMISSIONS` clause
5. Write tests verifying RBAC at each role level

### Adding a New Asset Ingester

1. Implement the `AssetIngester` trait in `fe-renderer/src/ingester.rs`
2. Register the ingester in the asset pipeline
3. Update content addressing if the hash input changes
4. Write tests for validation, hashing, and rejection cases

### Future Extension Points

| Trait | Current Impl | Future Target |
|---|---|---|
| `BrowserSurface` | wry overlay | Bevy native compositor |
| `SceneSerializer` | JSON in SurrealDB | BSN scene format (Bevy 0.19+) |
| `AssetIngester` | `GltfIngester` | `SplatIngester` (Gaussian splat) |
| `VerseReplicator` | `IrohDocsReplicator` (with an in-memory mock and the fe-sim virtual transport) | SurrealDB live-query (2027) |
| DID identity | did:key from ed25519 | Full VC wallet integration |
| Extension packages | Rhai/WASM plugin host (`fe-plugin`) | Sealed `.hexon` packages carrying a Wasm component (planned: `extension_platform_20261107`, `portal_surface_20261107`) |

---

## 17. Troubleshooting

### Build Issues

**"can't find crate for ..."**
```bash
# Clean and rebuild
cargo clean && cargo build
```

**SurrealDB compilation errors**
Ensure you're using the `kv-surrealkv` feature, not RocksDB. Check `Cargo.toml`:
```toml
surrealdb = { version = "3.0", features = ["kv-surrealkv", "kv-mem"] }
```
If rustc overflows its stack or hits an internal error while compiling `surrealdb-core`, see the `RUST_MIN_STACK` workaround under Known Issues in [BUILDING.md](../BUILDING.md).

**Bevy rendering issues**
Bevy 0.18 requires a Vulkan or DirectX 12 compatible GPU. Check your graphics drivers.

### Runtime Issues

**"Cannot enter a runtime from within a runtime"**
You're calling `block_on()` inside a Bevy system. Use channel sends instead:
```rust
// WRONG: block_on() in Bevy system
// let result = tokio::runtime::Runtime::new().unwrap().block_on(async_fn());

// RIGHT: Send via channel, drain in next frame
net_cmd_tx.send(NetworkCommand::Ping).unwrap();
```

**Database not persisting**
Check that `SURREAL_DATASTORE_SYNC_DATA` has not been overridden to `never`; both binaries default it to `every` when it is unset. `true` is not a valid value and makes startup fail.

**Keychain access denied**
On Linux, ensure `gnome-keyring` or `kwallet` is running. On macOS, grant Keychain Access permission. Without a keychain the app falls back to a key file under your per-user data folder, and only if that also fails does it run with a temporary identity (a banner says so).

**Peers do not sync**
iroh 0.35 and iroh 1.x peers cannot talk to each other, so every peer needs a build from 2026-10-11 or later. The default relays are n0's public iroh relays, which are shared and rate-limited: set `FE_SYNC_RELAY` to your own relay, or to `disabled` for a LAN-only swarm, and use `FE_SYNC_BOOTSTRAP` or an invitation to dial a known peer.

### Logging

With `RUST_LOG` unset, only errors are logged.

```bash
# See everything
RUST_LOG=trace cargo run -p fractalengine

# Focus on specific crates
RUST_LOG=fe_database=debug,fe_network=trace cargo run -p fractalengine

# Common filters
RUST_LOG=fe_database=debug # Debug DB + session cache
RUST_LOG=fe_sync=trace    # Trace replication
RUST_LOG=fe_webview=debug # Debug WebView IPC
```
