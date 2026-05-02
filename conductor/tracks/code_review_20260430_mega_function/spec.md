# Specification: Refactor `apply_db_results` Mega-Function

## Problem

`fe-ui/src/verse_manager.rs` contains the `apply_db_results` Bevy system — a single function of approximately **450 lines** that processes every `DbResult` variant in one giant `match` block.

This violates single-responsibility and makes the code:
- **Hard to unit-test** — tests must construct the full `apply_db_results` system param set.
- **Hard to trace in stack traces** — 18 match arms, all in one function.
- **Hard to review** — changes to one variant require scrolling through unrelated arms.
- **Hard to extend** — adding a new `DbResult` variant means editing a 450-line function.

## Current Structure

```rust
fn apply_db_results(
    mut reader: MessageReader<DbResult>,
    mut verse_mgr: ResMut<VerseManager>,
    mut nav: ResMut<NavigationManager>,
    // ... ~12 more system params
) {
    for result in reader.read() {
        match result {
            DbResult::Seeded { .. } => { ... }
            DbResult::HierarchyLoaded { verses } => { /* ~80 lines */ }
            DbResult::GltfImported { .. } => { ... }
            DbResult::NodeCreated { .. } => { ... }
            // ... 15 more variants
            _ => {}
        }
    }
}
```

## Acceptance Criteria

1. Each `DbResult` variant handled in its own private handler function.
2. `apply_db_results` reduced to a thin dispatcher (match + function call).
3. No behavioural change — all existing tests pass without modification.
4. New tests can be written against individual handlers without spinning up Bevy ECS.

## Scope

- `fe-ui/src/verse_manager.rs` only.
- No changes to `DbResult` variants or message definitions.
