# Specification: Fix Hot-Path Performance Regressions

## Problem

Three performance anti-patterns exist in systems that run every frame or on every user interaction:

### Issue 1: O(n³) Node Lookup (`verse_manager.rs`)

`VerseManager::update_node_position` and `update_node_url` walk the entire verse/fractal/petal tree to find a single node by ID. This happens on **every drag commit** (mouse release during gimbal interaction).

```rust
pub fn update_node_position(&mut self, node_id: &str, position: [f32; 3]) {
    for verse in &mut self.verses {
        for fractal in &mut verse.fractals {
            for petal in &mut fractal.petals {
                if let Some(node) = petal.nodes.iter_mut().find(|n| n.id == node_id) {
                    node.position = position;
                    return;
                }
            }
        }
    }
}
```

### Issue 2: Per-Frame Vec Allocation (`node_manager.rs`)

`ring_points` allocates a `Vec<Vec3>` (48 elements ≈ 576 bytes) **every frame** during hover detection:

```rust
pub fn ring_points(center: Vec3, axis: Vec3, count: usize) -> Vec<Vec3> {
    // allocates every call
}
```

This is called from `update_hovered_axis` which runs every frame when a node is selected.

### Issue 3: Full Tree Traversal on Petal Switch (`verse_manager.rs`)

`respawn_on_petal_change` walks the entire verse tree to find the active petal, even though `find_petal` already exists:

```rust
for verse in &verse_mgr.verses {
    for fractal in &verse.fractals {
        for petal in &fractal.petals {
            if petal.id != *pid { continue; }
            // ...
        }
    }
}
```

## Acceptance Criteria

1. Node position/URL updates are O(1) average case (via HashMap).
2. `ring_points` does not allocate on the heap during hover detection.
3. `respawn_on_petal_change` uses `find_petal` instead of full tree traversal.
4. All existing tests pass.
5. `cargo clippy -- -D warnings -p fe-ui` passes.

## Scope

- `fe-ui/src/verse_manager.rs`
- `fe-ui/src/node_manager.rs` (gimbal hover path)
- `fe-ui/src/gimbal.rs` (ring_points signature)
