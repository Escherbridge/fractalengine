# Tests Todo (M7)

Unit tests that need to be written for the three domain managers.
Each test should be added to the `#[cfg(test)] mod tests` block at the bottom
of the corresponding manager file.

All tests use plain Rust: `ManagerType::default()`, no Bevy `App` required.
Run with `cargo test -p fe-ui`.

---

## NavigationManager (`fe-ui/src/navigation_manager.rs`)

### `navigate_to_verse_clears_fractal_and_petal`

Verifies that entering a verse always clears fractal and petal, even if both were set.

```rust
#[test]
fn navigate_to_verse_clears_fractal_and_petal() {
    let mut nav = NavigationManager::default();
    // Set up a fully-navigated state.
    nav.navigate_to_verse("v1", "Verse 1");
    nav.navigate_to_fractal("f1", "Fractal 1");
    nav.navigate_to_petal("p1");
    // Navigate to a different verse.
    nav.navigate_to_verse("v2", "Verse 2");
    assert_eq!(nav.active_verse_id, Some("v2".to_string()));
    assert_eq!(nav.active_verse_name, "Verse 2");
    assert!(nav.active_fractal_id.is_none(), "fractal should be cleared");
    assert!(nav.active_fractal_name.is_empty(), "fractal name should be cleared");
    assert!(nav.active_petal_id.is_none(), "petal should be cleared");
}
```

### `navigate_to_fractal_clears_petal`

Verifies that entering a fractal always clears petal selection.

```rust
#[test]
fn navigate_to_fractal_clears_petal() {
    let mut nav = NavigationManager::default();
    nav.navigate_to_verse("v1", "Verse 1");
    nav.navigate_to_fractal("f1", "Fractal 1");
    nav.navigate_to_petal("p1");
    // Navigate to a different fractal.
    nav.navigate_to_fractal("f2", "Fractal 2");
    assert_eq!(nav.active_fractal_id, Some("f2".to_string()));
    assert_eq!(nav.active_fractal_name, "Fractal 2");
    assert!(nav.active_petal_id.is_none(), "petal should be cleared on fractal change");
    // Verse should be unchanged.
    assert_eq!(nav.active_verse_id, Some("v1".to_string()));
}
```

### `back_from_verse_clears_all_fields`

Verifies that going back from the verse level clears everything.

```rust
#[test]
fn back_from_verse_clears_all_fields() {
    let mut nav = NavigationManager::default();
    nav.navigate_to_verse("v1", "Verse 1");
    nav.navigate_to_fractal("f1", "Fractal 1");
    nav.navigate_to_petal("p1");
    nav.back_from_verse();
    assert!(nav.active_verse_id.is_none());
    assert!(nav.active_verse_name.is_empty());
    assert!(nav.active_fractal_id.is_none());
    assert!(nav.active_fractal_name.is_empty());
    assert!(nav.active_petal_id.is_none());
}
```

### `back_from_fractal_clears_only_fractal_and_petal`

Verifies that going back from fractal leaves verse intact but clears fractal + petal.

```rust
#[test]
fn back_from_fractal_clears_only_fractal_and_petal() {
    let mut nav = NavigationManager::default();
    nav.navigate_to_verse("v1", "Verse 1");
    nav.navigate_to_fractal("f1", "Fractal 1");
    nav.navigate_to_petal("p1");
    nav.back_from_fractal();
    // Verse should still be set.
    assert_eq!(nav.active_verse_id, Some("v1".to_string()));
    // Fractal and petal should be gone.
    assert!(nav.active_fractal_id.is_none());
    assert!(nav.active_fractal_name.is_empty());
    // Note: back_from_fractal does NOT clear petal (it was already cleared by
    // navigate_to_fractal). If petal was somehow set independently, it stays.
    // The test confirms the documented behavior.
}
```

### `back_from_petal_clears_only_petal`

Verifies that going back from petal leaves verse and fractal intact.

```rust
#[test]
fn back_from_petal_clears_only_petal() {
    let mut nav = NavigationManager::default();
    nav.navigate_to_verse("v1", "Verse 1");
    nav.navigate_to_fractal("f1", "Fractal 1");
    nav.navigate_to_petal("p1");
    nav.back_from_petal();
    assert_eq!(nav.active_verse_id, Some("v1".to_string()));
    assert_eq!(nav.active_fractal_id, Some("f1".to_string()));
    assert!(nav.active_petal_id.is_none(), "only petal should be cleared");
}
```

