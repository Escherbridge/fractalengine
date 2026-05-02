# Implementation Plan: Clippy Warnings, Code Quality, and Polish

## Overview

A cleanup track fixing compiler warnings, clippy lints, and code quality nits across `fe-ui`, `fe-webview`, and `fe-database`. Multiple small tasks — some mechanical (`cargo clippy --fix`), some requiring manual judgement.

---

## Phase 1: Mechanical Fixes (Run `cargo clippy --fix`)

Goal: Let clippy auto-fix what it can.

Tasks:

- [ ] Task 1.1: Auto-fix `fe-webview`
  - `cargo clippy --fix --lib -p fe-webview`
  - Review the diff — confirm only safe changes (redundant closures, unused imports, `clone()` → `Copy`).
  - Run `cargo test -p fe-webview`.

- [ ] Task 1.2: Auto-fix `fe-database`
  - `cargo clippy --fix --lib -p fe-database`
  - Review the diff.
  - Run `cargo test -p fe-database`.

- [ ] Task 1.3: Auto-fix `fe-ui`
  - `cargo clippy --fix --lib -p fe-ui`
  - Review the diff carefully — `fe-ui` has more complex code; some suggestions may need manual override.
  - Run `cargo test -p fe-ui`.

---

## Phase 2: `fe-ui` Manual Fixes

Goal: Fix issues that `cargo clippy --fix` cannot handle.

Tasks:

- [ ] Task 2.1: Fix unnecessary mutable query (Issue #6)
  - In `node_manager.rs`, change `compute_gimbal_center_inline` signature:
    ```rust
    // FROM:
    transform_query: &Query<(&mut Transform, &GlobalTransform)>,
    // TO:
    transform_query: &Query<&GlobalTransform>,
    ```
  - Update the one call site in `handle_gimbal_interaction`.
  - Run `cargo check -p fe-ui`.

- [ ] Task 2.2: Fix epsilon guard (Issue #7)
  - In `node_manager.rs`, `segment_dist_2d`:
    ```rust
    // FROM:
    let t = ((p - a).dot(ab) / ab.dot(ab).max(1e-6)).clamp(0.0, 1.0);
    // TO:
    let ab_len_sq = ab.dot(ab);
    if ab_len_sq < 1e-4 {
        return (p - a).length(); // degenerate segment — distance to endpoint
    }
    let t = ((p - a).dot(ab) / ab_len_sq).clamp(0.0, 1.0);
    ```
  - Run `cargo test -p fe-ui`.

- [ ] Task 2.3: Investigate `format!()` allocations (Issue #8)
  - In `sync_manager_to_inspector`, the `format!()` calls convert `f32` → `String` for the inspector form buffers.
  - **Decision needed**: Is this worth optimizing? The strings are only updated on selection change or `Changed<Transform>`, not every frame.
  - If optimizing: use a small stack buffer (e.g., `arrayvec` or `ryu` + `itoa` for no-alloc formatting).
  - If not: add a `// TODO: optimize if profiler shows this as hot` comment and move on.
  - Run `cargo test -p fe-ui`.

- [ ] Task 2.4: Fix brittle asset path concatenation (Issue #10)
  - In `verse_manager.rs`, `spawn_node_entity`:
    ```rust
    // FROM:
    let handle: Handle<Scene> = asset_server.load(format!("{}#Scene0", asset_path));
    // TO:
    let path = if asset_path.contains('#') {
        asset_path.to_string()
    } else {
        format!("{}#Scene0", asset_path)
    };
    let handle: Handle<Scene> = asset_server.load(path);
    ```
  - Run `cargo test -p fe-ui`.

- [ ] Task 2.5: Box large enum variant (Issue #11)
  - In `plugin.rs`, change:
    ```rust
    EntitySettings {
        // ... many fields
    }
    ```
    to:
    ```rust
    EntitySettings(Box<EntitySettingsData>),
    ```
  - Create `struct EntitySettingsData { /* move all fields here */ }`.
  - Update all pattern matches on `ActiveDialog::EntitySettings` to deref: `ActiveDialog::EntitySettings(data) => { data.active_tab = ... }`.
  - This is a large refactor — evaluate cost vs. benefit. If too disruptive, document the decision in `spec.md` and defer.
  - Run `cargo check -p fe-ui`.

- [ ] Task 2.6: Address 14-param `gardener_console` (Issue #12)
  - **Options**:
    a. Group into structs: `PanelInputs { ctx, sidebar, tool, ... }` and `PanelOutputs { camera_focus, ... }`.
    b. Accept as-is — AGENTS.md already acknowledges this as a known Bevy limitation.
  - If refactoring: update all call sites (likely just one in the main plugin).
  - If accepting: add `// NOTE: approaching Bevy 16-param limit; refactor if adding more params` comment.
  - Run `cargo check -p fe-ui`.

- [ ] Task 2.7: Remove dead `peer_registry` field (Issue #14)
  - Delete `peer_registry: Res<'w, fe_runtime::PeerRegistry>` from `P2pDialogParams`.
  - Run `cargo check -p fe-ui`.

- [ ] Task 2.8: Add `#[derive(Debug)]` (Issue #17)
  - Add to `NodeSelection` and `AxisDrag` in `node_manager.rs`.
  - Verify no fields block `Debug` (e.g., `Entity` already implements `Debug`).
  - Run `cargo check -p fe-ui`.

---

## Phase 3: Logging Consistency (Issue #16)

Goal: Pick one logging approach and document it.

Tasks:

- [ ] Task 3.1: Audit logging usage across `fe-ui`
  - `grep -rn "bevy::log::" fe-ui/src/ | head -20`
  - `grep -rn "tracing::" fe-ui/src/ | head -20`
  - Document which modules use which.

- [ ] Task 3.2: Standardize on `bevy::log` for ECS systems
  - ECS systems should use `bevy::log::info!` / `warn!` / `error!` because they integrate with Bevy's LogPlugin.
  - Replace any `tracing::` calls inside Bevy systems in `fe-ui`.
  - Run `cargo check -p fe-ui`.

- [ ] Task 3.3: Standardize on `tracing` for non-ECS code
  - DB thread, sync thread, network code should use `tracing` (they run outside Bevy's ECS).
  - Verify this is already the case; if not, migrate.
  - Run `cargo check` for affected crates.

---

## Phase 4: Final Verification

Tasks:

- [ ] Task 4.1: Clean build across all crates
  - `cargo clippy -- -D warnings` (workspace level)
  - `cargo fmt --check`
  - `cargo test` (all crates)

- [ ] Task 4.2: Warning count check
  - `cargo check --all 2>&1 | grep "^warning:" | wc -l` — target: < 5 (or zero if possible).

- [ ] Verification: Phase 4 Final Verification [checkpoint marker]
  - `cargo clippy -- -D warnings` passes for `fe-ui`, `fe-webview`, `fe-database`.
  - `cargo test` passes.
  - `cargo fmt --check` passes.
  - Zero dead-code warnings in `fe-ui`.
  - `NodeSelection` and `AxisDrag` have `Debug`.

---

## Completion Criteria

- [ ] All clippy warnings resolved or explicitly allowed with comments.
- [ ] `cargo clippy -- -D warnings` passes for `fe-ui`, `fe-webview`, `fe-database`.
- [ ] Logging approach documented (even if just via consistent usage).
- [ ] All tests pass.
