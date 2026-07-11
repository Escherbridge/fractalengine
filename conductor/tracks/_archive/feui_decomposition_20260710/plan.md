---
type: Implementation Plan
title: fe-ui Decomposition
tags: [chore, feui_decomposition_20260710, in_progress]
timestamp: 2026-07-10T00:00:00Z
resource: ./metadata.json
---

# Implementation Plan: fe-ui Decomposition

**Track ID:** `feui_decomposition_20260710`
**Type:** Chore (refactor)
**Crate:** `fe-ui`

See [./spec.md](./spec.md). Each phase is a pure physical split — no logic
changes, no behavior regressions. Order follows file size, largest first,
since later phases benefit from earlier ones establishing the module
pattern.

---

## Phase 1: Split panels.rs (FR-1)

**Files touched:** `panels.rs` → `panels/{mod,toolbar,sidebar,inspector,status_bar}.rs`

### Tasks

- [ ] Task 1.1: Extract toolbar rendering into `panels/toolbar.rs` (TDD: existing panel tests still pass after the move — no new test needed, this is a pure move)
- [ ] Task 1.2: Extract sidebar rendering into `panels/sidebar.rs`
- [ ] Task 1.3: Extract inspector rendering into `panels/inspector.rs` (split further by `InspectorTab` variant if still >300 lines)
- [ ] Task 1.4: Extract status bar into `panels/status_bar.rs`; leave `gardener_console()` in `panels/mod.rs` as dispatcher
- [ ] Verification: `cargo build -p fe-ui`, `cargo test -p fe-ui`, `cargo clippy -p fe-ui -- -D warnings`. Manual: launch app, confirm toolbar/sidebar/inspector/status bar render identically. [checkpoint marker]

## Phase 2: Split plugin.rs (FR-2)

**Files touched:** `plugin.rs` → thin `Plugin` impl; `resources/{actions,dialogs,inspector_state,petal_map,actions_dispatch}.rs`

### Tasks

- [ ] Task 2.1: Move `UiAction`/`UiManager` to `resources/actions.rs`
- [ ] Task 2.2: Move `ActiveDialog` to `resources/dialogs.rs`
- [ ] Task 2.3: Move `InspectorFormState` to `resources/inspector_state.rs`
- [ ] Task 2.4: Move `PetalMapState` + `tileset_to_terrain_json` to `resources/petal_map.rs`
- [ ] Task 2.5: Move `process_ui_actions` to `resources/actions_dispatch.rs`
- [ ] Verification: `cargo build -p fe-ui`, `cargo test -p fe-ui`. Manual: portal open/close/back, dialog open/close, URL save all still work. [checkpoint marker]

## Phase 3: Split dialogs.rs (FR-3)

**Files touched:** `dialogs.rs` → `dialogs/{mod,context_menu,create_dialog,gltf_import,node_options,invite_dialog,join_dialog,peer_debug}.rs`

### Tasks

- [ ] Task 3.1: One extraction task per dialog variant (7 files) — same pattern each time: move render fn, update `dialogs/mod.rs` dispatcher match arm
- [ ] Verification: `cargo test -p fe-ui`. Manual: open every dialog type, confirm mutual exclusion still holds. [checkpoint marker]

## Phase 4: Split verse_manager.rs (FR-4) — also closes code_review_20260430_mega_function

**Files touched:** `verse_manager.rs` → `verse_manager/{mod,tree,handlers}.rs`

### Tasks

- [ ] Task 4.1: Move `VerseEntry`/`NodeEntry`/tree types + `VerseManager` methods to `verse_manager/tree.rs`
- [ ] Task 4.2: Extract each `DbResult` match arm from `apply_db_results` into `verse_manager/handlers.rs::handle_<variant>()`, leave `apply_db_results` as pure dispatch in `mod.rs`
- [ ] Task 4.3: Update `conductor/tracks.md` — mark `code_review_20260430_mega_function` `[x]` referencing this task's commit, since this FR resolves it
- [ ] Verification: `cargo test -p fe-ui`. Manual: hierarchy load, GLTF import, verse join, petal terrain load all still update state correctly. [checkpoint marker]

## Phase 5: Split node_manager.rs (FR-5)

**Files touched:** `node_manager.rs` → `node_manager/{mod,systems/*}.rs`

### Tasks

- [ ] Task 5.1: Extract each of the 7 chained systems into `node_manager/systems/<name>.rs`, preserve `.chain()` order exactly in `mod.rs`
- [ ] Verification: `cargo test -p fe-ui`. Manual: selection, gimbal drag, tool shortcuts, transform broadcast all behave identically. [checkpoint marker]

## Phase 6: Docs + final sweep

**Files touched:** `AGENTS.md`

### Tasks

- [ ] Task 6.1: Update `AGENTS.md` "File Ownership Map" table for the new module layout
- [ ] Verification: full workspace sweep per `conductor/workflow.md` — `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test --workspace`. [checkpoint marker]
