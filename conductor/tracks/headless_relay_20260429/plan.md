# Implementation Plan: Headless Relay

## Overview

Five-phase implementation. Each phase delivers a compilable, testable milestone. No phase depends on CI infrastructure — CI (Phase 5) is layered on after the code works locally.

**TDD discipline applies throughout.** Every trait, every new message type, every endpoint gets a test before the implementation. Tests assert on behavior that doesn't exist yet (Red), then the implementation makes them pass (Green).

---

## Phase 1: SecretStore Trait Extraction

**Resolves:** FR-2 (SecretStore trait), unblocks all headless work
**Principle:** Introduce the abstraction without changing any behavior. Desktop builds continue using OS keychain. Zero functional change.

---

### Task 1.1 — SecretStore trait and error type (TDD Red)

Write tests asserting the `SecretStore` trait contract:

- Test: `InMemoryBackend::get` returns `None` for missing key.
- Test: `InMemoryBackend::set` then `get` returns the stored value.
- Test: `InMemoryBackend::set` overwrites previous value.
- Test: `InMemoryBackend::delete` removes the entry, subsequent `get` returns `None`.
- Test: `InMemoryBackend::delete` on missing key returns `Ok(())`.
- Tests must fail (Red) because the trait and types don't exist yet.

**Files:** new `fe-identity/src/secret_store.rs`, test module within.

---

### Task 1.2 — Implement SecretStore trait + InMemoryBackend (TDD Green)

Define `SecretStore` trait, `SecretStoreError` enum, and `InMemoryBackend`. All Task 1.1 tests pass.

```rust
pub trait SecretStore: Send + Sync + 'static {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, SecretStoreError>;
    fn set(&self, service: &str, account: &str, value: &str) -> Result<(), SecretStoreError>;
    fn delete(&self, service: &str, account: &str) -> Result<(), SecretStoreError>;
}
```

`InMemoryBackend` uses `RwLock<HashMap<(String, String), String>>`.

**Files:** `fe-identity/src/secret_store.rs`, `fe-identity/src/lib.rs` (add module).

---

### Task 1.3 — OsKeystoreBackend (TDD Red)

Write tests for `OsKeystoreBackend` behind `#[cfg(feature = "keyring-os")]`:

- Test: wraps `keyring::Entry` correctly (mock or integration test depending on CI environment).
- Test: `NoEntry` error maps to `Ok(None)`.

**Files:** `fe-identity/src/secret_store.rs` (add `OsKeystoreBackend` section).

---

### Task 1.4 — Implement OsKeystoreBackend (TDD Green)

Wrap `keyring::Entry::new`, `get_password`, `set_password`, `delete_credential` behind the trait. Feature-gate with `#[cfg(feature = "keyring-os")]`.

Update `fe-identity/Cargo.toml`:
```toml
[features]
default = ["keyring-os"]
keyring-os = ["dep:keyring"]

[dependencies]
keyring = { workspace = true, optional = true }
```

**Files:** `fe-identity/src/secret_store.rs`, `fe-identity/Cargo.toml`.

---

### Task 1.5 — EnvBackend (TDD Red + Green)

Write tests and implement `EnvBackend`:

- Test: `get` reads `FE_SECRET_{SERVICE}_{ACCOUNT}` env var (set in test via `std::env::set_var`).
- Test: `set` stores to internal map, `get` returns it.
- Test: env var takes precedence over runtime-written value.
- Test: key normalization — colons, dashes, dots all become underscores, uppercased.

**Files:** `fe-identity/src/secret_store.rs`.

---

### Task 1.6 — `default_backend()` factory function

Implement backend selection:
1. `FE_KEYSTORE_BACKEND=env` -> `EnvBackend`
2. `FE_KEYSTORE_BACKEND=os` -> `OsKeystoreBackend` (if feature enabled)
3. Auto-detect: headless environment (no `DISPLAY`/`WAYLAND_DISPLAY`, or `FE_HEADLESS=1`) -> `EnvBackend`
4. Default -> `OsKeystoreBackend`

Write test: with `FE_KEYSTORE_BACKEND=env` set, factory returns `EnvBackend`.

**Files:** `fe-identity/src/secret_store.rs`.

---

### Task 1.7 — Migrate all keyring call sites to SecretStore

Refactor the 3 call sites to accept `&dyn SecretStore` or `Arc<dyn SecretStore>`:

