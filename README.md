# FractalEngine

[![Build Artifacts](https://github.com/Escherbridge/fractalengine/actions/workflows/build-artifacts.yml/badge.svg)](https://github.com/Escherbridge/fractalengine/actions/workflows/build-artifacts.yml)

A **spatial analytics engine**. FractalEngine ingests real-world geodata — maps,
terrain, GPX paths, IoT readings — into an embedded spatial database, and its
headline capability is **BI egress**: copy a SQL string or API URL out of the
app and paste it into PowerBI, a spreadsheet, or DuckDB. No connector SDK, no
export wizard — your reporting tool talks straight to your data.

Under the analytics layer sits a **peer-to-peer 3D digital-twin substrate** — the
project's key differentiator: a Bevy-based editor where you build 3D spaces, place
models, draw paths over real terrain, and share worlds with peers **without a
central server**. Where other spatial analytics tools assume a hosted backend,
FractalEngine's data layer replicates peer-to-peer.

**Single binary. Local-first. Your data stays in your database.**

---

## Status: alpha

Pre-1.0, under active development. What that means concretely:

**Works today**

- 3D viewer/editor — Bevy 0.18 scene editor with egui panels, orbit camera, gizmos, node inspector
- Entity hierarchy (Verse > Fractal > Petal > Node) persisted in embedded SurrealDB (SurrealKV)
- GLTF/GLB asset import, content-addressed via BLAKE3
- Maps: per-petal terrain tiles with real-world scale metadata and a scale-bar HUD; maps package and install as `.hexon` files
- GPX import and path editing — pen tool with curves, vertex/segment selection, repeated-model stamping along paths
- HTTP/WS API gateway on `127.0.0.1:8765` — REST, live scene subscriptions, MCP tools — plus a headless relay binary (Docker image available)
- JWT auth, hierarchical RBAC, deny-by-default policy engine
- Plugin system — Rhai and WASM sandboxes against the stable `fe-sdk` API, with UI slots

**In progress**

- **BI egress** (the headline): GeoParquet/CSV export endpoints, signed share URLs, and an in-app "Copy for BI" card — core landed; end-to-end verification and docs remain
- Measurement tools (tape/area/bearing) on top of the landed real-world-scale plumbing
- IoT spatial reporting — ingestion and time-series queries landed; reading-shaped export remains
- Release pipeline — the 8-target build matrix exists; the first tag-triggered release has not yet run
- P2P sync — petal replication works over iroh; parts of verse-level replication are still mock-backed

**Planned**

- Road/path builder input layer, map-authoritative scale everywhere, portable petal snapshots, an expanded MCP vocabulary for AI scene construction, iroh 1.0 upgrade

**Known limitation:** op-log entries and gossip payloads carry ed25519
signature *fields*, but signing is not yet implemented — current entries hold
placeholder signatures. Do not rely on op-log integrity guarantees yet.

---

## Quick Start

### Prerequisites

| Requirement | Version | Install |
|---|---|---|
| Rust (stable) | 1.83+ | `rustup toolchain install stable` |
| rustfmt | latest | `rustup component add rustfmt` |
| clippy | latest | `rustup component add clippy` |

Platform-specific system packages (WebKitGTK on Linux, VS Build Tools on
Windows, Xcode CLT on macOS) are covered in [BUILDING.md](BUILDING.md), along
with a known `RUST_MIN_STACK` workaround for compiling `surrealdb-core`.

### Build & Run

```bash
# Release build (GUI binary)
cargo build --release

# Run
cargo run

# Debug logging
RUST_LOG=debug cargo run

# Headless relay (no GPU/windowing/keychain)
cargo build --release -p fractalengine-relay
```

### Test / Lint / Format

```bash
# The pre-commit trio
cargo fmt && cargo clippy -- -D warnings && cargo test

# Single crate
cargo test -p fe-identity
```

---

## Environment Variables

| Variable | Required | Default | Description |
|---|---|---|---|
| `RUST_LOG` | No | `info` | Log level filter (`trace`, `debug`, `info`, `warn`, `error`) |
| `SURREAL_SYNC_DATA` | Yes (set at runtime) | `false` | **Must be `true`** for crash-safe database writes |

---

## Architecture Overview

FractalEngine runs as a single native desktop application with a
**seven-thread topology**:

```
┌─────────────────────────────────────────────────────┐
│  T1 — Main Thread (Bevy ECS + wry WebView)          │
│  Renders 3D world, processes input, drains events   │
└──────────────┬────────────────────┬─────────────────┘
               │  crossbeam (256)   │  crossbeam (256)
┌──────────────▼──────────┐  ┌─────▼──────────────────┐
│  T2 — Network Thread    │  │  T3 — Database Thread   │
│  libp2p + iroh          │  │  SurrealDB (SurrealKV)  │
│  Dedicated Tokio runtime│  │  Dedicated Tokio runtime│
└─────────────────────────┘  └─────────────────────────┘
┌─────────────────────────┐  ┌─────────────────────────┐
│  T4 — Sync Thread       │  │  T5 — Replication Bridge│
│  Petal replication      │  │  DB ↔ Network bridge    │
└─────────────────────────┘  └─────────────────────────┘
┌─────────────────────────┐  ┌─────────────────────────┐
│  T6 — API Gateway       │  │  T7 — Plugin Host       │
│  axum HTTP/WS on :8765, │  │  Rhai + WASM sandboxes  │
│  MCP tools (multi-thread│  │  Dedicated Tokio runtime│
│  Tokio)                 │  │                         │
└─────────────────────────┘  └─────────────────────────┘
```

All cross-thread communication uses **typed crossbeam channels** — never raw
bytes, never shared mutable state.

> See [docs/diagrams.md](docs/diagrams.md) for full architecture diagrams
> including data flow, auth sequences, and component interactions.

---

## Entity Hierarchy

FractalEngine uses a botanical/fractal naming convention:

```
Verse                the top-level P2P namespace
  ├── VerseMember    peer membership records (invite-based)
  └── Fractal        groups Petals under a Verse
        └── Petal    a 3D world/space
              ├── Node     an interactive object (placed via right-click)
              │     └── Asset   GLTF/GLB model (content-addressed via BLAKE3)
              ├── Room     a zone within a Petal
              │     └── Model    a placed 3D object (legacy)
              └── Role     RBAC assignment (peer ↔ petal)
```

---

## Workspace Crates

22 crates; the load-bearing ones:

| Crate | Purpose | Thread |
|---|---|---|
| `fractalengine` | GUI binary — wires threads together | T1 |
| `fractalengine-relay` | Headless relay binary (`fe-relay`) | — |
| `fe-runtime` | Bevy ECS app, channel management, message types | T1 |
| `fe-network` | libp2p discovery + iroh data transport | T2 |
| `fe-database` | SurrealDB persistence, RBAC, op-log | T3 |
| `fe-identity` | Ed25519 keypair, JWT, DID:key, OS keychain | Shared |
| `fe-policy` | Deny-by-default policy engine (RBAC decisions) | Shared |
| `fe-renderer` | GLTF pipeline, content addressing, camera | T1 |
| `fe-webview` | wry browser overlay, typed IPC, URL security | T1 |
| `fe-sync` | Petal replication, caching, offline mode | T4 |
| `fe-ui` | egui panels, inspectors, managers | T1 |
| `fe-api` | axum HTTP/WS gateway, MCP tools, ApiClaims auth | T6 |
| `fe-query` | Query builder, GIS queries, GeoParquet export | T3/T6 |
| `fe-terrain` | Terrain tiles, GPX, mesh generation, IoT layers | T1 |
| `fe-format` / `fe-entity-store` | Canonical data formats + entity storage | Shared |
| `fe-hexon` / `fe-hexon-registry` | `.hexon` packaging, publishing, hosted registry | Shared |
| `fe-plugin` / `fe-sdk` / `fe-plugin-test` | Plugin engines, stable SDK, test kit | T7 |
| `fe-test-harness` | Multi-peer integration scenarios | Test |

---

## Key Dependencies

| Crate | Version | Role |
|---|---|---|
| `bevy` | 0.18 | ECS 3D engine |
| `surrealdb` | 3.0 | Embedded database (SurrealKV backend) |
| `libp2p` | 0.56 | Peer discovery (Kademlia DHT, mDNS, QUIC) |
| `iroh-blobs` | 0.35 | BLAKE3 content-addressed asset distribution |
| `iroh-gossip` | 0.35 | Epidemic broadcast for zone events |
| `iroh-docs` | 0.35 | CRDT key-value sync |
| `ed25519-dalek` | 2.2 | Ed25519 signatures |
| `jsonwebtoken` | 10.3 | JWT minting/verification |
| `wry` | 0.54 | Embedded WebView (Tauri org) |
| `bevy_egui` | 0.39 | In-game UI |
| `axum` | 0.8 | HTTP/WebSocket API gateway |

---

## Key Design Invariants

- **No Tokio on T1** — Bevy uses smol; use `AsyncComputeTaskPool` for async work
- **No `block_on()` in Bevy systems** — send commands via channels instead
- **RBAC enforced at the data layer** — policy engine + SurrealDB `PERMISSIONS`, never in Bevy systems
- **All assets content-addressed** — BLAKE3 hash is the canonical ID
- **All mutations logged** — append-only op-log with hybrid logical clock; per-op ed25519 signing is designed but **not yet implemented** (see Status)
- **WebView URL denylist** — blocks localhost, 127.0.0.1, RFC 1918 unconditionally
- **GLTF/GLB only** — no FBX/OBJ; operators convert via Blender

---

## API Gateway

The `fe-api` crate provides an axum-based HTTP/WebSocket API on port 8765.

### REST Endpoints

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/api/v1/health` | No | Liveness probe |
| GET | `/ready` | No | Readiness probe (DB ping) |
| GET | `/api/v1/hierarchy` | Bearer JWT | Full scene hierarchy snapshot |
| POST | `/api/v1/verses` | Bearer JWT | Create a verse |
| POST | `/api/v1/verses/:vid/fractals` | Bearer JWT | Create a fractal |
| POST | `/api/v1/verses/:vid/fractals/:fid/petals` | Bearer JWT | Create a petal |
| POST | `/api/v1/verses/:vid/fractals/:fid/petals/:pid/nodes` | Bearer JWT | Create a node |
| PATCH | `/api/v1/nodes/:id/transform` | Bearer JWT | Update node transform |
| GET | `/api/v1/nodes/:id/transform` | Bearer JWT | Read node transform |
| GET | `/api/v1/assets/:content_hash` | Bearer JWT | Content-addressed asset delivery (immutable caching) |

### WebSocket Protocol (`/ws`)

After connecting, the server sends `auth_required`. The client must respond with
an `auth` message containing a valid API token within 5 seconds.

**Thin client connection flow**:
1. Connect to `/ws`; server sends `auth_required`.
2. Client sends `auth { access_token }` (Bearer API token) → `auth_ok`.
3. Client sends `scene_subscribe { petal_id }` (scope-checked against the token).
4. Server responds with `scene_snapshot { petal_id, version, nodes }`.
5. On CUD mutations, server pushes `scene_delta { petal_id, version, changes }`
   where each change is a `SceneChange` (`node_added` / `node_removed` /
   `node_renamed` / `node_transform` / `property_changed`).

**Entity commands** (CUD over WS, editor role required):
Client sends `entity_command { request_id, command }` where `command` is one of
`create_node`, `delete_node`, `set_node_property`, `delete_node_property`
(tagged by `op`). Server replies with
`entity_command_result { request_id, ok, data?, error? }` echoing the request id;
the resulting `scene_delta` is broadcast to all scene subscribers.

**Transform streaming**: Subscribe to a petal channel, then receive real-time
`transform_update` messages as other clients modify node positions. Failed
persists are rolled back via `transform_rollback`.

---

## Documentation

| Document | Description |
|---|---|
| [BUILDING.md](BUILDING.md) | Per-platform build instructions and known issues |
| [docs/guide.md](docs/guide.md) | Comprehensive developer guide |
| [docs/diagrams.md](docs/diagrams.md) | Architecture diagrams index (Mermaid) |
| [docs/security-checklist.md](docs/security-checklist.md) | Security audit checklist |
| [docs/webview-threat-model.md](docs/webview-threat-model.md) | WebView threat model |
| [conductor/product.md](conductor/product.md) | Product vision and entity hierarchy |
| [conductor/tech-stack.md](conductor/tech-stack.md) | Technology decisions and constraints |
| [conductor/roadmap.md](conductor/roadmap.md) | Strategic roadmap + go-forward slate |
| [conductor/tracks.md](conductor/tracks.md) | Live implementation-track board |

---

## License

Licensed under the Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE)).

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
licensed as above, without any additional terms or conditions.

Note: the embedded SurrealDB engine is a dependency licensed separately under
the Business Source License 1.1.
