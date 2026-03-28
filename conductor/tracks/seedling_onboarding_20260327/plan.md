# Implementation Plan: Seedling Onboarding -- Local/Peer Instance Bootstrap + Entity CRUD

## Overview

This plan is divided into 6 phases, progressing from the data layer outward to the UI. Each phase builds on the previous, with verification checkpoints at each boundary.

- **Phase 1**: Database schema additions and DbCommand channel extension (data foundation)
- **Phase 2**: CRUD query functions for all entities (data operations)
- **Phase 3**: Onboarding state machine and first-launch detection (app flow)
- **Phase 4**: Onboarding UI screens (welcome, node setup, petal wizard)
- **Phase 5**: Entity management UI (sidebar lists, CRUD panels, dialogs)
- **Phase 6**: Peer connection UI and integration polish

---

## Phase 1: Schema & Channel Foundation

Goal: Extend the database schema with `node_profile` table, add new `OpType` variants, and expand `DbCommand`/`DbResult` to carry CRUD operations across the thread boundary.

Tasks:

- [ ] Task 1.1: Define `node_profile` schema in `fe-database/src/schema.rs` (TDD: write test asserting field definitions for `node_id`, `display_name`, `description`, `did_key`, `avatar_asset_id`, `created_at`, `updated_at`; implement the `DEFINE_NODE_PROFILE` constant; refactor)
- [ ] Task 1.2: Add new `OpType` variants to `fe-database/src/types.rs` (TDD: write test that all new variants — `CreateVerse`, `UpdateVerse`, `DeleteVerse`, `CreateFractal`, `UpdateFractal`, `DeleteFractal`, `UpdateNodeProfile`, `CreateNode`, `UpdateNode`, `DeleteNode`, `DeleteRoom` — serialize/deserialize correctly; implement; refactor)
- [ ] Task 1.3: Extend `DbCommand` enum in `fe-runtime/src/messages.rs` with CRUD variants (TDD: write tests for Debug/Clone of new variants — `CreateNodeProfile`, `GetNodeProfile`, `ListVerses`, `CreateVerse`, `UpdateVerse`, `DeleteVerse`, `ListFractals`, `CreateFractal`, `UpdateFractal`, `DeleteFractal`, `ListPetals`, `CreatePetal`, `UpdatePetal`, `DeletePetal`, `ListRooms`, `CreateRoom`, `UpdateRoom`, `DeleteRoom`, `ListNodes`, `CreateNode`, `UpdateNode`, `DeleteNode`; implement; refactor)
- [ ] Task 1.4: Extend `DbResult` enum with corresponding response variants (TDD: write tests for Debug/Clone of new result variants carrying typed payloads; implement; refactor)
- [ ] Task 1.5: Register `node_profile` schema in `rbac::apply_schema` (TDD: write test that `apply_schema` includes `DEFINE_NODE_PROFILE` execution; implement by adding `db.query(schema::DEFINE_NODE_PROFILE)` call; refactor)
- [ ] Verification: `cargo test -p fe-database -p fe-runtime`, `cargo clippy -- -D warnings` [checkpoint marker]

---

## Phase 2: CRUD Query Functions

Goal: Implement all database query functions for the entity hierarchy, called from the DB thread in response to `DbCommand` variants.

Tasks:

- [ ] Task 2.1: Implement `node_profile` CRUD queries in new `fe-database/src/node_profile.rs` (TDD: write tests for `create_node_profile`, `get_node_profile`, `update_node_profile` — assert round-trip via SurrealDB values; implement; refactor)
- [ ] Task 2.2: Implement Verse CRUD queries in `fe-database/src/queries.rs` or new `fe-database/src/verse_queries.rs` (TDD: write tests for `create_verse`, `list_verses`, `update_verse_name`, `delete_verse` — including guard that delete fails if fractals exist; implement; refactor)
- [ ] Task 2.3: Implement Fractal CRUD queries (TDD: write tests for `create_fractal`, `list_fractals_by_verse`, `update_fractal`, `delete_fractal` — including guard that delete fails if petals exist; implement; refactor)
- [ ] Task 2.4: Extend Petal queries with `list_petals_by_fractal`, `update_petal_name`, `delete_petal_cascade` (TDD: write tests asserting list returns only petals in the given fractal; delete cascades rooms and nodes; name update writes op-log; implement; refactor)
- [ ] Task 2.5: Extend Room queries with `list_rooms_by_petal`, `delete_room` (TDD: write tests for list filtering by petal_id; delete removes room and its contained nodes; implement; refactor)
- [ ] Task 2.6: Implement Node CRUD queries: `create_node`, `list_nodes_by_petal`, `update_node`, `delete_node` (TDD: write tests for full CRUD cycle; assert RBAC via `require_write_role`; implement; refactor)
- [ ] Task 2.7: Wire `DbCommand` dispatch in `fe-database/src/lib.rs` `spawn_db_thread` event loop (TDD: write test that sending a `DbCommand::ListVerses` through the channel returns a `DbResult::Verses` response; implement match arms for all new commands; refactor)
- [ ] Verification: `cargo test -p fe-database -p fe-runtime`, `cargo tarpaulin -p fe-database --out Html --output-dir coverage/` (target >80%) [checkpoint marker]

