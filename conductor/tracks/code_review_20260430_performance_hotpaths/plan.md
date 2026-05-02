# Implementation Plan: Fix Hot-Path Performance Regressions

## Overview

Address three performance anti-patterns in frame-critical systems. This is a refactor + data structure change track. TDD where testable (HashMap index, const ring buffer); mechanical refactor where not (find_petal usage).

---

## Phase 1: O(1) Node Lookup Index

Goal: Replace O(n³) tree walk with HashMap-indexed access.

Tasks:

- [ ] Task 1.1: Add `node_index: HashMap<String, (usize, usize, usize)>` to `VerseManager`
  - Keys: `node_id`
  - Values: `(verse_idx, fractal_idx, petal_idx)` — indices into the tree.
  - Add `RebuildIndex` internal event or rebuild on every tree mutation.

- [ ] Task 1.2: Write index tests (TDD Red)
  - Test: `update_node_position` with 100 nodes in deep tree completes in < 1µs (or just verify it uses the index).
  - Test: After `HierarchyLoaded`, index contains all nodes.
  - Test: After `NodeCreated`, index contains new node.
  - Test: After `EntityDeleted`, index no longer contains removed node.
  - Confirm tests fail (index not yet implemented).

- [ ] Task 1.3: Build index on tree mutations
  - Add `rebuild_index(&mut self)` method.
  - Call it after `HierarchyLoaded`, `GltfImported`, `NodeCreated`, `EntityDeleted`.
  - Implement `update_node_position` and `update_node_url` to use the index:
    ```rust
    pub fn update_node_position(&mut self, node_id: &str, position: [f32; 3]) {
        if let Some(&(vi, fi, pi)) = self.node_index.get(node_id) {
            if let Some(node) = self.verses[vi].fractals[fi].petals[pi]
                .nodes.iter_mut().find(|n| n.id == node_id)
            {
                node.position = position;
            }
        }
    }
    ```
  - Run tests, confirm green.

- [ ] Task 1.4: Refactor — consider index maintenance vs. rebuild
  - If tree mutations are infrequent, `rebuild_index()` on each mutation is fine.
  - If frequent, incrementally update the index in `NodeCreated` / `EntityDeleted`.
  - Run `cargo test -p fe-ui`.

---

## Phase 2: Zero-Allocation Ring Points

Goal: Remove heap allocation from `ring_points` during hover detection.

Tasks:

- [ ] Task 2.1: Add const ring buffer helper
  - In `gimbal.rs`, add:
    ```rust
    const RING_SEGMENTS: usize = 48;
    pub fn ring_points_buf(center: Vec3, axis: Vec3, buf: &mut [Vec3; RING_SEGMENTS]) {
        let (perp1, perp2) = ring_basis(axis);
        for i in 0..RING_SEGMENTS {
            let angle = i as f32 * std::f32::consts::TAU / RING_SEGMENTS as f32;
            buf[i] = center + (perp1 * angle.cos() + perp2 * angle.sin()) * GIMBAL_LEN;
        }
    }
    ```
  - Keep the old `ring_points` (returning `Vec<Vec3>`) for any callers that truly need a Vec, or mark it `#[deprecated]`.

- [ ] Task 2.2: Update `ring_screen_distance` to use stack buffer
  - In `node_manager.rs`, change:
    ```rust
    let points = ring_points(center_3d, axis, 48);
    // ... iterate points
    ```
    to:
    ```rust
    let mut points = [Vec3::ZERO; 48];
    ring_points_buf(center_3d, axis, &mut points);
    // ... iterate points
    ```
  - Run `cargo check -p fe-ui`.

- [ ] Task 2.3: Write hover performance test (TDD Red → Green)
  - Test: `ring_screen_distance` with stack buffer does not allocate (can use `dhat` or just verify no `Vec::new` in the call path via code inspection).
  - Simpler test: `ring_points_buf` fills exactly 48 elements with correct geometry.
  - Run `cargo test -p fe-ui`.

---

## Phase 3: Use `find_petal` in `respawn_on_petal_change`

Goal: Replace full tree traversal with direct petal lookup.

Tasks:

- [ ] Task 3.1: Refactor `respawn_on_petal_change`
  - Replace the nested loops:
    ```rust
    for verse in &verse_mgr.verses {
        for fractal in &verse.fractals {
            for petal in &fractal.petals {
                if petal.id != *pid { continue; }
                // spawn nodes
            }
        }
    }
    ```
    with:
    ```rust
    if let Some(petal) = verse_mgr.find_petal(pid) {
        for node in &petal.nodes {
            // spawn nodes
        }
    }
    ```
  - Run `cargo check -p fe-ui`.

- [ ] Task 3.2: Verify no behavioural change
  - `cargo test -p fe-ui` — all tests pass.
  - The `find_petal` method already exists and is tested.

---

## Phase 4: Final Verification

Tasks:

- [ ] Task 4.1: Full test suite
  - `cargo test -p fe-ui`
  - `cargo test` (all crates)

- [ ] Task 4.2: Clippy + format
  - `cargo clippy -- -D warnings -p fe-ui`
  - `cargo fmt --check`

- [ ] Task 4.3: Code review the changes
  - Verify `node_index` is rebuilt correctly on all tree mutations.
  - Verify `ring_points_buf` is used by all hot-path callers.
  - Verify `find_petal` is used in `respawn_on_petal_change`.

- [ ] Verification: Phase 4 Final Verification [checkpoint marker]
  - `cargo test` passes.
  - `cargo clippy` passes.
  - No `Vec` allocation in `update_hovered_axis` call path.
  - `update_node_position` uses index lookup.
  - `respawn_on_petal_change` uses `find_petal`.

---

## Completion Criteria

- [ ] `VerseManager` maintains a `node_index: HashMap<String, (usize, usize, usize)>`.
- [ ] `update_node_position` and `update_node_url` are O(1) average case.
- [ ] `ring_points_buf` exists and is used by hover detection.
- [ ] `respawn_on_petal_change` uses `find_petal`.
- [ ] All tests pass.
- [ ] `cargo clippy -- -D warnings -p fe-ui` passes.
