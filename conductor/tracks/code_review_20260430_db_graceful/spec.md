# Specification: Graceful Degradation on DB Init Failure

## Problem

`fe-database/src/lib.rs` panics if SurrealDB fails to initialize:

```rust
let db: surrealdb::Surreal<surrealdb::engine::local::Db> =
    surrealdb::Surreal::new::<SurrealKv>("data/fractalengine.db")
        .await
        .expect("SurrealDB init");
```

This `.expect()` crashes the entire application if:
- The `data/` directory is not writable (permissions).
- The disk is full.
- The database file is corrupted.
- Another process has the file locked.

In a production 3D editor, this should degrade gracefully — show an error dialog to the user instead of an opaque panic.

## Acceptance Criteria

1. `spawn_db_thread_with_sync` returns `Result<JoinHandle<()>, DbInitError>` instead of panicking.
2. The binary crate (`fractalengine/`) handles the error by showing a fatal-error dialog or message.
3. The error type carries enough context for the user to act (e.g., "Database file not writable: data/fractalengine.db").
4. All existing callers updated to handle the new `Result`.

## Scope

- `fe-database/src/lib.rs` — `spawn_db_thread` and `spawn_db_thread_with_sync`
- `fractalengine/src/main.rs` (or equivalent entry point) — caller site
- Any test callers that rely on the old non-fallible signature.