1. `fractalengine/src/main.rs` — `load_or_generate_keypair()` takes `&dyn SecretStore`
2. `fe-identity/src/keychain.rs` — `store_keypair`, `load_keypair`, `delete_keypair` take `&dyn SecretStore`
3. `fe-database/src/handlers/invite.rs` + `fe-database/src/lib.rs` — namespace secret functions take `Arc<dyn SecretStore>`

**Approach:** Thread `Arc<dyn SecretStore>` from `main()` into `spawn_db_thread_with_sync`. The DB thread stores it and passes to handlers.

**Verification:** `cargo build` succeeds. All existing tests pass. `cargo test` with `FE_KEYSTORE_BACKEND=env` in CI works without OS keychain.

**Files:** `fractalengine/src/main.rs`, `fe-identity/src/keychain.rs`, `fe-database/src/lib.rs`, `fe-database/src/handlers/invite.rs`, `fe-database/src/handlers/crud.rs`.

---

### Phase 1 Checkpoint

- `SecretStore` trait with 3 backends (InMemory, Os, Env)
- All keyring usage goes through the trait
- Desktop behavior unchanged (auto-selects Os backend)
- Tests run in CI without OS keychain (`FE_KEYSTORE_BACKEND=env`)

---

## Phase 2: Feature-Gated Bevy + Relay Binary

**Resolves:** FR-1 (headless binary), FR-3 (feature-gated build_app)
**Principle:** Separate binary crate. Minimal `#[cfg]` in shared code — only `fe-runtime/src/app.rs`.

---

### Task 2.1 — Feature-gate `build_app()` in fe-runtime (TDD Red)

Write a test that builds a headless Bevy app using `build_app` with the `headless` feature:

- Test: `build_app(handles)` returns an `App` that can call `app.update()` without panicking.
- Test: the app has `Time` resource (from `MinimalPlugins`).
- Test: the app does NOT have egui context (no `EguiPlugin`).
- Run with `--features headless --no-default-features`.

**Files:** `fe-runtime/src/app.rs` (test module), `fe-runtime/Cargo.toml`.

---

### Task 2.2 — Implement headless build_app path (TDD Green)

Update `fe-runtime/Cargo.toml`:
```toml
[features]
default = ["gui"]
gui = ["dep:bevy_egui"]
headless = []
```

Gate in `app.rs`:
- `#[cfg(feature = "gui")]`: `DefaultPlugins`, `EguiPlugin`, `FrameTimeDiagnosticsPlugin`
- `#[cfg(feature = "headless")]`: `MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(16ms))`, `LogPlugin`
- Shared: channel resources, drain systems, message subscriptions

Conditional import: `#[cfg(feature = "gui")] use bevy_egui::EguiPlugin;`

**Files:** `fe-runtime/Cargo.toml`, `fe-runtime/src/app.rs`.

---

### Task 2.3 — Extract shared thread-wiring function

Both `fractalengine/src/main.rs` and the future relay `main.rs` need identical wiring: channel creation, keypair loading, DB thread spawn, network thread spawn, sync thread spawn, API thread spawn.

Extract into `fe-runtime`:
```rust
pub struct EngineWiring {
    pub channels: ChannelHandles,
    pub api_channels: ApiChannels,
    pub secret_store: Arc<dyn SecretStore>,
    pub node_keypair: NodeKeypair,
    // thread join handles
}

pub fn wire_engine(config: EngineConfig) -> Result<EngineWiring>;
```

This function does NOT build the Bevy app — it sets up everything around it.

Write test: `wire_engine` with in-memory DB + in-memory keystore returns valid wiring with all channels connected.

**Files:** new `fe-runtime/src/wiring.rs`, update `fe-runtime/src/lib.rs`, update `fractalengine/src/main.rs` to use it.

---

### Task 2.4 — Create `fractalengine-relay` crate

Create the new binary crate:

```
fractalengine-relay/
  Cargo.toml
  src/
    main.rs
```

`Cargo.toml` depends on shared crates only (no `fe-renderer`, `fe-ui`, `fe-webview`). Uses `fe-runtime` with `features = ["headless"]`.

`main.rs`:
1. Initialize tracing
2. Call `wire_engine(config)` (from Task 2.3)
3. Build headless Bevy app via `build_app(handles)`
4. Run `app.run()`

Add `fractalengine-relay` to workspace members in root `Cargo.toml`.

**Verification:** `cargo build -p fractalengine-relay` succeeds on a headless Linux CI runner (no GPU, no display).

**Files:** new `fractalengine-relay/Cargo.toml`, new `fractalengine-relay/src/main.rs`, root `Cargo.toml`.

