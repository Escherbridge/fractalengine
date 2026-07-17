---
type: Code Styleguide
title: Rust Style Guide
tags: [enforcement, 2026-07-10]
timestamp: 2026-07-17T00:00:00Z
---

# Rust Style Guide — FractalEngine

This guide covers Rust-specific conventions for FractalEngine. All rules build on `general.md`. Where this guide and `general.md` conflict, `general.md`'s Safety & Security rules win.

## Enforcement — Rust-Specific Patterns (concrete, greppable)

`general.md` states the shared principles (file size, doc comments, typed
writes, fail-closed capabilities, central policy engine, test sweep timing).
This section gives the exact commands to check them in this codebase.

| Rule (see `general.md`) | Command / pattern |
|---|---|
| File soft cap ~300 lines | `wc -l fe-*/src/**/*.rs \| sort -rn \| head` — anything materially over 300 is a decomposition candidate |
| Terse one-line doc comments | `rg -U '//!.*\n//!.*\n//!' <changed files>` — 3+ consecutive `//!` lines in a diff is a candidate for moving into the directory `AGENTS.md` |
| Typed writes for schema-typed tables | `rg 'InsertBuilder\|UpdateBuilder' fe-database/src/handlers/` — any hit touching a `geometry<...>`-typed column is the `§geometry-inserts` regression pattern; see `fe-database/src/AGENTS.md` |
| Fail-closed extension APIs | `rg 'pub fn' fe-plugin/src/host_env.rs` cross-checked against `rg 'require\('  fe-plugin/src/host_env.rs` — every routed operation should have a matching `require(token, CAP_*)` call before touching `self.storage`/`self.query` |
| Central policy engine | `rg 'const \w+_ROLES: &\[&str\]'` and `rg 'fn require_\w+\('` across the workspace — new hits outside `fe-policy` are the ad-hoc-role-check pattern this rule exists to stop (`fe-database/src/rbac.rs::WRITE_ROLES` and `fe-api/src/auth.rs::require_role` are the pre-existing examples, not templates to copy) |
| Handler success = persisted state | For any new `fe-database` handler, confirm a paired test does a read-back query (`SELECT ... WHERE`), not just an assertion on the handler's `Ok` return |

## Formatting

- **`rustfmt`** is mandatory. All code must pass `cargo fmt --check` — enforced
  by the CI lint gate in `.github/workflows/build-artifacts.yml` (runs in CI as
  of 2026-07-17).
- **`clippy`** is mandatory. All code must pass `cargo clippy -- -D warnings` —
  same CI lint gate.
- Line length: 100 characters (configured in `rustfmt.toml`).
- Trailing commas in multi-line expressions: always.
- **Every `#[allow(...)]` carries a same-line justification comment** (e.g.
  `fe-terrain/src/lod_ring.rs:35` — `// NaN must take the early-out branch`).
  A bare `#[allow(dead_code)]` is a review flag: either wire the code or delete
  it. Greppable check: `rg '#\[allow\([^)]*\)\]\s*$'` finds bare allows.

## Naming

| Item | Convention | Example |
|---|---|---|
| Types, Traits, Enums | `UpperCamelCase` | `PetalState`, `AssetIngester` |
| Functions, methods, variables | `snake_case` | `verify_session_token`, `petal_id` |
| Constants | `SCREAMING_SNAKE_CASE` | `MAX_ASSET_SIZE_MB` |
| Modules | `snake_case` | `fe_network`, `fe_database` |
| Lifetimes | short lowercase | `'a`, `'db` |
| Type parameters | single uppercase or short `UpperCamelCase` | `T`, `Db`, `Msg` |
| Bevy systems | verb-first imperative `snake_case`, no `_system` suffix | `apply_camera_focus`, `update_terrain_lod` |

Channel-drain systems are named `surface_<domain>_status` or `drain_<domain>`
(`surface_gpx_import_status`, `drain_db_results`). `gardener_ui_system`
(`fe-ui/src/plugin.rs`) is legacy, not a template.

Use the fractal naming system in code identifiers:

```rust
// Correct
struct PetalMetadata { ... }
fn create_petal(...) { ... }
enum NodeEvent { ... }

// Wrong
struct WorldMetadata { ... }
fn create_world(...) { ... }
enum ServerEvent { ... }
```

## Error Handling

- **Never use `.unwrap()` or `.expect()` in production code paths.** Reserve them for tests and `main()` startup assertions only.
- **Error-type policy is tiered by consumer:**
  - Crates with external / API-stable consumers (`fe-sdk`, `fe-database`,
    `fe-policy`, `fe-plugin` host surfaces) **must** expose typed `thiserror`
    enums from public functions.
  - Internal library crates (`fe-format`, `fe-query`, `fe-terrain` internals)
    **may** use `anyhow` with `.context()` at fallible boundaries.
  - Binaries (`fractalengine`, `fe-relay`) use `anyhow`.
  - Greppable check: `anyhow::Result` in a `pub fn` of a must-tier crate → flag.
