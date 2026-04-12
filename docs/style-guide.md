# Style Guide

Code style and conventions for the FractalEngine codebase.
Follow these rules when adding or modifying any file in this repo.

---

## 1. Bevy Patterns

### Always use Plugin structs to register systems and resources

Every domain feature must be packaged as a `Plugin`. Never add systems or resources
directly in `main.rs` — do it in the plugin's `build()` method.

```rust
// CORRECT
pub struct MyFeaturePlugin;
impl Plugin for MyFeaturePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MyResource>();
        app.add_systems(Update, my_system);
    }
}

// WRONG — don't scatter registrations
app.add_systems(Update, my_system);  // directly in main.rs
```

### Chain systems deterministically with `.chain()` for ordered execution

If system B reads data that system A writes in the same frame, chain them:

```rust
app.add_systems(
    Update,
    (system_a, system_b, system_c).chain(),
);
```

Without `.chain()`, Bevy may run them in any order (or in parallel on multi-core).
The NodeManagerPlugin chains all 7 of its systems for this reason.

### Use `Changed<T>` filter for systems that should only run when data changes

```rust
// CORRECT — only runs when Transform has been mutated this frame
fn sync_transform(query: Query<&Transform, Changed<Transform>>) { ... }

// WRONG — runs every frame, wastes work
fn sync_transform(query: Query<&Transform>) { ... }
```

### Use `Local<T>` for per-system state that doesn't need to be a Resource

`Local<T>` is initialized once per system instance and not shared:

```rust
fn respawn_on_petal_change(
    nav: Res<NavigationManager>,
    mut last: Local<Option<String>>,   // "what was active last frame"
    mut initialized: Local<bool>,
) {
    if !*initialized { *initialized = true; *last = nav.active_petal_id.clone(); return; }
    if *last == nav.active_petal_id { return; }
    // ... do work ...
    *last = nav.active_petal_id.clone();
}
```

Don't promote per-system tracking values to global Resources just to avoid `Local`.

### Prefer `commands.entity(e).despawn()` over `.despawn_recursive()` unless hierarchy is needed

Spawned node entities (`SceneRoot`) are flat — they do not have a Bevy parent-child hierarchy
managed by this code. Use `.despawn()`. Reserve `.despawn_recursive()` only for entities that
genuinely own child entities via Bevy's `Parent`/`Children` components.

### Use `MessageReader<T>` (Bevy events) for DB and sync results, not shared state

Results from the DB thread and sync thread arrive as Bevy events consumed with `MessageReader<T>`.
Do not use a shared `Mutex<Vec<DbResult>>` or polling pattern.

```rust
fn apply_db_results(mut reader: MessageReader<DbResult>, ...) {
    for result in reader.read() {
        match result { ... }
    }
}
```

---

## 2. Manager Pattern Rules

### Each domain manager owns its data and systems — no other system mutates manager state directly

The three managers (`NavigationManager`, `VerseManager`, `NodeManager`) are the single source
of truth for their respective domains. Other systems read from them but do not write to their
fields directly.

```rust
// CORRECT — call a method that encodes the transition semantics
nav.navigate_to_verse(id, name);

// WRONG — direct field assignment bypasses clearing of downstream selections
nav.active_verse_id = Some(id);
// (this would leave active_fractal_id / active_petal_id stale)
```

### Managers expose methods for state transitions

All mutations go through named methods on the manager that enforce invariants:
- `NavigationManager::navigate_to_verse` always clears fractal + petal
- `NavigationManager::back_from_verse` clears all 3 fields
- `NodeManager::select(entity, node_id)` preserves drag state if same entity, resets if new

### UI code calls manager methods, never direct field assignments on manager resources

In panels.rs and other egui panel code, always call manager methods:

```rust
// CORRECT
if ui.button("Select").clicked() {
    node_manager.select(entity, node_id.clone());
}

// WRONG
node_manager.selected = Some(NodeSelection { entity, node_id, drag: None, drag_committed: false });
```

### Managers update in-memory state synchronously; DB persistence is async via channel

When a manager method changes state (e.g. updating a node position after a drag), the in-memory
change happens immediately in the system. The DB write is sent via channel and executes on the
DB thread asynchronously — it does not block the frame.

```rust
// In broadcast_transform:
verse_mgr.update_node_position(&marker.node_id, [pos.x, pos.y, pos.z]); // synchronous
db_sender.0.send(DbCommand::UpdateNodeTransform { ... });                 // async channel
```

---

## 3. Error Handling

### Never silently discard channel send failures with bare `.ok()` — always `warn!()` on failure

```rust
// CORRECT
if db_sender.0.send(cmd).is_err() {
    bevy::log::warn!("db_sender channel closed — DB thread may have crashed");
}

// WRONG — hides DB/sync thread crashes entirely
db_sender.0.send(cmd).ok();
```

Exception: sends in `apply_db_results` for `Seeded`/`VerseJoined`/`DatabaseReset` that just
re-trigger a `LoadHierarchy` can use `.ok()` because those are best-effort refreshes, not
critical mutations. Even then, adding a warn is preferred.

### DB/sync thread crashes should surface as user-visible errors

