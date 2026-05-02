# Implementation Plan: Fix Silent Channel Send Error Swallowing

## Overview

Replace every `.ok()` on crossbeam channel sends with explicit `warn!()` logging. This is a reliability fix — no behavioural change under normal operation, but catastrophic thread failures now become visible in logs.

---

## Phase 1: `navigation_manager.rs`

Goal: Fix `.ok()` in replica lifecycle system.

Tasks:

- [ ] Task 1.1: Fix `CloseVerseReplica` send
  - In `handle_verse_replica_lifecycle`, replace:
    ```rust
    sync.0.send(...).ok();
    ```
    with:
    ```rust
    if let Err(e) = sync.0.send(...) {
        bevy::log::warn!("Failed to send CloseVerseReplica: {}", e);
    }
    ```
  - Run `cargo check -p fe-ui`.

- [ ] Task 1.2: Fix `OpenVerseReplica` send
  - In `open_replica`, replace `.ok()` with the same explicit error logging pattern.
  - Run `cargo check -p fe-ui`.

- [ ] Task 1.3: Verify via grep
  - `grep -n "\.ok()" fe-ui/src/navigation_manager.rs` — confirm zero hits on channel sends.

---

## Phase 2: `verse_manager.rs`

Goal: Fix all `.ok()` calls in `apply_db_results` and helpers.

Tasks:

- [ ] Task 2.1: Audit and list all `.ok()` on channel sends
  - `grep -n "\.ok()" fe-ui/src/verse_manager.rs | grep -i "send"`
  - Document each location in a scratch note.

- [ ] Task 2.2: Fix `apply_db_results` match arms
  - For each `db_sender.0.send(...).ok()` in the `DbResult` match:
    - Replace with `if let Err(e) = ... { bevy::log::warn!("DB send failed: {}", e); }`.
    - Use a consistent log message format: `"Failed to send <CommandName>: {}"`.
  - Run `cargo check -p fe-ui` after each batch.

- [ ] Task 2.3: Fix `refresh_inspector_tokens` helper
  - Replace `.ok()` on `ListApiTokens` and `ListApiTokensByScope` sends.
  - Run `cargo check -p fe-ui`.

- [ ] Task 2.4: Verify clean
  - `grep -n "\.ok()" fe-ui/src/verse_manager.rs | grep -i "send"` — confirm zero hits.

---

## Phase 3: `node_manager.rs`

Goal: Fix sync sender `.ok()` in `broadcast_transform`.

Tasks:

- [ ] Task 3.1: Fix `SyncCommand::UpdateNodeTransform` send
  - The db_sender already has the correct pattern (custom `is_err()` + `warn!`).
  - The sync_sender uses `.ok()` — replace with the same `is_err()` + `warn!()` pattern for consistency.
  - Run `cargo check -p fe-ui`.

---

## Phase 4: Final Verification

Tasks:

- [ ] Task 4.1: Project-wide audit
  - `grep -rn "\.ok()" fe-ui/src/ | grep -i "send"` — confirm zero hits.
  - `grep -rn "\.ok()" fe-renderer/src/ | grep -i "send"` — check for stray patterns.
  - `grep -rn "\.ok()" fe-sync/src/ | grep -i "send"` — check for stray patterns.

- [ ] Task 4.2: Clippy + tests
  - `cargo clippy -- -D warnings -p fe-ui`
  - `cargo test -p fe-ui`

- [ ] Verification: Phase 4 Final Verification [checkpoint marker]
  - Confirm no `.ok()` on channel sends remains in `fe-ui`.
  - Confirm `cargo test` passes.
  - Confirm `cargo clippy` passes.

---

## Completion Criteria

- [ ] Zero `.ok()` calls on crossbeam channel sends in `fe-ui`.
- [ ] All sends have `warn!()` on failure.
- [ ] `cargo clippy -- -D warnings -p fe-ui` passes.
- [ ] `cargo test -p fe-ui` passes.
