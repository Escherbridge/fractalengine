# Implementation Plan: Headless Relay

## Overview

Four-phase implementation. Each phase delivers a compilable, testable milestone with updated documentation.

**TDD discipline applies throughout.** Every trait, every new message type, every endpoint gets a test before the implementation.

**Documentation is continuous.** Each phase ends with a docs task that captures what was built while it's fresh.

---

## Phase 1: SecretStore Trait Extraction

**Resolves:** FR-2 (SecretStore trait), FR-3 (zero-flag shared crates)
**Principle:** Introduce the abstraction without changing any behavior. Desktop builds continue using OS keychain. Zero functional change. Zero feature flags on shared crates.

---

### [x] Task 1.1 — SecretStore trait, error type, and InMemoryBackend (TDD Red+Green)

Define the trait and implement the test backend in one pass:

```rust
// fe-identity/src/secret_store.rs
pub trait SecretStore: Send + Sync + 'static {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, SecretStoreError>;
    fn set(&self, service: &str, account: &str, value: &str) -> Result<(), SecretStoreError>;
    fn delete(&self, service: &str, account: &str) -> Result<(), SecretStoreError>;
}
```

Tests:
- `get` returns `None` for missing key
- `set` then `get` returns stored value
- `set` overwrites previous value
- `delete` removes entry; `delete` on missing key returns `Ok(())`

`InMemoryBackend` uses `RwLock<HashMap<(String, String), String>>`. Always compiled (pure Rust, no optional deps).

**Files:** new `fe-identity/src/secret_store.rs`, `fe-identity/src/lib.rs` (add module).

---

### [x] Task 1.2 — OsKeystoreBackend

Wraps `keyring::Entry` behind the trait. Gated behind the existing `keyring` dependency — make `keyring` optional in `fe-identity/Cargo.toml`:

```toml
[dependencies]
keyring = { workspace = true, optional = true }
```

The `OsKeystoreBackend` struct and its `impl SecretStore` are compiled only when the `keyring` dep is present. Use `#[cfg(feature = "keyring")]` — this is the ONE conditional compilation in the workspace.

Tests:
- `NoEntry` maps to `Ok(None)`
- Compile-tests verify the type exists when keyring is present

**Files:** `fe-identity/src/secret_store.rs`, `fe-identity/Cargo.toml`.

---

### [x] Task 1.3 — EnvBackend

Maps `(service, account)` to env var `FE_SECRET_{SERVICE}_{ACCOUNT}` (uppercased, special chars -> underscore). Always compiled (pure Rust).

Tests:
- `get` reads matching env var
- `set` stores to internal map; `get` returns it
- Env var takes precedence over runtime-written value
- Key normalization: colons, dashes, dots become underscores

**Files:** `fe-identity/src/secret_store.rs`.

---

### [x] Task 1.4 — Migrate all keyring call sites to SecretStore

Refactor the 3 call sites to accept `Arc<dyn SecretStore>`:

1. `fractalengine/src/main.rs` — `load_or_generate_keypair(&dyn SecretStore)`
2. `fe-identity/src/keychain.rs` — `store_keypair`, `load_keypair`, `delete_keypair` take `&dyn SecretStore`
3. `fe-database/src/lib.rs` + `handlers/invite.rs` + `handlers/crud.rs` — namespace secret functions take `Arc<dyn SecretStore>`

Thread `Arc<dyn SecretStore>` from `main()` into `spawn_db_thread_with_sync`. GUI main creates `OsKeystoreBackend`. Tests use `InMemoryBackend`.

