# Track: Headless Relay — Cross-Platform Build Split, SecretStore Trait, Thin Client Surface

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
2. A `SecretStore` trait abstracting keyring access — OS keychain on desktop, env vars / encrypted file in containers
3. Feature-gated `fe-runtime` with clean `gui` / `headless` split in `build_app()` — no `#[cfg]` spaghetti
4. Every new abstraction is unit-testable with mock implementations
5. Cross-compilation CI producing 8 artifacts (3 GUI + 3 headless + macOS universal + Docker image)
6. Scene graph streaming additions to `fe-api` (snapshot + delta) so thin clients can render remote state

## Non-Goals (this track)

- Web client implementation (Three.js / Bevy WASM) — deferred to Web Client SDK track
- Mobile native client — deferred
- WebTransport / QUIC upgrade for browser clients — deferred
- Binary protocol encoding (MessagePack/FlatBuffers) — deferred
- TLS termination (use reverse proxy for now)

---

## Architecture Overview

### Binary Split

```
fractalengine (GUI)                 fractalengine-relay (headless)
+---------------------------------+ +----------------------------------+
| fe-runtime (gui feature)        | | fe-runtime (headless feature)    |
| fe-database                     | | fe-database                      |
| fe-network                      | | fe-network                       |
| fe-sync                         | | fe-sync                          |
| fe-identity (keyring-os)        | | fe-identity (headless)           |
| fe-auth                         | | fe-auth                          |
| fe-api                          | | fe-api                           |
| fe-renderer  <-- GUI only       | |                                  |
| fe-ui        <-- GUI only       | |                                  |
| fe-webview   <-- GUI only       | |                                  |
| Bevy DefaultPlugins             | | Bevy MinimalPlugins              |
| keyring (OS)                    | | SecretStore (env/file)           |
+---------------------------------+ +----------------------------------+
```

The relay is NOT a stripped-down GUI build. It is a **separate binary crate** that depends only on shared crates. The dependency graph enforces the split — no feature flags needed to exclude GUI crates.

### SecretStore Trait

```rust
pub trait SecretStore: Send + Sync + 'static {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, SecretStoreError>;
    fn set(&self, service: &str, account: &str, value: &str) -> Result<(), SecretStoreError>;
    fn delete(&self, service: &str, account: &str) -> Result<(), SecretStoreError>;
}
```

Implementations:
- `OsKeystoreBackend` — wraps `keyring` crate (desktop, feature-gated)
- `EnvBackend` — maps `(service, account)` to `FE_SECRET_{SERVICE}_{ACCOUNT}` env vars
- `InMemoryBackend` — for tests

Backend selection: `FE_KEYSTORE_BACKEND` env var, with auto-detection fallback.

### Bevy Headless Mode

`fe-runtime` gains `gui` (default) and `headless` features:
- `gui`: `DefaultPlugins` + `bevy_egui` + `FrameTimeDiagnosticsPlugin`
- `headless`: `MinimalPlugins` + `ScheduleRunnerPlugin::run_loop(16ms)` + `LogPlugin`

Both paths share: channel resources, drain systems, message types. The relay still runs Bevy ECS for scheduling — it is a headless Bevy app, not a non-Bevy app.

### Scene Graph Streaming (fe-api additions)

Thin clients need more than transforms. New WS message types:

| Message | Direction | Purpose |
|---------|-----------|---------|
| `SceneSubscribe` | client -> server | Request scene state for a petal |
| `SceneSnapshot` | server -> client | Full node list + asset manifest |
| `SceneDelta` | server -> client | Incremental changes (add/remove/rename) |
| `EntityCommand` | client -> server | CUD operations over WS (not just REST) |
| `EntityCommandResult` | server -> client | Response to EntityCommand |
| `AssetReady` | server -> client | Asset URL + content hash for fetching |

New REST endpoint: `GET /api/v1/assets/:content_hash` — serves blob store content with immutable cache headers.

New broadcast channel: `entity_change_tx/rx` — parallel to transform broadcast, carries `SceneChange` deltas when CUD operations modify the hierarchy.

---

## Functional Requirements

