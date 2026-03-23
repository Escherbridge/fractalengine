# TDD Implementation Plan: Fractal Atlas — Space Manager & Metadata System

**Track:** fractal_atlas_20260322
**Approach:** Test-first. Every task starts with a failing test, then implements the minimum code to make it pass, then refactors.

---

## Phase 1: Schema Extension

**Goal:** Extend the five existing SCHEMAFULL tables with new metadata fields and define the Rust types that mirror them. Nothing in this phase touches query logic or UI.

---

### Task 1.1 — Define `Visibility` enum and `atlas.rs` types

**Test first:**

```rust
// fe-database/src/atlas.rs (new file)
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn visibility_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Visibility::Public).unwrap(), "\"public\"");
        assert_eq!(serde_json::to_string(&Visibility::Private).unwrap(), "\"private\"");
        assert_eq!(serde_json::to_string(&Visibility::Unlisted).unwrap(), "\"unlisted\"");
    }

    #[test]
    fn visibility_deserializes_from_lowercase() {
        let v: Visibility = serde_json::from_str("\"unlisted\"").unwrap();
        assert!(matches!(v, Visibility::Unlisted));
    }

    #[test]
    fn petal_metadata_default_is_private_empty() {
        let m = PetalMetadata::default();
        assert!(matches!(m.visibility, Visibility::Private));
        assert!(m.tags.is_empty());
        assert!(m.description.is_none());
    }

    #[test]
    fn model_metadata_update_default_is_all_none() {
        let m = ModelMetadataUpdate::default();
        assert!(m.display_name.is_none());
        assert!(m.tags.is_empty());
        assert!(m.metadata.is_none());
    }

    #[test]
    fn space_overview_default_is_zeroes() {
        let o = SpaceOverview::default();
        assert_eq!(o.petal_count, 0);
        assert_eq!(o.peer_count, 0);
    }
}
```

**Implement:** `fe-database/src/atlas.rs` with `Visibility`, `PetalMetadata`, `RoomBounds`, `SpawnPoint`, `RoomMetadata`, `ModelMetadataUpdate`, `SpaceOverview`. Derive `serde::{Serialize, Deserialize}`, `Default`, `Debug`, `Clone`. Add `pub mod atlas;` to `fe-database/src/lib.rs`.

**Done when:** `cargo test -p fe-database atlas` passes.

---

### Task 1.2 — Petal schema extension

**Test first:**

```rust
// fe-database/src/schema.rs (or schema_test.rs)
#[test]
fn petal_schema_contains_description_field() {
    assert!(DEFINE_PETAL.contains("DEFINE FIELD IF NOT EXISTS description ON TABLE petal"));
}

#[test]
fn petal_schema_contains_visibility_field_with_assert() {
    assert!(DEFINE_PETAL.contains("DEFINE FIELD IF NOT EXISTS visibility ON TABLE petal"));
    assert!(DEFINE_PETAL.contains("ASSERT"));
    assert!(DEFINE_PETAL.contains("public"));
    assert!(DEFINE_PETAL.contains("private"));
    assert!(DEFINE_PETAL.contains("unlisted"));
}

#[test]
fn petal_schema_contains_tags_array_field() {
    assert!(DEFINE_PETAL.contains("DEFINE FIELD IF NOT EXISTS tags ON TABLE petal"));
    assert!(DEFINE_PETAL.contains("array<string>"));
}
```

**Implement:** Append three `DEFINE FIELD IF NOT EXISTS` statements to the `DEFINE_PETAL` constant in `fe-database/src/schema.rs` (or wherever schema SQL strings live).

**Done when:** All three tests pass. Run `cargo test -p fe-database schema` green.

---

### Task 1.3 — Room schema extension

**Test first:**

```rust
#[test]
fn room_schema_contains_description_field() {
    assert!(DEFINE_ROOM.contains("DEFINE FIELD IF NOT EXISTS description ON TABLE room"));
}

#[test]
fn room_schema_contains_bounds_object_field() {
    assert!(DEFINE_ROOM.contains("DEFINE FIELD IF NOT EXISTS bounds ON TABLE room"));
    assert!(DEFINE_ROOM.contains("option<object>"));
}

#[test]
fn room_schema_contains_spawn_point_field() {
    assert!(DEFINE_ROOM.contains("DEFINE FIELD IF NOT EXISTS spawn_point ON TABLE room"));
}
```

**Implement:** Append `description`, `bounds`, `spawn_point` fields to `DEFINE_ROOM`.

**Done when:** Tests pass.

---

### Task 1.4 — Model schema extension

**Test first:**