---

## Phase 3: Onboarding State Machine

Goal: Implement the first-launch detection logic and a Bevy `States`-based onboarding flow controller.

Tasks:

- [ ] Task 3.1: Define `OnboardingState` enum as a Bevy `States` in `fe-ui/src/onboarding/mod.rs` (TDD: write tests that `OnboardingState` has variants `Welcome`, `NodeSetup`, `PetalCreation`, `Complete` and derives necessary Bevy traits; implement; refactor)
- [ ] Task 3.2: Implement first-launch detection system (TDD: write test that if `load_keypair("local-node")` returns Err, the initial state is `OnboardingState::Welcome`; if it succeeds, state is `OnboardingState::Complete`; implement as a Bevy startup system; refactor)
- [ ] Task 3.3: Implement `OnboardingProgress` resource to carry wizard data across steps (TDD: write test for default values, serialization of intermediate state — node_name, petal_name, etc.; implement; refactor)
- [ ] Task 3.4: Implement state transition logic: `Welcome` -> `NodeSetup` -> `PetalCreation` -> `Complete` (TDD: write test that transition functions validate inputs — e.g., node name non-empty — before advancing; implement as Bevy systems gated by `in_state()`; refactor)
- [ ] Verification: `cargo test -p fe-ui`, ensure state machine transitions are deterministic [checkpoint marker]

---

## Phase 4: Onboarding UI Screens

Goal: Build the egui UI for each onboarding step, rendering over the Gardener Console when onboarding is active.

Tasks:

- [ ] Task 4.1: Implement Welcome screen UI in `fe-ui/src/onboarding/welcome.rs` (TDD: write test that welcome screen function produces an egui `CentralPanel` with "Get Started" and "Quick Start" buttons; implement using existing theme constants; refactor)
- [ ] Task 4.2: Implement Node Setup screen UI in `fe-ui/src/onboarding/node_setup.rs` (TDD: write test that node setup screen renders a text input for display name, a read-only `did:key` label, and a "Continue" button that is disabled when name is empty; implement; refactor)
- [ ] Task 4.3: Implement Petal Creation wizard screen in `fe-ui/src/onboarding/petal_creation.rs` (TDD: write test that wizard renders inputs for name, description, visibility dropdown, tags; "Create Petal" button disabled when name is empty; implement reusing `PetalWizardState` validation; refactor)
- [ ] Task 4.4: Wire onboarding UI to `DbCommand` channel — Node Setup sends `CreateNodeProfile`, Petal Creation sends `CreateVerse` + `CreateFractal` + `CreatePetal` (TDD: write test that on "Continue" with valid inputs, the appropriate `DbCommand` is sent to the channel; implement; refactor)
- [ ] Task 4.5: Implement "Quick Start" flow that auto-generates defaults and calls `seed_default_data` via `DbCommand::Seed` (TDD: write test that Quick Start transitions directly to `OnboardingState::Complete`; implement; refactor)
- [ ] Task 4.6: Gate Gardener Console rendering behind `OnboardingState::Complete` (TDD: write test that `gardener_ui_system` only runs when state is `Complete`; implement by adding `run_if(in_state(OnboardingState::Complete))` to the system; refactor)
- [ ] Verification: Manual test — launch with no existing data, complete onboarding, verify verse/fractal/petal created in DB. `cargo test -p fe-ui` [checkpoint marker]

---

## Phase 5: Entity Management UI

Goal: Build the interactive sidebar lists and CRUD panels for Verses, Fractals, Petals, Rooms, and Nodes in the Gardener Console.

Tasks:

