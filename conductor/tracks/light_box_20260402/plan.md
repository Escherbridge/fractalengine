# Implementation Plan: Light Box — Default Lighting Rig and Light Management System

**Track ID:** `light_box_20260402`
**Spec:** [spec.md](./spec.md)
**Dependencies:** Viewport Foundation (camera + scene rendering)

---

## Overview

Four phases: (1) light component data model and DB schema, (2) preset definitions and spawn logic, (3) Petal integration with auto-spawn and persistence round-trip, (4) inspector UI for light editing. Each phase follows TDD (Red -> Green -> Refactor).

---

## Phase 1: Light Data Model and Schema [checkpoint: pending]

**Goal:** Define all light-related types, DB schema, and message channel extensions. No spawning yet — pure data layer.

### Tasks

- [ ] **1.1 — Define `LightKind` enum and `LightConfig` data struct** (fe-renderer)
  TDD: Write tests that `LightKind` serializes to/from "directional"/"point"/"spot" strings. Test `LightConfig` round-trip serde. Test default values.
  Files: `fe-renderer/src/lighting.rs` (new module), `fe-renderer/src/lib.rs` (add `pub mod lighting`)

- [ ] **1.2 — Define `PetalLight` Bevy component and `LightPresetTag` marker** (fe-renderer)
  TDD: Test that `PetalLight` can be constructed with all fields. Test `LightPresetTag` stores preset name. Test `Clone`/`Debug` derive behavior.
  Files: `fe-renderer/src/lighting.rs`

- [ ] **1.3 — Add color temperature utility** (fe-renderer)
  TDD: Test `kelvin_to_linear_rgb(4500)` returns warm white (R > G > B). Test `kelvin_to_linear_rgb(6500)` returns cool white (B component higher than warm). Test boundary values (1000K, 12000K).
  Files: `fe-renderer/src/lighting.rs`

- [ ] **1.4 — Define `DEFINE_LIGHT` schema constant** (fe-database)
  TDD: Test schema string contains all required fields (`light_id`, `petal_id`, `kind`, `color`, `intensity`, `position`, `direction`, `shadows_enabled`, `preset_tag`, `created_at`). Test `kind` field has ASSERT constraint. Test SCHEMAFULL declaration.
  Files: `fe-database/src/schema.rs`

- [ ] **1.5 — Extend `DbCommand` and `DbResult` with light variants** (fe-runtime)
  TDD: Test `DbCommand::CreateLight` / `UpdateLight` / `DeleteLight` / `LoadLights` can be constructed, cloned, and debug-printed. Test channel send/recv roundtrip for each variant.
  Files: `fe-runtime/src/messages.rs`

- [ ] **1.6 — Verification: All types compile, all tests pass** [checkpoint marker]
  Run `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`. Verify no compilation errors across workspace.

---

## Phase 2: Presets and Spawn Logic [checkpoint: pending]

**Goal:** Define the three built-in presets as pure data and implement the spawn/despawn functions that convert preset data into Bevy entities.

### Tasks

- [ ] **2.1 — Define `LightPreset` struct and built-in preset data** (fe-renderer)
  TDD: Test "studio" preset contains exactly 4 light configs (key, fill, rim, ambient). Test "outdoor" preset contains 2 configs (sun + ambient). Test "dark" preset contains 1 config (ambient only). Test preset lookup by name returns correct preset. Test unknown preset name returns None.
  Files: `fe-renderer/src/lighting.rs`

- [ ] **2.2 — Implement `spawn_light_entity()` function** (fe-renderer)
  TDD: Test function returns a valid entity. Test that spawning a directional LightConfig produces an entity with `DirectionalLight` component. Test that spawning with shadows_enabled=true sets shadow config. Test that spawning with shadows_enabled=false has no shadow. (Use Bevy `World` directly in tests.)
  Files: `fe-renderer/src/lighting.rs`

- [ ] **2.3 — Implement `spawn_preset()` and `despawn_all_lights()` systems** (fe-renderer)
  TDD: Test `spawn_preset("studio")` creates exactly 4 entities with `PetalLight` component. Test `despawn_all_lights()` removes all entities with `PetalLight`. Test spawn then despawn leaves zero light entities. Test `spawn_preset("dark")` creates exactly 1 entity.
  Files: `fe-renderer/src/lighting.rs`

- [ ] **2.4 — Implement `switch_preset()` orchestration function** (fe-renderer)
  TDD: Test switching from "studio" to "outdoor" results in exactly 2 light entities. Test switching to same preset is a no-op (or clean re-spawn). Test switching updates the `LightPresetTag` on all spawned lights.
  Files: `fe-renderer/src/lighting.rs`

- [ ] **2.5 — Verification: Presets spawn correct Bevy light types** [checkpoint marker]
  Run `cargo test`. Manual verification: temporarily add `spawn_preset("studio")` in startup and run the app to visually confirm 3-point lighting on a GLTF model.