```rust
#[test]
fn model_schema_contains_display_name() {
    assert!(DEFINE_MODEL.contains("DEFINE FIELD IF NOT EXISTS display_name ON TABLE model"));
}

#[test]
fn model_schema_contains_metadata_flexible() {
    assert!(DEFINE_MODEL.contains("DEFINE FIELD IF NOT EXISTS metadata ON TABLE model"));
    assert!(DEFINE_MODEL.contains("FLEXIBLE"));
}

#[test]
fn model_schema_contains_tags_and_urls() {
    assert!(DEFINE_MODEL.contains("DEFINE FIELD IF NOT EXISTS tags ON TABLE model"));
    assert!(DEFINE_MODEL.contains("DEFINE FIELD IF NOT EXISTS external_url ON TABLE model"));
    assert!(DEFINE_MODEL.contains("DEFINE FIELD IF NOT EXISTS config_url ON TABLE model"));
}
```

**Implement:** Append `display_name`, `description`, `external_url`, `config_url`, `tags`, `metadata` fields to `DEFINE_MODEL`.

**Done when:** Tests pass.

---

### Task 1.5 — Schema idempotency integration test

**Test first:**

```rust
// fe-database/tests/schema_idempotent.rs
#[tokio::test]
async fn apply_schema_twice_is_idempotent() {
    let db = test_db().await;  // helper: in-memory SurrealDB
    apply_schema(&db).await.expect("first apply");
    apply_schema(&db).await.expect("second apply must not error");
}

#[tokio::test]
async fn petal_visibility_rejects_invalid_value() {
    let db = test_db().await;
    apply_schema(&db).await.unwrap();
    let result: surrealdb::Result<_> = db
        .query("CREATE petal SET visibility = 'forbidden'")
        .await;
    assert!(result.is_err() || {
        // SurrealDB may return an error in the response object
        result.unwrap().take::<Vec<serde_json::Value>>(0).unwrap().is_empty()
    });
}
```

**Implement:** Ensure `apply_schema` (or equivalent) applies all five tables' schema strings. The test calls it twice; `DEFINE FIELD IF NOT EXISTS` guarantees the second run is a no-op.

**Done when:** Integration tests pass against in-memory SurrealDB.

---

### Phase 1 Verification Checkpoint

```
cargo test -p fe-database
cargo clippy -p fe-database -- -D warnings
```

All Phase 1 tests green. No clippy warnings. Schema strings contain all required field definitions.

---

## Phase 2: Query Functions for Metadata CRUD

**Goal:** Implement async query functions for reading and writing metadata on petals, rooms, and models. All mutations write an op-log entry.

---

### Task 2.1 — `update_petal_metadata` and `set_petal_visibility`

**Test first:**

```rust
// fe-database/tests/petal_metadata.rs
#[tokio::test]
async fn update_petal_metadata_round_trips() {
    let db = seeded_test_db().await;
    let id = create_test_petal(&db, "Alpha").await;

    let meta = PetalMetadata {
        description: Some("A test petal".into()),
        visibility: Visibility::Public,
        tags: vec!["alpha".into(), "test".into()],
    };
    update_petal_metadata(&db, id.clone(), meta.clone()).await.unwrap();

    let row: PetalRecord = db.select(("petal", id.to_string())).await.unwrap();
    assert_eq!(row.description.as_deref(), Some("A test petal"));
    assert_eq!(row.tags, vec!["alpha", "test"]);
    assert_eq!(row.visibility, "public");
}

#[tokio::test]
async fn set_petal_visibility_does_not_touch_other_fields() {
    let db = seeded_test_db().await;
    let id = create_test_petal(&db, "Beta").await;
    // Set a description first
    update_petal_metadata(&db, id.clone(), PetalMetadata {
        description: Some("keep me".into()),
        visibility: Visibility::Private,
        tags: vec!["x".into()],
    }).await.unwrap();

    set_petal_visibility(&db, id.clone(), Visibility::Unlisted).await.unwrap();

    let row: PetalRecord = db.select(("petal", id.to_string())).await.unwrap();
    assert_eq!(row.visibility, "unlisted");
    assert_eq!(row.description.as_deref(), Some("keep me"));  // unchanged
    assert_eq!(row.tags, vec!["x"]);                           // unchanged
}

#[tokio::test]
async fn update_petal_metadata_writes_op_log_entry() {
    let db = seeded_test_db().await;
    let id = create_test_petal(&db, "Gamma").await;
    update_petal_metadata(&db, id.clone(), PetalMetadata::default()).await.unwrap();

    let logs: Vec<OpLog> = db.query("SELECT * FROM op_log WHERE target = $id")
        .bind(("id", id.to_string()))
        .await.unwrap().take(0).unwrap();
    assert!(!logs.is_empty());
}
```