Currently errors are logged. The future direction is an error state resource that the UI
can display as a toast or banner. Match on `DbResult::Error` and at minimum log it:

```rust
DbResult::Error(msg) => {
    bevy::log::error!("DB error: {msg}");
    // TODO: set ErrorState resource for UI display
}
```

### Use `anyhow::Result` for all DB async functions

All functions in `fe-database/src/lib.rs` should return `anyhow::Result<T>`.
Do not use `Box<dyn std::error::Error>` or `String` for error propagation in DB code.

### Match on `DbResult::Error` and log with `bevy::log::error!()`

Never leave `DbResult::Error` as `_ => {}` — it must be explicitly handled.

---

## 4. Performance Rules

### No allocations (String, Vec, clone) in hot-path systems (systems running every frame)

Systems that run every frame for every entity (like `handle_gimbal_interaction`) must not
allocate. Check before adding `.clone()`, `String::from(...)`, or `Vec::new()` in hot loops.

```rust
// WRONG — allocates a String every frame in a hot system
let label = format!("{} (dragging)", node_name);

// CORRECT — defer string formatting to panel render, not update systems
```

### Use `Changed<T>` and `Added<T>` query filters to skip unnecessary work

```rust
// Only runs the system body when a Transform has been modified
fn my_system(query: Query<Entity, Changed<Transform>>) { ... }

// Only processes entities that were just spawned
fn my_system(query: Query<Entity, Added<SpawnedNodeMarker>>) { ... }
```

### Keep per-frame system logic O(1) or O(selected_entities), not O(hierarchy_size)

`update_node_position` is O(N) over the whole tree — that is fine when called once per
committed drag. It must NOT be called every frame. Systems that run every frame should
only touch `NodeManager.selected` (O(1)) or iterate over spawned entities (O(active petal nodes)).

### Extract O(N) traversals into VerseManager/NavigationManager methods

If a system needs to search the tree, add a method to `VerseManager` and call it. This
keeps the system body clean and makes the traversal testable in isolation.

---

## 5. File Organization

### One manager per file

- `fe-ui/src/verse_manager.rs` — VerseManager + tree types + VerseManagerPlugin
- `fe-ui/src/navigation_manager.rs` — NavigationManager + NavigationManagerPlugin
- `fe-ui/src/node_manager.rs` — NodeManager + NodeManagerPlugin + all 7 chained systems

Do not merge managers into a single file or split a manager across multiple files.

### Pure visual rendering helpers in gimbal.rs (no state)

`gimbal.rs` contains only:
- `GimbalAxis` enum
- Visual constants (colors, lengths)
- `draw_gimbal_for_entity()` and its helper drawing functions

It must not own any Bevy Resources or Systems. Any state that affects gimbal rendering
(which axis is active, which tool is selected) is passed in as parameters.

### UI panels in panels.rs; dialogs in dialogs.rs; viewport browsers in viewport.rs

Large panel functions should be extracted into separate files rather than growing `panels.rs`
beyond ~800 lines. The current split:
- `panels.rs`: main `gardener_console()` entry point, toolbar, sidebar, inspector
- Future: `dialogs.rs` for create/import/invite/join dialogs once they grow further

### Database command handlers extracted as async fns in fe-database/src/lib.rs

Each DB operation (create verse, import GLTF, update transform, etc.) is a standalone
`async fn` in `fe-database/src/lib.rs`. The DB thread loop calls them via `tokio::spawn`
or direct `.await`. Do not inline SQL queries in the sync/UI crates.

---

## 6. Testing Requirements

### All manager state transitions must have unit tests

Every public method on `NavigationManager`, `VerseManager`, and `NodeManager` that changes
state must have at least one unit test covering:
- The happy path (expected state after the transition)
- An edge case (empty hierarchy, None selection, re-selecting same entity, etc.)

### Tests live in `#[cfg(test)] mod tests` at the bottom of each manager file

```rust
// At the bottom of navigation_manager.rs:
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigate_to_verse_clears_fractal_and_petal() {
        let mut nav = NavigationManager::default();
        nav.navigate_to_fractal("f1", "Fractal 1");
        nav.navigate_to_petal("p1");
        nav.navigate_to_verse("v2", "Verse 2");
        assert_eq!(nav.active_verse_id, Some("v2".to_string()));
        assert!(nav.active_fractal_id.is_none());
        assert!(nav.active_petal_id.is_none());
    }
}
```

### Use default() constructors; no Bevy App needed for pure logic tests

Manager state transitions are pure logic on Rust structs — they do not require a running
Bevy `App`. Tests should call `ManagerType::default()` and then call methods directly.
This keeps tests fast (no Bevy startup) and runnable with plain `cargo test`.

### Test both happy path and edge cases (empty hierarchy, None selection, etc.)

For every test group:
1. Happy path: the normal transition works correctly
2. Edge case: calling back_from_* when already at the top level (should be a no-op / clear gracefully)
3. Edge case: operating on an empty VerseManager (should not panic)
4. Edge case: selecting an entity that is already selected (NodeManager preserves drag)

See `docs/tests-todo.md` for the full list of specific test cases that need to be written.
