# Track Plan: Seed Runtime

**Track ID:** seed_runtime_20260321
**Status:** new
**Depends on:** none

---

## Phase 1: Cargo Workspace and Crate Skeleton

### Tasks

- [~] Task: Initialize Cargo workspace with `fractalengine` binary crate and `fe-runtime`, `fe-network`, `fe-database` library crates
- [~] Task: Add core dependencies to each crate's `Cargo.toml`:
  - `fractalengine`: `bevy 0.18`, `crossbeam`, `tracing`, `tracing-subscriber`
  - `fe-runtime`: `bevy 0.18`, `crossbeam`, `tracing`
  - `fe-network`: `tokio` (full features), `tracing`
  - `fe-database`: `surrealdb` (with `kv-surrealkv` feature), `tokio` (full features), `tracing`
- [~] Task: Configure `rustfmt.toml` (max line length 100) and `.clippy.toml` (deny warnings)
- [~] Task: Add `.cargo/config.toml` with `SURREAL_SYNC_DATA=true` as a build-time env var for the database crate
- [ ] Task: Write smoke test — `cargo build` and `cargo test` pass on empty crates
- [ ] Task: Conductor — User Manual Verification 'Phase 1: Cargo Workspace and Crate Skeleton' (Protocol in workflow.md)

---

## Phase 2: Typed Channel Message Definitions

### Tasks

- [~] Task: Write failing tests for `NetworkCommand` and `NetworkEvent` enum serialization/deserialization in `fe-runtime`
- [~] Task: Define `NetworkCommand` enum in `fe-runtime`:
  ```rust
  pub enum NetworkCommand { Ping, Shutdown }
  ```
- [~] Task: Define `NetworkEvent` enum in `fe-runtime`:
  ```rust
  pub enum NetworkEvent { Pong, Started, Stopped }
  ```
- [~] Task: Write failing tests for `DbCommand` and `DbResult` enum serialization/deserialization in `fe-runtime`
- [~] Task: Define `DbCommand` enum in `fe-runtime`:
  ```rust
  pub enum DbCommand { Ping, Shutdown }
  ```
- [~] Task: Define `DbResult` enum in `fe-runtime`:
  ```rust
  pub enum DbResult { Pong, Started, Stopped, Error(String) }
  ```
- [~] Task: Define `ChannelHandles` struct holding all four channel sender/receiver pairs, intended to be split across threads at startup
- [ ] Task: Verify all enums implement `Debug`, `Clone`, `Send` — run tests
- [ ] Task: Conductor — User Manual Verification 'Phase 2: Typed Channel Message Definitions' (Protocol in workflow.md)

---

## Phase 3: Database Thread (T3)

### Tasks

- [~] Task: Write failing integration test — send `DbCommand::Ping`, expect `DbResult::Pong` within 5ms
- [~] Task: Implement `fe-database::spawn_db_thread(rx: Receiver<DbCommand>, tx: Sender<DbResult>)`:
  - Spawns a dedicated OS thread
  - Creates an isolated `tokio::runtime::Builder::new_multi_thread().build()` runtime on that thread
  - Asserts `Handle::try_current()` returns `Err` before creating the runtime (proves isolation)
  - Initialises SurrealDB with `SurrealKV` backend and `SURREAL_SYNC_DATA=true`
  - Enters a `loop { select! { cmd = rx.recv() => { ... } } }` processing `DbCommand::Ping` → `DbResult::Pong`
  - Shuts down cleanly on `DbCommand::Shutdown`
- [~] Task: Log thread startup and shutdown events via `tracing::info!`
- [ ] Task: Run integration test — assert `DbResult::Pong` received within 5ms
- [ ] Task: Verify SurrealDB connects successfully with SurrealKV backend (assert no error on startup)
- [ ] Task: Conductor — User Manual Verification 'Phase 3: Database Thread (T3)' (Protocol in workflow.md)

---

## Phase 4: Network Thread (T2)

### Tasks

- [~] Task: Write failing integration test — send `NetworkCommand::Ping`, expect `NetworkEvent::Pong` within 2ms
- [~] Task: Implement `fe-network::spawn_network_thread(rx: Receiver<NetworkCommand>, tx: Sender<NetworkEvent>)`:
  - Spawns a dedicated OS thread
  - Creates an isolated `tokio::runtime::Builder::new_multi_thread().build()` runtime on that thread
  - Asserts `Handle::try_current()` returns `Err` before creating the runtime (proves isolation)
  - Sends `NetworkEvent::Started` to Bevy on successful startup
  - Enters a `loop { select! { cmd = rx.recv() => { ... } } }` processing `NetworkCommand::Ping` → `NetworkEvent::Pong`
  - Shuts down cleanly on `NetworkCommand::Shutdown`
  - Placeholder comment: `// TODO Track 4: initialise libp2p Swarm and iroh endpoint here`