**Implement:** `update_petal_metadata` and `set_petal_visibility` in `fe-database/src/petal.rs` (or `queries.rs`). Each issues a `UPDATE petal:$id SET ... ` query and then calls the existing `write_op_log` helper.

**Done when:** Three tests pass.

---

### Task 2.2 — `list_petals_by_tag` and `search_petals`

**Test first:**

```rust
#[tokio::test]
async fn list_petals_by_tag_returns_matching_only() {
    let db = seeded_test_db().await;
    let a = create_test_petal(&db, "A").await;
    let b = create_test_petal(&db, "B").await;
    update_petal_metadata(&db, a.clone(), PetalMetadata { tags: vec!["foo".into()], ..Default::default() }).await.unwrap();
    update_petal_metadata(&db, b.clone(), PetalMetadata { tags: vec!["bar".into()], ..Default::default() }).await.unwrap();

    let results = list_petals_by_tag(&db, "foo").await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, a);
}

#[tokio::test]
async fn search_petals_matches_name_description_tags() {
    let db = seeded_test_db().await;
    let id = create_test_petal(&db, "Galaxy Room").await;
    update_petal_metadata(&db, id.clone(), PetalMetadata {
        description: Some("A vast space".into()),
        tags: vec!["cosmos".into()],
        ..Default::default()
    }).await.unwrap();

    // Match on name
    assert!(!search_petals(&db, "Galaxy").await.unwrap().is_empty());
    // Match on description
    assert!(!search_petals(&db, "vast").await.unwrap().is_empty());
    // Match on tag
    assert!(!search_petals(&db, "cosmos").await.unwrap().is_empty());
    // No match
    assert!(search_petals(&db, "zzznomatch").await.unwrap().is_empty());
}
```

**Implement:** SurrealQL using `WHERE tags CONTAINS $tag` for `list_petals_by_tag`; `WHERE string::lowercase(name) CONTAINS string::lowercase($q) OR ...` for `search_petals`.

**Done when:** Tests pass.

---

### Task 2.3 — `update_room_metadata` and `get_room_detail`

**Test first:**

```rust
#[tokio::test]
async fn update_room_metadata_round_trips_bounds_and_spawn() {
    let db = seeded_test_db().await;
    let room_id = create_test_room(&db, "Lobby").await;

    let meta = RoomMetadata {
        description: Some("Main lobby".into()),
        bounds: Some(RoomBounds { min: [-10.0, 0.0, -10.0], max: [10.0, 5.0, 10.0] }),
        spawn_point: Some(SpawnPoint { position: [0.0, 0.5, 0.0], yaw: 90.0 }),
    };
    update_room_metadata(&db, room_id.clone(), meta).await.unwrap();

    let detail = get_room_detail(&db, room_id).await.unwrap().expect("room exists");
    assert_eq!(detail.description.as_deref(), Some("Main lobby"));
    let bounds = detail.bounds.unwrap();
    assert_eq!(bounds.min[0], -10.0);
}
```

**Implement:** `update_room_metadata` and `get_room_detail` in `fe-database/src/room.rs` (or `queries.rs`). Serialize `RoomBounds` and `SpawnPoint` as JSON objects into SurrealDB's `object` fields.

**Done when:** Test passes.

---

### Task 2.4 — Model metadata queries

**Test first:**

```rust
#[tokio::test]
async fn update_model_metadata_round_trips_all_fields() {
    let db = seeded_test_db().await;
    let model_id = create_test_model(&db).await;

    let update = ModelMetadataUpdate {
        display_name: Some("Reception Desk".into()),
        description: Some("Mahogany desk at the entrance".into()),
        external_url: Some("https://example.com/model".into()),
        config_url: Some("https://example.com/config.json".into()),
        tags: vec!["furniture".into(), "lobby".into()],
        metadata: Some(serde_json::json!({ "color": "brown", "seats": 2 })),
    };
    update_model_metadata(&db, model_id.clone(), update).await.unwrap();

    let row: ModelRecord = db.select(("model", model_id.to_string())).await.unwrap();
    assert_eq!(row.display_name.as_deref(), Some("Reception Desk"));
    assert_eq!(row.tags, vec!["furniture", "lobby"]);
}

#[tokio::test]
async fn upsert_model_kv_merges_without_clobbering() {
    let db = seeded_test_db().await;
    let id = create_test_model(&db).await;

    upsert_model_kv(&db, id.clone(), "color", serde_json::json!("red")).await.unwrap();
    upsert_model_kv(&db, id.clone(), "scale", serde_json::json!(1.5)).await.unwrap();

    let row: ModelRecord = db.select(("model", id.to_string())).await.unwrap();
    let meta = row.metadata.unwrap();
    assert_eq!(meta["color"], serde_json::json!("red"));
    assert_eq!(meta["scale"], serde_json::json!(1.5));
}

#[tokio::test]
async fn list_models_by_tag_returns_matching_only() {
    let db = seeded_test_db().await;
    let a = create_test_model(&db).await;
    let b = create_test_model(&db).await;
    update_model_metadata(&db, a.clone(), ModelMetadataUpdate { tags: vec!["hero".into()], ..Default::default() }).await.unwrap();
    update_model_metadata(&db, b.clone(), ModelMetadataUpdate { tags: vec!["prop".into()], ..Default::default() }).await.unwrap();

    let results = list_models_by_tag(&db, "hero").await.unwrap();
    assert_eq!(results.len(), 1);
}
```