- Error types must implement `std::error::Error` and provide context-rich messages.
- Propagate errors with `?`. Do not silently swallow errors.

```rust
// Correct
fn load_petal(id: &PetalId) -> Result<Petal, DatabaseError> {
    let record = db.query(id)?;
    Ok(record.into())
}

// Wrong
fn load_petal(id: &PetalId) -> Option<Petal> {
    db.query(id).ok().map(Into::into)
}
```

## `unsafe`

- **`unsafe` blocks require a `// SAFETY:` comment** explaining precisely why the invariants are upheld.
- `unsafe` is forbidden in all networking, authentication, RBAC, and WebView modules.
- Prefer safe abstractions. Only reach for `unsafe` when there is no safe alternative and the performance gain is measurable and documented.

```rust
// Correct
// SAFETY: `ptr` is guaranteed non-null by the caller contract documented in
// `AssetBuffer::as_ptr()`, which only returns a valid pointer from a non-empty Vec.
let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
```

## Async & Threading

- **Never call `block_on()` inside a Bevy system.** Bevy systems do no async
  work at all: they enqueue a typed command onto a dedicated thread's bounded
  channel, and a paired drain/`surface_*_status` system picks the result up in
  a later frame.
- **Never call `tokio::runtime::Handle::try_current()` from Bevy systems.** The Bevy executor and Tokio runtimes must remain completely isolated.
- **Cross-thread communication uses `crossbeam::channel::bounded` only.**
  Main command/event channels are sized by `fe_runtime::channels::CHANNEL_BUFFER`
  (256); oneshot replies use `bounded(1)`; small ad-hoc UI result channels use
  `bounded(8)`. `std::sync::mpsc` is legacy (one remaining site in
  `fe-database/src/lib.rs`) — do not add new uses. No `Arc<Mutex<T>>` as a
  communication primitive.
- Each dedicated thread (network, database, sync) owns its Tokio runtime. No runtime is shared between threads.

The real pattern, from `fe-ui/src/actions/path.rs` (enqueue) and
`fe-runtime/src/app.rs` (drain):

```rust
// Correct: enqueue a command onto the bounded DB channel; handle closure
pub(crate) fn query_tracks(
    db_sender: &DbCommandSender,
    path_state: &mut PathEditorState,
    petal_id: String,
) {
    let (sql, vars) = gis::track_query(&petal_id);
    path_state.tracks_pending = true;
    if db_sender.0.send(DbCommand::RawQuery { sql, vars }).is_err() {
        bevy::log::warn!("db_sender channel closed — Paths RawQuery not dispatched");
        path_state.tracks_pending = false;
    }
}

// Correct: paired drain system loops try_recv in Update — never blocks
fn drain_db_results(receiver: Res<DbResultReceiver>, mut writer: MessageWriter<DbResult>) {
    if let Ok(rx) = receiver.0.lock() {
        while let Ok(result) = rx.try_recv() {
            writer.write(result);
        }
    }
}

// Wrong: blocks the render loop
fn fetch_roles(db: Res<SurrealDbHandle>) {
    let roles = tokio::runtime::Handle::current().block_on(db.query_roles());
}
```

## Bevy ECS Conventions

- **Components** are plain data structs — no methods, no logic.
- **Systems** are pure functions that read components and emit events/commands.
- **Resources** hold shared state accessed by systems. Network and DB handles are Resources.
- **RBAC is never checked in systems.** Systems receive only pre-authorised data from SurrealDB queries.
- Use `EventWriter<T>` / `EventReader<T>` for cross-system communication within Bevy.
- Use typed crossbeam channels (wrapped as Resources, e.g. `DbCommandSender`) for Bevy ↔ network/DB thread communication.

## Cryptography Rules

- Import from `ed25519-dalek` only. Do not introduce alternative signing crates.
- Always call `VerifyingKey::verify_strict()` — never `verify()`.
- JWT creation and verification go through `fe_identity::jwt` only. No ad-hoc JWT logic elsewhere.
- All JWT tokens must include `sub: did:key:<multibase_pub>`. Use the `mint_session_token()` / `verify_session_token()` helpers — do not construct claims structs directly outside `fe-identity`.
- The Node's signing key lives behind `NodeIdentity` Resource. No system should hold a raw `SigningKey` — only a handle to the `NodeIdentity` Resource.

```rust
// Correct
fn sign_message(identity: Res<NodeIdentity>, msg: &[u8]) -> Signature {
    identity.sign(msg)
}

// Wrong
fn sign_message(key: &SigningKey, msg: &[u8]) -> Signature {
    key.sign(msg)
}
```

## Network & P2P Rules

- **All gossip messages carry a signature field.** The type system enforces this: `struct GossipMessage<T> { payload: T, sig: Signature, pub_key: VerifyingKey }`. Unsigned messages are not representable.
- **Verify signatures before any logic.** The first operation on any inbound network message is signature verification. If verification fails: log (peer pub key + failure reason) and drop. Never pass unverified data to application logic.
- **Rate limiting is structural.** The inbound channel from each peer is bounded (`crossbeam::bounded(N)`). Overflow drops the oldest message. No peer can block the network thread.