---

### Task 2.5 — Verify GUI binary unchanged

Run `cargo build -p fractalengine` and `cargo test -p fractalengine`. Confirm the GUI binary still compiles and all existing tests pass. No regressions.

**Files:** none (verification only).

---

### Phase 2 Checkpoint

- `fractalengine-relay` binary compiles without GPU/display deps
- `fractalengine` (GUI) binary unchanged
- Shared wiring function eliminates code duplication between binaries
- Headless Bevy app ticks at 60Hz via `ScheduleRunnerPlugin`

---

## Phase 3: Scene Graph Streaming

**Resolves:** FR-4 (scene graph streaming), FR-5 (asset delivery)
**Principle:** Extend existing WS protocol. Each new message type is a testable unit. Integration tests verify the full flow.

---

### Task 3.1 — SceneChange enum and EntityChangeBroadcast channel (TDD Red)

Write tests for `SceneChange` serialization:

- Test: `SceneChange::NodeAdded { node }` serializes to JSON with `"op": "node_added"`.
- Test: `SceneChange::NodeRemoved { node_id }` serializes correctly.
- Test: round-trip serde for all variants.

Define `EntityChangeBroadcast` as `tokio::sync::broadcast::Sender<SceneChange>`.

**Files:** `fe-runtime/src/messages.rs` (add `SceneChange` enum), `fe-runtime/src/channels.rs` (add to `ApiChannels`).

---

### Task 3.2 — Wire entity change broadcast into CUD handlers

When `create_node`, `delete_entity`, `rename_entity`, etc. succeed in `fe-database`, emit `SceneChange` on the broadcast channel.

Write integration test in `fe-test-harness`:
- Send `DbCommand::CreateNode` through the channel
- Assert `entity_change_rx` receives `SceneChange::NodeAdded`

**Files:** `fe-database/src/handlers/crud.rs`, `fe-database/src/handlers/entity.rs`, `fe-database/src/lib.rs` (thread the broadcast sender).

---

### Task 3.3 — SceneSubscribe + SceneSnapshot WS messages (TDD Red)

Write tests for the new WS message types:

- Test: `WsClientMsg::SceneSubscribe` deserializes from JSON.
- Test: `WsServerMsg::SceneSnapshot` serializes with `nodes` array and `version`.
- Test: `WsServerMsg::SceneDelta` serializes with `changes` array.

**Files:** `fe-api/src/types.rs` (extend enums).

---

### Task 3.4 — Implement scene subscription handler

In `fe-api/src/ws.rs`, handle `SceneSubscribe`:

1. Validate petal access via scope/role
2. Query current scene state via DB command (`GetHierarchy` scoped to petal)
3. Send `SceneSnapshot` with current nodes + scene version
4. Subscribe client to `entity_change_broadcast` for that petal
5. Forward matching `SceneDelta` messages to client

Write integration test: WS client subscribes, receives snapshot, then a separate REST call creates a node, WS client receives `SceneDelta::NodeAdded`.

**Files:** `fe-api/src/ws.rs`, `fe-api/src/scene.rs` (new — snapshot construction logic).

---

### Task 3.5 — Asset delivery endpoint (TDD Red + Green)

Add `GET /api/v1/assets/:content_hash`:

- Test: request known hash -> 200 with correct `Content-Type`, `ETag`, `Cache-Control: immutable`.
- Test: request unknown hash -> 404.
- Test: request without auth -> 401.

Implementation: look up hash in `BlobStoreHandle`, stream response body.

**Files:** new `fe-api/src/assets.rs`, update `fe-api/src/server.rs` (add route).

---

### Task 3.6 — Complete GET /api/v1/nodes/:id/transform

Currently returns 501. Implement: query DB for node transform, return JSON.

- Test: create node with transform, GET returns correct position/rotation/scale.

**Files:** `fe-api/src/rest.rs`.

---

### Phase 3 Checkpoint

- Thin clients can subscribe to petal scene graphs over WS
- Snapshot + delta streaming works end-to-end
- Assets are fetchable by content hash with immutable caching
- All new messages have serde tests
- Integration test covers full flow: connect -> subscribe -> snapshot -> mutation -> delta

---

## Phase 4: Relay Hardening

**Resolves:** Production readiness for the relay binary
**Principle:** Configuration, observability, graceful shutdown.

---

### Task 4.1 — Configurable bind address

Relay listens on `0.0.0.0:8765` by default (not `127.0.0.1`). Configurable via `FE_BIND_ADDR` env var.