**Implement:** `update_model_metadata`, `upsert_model_kv`, `list_models_by_tag`, `search_models` in the model query module.

**Done when:** All four tests pass.

---

### Task 2.5 — `space_overview`

**Test first:**

```rust
#[tokio::test]
async fn space_overview_global_counts_correctly() {
    let db = seeded_test_db().await;
    create_test_petal(&db, "P1").await;
    create_test_petal(&db, "P2").await;
    create_test_room(&db, "R1").await;
    create_test_model(&db).await;

    let overview = space_overview(&db, None).await.unwrap();
    assert_eq!(overview.petal_count, 2);
    assert_eq!(overview.room_count, 1);
    assert_eq!(overview.model_count, 1);
}
```

**Implement:** `space_overview` issues three `SELECT count() FROM ... GROUP ALL` queries in parallel with `tokio::join!`.

**Done when:** Test passes.

---

### Phase 2 Verification Checkpoint

```
cargo test -p fe-database
cargo clippy -p fe-database -- -D warnings
```

All CRUD and aggregate tests green. Clippy clean. Every mutation test explicitly asserts an op-log entry exists.

---

## Phase 3: Petal Wizard and Room Editor UI

**Goal:** Build the petal creation wizard and room metadata editor as Bevy/egui panels. Logic is unit-tested; rendering is compile-tested.

---

### Task 3.1 — `PetalWizardState` resource and step validation

**Test first:**

```rust
// fe-ui/src/atlas/petal_wizard.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wizard_starts_at_step_one() {
        let state = PetalWizardState::default();
        assert_eq!(state.step, WizardStep::Identity);
    }

    #[test]
    fn cannot_advance_with_empty_name() {
        let mut state = PetalWizardState::default();
        state.name = String::new();
        assert!(!state.can_advance());
    }

    #[test]
    fn cannot_advance_with_name_over_64_chars() {
        let mut state = PetalWizardState::default();
        state.name = "x".repeat(65);
        assert!(!state.can_advance());
    }

    #[test]
    fn can_advance_with_valid_name_at_step_one() {
        let mut state = PetalWizardState::default();
        state.name = "My Petal".into();
        assert!(state.can_advance());
    }

    #[test]
    fn reset_returns_to_step_one_with_cleared_fields() {
        let mut state = PetalWizardState::default();
        state.name = "Temp".into();
        state.step = WizardStep::Confirm;
        state.reset();
        assert_eq!(state.step, WizardStep::Identity);
        assert!(state.name.is_empty());
    }
}
```

**Implement:** `PetalWizardState`, `WizardStep`, `can_advance()`, `reset()` in `fe-ui/src/atlas/petal_wizard.rs`.

**Done when:** Unit tests pass.

---

### Task 3.2 — Tag validation logic

**Test first:**

```rust
#[test]
fn add_tag_rejects_duplicate_case_insensitive() {
    let mut state = PetalWizardState::default();
    state.name = "X".into();
    state.add_tag("Foo".into());
    let result = state.add_tag("foo".into());
    assert!(result.is_err());
    assert_eq!(state.tags.len(), 1);
}

#[test]
fn add_tag_rejects_over_32_chars() {
    let mut state = PetalWizardState::default();
    state.name = "X".into();
    let long_tag = "a".repeat(33);
    let result = state.add_tag(long_tag);
    assert!(result.is_err());
}

#[test]
fn add_tag_rejects_when_count_is_20() {
    let mut state = PetalWizardState::default();
    state.name = "X".into();
    for i in 0..20 {
        state.add_tag(format!("tag{}", i)).unwrap();
    }
    let result = state.add_tag("one_more".into());
    assert!(result.is_err());
}
```