- [ ] Task 5.1: Implement Verse list + switcher dialog in sidebar (TDD: write test that verse list renders items from `DbResult::Verses` response; clicking a verse sets `NavigationState::active_verse_id`; implement in `fe-ui/src/panels.rs` replacing the TODO in `sidebar_verse_header`; refactor)
- [ ] Task 5.2: Implement Verse create/edit/delete dialogs (TDD: write test that create dialog sends `DbCommand::CreateVerse`; delete dialog shows confirmation and is blocked if fractals exist; implement as egui `Window` overlays; refactor)
- [ ] Task 5.3: Implement Fractal list in sidebar "Fractal" section (TDD: write test that fractal list shows items filtered by active verse; clicking sets `NavigationState::active_fractal_id`; implement in `sidebar_section_fractal`; refactor)
- [ ] Task 5.4: Implement Fractal create/edit/delete dialogs (TDD: write test that create sends `DbCommand::CreateFractal` with active verse_id; delete blocked if petals exist; implement; refactor)
- [ ] Task 5.5: Implement interactive Petal list in sidebar (TDD: write test that petal list shows names from DB, not just counts; clicking sets `active_petal_id`; "New Petal" button sends create flow; implement replacing static count display in `sidebar_section_petals`; refactor)
- [ ] Task 5.6: Implement Petal edit panel and delete dialog (TDD: write test that edit panel populates from `PetalMetadata`; save sends `DbCommand::UpdatePetal`; delete shows cascade warning; implement; refactor)
- [ ] Task 5.7: Implement Room list and CRUD panel (TDD: write test that rooms list shows rooms for active petal; create/edit/delete flows send appropriate `DbCommand`; implement as a collapsible section under the selected petal; refactor)
- [ ] Task 5.8: Implement Node list and CRUD integration with inspector (TDD: write test that nodes list shows nodes for active petal; clicking a node populates inspector with its data; save in inspector sends `DbCommand::UpdateNode`; implement; refactor)
- [ ] Task 5.9: Implement Node creation dialog (TDD: write test that create dialog collects display_name, selects asset, sets default transform; sends `DbCommand::CreateNode`; implement; refactor)
- [ ] Task 5.10: Implement Node Settings panel accessible from toolbar (TDD: write test that Node Settings shows `node_profile` data; edits send `DbCommand::UpdateNodeProfile`; `did:key` is read-only; implement as toolbar menu item opening a dialog; refactor)
- [ ] Task 5.11: Implement confirmation dialog component (TDD: write test for reusable confirmation dialog that accepts title, message, confirm/cancel labels, and returns a boolean; implement; refactor)
- [ ] Verification: Manual test — create verse, fractal, petal, room, node through UI; edit each; delete in reverse order. `cargo test -p fe-ui` [checkpoint marker]

---

## Phase 6: Peer Connection UI & Polish

Goal: Build the peer management UI and perform integration polish.

Tasks:

- [ ] Task 6.1: Implement "Peers" sidebar section with connected peer list (TDD: write test that peers section renders peer count and list of `did:key` + display_name pairs; implement by reading peer state from `fe-network` via a new Bevy resource; refactor)
- [ ] Task 6.2: Implement "Add Peer" dialog (TDD: write test that dialog accepts a `did:key` or multiaddr string; validate format; on confirm, send a `NetworkCommand::ConnectPeer` variant; implement; refactor)
- [ ] Task 6.3: Implement peer disconnect/block action (TDD: write test that disconnect sends `NetworkCommand::DisconnectPeer`; block adds to a local denylist in SurrealDB; implement; refactor)
- [ ] Task 6.4: Extend `NetworkCommand`/`NetworkEvent` with peer management variants (TDD: write tests for `ConnectPeer`, `DisconnectPeer`, `PeerConnected`, `PeerDisconnected` variants; implement; refactor)
- [ ] Task 6.5: Polish — loading states for async operations (TDD: write test that while a `DbCommand` is in-flight, the UI shows a spinner or "Loading..." label; implement a `PendingOperation` resource; refactor)
- [ ] Task 6.6: Polish — error handling and user feedback (TDD: write test that `DbResult::Error` triggers a toast/notification in the UI; implement a `ToastQueue` resource rendered in the status bar area; refactor)
- [ ] Task 6.7: Polish — keyboard shortcuts for common actions (TDD: write test that Ctrl+N opens "New Petal" dialog; Escape closes open dialogs; implement in the `gardener_ui_system`; refactor)
- [ ] Verification: Full integration test — two-node scenario on localhost, onboarding both nodes, connecting as peers, verifying peer list updates. `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` [checkpoint marker]
