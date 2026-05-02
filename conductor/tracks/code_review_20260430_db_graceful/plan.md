# Implementation Plan: Graceful Degradation on DB Init Failure

## Overview

Replace the `expect("SurrealDB init")` panic with a proper `Result` so the application can show a user-friendly error instead of crashing. This touches the DB crate API and the binary entry point.

---

## Phase 1: Define Error Type and Update DB Crate

Goal: Make DB thread spawning fallible.

Tasks:

- [ ] Task 1.1: Define `DbInitError`
  - In `fe-database/src/lib.rs`, add:
    ```rust
    #[derive(Debug, thiserror::Error)]
    pub enum DbInitError {
        #[error("Failed to open SurrealDB at {path}: {source}")]
        SurrealOpen { path: String, source: surrealdb::Error },
        #[error("Failed to build Tokio runtime: {0}")]
        RuntimeBuild(#[from] std::io::Error),
    }
    ```
  - Run `cargo check -p fe-database`.

- [ ] Task 1.2: Update `spawn_db_thread` signature
  - Change return type from `JoinHandle<()>` to `Result<JoinHandle<()>, DbInitError>`.
  - Replace `.expect("SurrealDB init")` with `?` or `map_err`:
    ```rust
    let db = surrealdb::Surreal::new::<SurrealKv>("data/fractalengine.db")
        .await
        .map_err(|e| DbInitError::SurrealOpen {
            path: "data/fractalengine.db".to_string(),
            source: e,
        })?;
    ```
  - Run `cargo check -p fe-database`.

- [ ] Task 1.3: Update `spawn_db_thread_with_sync` signature
  - Same `Result` return type.
  - Same `.expect()` → `?` conversion.
  - Run `cargo check -p fe-database`.

- [ ] Task 1.4: Write DB init failure tests (TDD Red)
  - Test: `spawn_db_thread_with_sync` with an invalid path (e.g., a read-only directory) returns `Err(DbInitError::SurrealOpen { .. })`.
  - Test: `spawn_db_thread` returns `Err` when given an unwritable path.
  - These may need temp directories or mock filesystems. Use `tempfile` crate if not already in deps.
  - Confirm tests fail (current code panics, doesn't return Err).

- [ ] Task 1.5: Make tests pass (TDD Green)
  - With the new `Result` signature, the tests should now pass.
  - Run `cargo test -p fe-database`.

---

## Phase 2: Update Binary Entry Point

Goal: Handle `DbInitError` in the application startup.

Tasks:

- [ ] Task 2.1: Find the caller site
  - Search for `spawn_db_thread` or `spawn_db_thread_with_sync` in `fractalengine/src/`.
  - Document the current call site and surrounding startup logic.

- [ ] Task 2.2: Add error handling
  - If the spawn returns `Err(e)`, log the error and show a fatal-error dialog.
  - Since this is before Bevy's window may be ready, options are:
    a. Use `rfd` or `native-dialog` for a blocking error dialog.
    b. Print to stderr and exit with non-zero code.
    c. Initialize a minimal Bevy app that only shows an error screen.
  - **Recommended**: Use `tracing::error!` + `eprintln!` + `std::process::exit(1)` as the minimal fix. A GUI error dialog can be a follow-up track.
  - Example:
    ```rust
    let db_handle = match fe_database::spawn_db_thread_with_sync(...) {
        Ok(handle) => handle,
        Err(e) => {
            tracing::error!("Failed to start database: {}", e);
            eprintln!("Fatal error: Could not initialize database.\n{}", e);
            eprintln!("Please ensure the 'data/' directory is writable.");
            std::process::exit(1);
        }
    };
    ```
  - Run `cargo check -p fractalengine`.

- [ ] Task 2.3: Update any other callers
  - Check `fe-test-harness/` and integration tests for callers.
  - Update them to handle the `Result` (or `.expect()` with a comment if test-only).
  - Run `cargo check --all`.

---

## Phase 3: Verification

Tasks:

- [ ] Task 3.1: Full test suite
  - `cargo test --all`

- [ ] Task 3.2: Simulate failure
  - Temporarily change the DB path to `/nonexistent/path/fractalengine.db`.
  - Run the binary — confirm it prints a clear error and exits gracefully instead of panicking.
  - Revert the temporary path change.

- [ ] Task 3.3: Clippy + format
  - `cargo clippy -- -D warnings`
  - `cargo fmt --check`

- [ ] Verification: Phase 3 Final Verification [checkpoint marker]
  - `cargo test --all` passes.
  - `cargo clippy -- -D warnings` passes.
  - Binary exits gracefully (non-zero, clear message) when DB path is invalid.
  - No `.expect("SurrealDB init")` remains in the codebase.

---

## Completion Criteria

- [ ] `spawn_db_thread` and `spawn_db_thread_with_sync` return `Result<_, DbInitError>`.
- [ ] `DbInitError` is a rich error type with context.
- [ ] Binary entry point handles DB init failure gracefully.
- [ ] `cargo test --all` passes.
- [ ] `cargo clippy -- -D warnings` passes.