**Implement:** `add_tag(&mut self, tag: String) -> Result<(), TagError>` on `PetalWizardState`. `TagError` is an enum with `Duplicate`, `TooLong`, `LimitReached` variants.

**Done when:** Tests pass.

---

### Task 3.3 — `tag_input_ui` shared component

**Test first (logic only; rendering is compile-tested):**

```rust
// fe-ui/src/atlas/tag_panel.rs
#[test]
fn remove_tag_removes_correct_index() {
    let mut tags = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    remove_tag(&mut tags, 1);
    assert_eq!(tags, vec!["a", "c"]);
}
```

**Implement:** `tag_input_ui(ui, tags)` public function and `remove_tag` helper. The function renders chips with `×` buttons and an "Add" text input. Validate with `TagError` logic extracted from `PetalWizardState` into a shared `validate_tag` free function.

**Done when:** Unit test passes; `cargo build -p fe-ui` compiles.

---

### Task 3.4 — `RoomEditorState` and bounds validation

**Test first:**

```rust
// fe-ui/src/atlas/room_editor.rs
#[test]
fn bounds_invalid_when_min_gte_max_on_any_axis() {
    let state = RoomEditorState {
        min: [0.0, 5.0, 0.0],
        max: [10.0, 3.0, 10.0],  // y: min(5) > max(3)
        ..Default::default()
    };
    assert!(!state.bounds_valid());
}

#[test]
fn bounds_valid_when_all_min_lt_max() {
    let state = RoomEditorState {
        min: [-1.0, 0.0, -1.0],
        max: [1.0, 2.0, 1.0],
        ..Default::default()
    };
    assert!(state.bounds_valid());
}

#[test]
fn yaw_clamped_on_set() {
    let mut state = RoomEditorState::default();
    state.set_yaw(270.0);
    assert!(state.yaw >= -180.0 && state.yaw <= 180.0);
}
```

**Implement:** `RoomEditorState` with `bounds_valid()` and `set_yaw()` in `fe-ui/src/atlas/room_editor.rs`.

**Done when:** Tests pass.

---

### Task 3.5 — Wizard and room editor egui render systems (compile check)

No additional unit tests beyond what compile-guards provide. The Bevy systems (`petal_wizard_system`, `room_editor_system`) must compile and be registered in `fe-ui/src/atlas/mod.rs`.

**Verify:** `cargo build -p fe-ui` with no errors.

---

### Phase 3 Verification Checkpoint

```
cargo test -p fe-ui
cargo build -p fe-ui
cargo clippy -p fe-ui -- -D warnings
```

All wizard and room editor logic tests pass. UI modules compile cleanly.

---

## Phase 4: Model Metadata Editor and Tag System

**Goal:** Model metadata editor with key-value pair support, the shared tag chip component wired into all editors, and search bar debounce logic.

---

### Task 4.1 — `ModelEditorState` and key-value pair logic

**Test first:**

```rust
// fe-ui/src/atlas/model_editor.rs
#[test]
fn add_kv_row_appends_blank_row() {
    let mut state = ModelEditorState::default();
    state.add_kv_row();
    assert_eq!(state.kv_rows.len(), 1);
    assert!(state.kv_rows[0].key.is_empty());
}

#[test]
fn delete_kv_row_removes_correct_index() {
    let mut state = ModelEditorState::default();
    state.add_kv_row();
    state.add_kv_row();
    state.kv_rows[0].key = "k1".into();
    state.kv_rows[1].key = "k2".into();
    state.delete_kv_row(0);
    assert_eq!(state.kv_rows.len(), 1);
    assert_eq!(state.kv_rows[0].key, "k2");
}

#[test]
fn kv_row_invalid_json_value_detected() {
    let row = KvRow { key: "x".into(), value_raw: "{not json}".into() };
    assert!(!row.value_is_valid_json());
}

#[test]
fn kv_row_valid_json_value_accepted() {
    let row = KvRow { key: "x".into(), value_raw: "\"hello\"".into() };
    assert!(row.value_is_valid_json());
}

#[test]
fn can_save_false_when_any_kv_row_has_invalid_json() {
    let mut state = ModelEditorState::default();
    state.add_kv_row();
    state.kv_rows[0].key = "k".into();
    state.kv_rows[0].value_raw = "not_json".into();
    assert!(!state.can_save());
}
```

**Implement:** `ModelEditorState`, `KvRow`, `value_is_valid_json()`, `can_save()` in `fe-ui/src/atlas/model_editor.rs`.

**Done when:** All five tests pass.

---

### Task 4.2 — `visibility_radio_ui` component

**Test first (logic only):**