### FR-1: Headless Relay Binary
The `fractalengine-relay` binary compiles without `fe-renderer`, `fe-ui`, `fe-webview`, or `bevy_egui`. It runs Bevy with `MinimalPlugins`, spawns DB/network/sync/API threads, and blocks until shutdown signal.

### FR-2: SecretStore Trait
All keyring access (3 call sites: node keypair, named keypair, verse namespace secret) goes through `Arc<dyn SecretStore>`. Desktop uses OS keychain. Headless uses env vars. Both are injectable and testable.

### FR-3: Feature-Gated build_app
`fe-runtime::app::build_app()` branches on `#[cfg(feature = "gui")]` / `#[cfg(feature = "headless")]`. `bevy_egui` becomes an optional dependency of `fe-runtime`. No other shared crate changes.

### FR-4: Scene Graph Streaming
WS clients can subscribe to a petal's scene graph and receive a snapshot followed by incremental deltas. CUD operations broadcast `SceneChange` events to all subscribed clients.

### FR-5: Asset Delivery
HTTP endpoint serves content-addressed assets from the blob store. Responses are cacheable (ETag + `Cache-Control: immutable`).

### FR-6: Cross-Compilation CI
GitHub Actions workflow builds 8 targets: Linux x64 GUI, Windows x64/ARM64 GUI, macOS universal GUI, Linux x64/ARM64 musl headless, Linux ARM64 GNU headless. Uses sccache, cargo-zigbuild (musl), cross-rs (ARM GNU).

### FR-7: Docker Image
Multi-stage Dockerfile producing a `FROM scratch` image with the musl static relay binary. Configurable via `FE_KEYSTORE_BACKEND`, `FE_DB_PATH`, and `FE_SECRET_*` env vars.

---

## Target Platform Matrix

| Target | Type | Tool | Binary |
|--------|------|------|--------|
| `x86_64-unknown-linux-gnu` | GUI | native | `fractalengine-linux-x86_64` |
| `x86_64-pc-windows-msvc` | GUI | native | `fractalengine-windows-x86_64.exe` |
| `aarch64-pc-windows-msvc` | GUI | MSVC cross | `fractalengine-windows-aarch64.exe` |
| `aarch64-apple-darwin` | GUI | native (M1 runner) | `fractalengine-macos-universal` |
| `x86_64-apple-darwin` | GUI | cross from M1 | (merged into universal) |
| `x86_64-unknown-linux-musl` | Headless | cargo-zigbuild | `fe-relay-linux-musl-x86_64` |
| `aarch64-unknown-linux-musl` | Headless | cargo-zigbuild | `fe-relay-linux-musl-aarch64` |
| `aarch64-unknown-linux-gnu` | Headless | cross-rs | `fe-relay-linux-aarch64` |

---

## Testing Strategy

Every new abstraction introduces a testable seam:

- **SecretStore**: `InMemoryBackend` for all tests. No OS keychain needed in CI.
- **build_app headless**: `cargo test -p fe-runtime --features headless` — Bevy app builds and ticks without GPU.
- **Relay binary**: `cargo build -p fractalengine-relay` succeeds on Linux CI without display deps.
- **Scene streaming**: Integration tests in `fe-test-harness` — connect WS client, subscribe to petal, verify snapshot + delta messages.
- **Asset delivery**: Integration test — upload blob, GET by hash, verify content + headers.
- **Cross-compilation**: CI matrix — all 8 targets build green.

---

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| `ring` crate fails under musl | Verified: ring 0.17.x builds under musl with zig cc. Fallback: `aws-lc-sys`. |
| SurrealKV mmap issues on musl | SurrealKV uses `memmap2` which is POSIX-compliant. No musl-specific issues known. |
| Bevy `MinimalPlugins` missing systems the relay needs | Verified: only `TransformPlugin` propagation needed, included in MinimalPlugins. |
| `bevy_egui` conditional import breaks compile | Gate with `#[cfg(feature = "gui")] use bevy_egui::EguiPlugin;` — single import change. |
| Relay `main.rs` diverges from GUI `main.rs` | Extract shared wiring into a function in `fe-runtime` that both binaries call. |
