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

FractalEngine is a decentralized, peer-to-peer 3D digital twin platform. It ships as a single native binary that embeds:

- A **3D engine** (Bevy) for rendering interactive worlds
- An **embedded database** (SurrealDB) for persistence and RBAC
- A **P2P network stack** (libp2p + iroh) for peer discovery and data sync
- An **embedded browser** (wry) for in-world web content

No cloud server, no monthly subscription, no data lock-in. An operator runs a Node, hosts Petals (3D worlds), and invites peers to interact under role-based access control.

### Who is this for?

- **Small businesses** wanting self-hosted 3D collaborative spaces (showrooms, digital twins, meeting rooms)
- **Indie developers** building decentralized 3D experiences
- **Creative professionals** using Petals as interactive portfolios

### The Naming System

FractalEngine uses botanical/fractal metaphors:

| Entity | Description |
|---|---|
| **Fractal** | The entire P2P network of all connected Nodes |
| **Node** | One peer running the desktop app |
| **Petal** | A 3D world/space hosted by a Node |
| **Room** | A zone within a Petal |
| **Model** | A 3D object (GLTF/GLB) placed in a Room |
| **BrowserInteraction** | Tabbed embedded browser on a Model |

---

## 2. Getting Started

### Prerequisites

- **Rust 1.82+** (stable toolchain)
- **Git** for version control
- Platform-specific build deps for Bevy (see [Bevy setup guide](https://bevyengine.org/learn/quick-start/getting-started/setup/))

### First-Time Setup

```bash
# Clone the repository
git clone <repo-url>
cd fractalengine

# Install Rust stable toolchain
rustup toolchain install stable
rustup component add rustfmt clippy

# Install coverage tool
cargo install cargo-tarpaulin

# Build the entire workspace
cargo build
```

### Running

```bash
# Default run
cargo run

# With debug logging
RUST_LOG=debug cargo run

# With per-crate log filtering
RUST_LOG=fe_network=trace,fe_database=debug,fe_runtime=info cargo run

# Release mode
cargo run --release
```

On first launch, FractalEngine will:
1. Generate an Ed25519 keypair
2. Store the private key in your OS keychain (macOS Keychain, Windows Credential Manager, or Linux Secret Service)
3. Create a SurrealDB data directory at `./data/fractalengine.db`
4. Start the Bevy 3D window with the admin panel

### Build Variants

| Command | Use Case |
|---|---|
| `cargo build` | Debug build with dynamic linking (fast iteration) |
| `cargo build --release` | Optimized single binary for deployment |
| `cargo build -p fe-database` | Build only the database crate |
| `cargo build -p fe-network` | Build only the networking crate |
| `cargo test -p fe-identity` | Test only the identity crate |

---

## 3. Architecture Deep Dive

### The Three-Thread Model

FractalEngine's runtime is split across three dedicated threads, each with specific responsibilities:

**T1 — Main Thread (OS thread)**
- Bevy ECS scheduler (uses smol async executor, NOT Tokio)
- wry WebView event loop (injected into Bevy's winit loop)
- egui UI rendering
- Event drain systems that consume messages from T2 and T3
- Asset loading via Bevy's `AsyncComputeTaskPool` (T4-TN)

**T2 — Network Thread**
- Spawned as a dedicated OS thread with its own Tokio runtime
- Runs the libp2p Swarm (Kademlia DHT + mDNS + QUIC transport)
- Runs iroh endpoints (blobs, gossip, docs)
- Receives `NetworkCommand` from T1, sends `NetworkEvent` back

**T3 — Database Thread**
- Spawned as a dedicated OS thread with its own Tokio runtime
- Runs SurrealDB with SurrealKV backend
- Enforces all RBAC via `DEFINE TABLE ... PERMISSIONS`
- Receives `DbCommand` from T1, sends `DbResult` back

### Why This Design?

- **No Tokio on T1**: Bevy uses smol internally. Mixing Tokio and smol on the same thread causes subtle bugs and panics.
- **Isolation**: T2 and T3 each own their Tokio runtime. No nested `block_on()` calls, no runtime conflicts.
- **Backpressure**: All channels are bounded at 256. If a thread falls behind, the sender blocks — preventing unbounded memory growth.
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

The entry point. `main.rs` is ~23 lines:
1. Initialize `tracing_subscriber` with `RUST_LOG` env filter
2. Create `ChannelHandles` (all 8 channels)
3. Spawn T2 (network thread)
4. Spawn T3 (database thread)
5. Build the Bevy app with `BevyHandles`
6. Call `app.run()` (blocks forever)

### `fe-runtime`

The Bevy application core:
- `app.rs` — `build_app()` constructs the Bevy `App` with all plugins, resources, and systems
- `channels.rs` — `ChannelHandles` struct holding all 8 channel endpoints
- `messages.rs` — `NetworkCommand`, `NetworkEvent`, `DbCommand`, `DbResult` enums

### `fe-identity`

Cryptographic identity:
- `keypair.rs` — `NodeKeypair` wraps ed25519-dalek `SigningKey`/`VerifyingKey`
- `jwt.rs` — `FractalClaims` struct, `mint_jwt()`, `verify_jwt()` (300s max lifetime)
- `did_key.rs` — W3C DID:key generation from ed25519 public key
- `keychain.rs` — OS keychain read/write via `keyring` crate
- `resource.rs` — `NodeIdentity` Bevy resource

### `fe-database`

SurrealDB persistence:
- `lib.rs` — `spawn_db_thread()` initializes SurrealDB and enters command loop
- `schema.rs` — `DEFINE TABLE` statements for all 5 tables with PERMISSIONS
- `rbac.rs` — Role hierarchy enforcement (public → custom → admin)
- `queries.rs` — Query builder functions
- `op_log.rs` — Immutable mutation log with Lamport clock
- `admin.rs` — Admin utilities (clear, dump, reset)
- `types.rs` — `PetalId`, `NodeId`, `RoleId`, `OpLogEntry`

### `fe-network`

P2P networking:
- `lib.rs` — `spawn_network_thread()` creates Tokio runtime and command loop
- `discovery.rs` — Kademlia DHT + mDNS peer discovery
- `swarm.rs` — libp2p Swarm event handling
- `gossip.rs` — iroh-gossip epidemic broadcast
- `iroh_blobs.rs` — Content-addressed asset distribution
- `iroh_docs.rs` — CRDT key-value sync for Petal state

### `fe-auth`

Session management:
- `handshake.rs` — Peer authentication protocol
- `session.rs` — Session type definitions
- `cache.rs` — Session cache with 60-second TTL
- `revocation.rs` — Signed session revocation broadcast

### `fe-renderer`

Asset pipeline:
- `addressing.rs` — BLAKE3 content addressing
- `ingester.rs` — `AssetIngester` trait + `GltfIngester`
- `loader.rs` — Bevy GLTF/GLB loader integration
- `dead_reckoning.rs` — Transform prediction for bandwidth reduction

### `fe-webview`

Embedded browser:
- `plugin.rs` — `WebViewPlugin` Bevy plugin, `WryBrowserSurface`
- `ipc.rs` — `BrowserCommand`/`BrowserEvent` typed IPC
- `overlay.rs` — Window positioning via `Camera::world_to_viewport()`
- `security.rs` — URL denylist (localhost, RFC 1918)

### `fe-sync`

Replication:
- `replication.rs` — iroh-docs range reconciliation
- `cache.rs` — Local SurrealDB + asset caching
- `offline.rs` — Offline mode for visited Petals
- `reconciliation.rs` — Delta sync on reconnect

### `fe-ui`

Admin interface:
- `plugin.rs` — `UIPlugin` Bevy plugin
- `panels.rs` — Admin panels (node info, Petal browser, etc.)
- `role_chip.rs` — Role badge display component

---

## 5. Threading Model

### Rules (Non-Negotiable)

| Rule | Rationale |
|---|---|
| No `tokio::runtime` on T1 | Bevy uses smol; Tokio conflicts cause panics |
| No `block_on()` in Bevy systems | Blocks the ECS scheduler; use channels instead |
| No nested Tokio runtimes | T2 and T3 each own one; nesting panics |
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

All channels are `crossbeam::channel::bounded(256)`.

### Event Draining

Bevy systems `drain_network_events()` and `drain_db_results()` run every frame. They call `try_recv()` in a loop to consume all pending messages, converting them into Bevy events or ECS state changes.

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

| Variant | Description |
|---|---|
| `Ping` | Health check |
| `Shutdown` | Graceful shutdown request |

### DbResult (T3 → T1)

| Variant | Description |
|---|---|
| `Pong` | Response to Ping |
| `Started` | Database initialized and schema applied |
| `Stopped` | Database thread shutting down |
| `Error(String)` | Operation failed with reason |

> These are the v1 message types. As features are connected, these enums will grow to include `CreatePetal`, `QueryModels`, `PublishPetal`, etc.

---

## 7. Identity & Cryptography

### Key Generation

On first launch, `NodeKeypair::generate()` creates an Ed25519 keypair:
- Private key stored in OS keychain via the `keyring` crate
- Public key used to derive a W3C DID:key identifier: `did:key:z6Mk<multibase_pub>`
- The DID is the Node's canonical identity across the Fractal

### JWT Sessions

- Minted by the host Node when a peer authenticates
- Claims: `sub` (DID), `role`, `petal_id`, `exp` (max 300 seconds)
- Signed with the host's Ed25519 private key
- Verified with `verify_strict()` — never `verify()` (prevents weak-key forgery)

### Security Invariants

- Private keys never exposed in UI or logs
- `ed25519-dalek` `legacy_compatibility` feature never enabled
- All gossip payloads carry Ed25519 signatures; unsigned messages rejected at ingest
- Signed revocations broadcast within 5 seconds via iroh-gossip

---

## 8. Database Layer

### SurrealDB Configuration

- **Version**: 3.0 (embedded, in-process)
- **Backend**: SurrealKV (pure Rust — no C++ deps, no RocksDB)
- **Crash safety**: `SURREAL_SYNC_DATA=true` (mandatory, not the SurrealDB default)
- **Namespace**: `fractalengine`
- **Database**: `fractalengine`
- **Data directory**: `./data/fractalengine.db`

### Schema

5 tables defined in `fe-database/src/schema.rs`:

| Table | Purpose | Key Fields |
|---|---|---|
| `petal` | 3D worlds | petal_id, name, node_id, created_at |
| `room` | Zones within Petals | room_id, petal_id, name |
| `model` | 3D objects | model_id, room_id, asset_hash, position |
| `role` | Role assignments | role_id, node_id, petal_id, role_name |
| `op_log` | Immutable mutation log | lamport_clock, node_id, op_type, payload, sig |

### RBAC Model

- **public** — hardcoded floor, always available
- **custom roles** — admin-defined, named, scoped per Petal
- **admin** — hardcoded ceiling, full control
- Roles overridable at Room or Model level
- Enforced via SurrealDB `DEFINE TABLE ... PERMISSIONS` — never in Bevy systems
- Session cache with 60-second TTL and mandatory re-validation

### Op-Log

Every mutation writes an immutable entry to `op_log`:
- `lamport_clock` — deterministic ordering across Nodes
- `node_id` — author's DID
- `op_type` — CREATE, UPDATE, DELETE
- `payload` — JSON mutation data
- `sig` — Ed25519 signature over the payload

---

## 9. Networking Layer

### Discovery

| Method | Scope | Purpose |
|---|---|---|
| Kademlia DHT | WAN | Node discovery + Petal metadata publication |
| mDNS | LAN | Automatic local peer discovery |
| iroh relay | WAN (fallback) | NAT traversal when direct connection fails |

### Transport

- **QUIC** — UDP-based, multiplexed, 0-RTT connections
- **Noise** — Encryption layer
- **Yamux** — Stream multiplexing

### Data Distribution

| Layer | Protocol | Data Type | Pattern |
|---|---|---|---|
| iroh-blobs | Content-addressed | GLTF/GLB assets | Request by BLAKE3 hash, verified delivery |
| iroh-gossip | Epidemic broadcast | Zone events | HyParView + PlumTree, signed payloads |
| iroh-docs | CRDT sync | Petal world state | Range reconciliation, delta-only |

---

## 10. Asset Pipeline

### Supported Formats

**GLTF/GLB only.** No FBX, OBJ, or other formats. Operators convert using Blender (free, open-source).

Always embed textures in GLB binary to avoid Bevy hot-reload bug #18267.

### Ingestion Flow

1. Operator uploads a GLB file
2. Validate format (GLTF/GLB) and size (max 256 MB)
3. Compute BLAKE3 hash — this becomes the canonical asset ID
4. Store in local asset cache
5. Record in SurrealDB `model` table
6. Publish hash via iroh-blobs for P2P distribution

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

FractalEngine embeds a browser via wry (maintained by the Tauri organization):
- **V1**: Overlay window positioned per-frame via `Camera::world_to_viewport()`
- **V2** (future): Offscreen render composited as wgpu texture

### Browser Interaction Model

Each Model can have a `BrowserInteraction` with two tabs:
1. **External URL tab** — admin-configured URL (product page, web app, video)
2. **Config tab** — model properties + per-setting role mapping

### Security

- **URL denylist**: blocks localhost, 127.0.0.1, and all RFC 1918 private ranges unconditionally
- **CSP headers**: Content Security Policy enforced on all loaded pages
- **Trust bar**: non-dismissible "External Website" indicator on all overlays
- **Typed IPC only**: no raw `eval()` — all JS-Rust calls go through `BrowserCommand`/`BrowserEvent` enums
- See [docs/webview-threat-model.md](webview-threat-model.md) for the full threat model

---

## 12. Sync & Offline

### Replication Strategy

When a visitor connects to a Petal:
1. iroh-docs performs **range reconciliation** (delta only, not full state)
2. Missing entries transferred
3. Assets fetched by BLAKE3 hash via iroh-blobs
4. All cached locally

### Cache Management

- Per-visited-Petal namespace in local SurrealDB
- Assets cached by BLAKE3 hash
- **Default limit**: 2 GB
- **Eviction**: LRU after 7 days of inactivity
- Both limits are configurable per Node

### Persistence Tiers

Operators can set per-Petal:

| Tier | Behavior |
|---|---|
| `EPHEMERAL` | No local caching on visitors |
| `CACHED` | Cache on visit, evict via LRU |
| `REPLICATED` | Full local replica maintained |

### Offline Mode

Previously-visited Petals load from local cache without network connectivity. A "stale" indicator appears in the UI when offline.

---

## 13. Security Model

### Principle: Defense in Depth

Security is enforced at multiple layers:

| Layer | Mechanism |
|---|---|
| **Identity** | Ed25519 keypair + `verify_strict()` |
| **Authentication** | JWT sessions (300s max, 60s cache TTL) |
| **Authorization** | SurrealDB PERMISSIONS (not Bevy systems) |
| **Transport** | Noise encryption + QUIC |
| **Data integrity** | BLAKE3 content addressing + ed25519 signatures |
| **Gossip** | All payloads signed; unsigned rejected |
| **WebView** | URL denylist + CSP + trust bar + typed IPC |
| **Revocation** | Signed broadcast <5s via iroh-gossip |

### Security Documentation

| Document | Purpose |
|---|---|
| [security-checklist.md](security-checklist.md) | Pre-launch audit checklist |
| [webview-threat-model.md](webview-threat-model.md) | wry WebView threat analysis |
| [unwrap-audit.md](unwrap-audit.md) | `unwrap()`/`expect()` inventory |

---

## 14. Development Workflow

### TDD Cycle

FractalEngine follows strict Test-Driven Development:

1. **Red** — write a failing test that defines the expected behavior
2. **Green** — write the minimum code to make the test pass
3. **Refactor** — clean up with the safety net of passing tests

### Commit Format

```
<type>(<scope>): <description>
```

Types: `feat`, `fix`, `docs`, `style`, `refactor`, `test`, `chore`, `conductor`

Scopes: `fe-runtime`, `fe-identity`, `fe-database`, `fe-network`, `fe-renderer`, `fe-webview`, `fe-auth`, `fe-ui`, `fe-sync`

### Quality Gates

Before any task is complete:

- [ ] `cargo test` passes
- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy -- -D warnings` passes
- [ ] Coverage >80% for new code
- [ ] No `unwrap()`/`expect()` in production paths
- [ ] No `block_on()` in Bevy systems
- [ ] RBAC checks only in SurrealDB layer
- [ ] All gossip payloads signed

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
- RBAC enforcement at SurrealDB layer
- WebView IPC command dispatch

### Security Tests

- Tampered message rejection for all signature verification
- WebView denylist coverage (localhost, 127.0.0.1, all RFC 1918 ranges)
- JWT expiry and revocation with synthetic clock advancement

### Coverage

```bash
# Generate HTML report
cargo tarpaulin --out Html --output-dir coverage/

# Target: >80% for all modules
```

---

## 16. Extending FractalEngine

### Adding a New Message Type

1. Add variant to enum in `fe-runtime/src/messages.rs`
2. Handle the new variant in the receiving thread's command loop
3. Add drain logic in `fe-runtime/src/app.rs`
4. Write tests for the new message flow

### Adding a New Database Table

1. Add `DEFINE TABLE` statement in `fe-database/src/schema.rs`
2. Add type definitions in `fe-database/src/types.rs`
3. Add query functions in `fe-database/src/queries.rs`
4. Include `PERMISSIONS` clauses for RBAC enforcement
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
| `ReplicationStore` | iroh-docs + file cache | SurrealDB live-query (2027) |
| DID identity | did:key from ed25519 | Full VC wallet integration |

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
surrealdb = { version = "3.0", features = ["kv-surrealkv"] }
```

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
Ensure `SURREAL_SYNC_DATA=true` is set. This is NOT the SurrealDB default.

**Keychain access denied**
On Linux, ensure `gnome-keyring` or `kwallet` is running. On macOS, grant Keychain Access permission.

### Logging

```bash
# See everything
RUST_LOG=trace cargo run

# Focus on specific crates
RUST_LOG=fe_database=debug,fe_network=trace cargo run

# Common filters
RUST_LOG=fe_auth=debug    # Debug auth handshakes
RUST_LOG=fe_sync=trace    # Trace replication
RUST_LOG=fe_webview=debug # Debug WebView IPC
```