```rust
// fe-ui/src/atlas/visibility_control.rs
#[test]
fn visibility_display_names_are_human_readable() {
    assert_eq!(Visibility::Public.display_name(), "Public");
    assert_eq!(Visibility::Private.display_name(), "Private");
    assert_eq!(Visibility::Unlisted.display_name(), "Unlisted");
}
```

**Implement:** `display_name()` on `Visibility` (add as an `impl` in `fe-ui` re-exporting the type, or a wrapper trait). `visibility_radio_ui` function in `visibility_control.rs`.

**Done when:** Test passes; module compiles.

---

### Task 4.3 — Search debounce logic

**Test first:**

```rust
// fe-ui/src/atlas/search_bar.rs
#[test]
fn search_query_resource_default_is_empty() {
    let q = SearchQuery::default();
    assert!(q.text.is_empty());
    assert!(q.tag_filters.is_empty());
}

#[test]
fn debounce_timer_resets_on_new_input() {
    let mut q = SearchQuery::default();
    q.on_text_changed("abc");
    let t1 = q.pending_since;
    std::thread::sleep(std::time::Duration::from_millis(10));
    q.on_text_changed("abcd");
    assert!(q.pending_since > t1);
}

#[test]
fn debounce_not_ready_before_300ms() {
    let mut q = SearchQuery::default();
    q.on_text_changed("x");
    assert!(!q.is_debounce_ready());
}

#[test]
fn filter_chips_compose_and() {
    let mut q = SearchQuery::default();
    q.tag_filters = vec!["foo".into(), "bar".into()];
    // Both tags must be present — verified by the search function, not this struct
    // Assert the filter vec is correct
    assert_eq!(q.tag_filters.len(), 2);
}
```

**Implement:** `SearchQuery` resource with `on_text_changed`, `is_debounce_ready` (checks `Instant::now() - pending_since >= 300ms`), `tag_filters`. `SearchResults` resource holding `Vec<PetalRecord>` and `Vec<ModelRecord>`.

**Done when:** Tests pass.

---

### Task 4.4 — Model editor egui render system (compile check)

Register `model_editor_system` in `fe-ui/src/atlas/mod.rs`. No additional unit tests.

**Verify:** `cargo build -p fe-ui` clean.

---

### Phase 4 Verification Checkpoint

```
cargo test -p fe-ui
cargo build -p fe-ui
cargo clippy -p fe-ui -- -D warnings
```

All model editor and search debounce tests pass. No compile errors.

---

## Phase 5: Dashboard, Import/Export, and Search/Filter

**Goal:** Wire the space overview dashboard, the JSON import/export round-trip, and the search/filter system end-to-end.

---

### Task 5.1 — JSON export round-trip

**Test first:**

```rust
// fe-database/tests/atlas_export.rs
#[tokio::test]
async fn export_petal_produces_valid_json() {
    let db = seeded_test_db().await;
    let petal_id = create_test_petal(&db, "Export Test").await;
    let room_id = create_test_room_in_petal(&db, petal_id.clone(), "Hall").await;
    create_test_model_in_room(&db, room_id.clone()).await;

    let json_str = export_petal_config(&db, petal_id).await.unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(parsed["schema_version"], 1);
    assert!(parsed["petal"]["rooms"].is_array());
    assert!(!parsed["petal"]["rooms"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn export_preserves_blake3_hashes() {
    let db = seeded_test_db().await;
    let petal_id = create_test_petal(&db, "Hash Test").await;
    let room_id = create_test_room_in_petal(&db, petal_id.clone(), "Room").await;
    let model_id = create_test_model_in_room_with_asset(
        &db, room_id.clone(), "abc123deadbeef"
    ).await;

    let json_str = export_petal_config(&db, petal_id).await.unwrap();
    assert!(json_str.contains("abc123deadbeef"));
}
```

**Implement:** `export_petal_config(db, petal_id) -> surrealdb::Result<String>` in `fe-database/src/atlas_export.rs`. Queries petal + rooms + models, serializes to the bundle format with `serde_json`.

**Done when:** Both tests pass.

---

### Task 5.2 — JSON import round-trip

**Test first:**

