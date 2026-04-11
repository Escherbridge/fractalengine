# FractalEngine

A decentralized, peer-to-peer 3D digital twin platform. Run a Node, host Petals (3D worlds), place interactive Models, and connect with peers — all without a central server.

**Single binary. No cloud. No subscription. You own everything.**

---

## Quick Start

### Prerequisites

| Requirement | Version | Install |
|---|---|---|
| Rust (stable) | 1.82+ | `rustup toolchain install stable` |
| rustfmt | latest | `rustup component add rustfmt` |
| clippy | latest | `rustup component add clippy` |
| cargo-tarpaulin | latest | `cargo install cargo-tarpaulin` |

### Build

```bash
# Debug build (faster compilation, includes dynamic linking)
cargo build

# Release build (optimized, single binary)
cargo build --release

# Build a specific crate only
cargo build -p fe-runtime
cargo build -p fe-database
cargo build -p fe-network
```

### Run

```bash
# Run with default logging
cargo run

# Run with debug logging
RUST_LOG=debug cargo run

# Run with per-crate log levels
RUST_LOG=fe_network=trace,fe_database=debug cargo run

# Run the release binary directly
./target/release/fractalengine
```

### Test

```bash
# Run all tests
cargo test

# Run tests for a specific crate
cargo test -p fe-identity
cargo test -p fe-auth
cargo test -p fe-database

# Run a specific test by name
cargo test -p fe-identity test_jwt_roundtrip

# Run tests with output
cargo test -- --nocapture
```

### Lint & Format

```bash
# Check formatting
cargo fmt --check

# Auto-format
cargo fmt

# Lint (strict — treats warnings as errors)
cargo clippy -- -D warnings
```

### Coverage

```bash
# Generate HTML coverage report
cargo tarpaulin --out Html --output-dir coverage/

# Console summary only
cargo tarpaulin
```

### Pre-Commit Check (run all three)

```bash
cargo fmt && cargo clippy -- -D warnings && cargo test
```

---

## Environment Variables

| Variable | Required | Default | Description |
|---|---|---|---|
| `RUST_LOG` | No | `info` | Log level filter (`trace`, `debug`, `info`, `warn`, `error`) |
| `SURREAL_SYNC_DATA` | Yes (set at runtime) | `false` | **Must be `true`** for crash-safe database writes |

---

## Architecture Overview

FractalEngine runs as a single native desktop application with a strict **three-thread topology**:

```
┌─────────────────────────────────────────────────────┐
│  T1 — Main Thread (Bevy ECS + wry WebView)         │
│  Renders 3D world, processes input, drains events   │
└──────────────┬────────────────────┬─────────────────┘
               │  crossbeam (256)   │  crossbeam (256)
┌──────────────▼──────────┐  ┌─────▼──────────────────┐
│  T2 — Network Thread    │  │  T3 — Database Thread   │
│  libp2p + iroh          │  │  SurrealDB (SurrealKV)  │
│  Dedicated Tokio runtime│  │  Dedicated Tokio runtime │
└─────────────────────────┘  └──────────────────────────┘
```

All cross-thread communication uses **typed crossbeam channels** — never raw bytes, never shared mutable state.

> See [docs/diagrams.md](docs/diagrams.md) for full architecture diagrams including data flow, auth sequences, and component interactions.

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

| Crate | Purpose | Thread |
|---|---|---|
| `fractalengine` | Binary entry point — wires threads together | T1 |
| `fe-runtime` | Bevy ECS app, channel management, message types | T1 |
| `fe-network` | libp2p discovery + iroh data transport | T2 |
| `fe-database` | SurrealDB persistence, RBAC, op-log | T3 |
| `fe-identity` | Ed25519 keypair, JWT, DID:key, OS keychain | Shared |
| `fe-auth` | Session handshake, cache, revocation | Shared |
| `fe-renderer` | GLTF pipeline, content addressing, dead-reckoning | T1 |
| `fe-webview` | wry browser overlay, typed IPC, URL security | T1 |
| `fe-sync` | Petal replication, caching, offline mode | T2/T3 |
| `fe-ui` | egui admin panels and role display | T1 |
| `fe-test-harness` | Integration test scenarios (multi-peer) | Test |

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
| `crossbeam` | 0.8 | Typed channel bridges |
| `blake3` | 1.x | Content addressing |

---

## Key Design Invariants

- **No Tokio on T1** — Bevy uses smol; use `AsyncComputeTaskPool` for async work
- **No `block_on()` in Bevy systems** — sends commands via channels instead
- **RBAC enforced at database layer** — SurrealDB `PERMISSIONS` clauses, never in Bevy systems
- **All assets content-addressed** — BLAKE3 hash is the canonical ID
- **All mutations logged** — immutable op-log with Lamport clock + ed25519 signature
- **All gossip payloads signed** — unsigned messages rejected at ingest
- **WebView URL denylist** — blocks localhost, 127.0.0.1, RFC 1918 unconditionally
- **GLTF/GLB only** — no FBX/OBJ; operators convert via Blender

---

## Documentation

| Document | Description |
|---|---|
| [docs/guide.md](docs/guide.md) | Comprehensive developer guide |
| [docs/diagrams.md](docs/diagrams.md) | Architecture diagrams index (Mermaid) |
| [docs/diagrams/](docs/diagrams/) | Individual diagram files |
| [docs/security-checklist.md](docs/security-checklist.md) | Security audit checklist |
| [docs/webview-threat-model.md](docs/webview-threat-model.md) | WebView threat model |
| [docs/unwrap-audit.md](docs/unwrap-audit.md) | `unwrap()`/`expect()` inventory |
| [conductor/product.md](conductor/product.md) | Product vision and entity hierarchy |
| [conductor/tech-stack.md](conductor/tech-stack.md) | Technology decisions and constraints |
| [conductor/workflow.md](conductor/workflow.md) | Development process and TDD workflow |
| [conductor/tracks.md](conductor/tracks.md) | Implementation tracks index |
| [PLAYBOOK.md](PLAYBOOK.md) | Sprint-by-sprint progress tracker |

---

## Project Status

All 10 implementation tracks complete. Currently in validation phase:

| Wave | Description | Status |
|---|---|---|
| 6.1 | Build Fix | Done |
| 6.2 | Lint Pass | Done |
| 6.3 | Per-Crate Tests | Done |
| 6.4 | Integration Tests | Done |
| 6.5 | Coverage + Quality Gate | Pending |

---

## License

All rights reserved. See LICENSE for details.