**Verification:** `cargo build` succeeds. All existing tests pass unchanged (they already don't touch keyring in tests). Direct `keyring::Entry` usage grep returns zero results outside `secret_store.rs`.

**Files:** `fractalengine/src/main.rs`, `fe-identity/src/keychain.rs`, `fe-database/src/lib.rs`, `fe-database/src/handlers/invite.rs`, `fe-database/src/handlers/crud.rs`.

---

### [x] Task 1.5 — Phase 1 docs

- Add `/// doc comments` to all public types in `secret_store.rs` (trait, error, backends)
- Update `README.md` architecture section: mention SecretStore abstraction
- Add a section to `BUILDING.md` (create if not exists): "Running tests without OS keychain: set `FE_KEYSTORE_BACKEND=env`" — actually, no env var needed anymore, tests just use InMemoryBackend directly

**Files:** `fe-identity/src/secret_store.rs` (doc comments), `README.md`.

---

### Phase 1 Checkpoint

- `SecretStore` trait with 3 backends (InMemory, Os, Env)
- All keyring usage goes through the trait
- Desktop behavior unchanged (`main.rs` creates `OsKeystoreBackend`)
- Tests use `InMemoryBackend` — no OS keychain in CI
- ONE optional dep (`keyring` on `fe-identity`), zero feature flags on any other crate

---

## Phase 2: Relay Binary

**Resolves:** FR-1 (headless binary), FR-3 (zero-flag shared crates)
**Principle:** Separate binary crate. Zero `#[cfg]` in shared code. Plugin selection in `main.rs`, not behind flags.

---

### [x] Task 2.1 — Extract `setup_core_systems()` from `build_app()`

Refactor `fe-runtime/src/app.rs`: split the current `build_app()` into:

1. `setup_core_systems(app: &mut App, handles: BevyHandles)` — inserts channel resources, registers drain systems, adds message subscriptions. Pure ECS setup, no plugins.
2. The existing `build_app()` calls `setup_core_systems` internally (preserves backward compat during migration).

Test: build a `MinimalPlugins` app, call `setup_core_systems`, assert channel resources exist, call `app.update()` without panic.

**No feature flags. No `#[cfg]`.** Just a function that takes `&mut App`.

**Files:** `fe-runtime/src/app.rs`.

---

### [x] Task 2.2 — Extract shared thread-wiring function

Both binaries need identical wiring: channel creation, keypair loading, DB thread spawn, network thread spawn, sync thread spawn, API thread spawn.

Extract into `fe-runtime/src/wiring.rs`:

```rust
pub struct EngineWiring {
    pub handles: BevyHandles,
    pub api_channels: ApiChannels,
    pub secret_store: Arc<dyn SecretStore>,
    pub node_keypair: NodeKeypair,
    // thread join handles for cleanup
}

pub fn wire_engine(config: EngineConfig) -> Result<EngineWiring>;
```

This function does NOT build the Bevy app — it sets up everything around it. Each binary calls it, then builds its own app.

Test: `wire_engine` with in-memory DB (`kv-mem`) + `InMemoryBackend` returns valid wiring.

**Files:** new `fe-runtime/src/wiring.rs`, `fe-runtime/src/lib.rs`, refactor `fractalengine/src/main.rs` to use it.

---

### [x] Task 2.3 — Create `fractalengine-relay` crate

New binary crate:

```toml
# fractalengine-relay/Cargo.toml
[package]
name = "fractalengine-relay"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "fe-relay"
path = "src/main.rs"

[dependencies]
fe-runtime  = { path = "../fe-runtime" }
fe-network  = { path = "../fe-network" }
fe-database = { path = "../fe-database" }
fe-identity = { path = "../fe-identity", default-features = false }  # no keyring
fe-sync     = { path = "../fe-sync" }
fe-api      = { path = "../fe-api" }
bevy        = { workspace = true }
crossbeam   = { workspace = true }
tokio       = { workspace = true }
tracing     = { workspace = true }
tracing-subscriber = { workspace = true }
anyhow      = { workspace = true }
```

`main.rs`:
```rust
fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let wiring = fe_runtime::wiring::wire_engine(EngineConfig {
        secret_store: Arc::new(fe_identity::secret_store::EnvBackend::new()),
        db_path: env::var("FE_DB_PATH").unwrap_or("data/fractalengine.db".into()),
        ..Default::default()
    })?;

    let mut app = App::new();
    app.add_plugins(MinimalPlugins.set(
        ScheduleRunnerPlugin::run_loop(Duration::from_millis(16))
    ));
    fe_runtime::app::setup_core_systems(&mut app, wiring.handles);
    app.run();
    Ok(())
}
```

Add to workspace `members` in root `Cargo.toml`.

**Verification:** `cargo build -p fractalengine-relay` succeeds. Binary is ~30MB smaller than GUI binary (no renderer, no wgpu, no egui).

**Files:** new `fractalengine-relay/Cargo.toml`, new `fractalengine-relay/src/main.rs`, root `Cargo.toml`.

---

### [x] Task 2.4 — Verify GUI binary unchanged

`cargo build -p fractalengine` and `cargo test -p fractalengine` pass. No regressions.

Migrate GUI `main.rs` to use `wire_engine()` + `setup_core_systems()` (replacing the old monolithic `build_app()`). GUI-specific plugins (`DefaultPlugins`, `EguiPlugin`, `ViewportPlugin`, etc.) added in GUI `main.rs` after `setup_core_systems`.

**Files:** `fractalengine/src/main.rs`.

---

### [x] Task 2.5 — Phase 2 docs [df0d175]

- `BUILDING.md`: add "Building the relay" section with `cargo build -p fractalengine-relay`
- `README.md`: update architecture diagram showing two binaries sharing core crates
- `fractalengine-relay/README.md`: brief usage doc (env vars, what it does, how to run)
- Doc comments on `wire_engine()`, `setup_core_systems()`, `EngineConfig`, `EngineWiring`

**Files:** `BUILDING.md`, `README.md`, new `fractalengine-relay/README.md`, `fe-runtime/src/wiring.rs`, `fe-runtime/src/app.rs`.

---

### Phase 2 Checkpoint

- `fractalengine-relay` binary compiles without GPU/display deps
- `fractalengine` (GUI) binary unchanged in behavior, refactored to share wiring
- Zero feature flags on `fe-runtime` — just two functions
- `bevy_egui` is a dep of `fractalengine` crate only, not any shared crate
- Documentation current with the architecture

---

## Phase 3: Scene Graph Streaming

**Resolves:** FR-4 (scene graph streaming), FR-5 (asset delivery)
**Principle:** Extend existing WS protocol. Each new message type is a testable unit.

---

### [x] Task 3.1 — SceneChange enum and EntityChangeBroadcast channel (TDD Red+Green)

Define `SceneChange` in `fe-runtime/src/messages.rs`:

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SceneChange {
    NodeAdded { node: NodeDto },
    NodeRemoved { node_id: String },
    NodeRenamed { node_id: String, new_name: String },
    NodeTransform { node_id: String, position: [f32; 3], rotation: [f32; 3], scale: [f32; 3] },
}
```

Add `entity_change_tx: tokio::sync::broadcast::Sender<SceneChange>` to `ApiChannels`.

Tests: serde round-trip for each variant. Channel send/recv works.

**Files:** `fe-runtime/src/messages.rs`, `fe-runtime/src/channels.rs`.

---

### [ ] Task 3.2 — Wire entity change broadcast into CUD handlers

When `create_node`, `delete_entity`, `rename_entity` succeed in `fe-database`, emit `SceneChange` on the broadcast channel.

Integration test in `fe-test-harness`:
- Send `DbCommand::CreateNode` through channel
- Assert `entity_change_rx` receives `SceneChange::NodeAdded`

**Files:** `fe-database/src/handlers/crud.rs`, `fe-database/src/handlers/entity.rs`, `fe-database/src/lib.rs`.

---

### [x] Task 3.3 — SceneSubscribe + SceneSnapshot WS messages (TDD Red+Green)

Extend `WsClientMsg` and `WsServerMsg` in `fe-api/src/types.rs`:

- `WsClientMsg::SceneSubscribe { petal_id, last_known_version: Option<u64> }`
- `WsServerMsg::SceneSnapshot { petal_id, version, nodes: Vec<NodeDto> }`
- `WsServerMsg::SceneDelta { petal_id, version, changes: Vec<SceneChange> }`
- `WsServerMsg::EntityCommandResult { request_id, ok, data, error }`

Tests: serde round-trip for each new variant.

**Files:** `fe-api/src/types.rs`.

---

### [~] Task 3.4 — Scene subscription handler

In `fe-api/src/ws.rs`, handle `SceneSubscribe`:

1. Validate petal access via scope/role
2. Query current scene state via DB command
3. Send `SceneSnapshot` with current nodes
4. Subscribe client to `entity_change_broadcast` for that petal
5. Forward matching `SceneDelta` messages

Integration test: WS client subscribes -> receives snapshot -> REST creates node -> WS receives delta.

**Files:** `fe-api/src/ws.rs`, new `fe-api/src/scene.rs`.

---

### [x] Task 3.5 — Asset delivery endpoint (TDD Red+Green)

`GET /api/v1/assets/:content_hash`:

- 200 with `Content-Type`, `ETag: "blake3:{hash}"`, `Cache-Control: public, max-age=31536000, immutable`
- 404 for unknown hash
- 401 without auth

**Files:** new `fe-api/src/assets.rs`, `fe-api/src/server.rs` (add route).

---

### [x] Task 3.6 — Complete GET /api/v1/nodes/:id/transform

Currently returns 501. Implement: query DB, return JSON.

Test: create node with transform, GET returns correct values.

**Files:** `fe-api/src/rest.rs`.

---

### [ ] Task 3.7 — Phase 3 docs

- Document new WS message types in `fe-api/README.md` or inline module docs
- Document asset delivery endpoint (URL, headers, caching behavior)
- Update `README.md` API section with thin client connection flow

**Files:** `fe-api/src/types.rs` (doc comments), `fe-api/src/assets.rs` (doc comments), `README.md`.

---

### Phase 3 Checkpoint

- Thin clients can subscribe to petal scene graphs over WS
- Snapshot + delta streaming works end-to-end
- Assets fetchable by content hash with immutable caching
- All new types have doc comments and serde tests
- Integration test covers: connect -> subscribe -> snapshot -> mutation -> delta

---

## Phase 4: Relay Hardening

**Resolves:** Production readiness for the relay binary.
**Principle:** Only the relay binary gets new configuration — shared crates stay clean.

---

### [x] Task 4.1 — Configurable bind address and CORS

Relay `main.rs` reads `FE_BIND_ADDR` (default `0.0.0.0:8765`) and `FE_CORS_ORIGINS` (default `*`). Pass to `fe-api` server config.

GUI `main.rs` keeps hardcoded `127.0.0.1:8765` and `localhost` CORS. No env var complexity for desktop users.

Config is in each binary's `main.rs`, not in shared crates. `fe-api/src/server.rs` accepts bind address and CORS origins as parameters (it already does for bind address).

**Files:** `fractalengine-relay/src/main.rs`, `fe-api/src/server.rs` (add CORS origins parameter).

---

### [x] Task 4.2 — Graceful shutdown

Relay handles `SIGTERM` / `SIGINT` via `tokio::signal::ctrl_c()`:
1. Stop accepting new WS connections
2. Drain in-flight DB commands
3. Flush SurrealDB
4. Exit cleanly

Test: start relay in background, send ctrl_c, verify clean exit (no panic, DB flushed).

**Files:** `fractalengine-relay/src/main.rs`, `fe-database/src/lib.rs` (add shutdown command).

---

### [x] Task 4.3 — Health and readiness endpoints

- `GET /health` — always 200 (liveness)
- `GET /ready` — 200 when DB is initialized and channels are healthy

Test: relay starts, `/ready` returns 200 after init.

**Files:** `fe-api/src/server.rs`.

---

### [x] Task 4.4 — Dockerfile

```dockerfile
FROM rust:alpine AS builder
RUN apk add --no-cache musl-dev perl make
WORKDIR /app
COPY . .
RUN cargo build --release -p fractalengine-relay

FROM scratch
COPY --from=builder /app/target/release/fe-relay /fe-relay
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
VOLUME ["/data"]
EXPOSE 8765
ENTRYPOINT ["/fe-relay"]
```

**Files:** new `docker/Dockerfile.relay`.

---

### [ ] Task 4.5 — Phase 4 docs

- `fractalengine-relay/README.md`: complete with Docker usage, env vars, health endpoints
- `docker/README.md`: Docker Compose example for running relay + volume mount
- Update `BUILDING.md`: add "Running in Docker" section

**Files:** `fractalengine-relay/README.md`, new `docker/README.md`, `BUILDING.md`.

---

### Phase 4 Checkpoint

- Relay runs as a proper server: configurable bind, CORS, graceful shutdown
- Health/readiness endpoints for load balancer integration
- Docker image builds and runs
- Complete documentation for relay deployment

---

## Complexity Budget

**Feature flags across entire workspace: 1**
- `keyring` optional dep on `fe-identity` (default on, omitted by relay)

**`#[cfg]` blocks added: 1**
- `#[cfg(feature = "keyring")]` around `OsKeystoreBackend` in `secret_store.rs`

**Env vars for relay: 3**
- `FE_BIND_ADDR`, `FE_DB_PATH`, `FE_CORS_ORIGINS`

**Env vars for secrets: N**
- `FE_SECRET_{SERVICE}_{ACCOUNT}` pattern (only in relay/container mode)

**New public functions: 2**
- `setup_core_systems(app, handles)` in `fe-runtime`
- `wire_engine(config)` in `fe-runtime`

---

## Summary

| Phase | Delivers | Testable Artifact | Docs Updated |
|-------|----------|-------------------|--------------|
| 1 | SecretStore trait + migration | `InMemoryBackend` unit tests | doc comments, README |
| 2 | Headless relay binary | `cargo build -p fractalengine-relay` | BUILDING.md, relay README |
| 3 | Scene graph streaming | WS integration test: subscribe -> snapshot -> delta | API docs, message types |
| 4 | Production relay config | Health/ready endpoints, Docker image | Docker docs, deployment guide |

**Note:** Cross-compilation CI is covered by the separate [Release CI track](../release_ci_20260429/plan.md).