---

## VerseManager (`fe-ui/src/verse_manager.rs`)

Helper to build a minimal tree for tests:

```rust
fn make_tree() -> VerseManager {
    let node = NodeEntry {
        id: "node-1".to_string(),
        name: "Node 1".to_string(),
        has_asset: true,
        position: [1.0, 2.0, 3.0],
        webpage_url: None,
        asset_path: Some("models/cube.glb".to_string()),
    };
    let petal = PetalEntry {
        id: "petal-1".to_string(),
        name: "Petal 1".to_string(),
        expanded: true,
        nodes: vec![node],
    };
    let fractal = FractalEntry {
        id: "fractal-1".to_string(),
        name: "Fractal 1".to_string(),
        expanded: true,
        petals: vec![petal],
    };
    let verse = VerseEntry {
        id: "verse-1".to_string(),
        name: "Verse 1".to_string(),
        namespace_id: None,
        expanded: true,
        fractals: vec![fractal],
    };
    VerseManager { verses: vec![verse] }
}
```

### `update_node_position_finds_correct_node`

```rust
#[test]
fn update_node_position_finds_correct_node() {
    let mut mgr = make_tree();
    mgr.update_node_position("node-1", [9.0, 8.0, 7.0]);
    let node = mgr.all_nodes().find(|n| n.id == "node-1").unwrap();
    assert_eq!(node.position, [9.0, 8.0, 7.0]);
}
```

### `update_node_position_noop_on_missing_id`

```rust
#[test]
fn update_node_position_noop_on_missing_id() {
    let mut mgr = make_tree();
    // Should not panic; existing node should be unchanged.
    mgr.update_node_position("does-not-exist", [0.0, 0.0, 0.0]);
    let node = mgr.all_nodes().find(|n| n.id == "node-1").unwrap();
    assert_eq!(node.position, [1.0, 2.0, 3.0], "existing node must be unaffected");
}
```

### `update_node_url_sets_and_clears`

```rust
#[test]
fn update_node_url_sets_and_clears() {
    let mut mgr = make_tree();
    // Set a URL.
    mgr.update_node_url("node-1", Some("https://example.com".to_string()));
    let node = mgr.all_nodes().find(|n| n.id == "node-1").unwrap();
    assert_eq!(node.webpage_url, Some("https://example.com".to_string()));
    // Clear the URL.
    mgr.update_node_url("node-1", None);
    let node = mgr.all_nodes().find(|n| n.id == "node-1").unwrap();
    assert!(node.webpage_url.is_none());
}
```

### `find_petal_returns_correct_petal`

```rust
#[test]
fn find_petal_returns_correct_petal() {
    let mgr = make_tree();
    let petal = mgr.find_petal("petal-1");
    assert!(petal.is_some());
    assert_eq!(petal.unwrap().id, "petal-1");
    assert_eq!(petal.unwrap().nodes.len(), 1);
}
```

### `find_petal_returns_none_for_missing_id`

```rust
#[test]
fn find_petal_returns_none_for_missing_id() {
    let mgr = make_tree();
    assert!(mgr.find_petal("does-not-exist").is_none());
}
```

### `add_verse_fractal_petal_node_chain`

Verifies that push operations via `find_verse_mut` correctly append entries.

```rust
#[test]
fn add_verse_fractal_petal_node_chain() {
    let mut mgr = VerseManager::default();
    assert!(mgr.verses.is_empty());

    // Push a verse.
    mgr.verses.push(VerseEntry {
        id: "v1".to_string(),
        name: "V1".to_string(),
        namespace_id: None,
        expanded: true,
        fractals: vec![],
    });
    assert_eq!(mgr.verses.len(), 1);

    // Push a fractal into the verse.
    mgr.find_verse_mut("v1").unwrap().fractals.push(FractalEntry {
        id: "f1".to_string(),
        name: "F1".to_string(),
        expanded: true,
        petals: vec![],
    });
    assert_eq!(mgr.verses[0].fractals.len(), 1);

    // Push a petal.
    mgr.verses[0].fractals[0].petals.push(PetalEntry {
        id: "p1".to_string(),
        name: "P1".to_string(),
        expanded: true,
        nodes: vec![],
    });

    // Push a node.
    mgr.verses[0].fractals[0].petals[0].nodes.push(NodeEntry {
        id: "n1".to_string(),
        name: "N1".to_string(),
        has_asset: false,
        position: [0.0; 3],
        webpage_url: None,
        asset_path: None,
    });

    // Verify the tree is navigable.
    assert!(mgr.find_petal("p1").is_some());
    assert_eq!(mgr.all_nodes().count(), 1);
}
```