---

## Phase 3: Persistence and Petal Integration [checkpoint: pending]

**Goal:** Wire light persistence through SurrealDB and implement auto-spawn on Petal load.

### Tasks

- [ ] **3.1 — Implement DB handler for `CreateLight` / `LoadLights`** (fe-database)
  TDD: Test `CreateLight` inserts a light record into SurrealDB and returns success. Test `LoadLights { petal_id }` returns all lights for a given Petal. Test loading from empty Petal returns empty vec. (Use embedded SurrealDB in tests.)
  Files: `fe-database/src/lib.rs` (extend match arm for DbCommand)

- [ ] **3.2 — Implement DB handler for `UpdateLight` / `DeleteLight`** (fe-database)
  TDD: Test `UpdateLight` modifies intensity field and returns success. Test `DeleteLight` removes the record. Test deleting non-existent light returns appropriate result. Test RBAC: non-admin role cannot mutate lights (if RBAC test harness exists).
  Files: `fe-database/src/lib.rs`

- [ ] **3.3 — Implement `LightConfig` to/from DB record conversion** (fe-renderer or fe-database)
  TDD: Test round-trip: LightConfig -> DB fields -> LightConfig produces identical values. Test color array serialization (4 floats). Test direction/position array serialization (3 floats).
  Files: `fe-renderer/src/lighting.rs`

- [ ] **3.4 — Implement Petal load light system** (fe-renderer)
  TDD: Test system sends `DbCommand::LoadLights` on Petal load event. Test when `DbResult::LightsLoaded` returns data, entities are spawned from DB records. Test when `DbResult::LightsLoaded` returns empty, default "studio" preset is spawned. Test default lights are persisted after auto-spawn (CreateLight commands sent).
  Files: `fe-renderer/src/lighting.rs`

- [ ] **3.5 — Implement Petal unload light cleanup** (fe-renderer)
  TDD: Test all `PetalLight` entities are despawned on Petal unload/switch. Test cleanup runs before new Petal lights are loaded.
  Files: `fe-renderer/src/lighting.rs`

- [ ] **3.6 — Register light schema in DB bootstrap** (fe-database)
  TDD: Test that the schema bootstrap sequence includes `DEFINE_LIGHT`. Test DB accepts a CREATE query against the light table after bootstrap.
  Files: `fe-database/src/lib.rs` or `fe-database/src/schema.rs`

- [ ] **3.7 — Verification: Full persistence round-trip** [checkpoint marker]
  Run `cargo test`. Manual verification: launch app, load a Petal, verify Studio lights appear, quit, relaunch, verify same lights load from DB.

---

## Phase 4: Light Inspector UI [checkpoint: pending]

**Goal:** Add light-specific controls to the right inspector panel.

### Tasks

- [ ] **4.1 — Extend `InspectorState` with light fields** (fe-ui)
  TDD: Test `InspectorState` default has `light_data: None`. Test light data struct holds kind, color (3 floats), intensity (f32), shadows (bool). Test light data can be populated from a `PetalLight` component.
  Files: `fe-ui/src/plugin.rs`

- [ ] **4.2 — Implement `inspector_light_section()` UI function** (fe-ui)
  TDD: Test function renders without panic when light_data is Some. Test function does not render when light_data is None. (Use egui test harness or snapshot testing if available.)
  Files: `fe-ui/src/panels.rs`

- [ ] **4.3 — Wire light inspector to Bevy component updates** (fe-ui)
  TDD: Test that changing intensity in inspector state updates the `PetalLight` component's intensity. Test that toggling shadows updates the component. Test that color change updates the Bevy light's color.
  Files: `fe-ui/src/panels.rs` or new `fe-ui/src/light_inspector.rs`

- [ ] **4.4 — Wire light inspector "Save" to `DbCommand::UpdateLight`** (fe-ui)
  TDD: Test that clicking Save sends an `UpdateLight` command through the DB channel. Test that the command payload matches the current inspector state values.
  Files: `fe-ui/src/panels.rs`

- [ ] **4.5 — Add preset switcher dropdown to UI** (fe-ui)
  TDD: Test dropdown lists all three presets. Test selecting a preset triggers the `switch_preset()` function. Test current active preset is highlighted.
  Files: `fe-ui/src/panels.rs`

- [ ] **4.6 — Verification: Full UI integration** [checkpoint marker]
  Run `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`. Manual verification: launch app, select a light entity, verify inspector shows light controls, change intensity, save, verify persistence.

---

## Summary

| Phase | Tasks | Focus |
|-------|-------|-------|
| 1 | 6 | Data types, schema, message types |
| 2 | 5 | Preset definitions, spawn/despawn logic |
| 3 | 7 | SurrealDB persistence, Petal load integration |
| 4 | 6 | Inspector panel, UI controls, preset switcher |
| **Total** | **24** | |