```rust
// Correct: verification at the boundary
fn handle_inbound(msg: GossipMessage<PetalEvent>, state: &mut NetworkState) {
    if msg.verify().is_err() {
        tracing::warn!(peer = %msg.pub_key, "signature verification failed — dropping message");
        return;
    }
    state.apply(msg.payload);
}
```

## WebView IPC Rules

- The JS↔Rust boundary is defined by a single versioned enum: `BrowserCommand`.
- **All IPC handlers are exhaustive match arms.** No wildcard `_` arms in command dispatch.
- **No string-based command dispatch.** Commands are typed enum variants serialized as JSON — never raw string matching.
- Log every IPC call at `tracing::debug!` level with the command variant name (not the full payload, which may contain URLs).

## Logging

- Use `tracing` crate throughout. No `println!` or `eprintln!` in production code.
- Security-relevant events log at `tracing::warn!` or above.
- Network events (connect, disconnect, JWT issue/reject, revocation) log at `tracing::info!`.
- Debug/trace instrumentation uses `#[tracing::instrument]` on public functions in networking and auth modules.
- Never log: private keys, JWT secrets, full JWT tokens, peer IP addresses (log peer public key hash instead).

## Module Structure — Workspace Crates

`[workspace] members` in the root `Cargo.toml` is the source of truth (22
crates as of 2026-07-17; `fe-auth` was absorbed into `fe-database` on
2026-07-17 — `SessionCache` now lives at `fe_database::session_cache`).
Grouped by lane:

**Binaries**
- `fractalengine` — GUI binary (Bevy `DefaultPlugins` + `EguiPlugin`)
- `fractalengine-relay` — headless `fe-relay` binary (`MinimalPlugins` + `ScheduleRunnerPlugin`)

**Engine & UI**
- `fe-runtime` — thread topology, channel definitions (`CHANNEL_BUFFER`), app startup
- `fe-renderer` — Bevy render systems, GLTF loading
- `fe-ui` — bevy_egui panels, dialogs, HUD, viewport interactions (largest crate)
- `fe-webview` — wry portal overlay, typed IPC bridge

**Data**
- `fe-database` — SurrealDB handle, schema, op-log, RBAC queries, `session_cache::SessionCache`
- `fe-entity-store` — entity/asset store: blob store, cache, compute, Bevy asset reader
- `fe-query` — `QueryBuilder` + GIS read/analytics lane
- `fe-format` — `.hexon` v1.0.0 ZIP archive format: manifest, licensing, Ed25519 signing
- `fe-api` — HTTP/WS API gateway

**Identity & Policy**
- `fe-identity` — keypair, `NodeIdentity`, JWT (`jwt::mint_session_token`), did:key
- `fe-policy` — deny-by-default `Policy` engine; canonical home of `RoleLevel`

**Network, P2P & Sync**
- `fe-network` — libp2p swarm, iroh endpoint, asset distribution
- `fe-sync` — iroh gossip sync, log-first WAL

**Terrain & GIS**
- `fe-terrain` — GPX, tile sources, terrain mesh, layers, IoT, first-party extension

**Hexon packaging**
- `fe-hexon` — hexon packaging, registry client, publisher, P2P distribution
- `fe-hexon-registry` — hosted hexon registry HTTP service

**Plugin system**
- `fe-plugin` — Rhai + WASM engines, capabilities, lifecycle
- `fe-sdk` — stable extension API (serde-only deps), UI slots
- `fe-plugin-test` — plugin test kit (`MockHostEnv`, fixtures, assertions)

**Test tooling**
- `fe-test-harness` — integration harness (package name `fractalengine-test-harness`)

## Testing

- Unit tests live in `#[cfg(test)]` modules within the source file they test.
- Integration tests live in `tests/` at the crate root.
- Every public function in `fe-identity`, `fe-network`, and `fe-database`'s `session_cache` must have at least one unit test.
- Security-critical paths (JWT sign/verify, signature verify, role check at DB) must have both success and failure case tests.
- Use `tokio::test` for async tests in the network and database modules.
- `unwrap()`/`expect()` are acceptable in `#[cfg(test)]` code; prefer
  `expect("reason")` when the invariant is non-obvious, and `?` with
  `#[tokio::test] -> Result` for async setup chains.

### Test harness (`fe-test-harness`)

- Package name is `fractalengine-test-harness` — import as
  `fractalengine_test_harness`, not `fe_test_harness` (the directory name is a
  trap).
- Reach for it for multi-thread / channel integration tests (runtime thread
  topologies, channel wiring, DB fixtures); plain `#[cfg(test)]` modules stay
  in-file for unit-level tests.
- Gotcha: the harness matches runtime message enums exhaustively — adding a
  variant to a `fe-runtime` message enum breaks harness matches; update the
  harness in the same change.