GUI binary keeps `127.0.0.1:8765` default.

**Files:** `fe-api/src/server.rs`, `fractalengine-relay/src/main.rs`.

---

### Task 4.2 — Configurable CORS origins

Replace hardcoded localhost CORS with `FE_CORS_ORIGINS` env var (comma-separated). Default: `*` for relay, `http://localhost:*` for GUI.

**Files:** `fe-api/src/server.rs`.

---

### Task 4.3 — Graceful shutdown

Relay handles `SIGTERM` / `SIGINT`:
1. Stop accepting new WS connections
2. Drain in-flight DB commands
3. Flush SurrealDB
4. Exit cleanly

Write test: start relay, send shutdown signal, verify DB flush completes.

**Files:** `fractalengine-relay/src/main.rs`, `fe-database/src/lib.rs` (add `shutdown` command).

---

### Task 4.4 — Health and readiness endpoints

- `GET /health` — always 200 (liveness)
- `GET /ready` — 200 only when DB is connected and channels are healthy

Write test: relay starts, `/ready` returns 200 after DB init completes.

**Files:** `fe-api/src/server.rs`.

---

### Task 4.5 — Dockerfile

Multi-stage Docker build:
```dockerfile
FROM rust:alpine AS builder
# ... build fractalengine-relay with musl
FROM scratch
COPY --from=builder /app/target/.../fe-relay /fe-relay
EXPOSE 8765
ENTRYPOINT ["/fe-relay"]
```

**Files:** new `Dockerfile` at repo root (or `docker/Dockerfile.relay`).

---

### Phase 4 Checkpoint

- Relay runs as a proper server: configurable bind, CORS, graceful shutdown
- Health/readiness endpoints for load balancer integration
- Docker image builds and runs

---

## Phase 5: Cross-Compilation CI

**Resolves:** FR-6 (CI matrix), FR-7 (Docker image)
**Principle:** CI validates what we claim about cross-platform support. No build target is real until CI proves it.

---

### Task 5.1 — GitHub Actions workflow: check + test

Workflow runs on every PR:
- `cargo check -p fractalengine` (GUI)
- `cargo check -p fractalengine-relay` (headless)
- `cargo test --workspace` with `FE_KEYSTORE_BACKEND=env`

Matrix: `ubuntu-latest`, `windows-latest`, `macos-latest`.

**Files:** new `.github/workflows/ci.yml`.

---

### Task 5.2 — GitHub Actions workflow: release

On tag push `v*`, build all 8 targets in parallel:

| Job | Runner | Target | Tool |
|-----|--------|--------|------|
| Linux x64 GUI | ubuntu-latest | x86_64-unknown-linux-gnu | native |
| Windows x64 GUI | windows-latest | x86_64-pc-windows-msvc | native |
| Windows ARM64 GUI | windows-latest | aarch64-pc-windows-msvc | MSVC cross |
| macOS ARM64 GUI | macos-latest | aarch64-apple-darwin | native |
| macOS x64 GUI | macos-latest | x86_64-apple-darwin | cross from M1 |
| macOS Universal | macos-latest | lipo | needs ARM64 + x64 |
| Linux musl x64 headless | ubuntu-latest | x86_64-unknown-linux-musl | cargo-zigbuild |
| Linux musl ARM64 headless | ubuntu-latest | aarch64-unknown-linux-musl | cargo-zigbuild |

Publish GitHub Release with all artifacts + SHA256 checksums.

**Files:** new `.github/workflows/release.yml`, new `Cross.toml`.

---

### Task 5.3 — Docker image CI

Build and push Docker image on release:
- `docker build -f docker/Dockerfile.relay -t fractalengine-relay:$TAG .`
- Push to GitHub Container Registry

**Files:** update `.github/workflows/release.yml`.

---

### Phase 5 Checkpoint

- All 8 build targets green in CI
- Release workflow produces downloadable artifacts
- Docker image published to GHCR
- README updated with platform support table

---

## Summary

| Phase | Delivers | Key Testable Artifact |
|-------|----------|-----------------------|
| 1 | SecretStore trait + migration | `InMemoryBackend` tests, CI runs without keychain |
| 2 | Headless relay binary | `cargo build -p fractalengine-relay` on headless CI |
| 3 | Scene graph streaming | WS integration test: subscribe -> snapshot -> delta |
| 4 | Production relay config | Health/ready endpoints, graceful shutdown test |
| 5 | Cross-compilation CI | 8 build targets green, Docker image published |