- [~] Task: Log thread startup and shutdown events via `tracing::info!`
- [ ] Task: Run integration test — assert `NetworkEvent::Pong` received within 2ms
- [ ] Task: Conductor — User Manual Verification 'Phase 4: Network Thread (T2)' (Protocol in workflow.md)

---

## Phase 5: Bevy App Integration (T1)

### Tasks

- [~] Task: Write failing test — Bevy app starts, all three threads report `Started`/`Pong` within 2 seconds
- [~] Task: Implement `fe-runtime::build_app()`:
  - Creates all four `crossbeam::bounded(256)` channel pairs
  - Calls `fe-network::spawn_network_thread(...)` with the network channel pair
  - Calls `fe-database::spawn_db_thread(...)` with the database channel pair
  - Inserts `NetworkCommandSender`, `DbCommandSender` as Bevy `Resource`s
  - Inserts `NetworkEventReceiver`, `DbResultReceiver` as Bevy `Resource`s (wrapped in `Arc<Mutex<Receiver<_>>>` for `Send` compatibility)
  - Adds `drain_network_events` and `drain_db_results` Bevy systems to the `Update` schedule
- [~] Task: Implement `drain_network_events` system:
  - Non-blocking `try_recv()` loop — drains all pending events per frame
  - Emits each `NetworkEvent` into Bevy's `EventWriter<NetworkEvent>`
  - Never blocks or sleeps
- [~] Task: Implement `drain_db_results` system:
  - Non-blocking `try_recv()` loop — drains all pending results per frame
  - Emits each `DbResult` into Bevy's `EventWriter<DbResult>`
  - Never blocks or sleeps
- [~] Task: Add `FrameTimeDiagnosticsPlugin` to the Bevy app for frame time monitoring
- [ ] Task: Run startup test — assert all three threads report ready within 2 seconds
- [ ] Task: Conductor — User Manual Verification 'Phase 5: Bevy App Integration (T1)' (Protocol in workflow.md)

---

## Phase 6: Integration Tests and Frame Budget Validation

### Tasks

- [~] Task: Write round-trip latency integration test:
  - Send `NetworkCommand::Ping` from a Bevy system
  - Receive `NetworkEvent::Pong` in the next Bevy frame via `drain_network_events`
  - Assert total elapsed time < 2ms
- [~] Task: Write DB round-trip latency integration test:
  - Send `DbCommand::Ping` from a Bevy system
  - Receive `DbResult::Pong` via `drain_db_results`
  - Assert total elapsed time < 5ms
- [~] Task: Write frame budget test:
  // DEFERRED TO WAVE 6 VALIDATION — requires headless Bevy runner
- [~] Task: Write thread isolation assertion test:
  - From the main thread, assert `tokio::runtime::Handle::try_current()` returns `Err` (no ambient Tokio context on Bevy's thread)
- [~] Task: Write clean shutdown test:
  - Send `NetworkCommand::Shutdown` and `DbCommand::Shutdown`
  - Assert both threads exit within 1 second
  - Assert no panics or resource leaks (verified by absence of `tracing::error!` output)
- [ ] Task: Run full test suite — `cargo test` passes all tests with `cargo tarpaulin` >80% coverage
- [ ] Task: Run `cargo clippy -- -D warnings` and `cargo fmt --check` — both pass
- [ ] Task: Conductor — User Manual Verification 'Phase 6: Integration Tests and Frame Budget Validation' (Protocol in workflow.md)

---

## Completion Checklist

- [ ] All 6 phases complete with phase checkpoints committed
- [ ] `cargo test` green
- [ ] `cargo clippy -- -D warnings` clean
- [ ] `cargo fmt --check` clean
- [ ] Code coverage >80% (`cargo tarpaulin`)
- [ ] No `unwrap()` / `expect()` in production paths
- [ ] No `block_on()` in any Bevy system
- [ ] Thread isolation verified by test
- [ ] Frame budget test passes (no frame > 20ms under synthetic load)
- [ ] Track marked complete in `conductor/tracks.md`
