# Track: Headless Relay — Build Split, SecretStore Trait, Thin Client Surface

**Created:** 2026-04-29
**Status:** Draft
**Priority:** P1
**Depends on:** Realtime API Gateway (complete)
**Blocks:** Web Client SDK, IoT Integration, Docker Deployment, Mobile Client

---

## Problem Statement

FractalEngine is a single monolithic desktop binary that requires a GPU, OS windowing, and OS keychain. This makes it impossible to:

- Run a headless relay server on Linux/Docker/ARM for thin clients to connect to
- Build static musl binaries for zero-dependency container deployment
- Cross-compile for platforms where `wry`/`webkit2gtk`/`keyring` are unavailable
- Test the core logic (DB, networking, auth, API) without a display server or GPU

The existing `fe-api` gateway (axum on `:8765`) already provides ~70% of the relay surface (REST, WS, MCP). The gap is architectural: all 12 crates compile as one unit with no way to exclude GUI dependencies.

---

## Goals

1. A `fractalengine-relay` binary crate that compiles and runs without GPU, windowing, WebView, or OS keychain
2. A `SecretStore` trait abstracting keyring access — injectable, testable, zero configuration for the common case
3. Zero feature flags on shared crates — the dependency graph alone controls what compiles
4. Every new abstraction is unit-testable with mock implementations
5. Scene graph streaming additions to `fe-api` (snapshot + delta) so thin clients can render remote state
6. Documentation updated at every phase, not bolted on at the end

## Non-Goals (this track)

- Web client implementation (Three.js / Bevy WASM) — deferred to Web Client SDK track
- Mobile native client — deferred
- WebTransport / QUIC upgrade for browser clients — deferred
- Binary protocol encoding (MessagePack/FlatBuffers) — deferred
- TLS termination (use reverse proxy for now)
- CI/CD pipeline (covered by Release CI track)

---

## Design Principle: No Flags, Just Crates

The key architectural insight is that **the Cargo dependency graph is the feature flag**. The relay binary simply does not depend on `fe-renderer`, `fe-ui`, or `fe-webview`. No `#[cfg]` needed to exclude them — they never compile.

For the one place where behavior must differ (`build_app` — plugin selection), we use **two plain functions** in `fe-runtime` instead of conditional compilation:

```rust
// fe-runtime/src/app.rs

/// Shared setup: inserts channel resources, registers drain systems.
/// Called by both GUI and relay after they add their own plugins.
pub fn setup_core_systems(app: &mut App, handles: BevyHandles) { ... }
```

Each binary's `main.rs` does:
```rust
// GUI
app.add_plugins(DefaultPlugins.set(AssetPlugin { ... }));
app.add_plugins(EguiPlugin);
fe_runtime::app::setup_core_systems(&mut app, handles);

// Relay
app.add_plugins(MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_millis(16))));
fe_runtime::app::setup_core_systems(&mut app, handles);
```

**Result:** Zero feature flags on `fe-runtime`. Zero `#[cfg]` blocks. `bevy_egui` is a dependency of the GUI binary crate, never of `fe-runtime`.

---

## Architecture Overview

### Binary Split

```
fractalengine (GUI)                 fractalengine-relay (headless)
+---------------------------------+ +----------------------------------+
| fe-runtime                      | | fe-runtime                       |
| fe-database                     | | fe-database                      |
| fe-network                      | | fe-network                       |
| fe-sync                         | | fe-sync                          |
| fe-identity                     | | fe-identity (no keyring dep)     |
| fe-auth                         | | fe-auth                          |
| fe-api                          | | fe-api                           |
| fe-renderer  <-- GUI only       | |                                  |
| fe-ui        <-- GUI only       | |                                  |
| fe-webview   <-- GUI only       | |                                  |
| bevy (full)                     | | bevy (full, MinimalPlugins used) |
| bevy_egui    <-- GUI only       | |                                  |
| keyring      <-- GUI only       | |                                  |
+---------------------------------+ +----------------------------------+
```

**Only one optional dependency in the entire workspace:** `keyring` on `fe-identity` (default-enabled, omitted by the relay crate).

### SecretStore Trait

```rust
pub trait SecretStore: Send + Sync + 'static {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, SecretStoreError>;
    fn set(&self, service: &str, account: &str, value: &str) -> Result<(), SecretStoreError>;
    fn delete(&self, service: &str, account: &str) -> Result<(), SecretStoreError>;
}
```

Three implementations, all always compiled (pure Rust):
- `OsKeystoreBackend` — wraps `keyring` crate, available when `keyring` dep is present
- `EnvBackend` — maps `(service, account)` to `FE_SECRET_{SERVICE}_{ACCOUNT}` env vars
- `InMemoryBackend` — for tests