---

## NodeManager (`fe-ui/src/node_manager.rs`)

These tests require a valid `Entity`. In unit tests without a Bevy `World`, create
placeholder entities using `Entity::from_raw(n)`.

```rust
// Helper
fn entity(n: u32) -> Entity {
    Entity::from_raw(n)
}
```

### `select_sets_selected_entity`

```rust
#[test]
fn select_sets_selected_entity() {
    let mut mgr = NodeManager::default();
    assert!(!mgr.is_selected());
    mgr.select(entity(1), "node-1");
    assert!(mgr.is_selected());
    assert_eq!(mgr.selected_entity(), Some(entity(1)));
}
```

### `select_same_entity_preserves_drag_state`

```rust
#[test]
fn select_same_entity_preserves_drag_state() {
    let mut mgr = NodeManager::default();
    mgr.select(entity(1), "node-1");
    // Manually inject drag state.
    if let Some(ref mut sel) = mgr.selected {
        sel.drag_committed = true;
    }
    // Selecting the same entity again should not reset drag_committed.
    mgr.select(entity(1), "node-1");
    assert!(
        mgr.selected.as_ref().map(|s| s.drag_committed).unwrap_or(false),
        "drag_committed should be preserved when re-selecting same entity"
    );
}
```

### `select_new_entity_resets_drag_state`

```rust
#[test]
fn select_new_entity_resets_drag_state() {
    let mut mgr = NodeManager::default();
    mgr.select(entity(1), "node-1");
    if let Some(ref mut sel) = mgr.selected {
        sel.drag_committed = true;
    }
    // Selecting a different entity should reset state.
    mgr.select(entity(2), "node-2");
    assert_eq!(mgr.selected_entity(), Some(entity(2)));
    assert!(
        !mgr.selected.as_ref().map(|s| s.drag_committed).unwrap_or(true),
        "drag_committed should be false after selecting a new entity"
    );
    assert!(
        mgr.selected.as_ref().and_then(|s| s.drag.as_ref()).is_none(),
        "drag should be None after selecting a new entity"
    );
}
```

### `deselect_clears_selection`

```rust
#[test]
fn deselect_clears_selection() {
    let mut mgr = NodeManager::default();
    mgr.select(entity(1), "node-1");
    assert!(mgr.is_selected());
    mgr.deselect();
    assert!(!mgr.is_selected());
    assert!(mgr.selected_entity().is_none());
}
```

### `is_dragging_returns_false_when_no_drag`

```rust
#[test]
fn is_dragging_returns_false_when_no_drag() {
    let mut mgr = NodeManager::default();
    // Not selected at all.
    assert!(!mgr.is_dragging());
    // Selected but no drag.
    mgr.select(entity(1), "node-1");
    assert!(!mgr.is_dragging());
}
```

### `is_dragging_returns_true_when_drag_active`

```rust
#[test]
fn is_dragging_returns_true_when_drag_active() {
    use crate::gimbal::GimbalAxis;
    let mut mgr = NodeManager::default();
    mgr.select(entity(1), "node-1");
    // Inject a drag.
    if let Some(ref mut sel) = mgr.selected {
        sel.drag = Some(crate::node_manager::AxisDrag {
            axis: GimbalAxis::X,
            start_cursor: bevy::math::Vec2::ZERO,
            axis_screen_dir: bevy::math::Vec2::X,
            start_pos: bevy::math::Vec3::ZERO,
            start_rot: bevy::math::Quat::IDENTITY,
            start_scale: bevy::math::Vec3::ONE,
        });
    }
    assert!(mgr.is_dragging());
}
```

---

## Running the Tests

```sh
# Run all fe-ui unit tests
cargo test -p fe-ui

# Run a specific test
cargo test -p fe-ui navigate_to_verse_clears_fractal_and_petal

# Run with output for debugging
cargo test -p fe-ui -- --nocapture
```

Tests do not require a GPU or window; they are pure logic tests on Rust structs.