```rust
#[tokio::test]
async fn import_creates_new_ids_not_original() {
    let db = seeded_test_db().await;
    let petal_id = create_test_petal(&db, "Original").await;
    let json_str = export_petal_config(&db, petal_id.clone()).await.unwrap();

    let new_id = import_petal_config(&db, "node:main", &json_str).await.unwrap();
    assert_ne!(new_id, petal_id);
}

#[tokio::test]
async fn import_invalid_json_returns_error_not_panic() {
    let db = seeded_test_db().await;
    let result = import_petal_config(&db, "node:main", "not json at all").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn import_wrong_schema_version_returns_error() {
    let db = seeded_test_db().await;
    let bad_json = r#"{"schema_version": 99, "petal": {}}"#;
    let result = import_petal_config(&db, "node:main", bad_json).await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("schema_version") || msg.contains("unsupported"));
}

#[tokio::test]
async fn import_export_round_trip_preserves_metadata() {
    let db = seeded_test_db().await;
    let petal_id = create_test_petal(&db, "RoundTrip").await;
    update_petal_metadata(&db, petal_id.clone(), PetalMetadata {
        description: Some("Round-trip test".into()),
        visibility: Visibility::Public,
        tags: vec!["round".into(), "trip".into()],
    }).await.unwrap();

    let json_str = export_petal_config(&db, petal_id).await.unwrap();
    let new_id = import_petal_config(&db, "node:main", &json_str).await.unwrap();

    let row: PetalRecord = db.select(("petal", new_id.to_string())).await.unwrap();
    assert_eq!(row.description.as_deref(), Some("Round-trip test"));
    assert_eq!(row.tags, vec!["round", "trip"]);
}
```

**Implement:** `import_petal_config(db, node_id, json) -> Result<PetalId, ImportError>` in `fe-database/src/atlas_import.rs`. `ImportError` variants: `InvalidJson`, `UnsupportedVersion`, `DbError(surrealdb::Error)`.

**Done when:** All four tests pass.

---

### Task 5.3 — Dashboard polling system

**Test first:**

```rust
// fe-ui/src/atlas/dashboard.rs
#[test]
fn dashboard_state_default_shows_all_zeroes() {
    let state = DashboardState::default();
    assert_eq!(state.petal_count, 0);
    assert_eq!(state.is_online, false);
}

#[test]
fn dashboard_state_marks_offline_on_error() {
    let mut state = DashboardState::default();
    state.apply_error();
    assert!(!state.is_online);
}

#[test]
fn dashboard_state_updates_counts_on_success() {
    let mut state = DashboardState::default();
    let overview = SpaceOverview { petal_count: 3, room_count: 7, model_count: 12, peer_count: 2, estimated_storage_bytes: 0 };
    state.apply_overview(overview);
    assert!(state.is_online);
    assert_eq!(state.petal_count, 3);
}
```

**Implement:** `DashboardState` resource with `apply_overview` and `apply_error`. Bevy system `dashboard_poll_system` checks elapsed time against `poll_interval` (10 s default), spawns an async task to call `space_overview`, sends result back via channel to update `DashboardState`.

**Done when:** Unit tests pass; system compiles.

---

### Task 5.4 — Search system integration with DB channel

**Test first:**

```rust
// fe-ui/src/atlas/search_bar.rs
#[test]
fn empty_query_does_not_set_pending() {
    let mut q = SearchQuery::default();
    q.on_text_changed("");
    assert!(q.pending_since.is_none());
}

#[test]
fn tag_filter_toggle_adds_and_removes() {
    let mut q = SearchQuery::default();
    q.toggle_tag_filter("foo".into());
    assert!(q.tag_filters.contains(&"foo".to_string()));
    q.toggle_tag_filter("foo".into());
    assert!(!q.tag_filters.contains(&"foo".to_string()));
}
```

**Implement:** Update `SearchQuery` so `on_text_changed("")` sets `pending_since = None`. Add `toggle_tag_filter`. Bevy system `search_system` fires DB commands when `is_debounce_ready()` and updates `SearchResults`.

**Done when:** Tests pass.

---

### Task 5.5 — Import/export UI panel (compile check)

`import_export.rs` panel: "Export" button writes JSON to `exports/<petal_id>.json` using `std::fs::write`. "Import" button reads from a path text field. Errors surface as egui label in red. No additional unit tests.

**Verify:** `cargo build -p fe-ui` clean.

---

### Task 5.6 — End-to-end integration test

**Test first:**

```rust
// fe-database/tests/atlas_e2e.rs
#[tokio::test]
async fn full_atlas_flow() {
    let db = test_db().await;
    apply_schema(&db).await.unwrap();

    // Create petal with metadata
    let id = create_petal(&db, "node:test", "E2E Petal").await.unwrap();
    update_petal_metadata(&db, id.clone(), PetalMetadata {
        description: Some("End-to-end test petal".into()),
        visibility: Visibility::Public,
        tags: vec!["e2e".into(), "test".into()],
    }).await.unwrap();

    // Search finds it by tag
    let found = list_petals_by_tag(&db, "e2e").await.unwrap();
    assert_eq!(found.len(), 1);

    // Export
    let json = export_petal_config(&db, id.clone()).await.unwrap();
    assert!(json.contains("e2e"));

    // Import produces new petal
    let new_id = import_petal_config(&db, "node:test", &json).await.unwrap();
    assert_ne!(new_id, id);

    // Overview counts both petals
    let overview = space_overview(&db, None).await.unwrap();
    assert_eq!(overview.petal_count, 2);
}
```

