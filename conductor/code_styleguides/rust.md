# Rust Style Guide — FractalEngine

This guide covers Rust-specific conventions for FractalEngine. All rules build on `general.md`. Where this guide and `general.md` conflict, `general.md`'s Safety & Security rules win.

## Formatting

- **`rustfmt`** is mandatory. All code must pass `cargo fmt --check` in CI.
- **`clippy`** is mandatory. All code must pass `cargo clippy -- -D warnings` in CI.
- Line length: 100 characters (configured in `rustfmt.toml`).
- Trailing commas in multi-line expressions: always.

## Naming

| Item | Convention | Example |
|---|---|---|
| Types, Traits, Enums | `UpperCamelCase` | `PetalState`, `AssetIngester` |
| Functions, methods, variables | `snake_case` | `verify_session_token`, `petal_id` |
| Constants | `SCREAMING_SNAKE_CASE` | `MAX_ASSET_SIZE_MB` |
| Modules | `snake_case` | `fe_network`, `fe_database` |
| Lifetimes | short lowercase | `'a`, `'db` |
| Type parameters | single uppercase or short `UpperCamelCase` | `T`, `Db`, `Msg` |

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
- Use `thiserror` for library/module error types. Use `anyhow` for application-level error propagation.
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

- **Never call `block_on()` inside a Bevy system.** All async work from Bevy uses `AsyncComputeTaskPool::get().spawn(...)`.
- **Never call `tokio::runtime::Handle::try_current()` from Bevy systems.** The Bevy executor (smol) and Tokio runtimes must remain completely isolated.
- Cross-thread communication uses only typed `std::sync::mpsc` or `crossbeam` channels. No `Arc<Mutex<T>>` as a communication primitive.
- Each dedicated thread (T2 network, T3 database) owns its Tokio runtime. No runtime is shared between threads.

```rust
// Correct: enqueue work from a Bevy system
fn query_roles_system(
    db: Res<DbCommandSender>,
    task_pool: Res<AsyncComputeTaskPool>,
    mut commands: Commands,
) {
    let db = db.0.clone();
    let task = task_pool.spawn(async move {
        db.send(DbCommand::QueryRoles { petal_id }).await
    });
    commands.spawn(DbTask(task));
}

// Wrong: blocks the render loop
fn query_roles_system(db: Res<SurrealDbHandle>) {
    let roles = tokio::runtime::Handle::current().block_on(db.query_roles());
}
```

## Bevy ECS Conventions

- **Components** are plain data structs — no methods, no logic.
- **Systems** are pure functions that read components and emit events/commands.
- **Resources** hold shared state accessed by systems. Network and DB handles are Resources.
- **RBAC is never checked in systems.** Systems receive only pre-authorised data from SurrealDB queries.
- Use `EventWriter<T>` / `EventReader<T>` for cross-system communication within Bevy.
- Use typed `mpsc` channels (wrapped as Resources) for Bevy ↔ network/DB thread communication.

## Cryptography Rules

- Import from `ed25519-dalek` only. Do not introduce alternative signing crates.
- Always call `VerifyingKey::verify_strict()` — never `verify()`.
- JWT creation and verification go through `fe_auth::session` module only. No ad-hoc JWT logic elsewhere.
- All JWT tokens must include `sub: did:key:<multibase_pub>`. Use the `build_session_token()` helper — do not construct claims structs directly outside the auth module.
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

## Module Structure

```
fractalengine/
├── fe-runtime/       # Thread topology, channel definitions, app startup
├── fe-identity/      # Keypair, NodeIdentity, JWT, did:key
├── fe-database/      # SurrealDB handle, schema, op-log, RBAC queries
├── fe-network/       # libp2p swarm, iroh endpoint, gossip, asset distribution
├── fe-world/         # Bevy ECS: Petal, Room, Model, BrowserInteraction components
├── fe-renderer/      # Bevy render systems, GLTF loading, dead-reckoning
├── fe-webview/       # wry thin plugin, overlay positioning, typed IPC
├── fe-auth/          # Session handshake, SessionCache, revocation
└── fe-ui/            # bevy_egui admin panels, HUD, role chip
```

## Testing

- Unit tests live in `#[cfg(test)]` modules within the source file they test.
- Integration tests live in `tests/` at the crate root.
- Every public function in `fe-identity`, `fe-auth`, and `fe-network` must have at least one unit test.
- Security-critical paths (JWT sign/verify, signature verify, role check at DB) must have both success and failure case tests.
- Use `tokio::test` for async tests in the network and database modules.
- Never use `unwrap()` in tests — use `?` with `#[tokio::test]` or `assert!(result.is_ok(), "{:?}", result)`.