**No factory function. No env var to select backend.** Each binary constructs the one it needs:
- GUI `main.rs`: `let store = Arc::new(OsKeystoreBackend);`
- Relay `main.rs`: `let store = Arc::new(EnvBackend::new());`
- Tests: `let store = Arc::new(InMemoryBackend::new());`

### Relay Configuration

Three env vars, all with sensible defaults:

| Env Var | Default | Purpose |
|---------|---------|---------|
| `FE_BIND_ADDR` | `0.0.0.0:8765` | Listen address |
| `FE_DB_PATH` | `data/fractalengine.db` | SurrealDB storage path |
| `FE_CORS_ORIGINS` | `*` | Allowed CORS origins (comma-separated) |

Secrets are injected via the `FE_SECRET_*` pattern. No other configuration is needed.

### Scene Graph Streaming (fe-api additions)

Thin clients need more than transforms. New WS message types:

| Message | Direction | Purpose |
|---------|-----------|---------|
| `SceneSubscribe` | client -> server | Request scene state for a petal |
| `SceneSnapshot` | server -> client | Full node list + asset manifest |
| `SceneDelta` | server -> client | Incremental changes (add/remove/rename) |
| `EntityCommand` | client -> server | CUD operations over WS (not just REST) |
| `EntityCommandResult` | server -> client | Response to EntityCommand |

New REST endpoint: `GET /api/v1/assets/:content_hash` — serves blob store content with immutable cache headers.

New broadcast channel: `entity_change_tx/rx` — parallel to transform broadcast, carries `SceneChange` deltas when CUD operations modify the hierarchy.

---

## Functional Requirements

### FR-1: Headless Relay Binary
The `fractalengine-relay` binary compiles without `fe-renderer`, `fe-ui`, `fe-webview`, `bevy_egui`, or `keyring`. It runs Bevy with `MinimalPlugins`, spawns DB/network/sync/API threads, and blocks until shutdown signal.

### FR-2: SecretStore Trait
All keyring access (3 call sites: node keypair, named keypair, verse namespace secret) goes through `Arc<dyn SecretStore>`. Each binary picks its backend. Tests use `InMemoryBackend`.

### FR-3: Zero-Flag Shared Crates
No feature flags on `fe-runtime`, `fe-database`, `fe-network`, `fe-sync`, `fe-api`, or `fe-auth`. The only optional dep is `keyring` on `fe-identity`. Plugin selection happens in each binary's `main.rs`, not behind `#[cfg]`.

### FR-4: Scene Graph Streaming
WS clients can subscribe to a petal's scene graph and receive a snapshot followed by incremental deltas. CUD operations broadcast `SceneChange` events to all subscribed clients.

### FR-5: Asset Delivery
HTTP endpoint serves content-addressed assets from the blob store. Responses are cacheable (ETag + `Cache-Control: immutable`).

### FR-6: Incremental Documentation
Each phase updates `README.md` and/or `BUILDING.md` with what was just built. Inline doc comments on all new public types and functions. Architecture docs stay current, not aspirational.

**Note:** Cross-compilation CI and Docker image publishing are covered by the separate [Release CI track](../release_ci_20260429/spec.md).

---

## Testing Strategy

Every new abstraction introduces a testable seam:

- **SecretStore**: `InMemoryBackend` for all tests. No OS keychain needed in CI.
- **setup_core_systems**: Unit test builds a `MinimalPlugins` app, calls `setup_core_systems`, asserts resources exist.
- **Relay binary**: `cargo build -p fractalengine-relay` succeeds on Linux CI without display deps.
- **Scene streaming**: Integration tests in `fe-test-harness` — connect WS client, subscribe to petal, verify snapshot + delta messages.
- **Asset delivery**: Integration test — upload blob, GET by hash, verify content + headers.

---

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| `ring` crate fails under musl | Verified: ring 0.17.x builds under musl with zig cc. Fallback: `aws-lc-sys`. |
| SurrealKV mmap issues on musl | SurrealKV uses `memmap2` which is POSIX-compliant. No musl-specific issues known. |
| Bevy `MinimalPlugins` missing systems the relay needs | Verified: `TransformPlugin` propagation included in MinimalPlugins. |
| Relay `main.rs` diverges from GUI `main.rs` | Extract shared wiring into `fe-runtime::wiring::wire_engine()` that both call. |
| `bevy_egui` pulled transitively into relay | Verified: only the GUI binary depends on it. No shared crate imports it. |