**Implement:** No new code — this test exercises all code from Phases 1–5. If it fails, identify and fix the gap.

**Done when:** Test passes.

---

### Phase 5 Verification Checkpoint

```
cargo test --workspace
cargo fmt --check
cargo clippy --workspace -- -D warnings
```

All tests across all crates pass. No `unwrap()` calls in production paths (search with `rg 'unwrap\(\)' fe-database/src fe-ui/src`). Format check clean. Full end-to-end test green.

---

## Summary

| Phase | Tasks | Key Deliverables                                  | Status |
|-------|-------|---------------------------------------------------|--------|
| 1     | 5     | `atlas.rs` types, schema SQL strings extended     | [~]    |
| 2     | 5     | CRUD query functions + op-log integration         | [~]    |
| 3     | 5     | Petal wizard + room editor (logic + render stub)  | [~]    |
| 4     | 4     | Model editor + tag system + search debounce       | [~]    |
| 5     | 6     | Import/export + dashboard + search wiring + E2E   | deferred |

**Total tasks:** 25
**Total verification checkpoints:** 5 (one per phase)

---

## Implementation Status (fractal_atlas_20260322)

### [~] Phase 1 — Schema Extension
- [~] Task 1.1 — `Visibility`, `PetalMetadata`, `RoomBounds`, `SpawnPoint`, `RoomMetadata`, `ModelMetadataUpdate`, `SpaceOverview` in `fe-database/src/atlas.rs`
- [~] Task 1.2 — Petal schema extended with `description`, `visibility` (ASSERT), `tags`
- [~] Task 1.3 — Room schema extended with `description`, `bounds`, `spawn_point`
- [~] Task 1.4 — Model schema extended with `display_name`, `description`, `external_url`, `config_url`, `tags`, `metadata`
- [~] Task 1.5 — Schema idempotency integration test stub in `fe-database/tests/schema_idempotent.rs`

### [~] Phase 2 — Query Functions
- [~] Task 2.1 — `update_petal_metadata`, `set_petal_visibility` in `SpaceManager`
- [~] Task 2.2 — `list_petals_by_tag`, `search_petals`
- [~] Task 2.3 — `update_room_metadata`, `get_room_detail`
- [~] Task 2.4 — `update_model_metadata`, `upsert_model_kv`, `list_models_by_tag`, `search_models`
- [~] Task 2.5 — `space_overview` with `tokio::join!`

### [~] Phase 3 — Op-Log Integration
- [~] Added `UpdatePetalMeta`, `UpdateRoomMeta`, `UpdateModelMeta`, `ExportPetal`, `ImportPetal` to `OpType` enum
- [~] All `SpaceManager` mutation methods write op-log entries

### [~] Phase 4 — UI Panels
- [~] Task 3.1 — `PetalWizardState`, `WizardStep`, `can_advance`, `reset` in `fe-ui/src/atlas/petal_wizard.rs`
- [~] Task 3.2 — `add_tag` with `TagError` (Duplicate/TooLong/LimitReached)
- [~] Task 3.3 — `tag_input_ui`, `remove_tag` in `fe-ui/src/atlas/tag_panel.rs`
- [~] Task 3.4 — `RoomEditorState`, `bounds_valid`, `set_yaw` in `fe-ui/src/atlas/room_editor.rs`
- [~] Task 3.5 — `petal_wizard_system`, `room_editor_system`, `model_editor_system` registered in `fe-ui/src/atlas/mod.rs`
- [~] Task 4.1 — `ModelEditorState`, `KvRow`, `value_is_valid_json`, `can_save` in `fe-ui/src/atlas/model_editor.rs`
- [~] Task 4.2 — `VisibilityExt::display_name`, `visibility_radio_ui` in `fe-ui/src/atlas/visibility_control.rs`
- [~] Task 4.3 — `SearchQuery` with debounce + tag filter toggle in `fe-ui/src/atlas/search_bar.rs`
- [~] Task 4.4 — `DashboardState` with `apply_overview`/`apply_error` in `fe-ui/src/atlas/dashboard.rs`
- [~] `space_overview_panel` added to `fe-ui/src/panels.rs` (SpaceOverview display, tag filter, admin-gated config URL)
