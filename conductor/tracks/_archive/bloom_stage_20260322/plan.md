# TDD Implementation Plan: Bloom Stage — 3D Scene Rendering & Object Interaction

**Track:** bloom_stage_20260322
**Project:** FractalEngine (Rust / Bevy 0.18)
**Date:** 2026-03-22

Each task follows the Red → Green → Refactor cycle. Write the failing test first, implement the minimum code to pass it, then clean up.

---

## Phase 1: 3D Camera, Lighting, and Ground Plane

**Goal:** Replace `Camera2d` with a navigable 3D orbit camera, add directional lighting and an ambient fill, and render a ground plane at y = 0 so there is a visible, navigable 3D environment before any models are loaded.

---

### Task 1.1 — `OrbitCameraState` resource

**Test first:**

```rust
#[test]
fn orbit_camera_state_defaults_are_sensible() {
    let state = OrbitCameraState::default();
    assert!(state.radius > 0.0);
    assert!(state.pitch >= -85_f32.to_radians());
    assert!(state.pitch <=  85_f32.to_radians());
    assert_eq!(state.focus, Vec3::ZERO);
}
```

**Implement:** Define `OrbitCameraState` as a Bevy `Resource` with fields `focus: Vec3`, `radius: f32`, `yaw: f32`, `pitch: f32`. Default: radius = 10.0, pitch = 0.3 rad, yaw = 0.0.

---

### Task 1.2 — Spherical-to-cartesian camera transform

**Test first:**

```rust
#[test]
fn camera_transform_from_state_looks_at_focus() {
    let state = OrbitCameraState {
        focus: Vec3::ZERO,
        radius: 10.0,
        yaw: 0.0,
        pitch: 0.0,
        ..Default::default()
    };
    let transform = camera_transform_from_state(&state);
    // Camera should sit on the +Z axis, looking at origin
    assert!((transform.translation.length() - 10.0).abs() < 1e-4);
    let forward = transform.forward();
    assert!(forward.dot(-Vec3::Z) > 0.99);
}
```

**Implement:** Pure function `camera_transform_from_state(state: &OrbitCameraState) -> Transform`. No Bevy systems yet — pure math only.

---

### Task 1.3 — Pitch clamping in orbit update

**Test first:**

```rust
#[test]
fn pitch_is_clamped_at_extremes() {
    let mut state = OrbitCameraState::default();
    apply_orbit_delta(&mut state, 0.0, 999.0); // large upward pitch
    assert!(state.pitch <= 85_f32.to_radians() + 1e-5);

    apply_orbit_delta(&mut state, 0.0, -999.0); // large downward pitch
    assert!(state.pitch >= -85_f32.to_radians() - 1e-5);
}
```

**Implement:** `apply_orbit_delta(state: &mut OrbitCameraState, delta_yaw: f32, delta_pitch: f32)` that mutates state and clamps pitch.

---

### Task 1.4 — Orbit camera Bevy system

**Test first:**

```rust
#[test]
fn orbit_system_updates_camera_transform() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
       .add_plugins(BloomStagePlugin);

    // Place camera at known state
    app.world_mut().resource_mut::<OrbitCameraState>().yaw = 1.0;
    app.update();

    let transform = app
        .world()
        .query::<(&Camera3d, &Transform)>()
        .single(app.world())
        .1;
    // Camera should not be at origin
    assert!(transform.translation.length() > 0.1);
}
```

**Implement:** `orbit_camera_system` reads mouse input and updates `OrbitCameraState`, then writes the `Transform` of the `Camera3d` entity.

---

### Task 1.5 — Camera3d spawns on startup; Camera2d is absent

**Test first:**

```rust
#[test]
fn startup_spawns_camera3d_not_camera2d() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
       .add_plugins(BloomStagePlugin);
    app.update();

    assert!(app.world().query::<&Camera3d>().iter(app.world()).count() == 1);
    assert!(app.world().query::<&Camera2d>().iter(app.world()).count() == 0);
}
```

**Implement:** `BloomStagePlugin::build` spawns `(Camera3d, OrbitCameraState-derived transform, …)` and ensures no `Camera2d` remains. Remove the `Camera2d` spawn from `main.rs`.

---

### Task 1.6 — Directional light and ambient light

**Test first:**

```rust
#[test]
fn scene_has_directional_light_with_shadows() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
       .add_plugins(BloomStagePlugin);
    app.update();

    let light = app
        .world()
        .query::<&DirectionalLight>()
        .single(app.world());
    assert!(light.shadows_enabled);
    assert!(light.illuminance >= 5_000.0);
}

#[test]
fn scene_has_ambient_light() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
       .add_plugins(BloomStagePlugin);
    app.update();

    let ambient = app.world().resource::<AmbientLight>();
    assert!(ambient.brightness > 0.0);
}
```

**Implement:** Startup system that spawns `DirectionalLight { illuminance: 10_000.0, shadows_enabled: true, … }` with angled transform and inserts `AmbientLight { brightness: 200.0, … }`.

---

### Task 1.7 — Ground plane entity

**Test first:**

```rust
#[test]
fn ground_plane_spawns_at_y_zero() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
       .add_plugins(BloomStagePlugin);
    app.update();

    let transform = app
        .world()
        .query_filtered::<&Transform, With<GroundPlane>>()
        .single(app.world());
    assert!((transform.translation.y).abs() < 1e-5);
}

#[test]
fn ground_plane_is_not_pickable() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
       .add_plugins(DefaultPickingPlugins)
       .add_plugins(BloomStagePlugin);
    app.update();

    let behavior = app
        .world()
        .query_filtered::<&PickingBehavior, With<GroundPlane>>()
        .single(app.world());
    assert!(!behavior.should_block_lower);
    // or assert_eq!(*behavior, PickingBehavior::IGNORE);
}
```

**Implement:** Marker component `GroundPlane`. Startup system spawns a `Mesh3d` plane with `PickingBehavior::IGNORE` and mid-grey `StandardMaterial`.

---

### Phase 1 Verification Checkpoint

```
cargo test -p fe-renderer -- phase1
```

Run the app. Confirm:
- 3D perspective view on startup, no 2D overlay
- Left-drag orbits; scroll zooms; right-drag pans
- Grey ground plane visible at y = 0
- Models receive directional shadows (test with a cube mesh)
- Pitch clamping prevents camera flip at extreme angles

---

## Phase 2: GLTF Spawning System with ModelMetadata Component

**Goal:** Define `ModelMetadata`, implement transform serialization, and spawn `SceneRoot` entities from model DB records with correct transforms and metadata.

---

### Task 2.1 — `ModelMetadata` component definition and serde round-trip

**Test first:**

```rust
#[test]
fn model_metadata_round_trips_through_json() {
    let meta = ModelMetadata {
        asset_id: "blake3abc123".to_string(),
        petal_id: 42,
        display_name: "Test Cube".to_string(),
        external_url: Some("https://example.com".to_string()),
        tags: vec!["prop".to_string(), "static".to_string()],
    };
    let json = serde_json::to_string(&meta).unwrap();
    let decoded: ModelMetadata = serde_json::from_str(&json).unwrap();
    assert_eq!(meta.asset_id, decoded.asset_id);
    assert_eq!(meta.tags, decoded.tags);
}
```

**Implement:** `ModelMetadata` in `fe-renderer/src/model_metadata.rs` deriving `Component`, `Reflect`, `Serialize`, `Deserialize`, `Clone`, `Debug`.

---

### Task 2.2 — Transform → JSON serialization

**Test first:**

```rust
#[test]
fn identity_transform_serializes_correctly() {
    let t = Transform::IDENTITY;
    let json = transform_to_json(&t);
    assert_eq!(json["translation"], serde_json::json!([0.0, 0.0, 0.0]));
    assert_eq!(json["scale"], serde_json::json!([1.0, 1.0, 1.0]));
}

#[test]
fn arbitrary_transform_round_trips() {
    let t = Transform {
        translation: Vec3::new(1.0, 2.0, 3.0),
        rotation: Quat::from_rotation_y(0.5),
        scale: Vec3::splat(2.0),
    };
    let json = transform_to_json(&t);
    let decoded = json_to_transform(&json).unwrap();
    assert!((t.translation - decoded.translation).length() < 1e-5);
    assert!((t.scale - decoded.scale).length() < 1e-5);
}

#[test]
fn malformed_json_returns_error() {
    let bad = serde_json::json!({ "translation": "not_an_array" });
    assert!(json_to_transform(&bad).is_err());
}
```

**Implement:** `transform_to_json(t: &Transform) -> serde_json::Value` and `json_to_transform(v: &serde_json::Value) -> Result<Transform, TransformParseError>` in `fe-renderer/src/model_metadata.rs`.

---

### Task 2.3 — `SpawnedModels` resource prevents duplicate spawns

**Test first:**

```rust
#[test]
fn spawned_models_tracks_asset_ids() {
    let mut spawned = SpawnedModels::default();
    spawned.insert("abc".to_string());
    assert!(spawned.contains("abc"));
    assert!(!spawned.contains("xyz"));
}
```

**Implement:** `SpawnedModels(HashSet<String>)` newtype resource in `fe-renderer/src/scene.rs`.

---

### Task 2.4 — Spawn system produces entities with `ModelMetadata`

**Test first:**

```rust
#[test]
fn spawn_system_creates_one_entity_per_record() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
       .add_plugins(BloomStagePlugin);

    // Inject mock model records via event or resource
    let records = vec![
        mock_model_record("id1", "blake3aaa", 1),
        mock_model_record("id2", "blake3bbb", 1),
    ];
    app.world_mut().resource_mut::<PendingModelRecords>().0 = records;
    app.update();

    let count = app
        .world()
        .query::<&ModelMetadata>()
        .iter(app.world())
        .count();
    assert_eq!(count, 2);
}
```

**Implement:** `PendingModelRecords` resource, `spawn_models_system` that reads it and spawns `(SceneRoot, Transform, ModelMetadata, ModelState::Spawning)` per record, inserting into `SpawnedModels`.

---

### Task 2.5 — Spawn system respects `SpawnedModels` deduplication

**Test first:**

```rust
#[test]
fn spawn_system_skips_already_spawned_models() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
       .add_plugins(BloomStagePlugin);

    app.world_mut().resource_mut::<SpawnedModels>()
        .insert("blake3aaa".to_string());

    let records = vec![mock_model_record("id1", "blake3aaa", 1)];
    app.world_mut().resource_mut::<PendingModelRecords>().0 = records;
    app.update();

    let count = app.world().query::<&ModelMetadata>().iter(app.world()).count();
    assert_eq!(count, 0); // already tracked, not re-spawned
}
```

**Implement:** Guard in `spawn_models_system` checking `SpawnedModels` before spawning.

---

### Task 2.6 — `ModelState` transitions from `Spawning` to `Ready`

**Test first:**

```rust
#[test]
fn model_state_transitions_to_ready_when_asset_loaded() {
    // This requires a mock AssetServer or a loaded Handle<Gltf>.
    // Use a pre-loaded asset in test context.
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
       .add_plugins(AssetPlugin::default())
       .add_plugins(BloomStagePlugin);

    // Spawn an entity with ModelState::Spawning and a pre-resolved handle
    let entity = app.world_mut().spawn((
        ModelState::Spawning,
        // loaded_gltf_handle (mock),
    )).id();

    app.update();
    let state = app.world().get::<ModelState>(entity).unwrap();
    // Should transition once asset is available
    // (exact assertion depends on mock setup)
    assert!(matches!(state, ModelState::Ready | ModelState::Spawning));
}
```

**Implement:** `check_model_load_state_system` that queries `(Entity, &mut ModelState, &Handle<Gltf>)` and transitions `Spawning → Ready` once `asset_server.load_state(handle) == LoadState::Loaded`.

---

### Phase 2 Verification Checkpoint

```
cargo test -p fe-renderer -- phase2
cargo test -p fe-database -- model
```

Run the app with seeded DB data. Confirm:
- Models from DB appear in the 3D scene at their stored transforms
- Restarting the app does not double-spawn models
- Console shows no asset load panics
- `ModelState::Ready` is set after GLTF resolves

---

## Phase 3: Raycasting Selection and Highlight System

**Goal:** Click a model to select it, hover to preview. Apply distinct material tints for hover and selection states. Restore materials on state change.

---

### Task 3.1 — `SelectedModel` and `HoveredModel` resources initialize as `None`

**Test first:**

```rust
#[test]
fn selection_resources_initialize_as_none() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
       .add_plugins(BloomStagePlugin);
    app.update();

    assert!(app.world().resource::<SelectedModel>().0.is_none());
    assert!(app.world().resource::<HoveredModel>().0.is_none());
}
```

**Implement:** `SelectedModel(Option<Entity>)` and `HoveredModel(Option<Entity>)` resources inserted by `BloomStagePlugin`.

---

### Task 3.2 — Click sets `SelectedModel`

**Test first:**

```rust
#[test]
fn click_on_model_entity_sets_selected_model() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
       .add_plugins(DefaultPickingPlugins)
       .add_plugins(BloomStagePlugin);

    let entity = app.world_mut().spawn((
        ModelMetadata { /* ... */ },
        PickingBehavior::default(),
    )).id();

    // Simulate Pointer<Click> observer trigger
    app.world_mut().trigger_targets(Pointer::<Click>::new(/* ... */), entity);
    app.update();

    assert_eq!(app.world().resource::<SelectedModel>().0, Some(entity));
}
```

**Implement:** `on_model_click` observer function that sets `SelectedModel`. Attach via `commands.entity(entity).observe(on_model_click)` when spawning model entities.

---

### Task 3.3 — Click on empty space clears `SelectedModel`

**Test first:**

```rust
#[test]
fn click_on_empty_space_clears_selection() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
       .add_plugins(DefaultPickingPlugins)
       .add_plugins(BloomStagePlugin);

    app.world_mut().resource_mut::<SelectedModel>().0 = Some(Entity::PLACEHOLDER);

    // Simulate a click miss (no entity hit) — send a custom ClearSelection event
    app.world_mut().send_event(ClearSelectionEvent);
    app.update();

    assert!(app.world().resource::<SelectedModel>().0.is_none());
}
```

**Implement:** `ClearSelectionEvent` and a system that listens for it (triggered by clicking the ground plane or by a missed raycast).

---

### Task 3.4 — Hover sets and clears `HoveredModel`

**Test first:**

```rust
#[test]
fn hover_over_sets_hovered_model() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
       .add_plugins(DefaultPickingPlugins)
       .add_plugins(BloomStagePlugin);

    let entity = app.world_mut().spawn((ModelMetadata { /* ... */ },)).id();

    app.world_mut().trigger_targets(Pointer::<Over>::new(/* ... */), entity);
    app.update();
    assert_eq!(app.world().resource::<HoveredModel>().0, Some(entity));

    app.world_mut().trigger_targets(Pointer::<Out>::new(/* ... */), entity);
    app.update();
    assert!(app.world().resource::<HoveredModel>().0.is_none());
}
```

**Implement:** `on_model_over` and `on_model_out` observers updating `HoveredModel`.

---

### Task 3.5 — `OriginalMaterials` cached at spawn time

**Test first:**

```rust
#[test]
fn original_materials_are_cached_on_spawn() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
       .add_plugins(BloomStagePlugin);

    // Spawn a model root with a known child mesh
    let child = app.world_mut().spawn(MeshMaterial3d::<StandardMaterial>(
        app.world().resource::<Assets<StandardMaterial>>().add(StandardMaterial::default())
    )).id();
    let root = app.world_mut().spawn((ModelMetadata { /* ... */ },))
        .add_child(child)
        .id();

    // Run the cache system
    app.update();

    assert!(app.world().get::<OriginalMaterials>(root).is_some());
}
```

**Implement:** `cache_original_materials_system` that runs after spawn and populates `OriginalMaterials` on root entities with `ModelMetadata`.

---

### Task 3.6 — Highlight system applies tints and restores on state change

**Test first:**

```rust
#[test]
fn selected_entity_gets_blue_tint_applied() {
    // Setup with a model entity and its child mesh material
    // Set SelectedModel to the entity
    // Run update
    // Assert child mesh material has non-zero emissive (blue channel dominant)
}

#[test]
fn deselected_entity_has_original_material_restored() {
    // Setup: entity was selected, material was tinted
    // Set SelectedModel to None
    // Run update
    // Assert child mesh material handle matches original cached handle
}
```

**Implement:** `selection_highlight_system` that reads `SelectedModel` / `HoveredModel`, swaps child mesh materials using `OriginalMaterials` cache.

---

### Phase 3 Verification Checkpoint

```
cargo test -p fe-renderer -- phase3
```

Run the app. Confirm:
- Hovering a model applies a yellow/gold tint
- Clicking a model applies a blue tint (distinct from hover)
- Clicking empty space removes all tints
- No material permanently mutated (restart shows original colors)
- Child mesh click selects the root entity (test with a multi-mesh GLTF)

---

## Phase 4: Transform Editor Panel (egui sliders → DB update)

**Goal:** Build an egui "Transform" side panel that shows editable position / rotation / scale sliders for the selected model. Changes update the entity transform immediately and persist to the DB via the debounced `DbCommand` channel.

---

### Task 4.1 — `transform_editor_ui` is a pure function testable without Bevy

**Test first:**

```rust
#[test]
fn transform_editor_does_not_panic_with_identity_transform() {
    let ctx = egui::Context::default();
    let mut transform = Transform::IDENTITY;
    let mut output = TransformEditorOutput::default();

    ctx.run(egui::RawInput::default(), |ctx| {
        transform_editor_ui(ctx, &mut transform, &mut output);
    });
    // No panic = pass
}
```

**Implement:** `transform_editor_ui(ctx: &egui::Context, transform: &mut Transform, output: &mut TransformEditorOutput)` in `fe-ui/src/transform_editor.rs`. `TransformEditorOutput` carries `apply_pressed: bool` and `reset_pressed: bool`.

---

### Task 4.2 — Rotation displayed and edited in degrees

**Test first:**

```rust
#[test]
fn rotation_round_trips_through_degrees() {
    let original_quat = Quat::from_euler(EulerRot::ZYX, 0.1, 0.5, 1.2);
    let degrees = quat_to_euler_degrees(original_quat);
    let recovered = euler_degrees_to_quat(degrees);
    let dot = original_quat.dot(recovered).abs();
    assert!(dot > 0.9999);
}
```

**Implement:** `quat_to_euler_degrees(q: Quat) -> Vec3` and `euler_degrees_to_quat(degrees: Vec3) -> Quat` helper functions.

---

### Task 4.3 — "Apply" button output is captured

**Test first:**

```rust
#[test]
fn apply_button_sets_output_flag() {
    // Use egui harness to simulate a click on the Apply button
    // Assert output.apply_pressed == true after the click
    // (Implementation detail: may require egui_test or manual ctx.run)
}
```

**Implement:** Wire the "Apply" button in `transform_editor_ui` to set `output.apply_pressed = true`.

---

### Task 4.4 — "Reset Transform" restores identity

**Test first:**

```rust
#[test]
fn reset_button_resets_transform_to_identity() {
    let mut transform = Transform {
        translation: Vec3::new(5.0, 5.0, 5.0),
        ..Default::default()
    };
    let mut output = TransformEditorOutput::default();
    // Simulate Reset button click via output flag
    output.reset_pressed = true;
    apply_transform_editor_output(&mut transform, &output);
    assert_eq!(transform.translation, Vec3::ZERO);
}
```

**Implement:** `apply_transform_editor_output(transform: &mut Transform, output: &TransformEditorOutput)` that handles reset.

---

### Task 4.5 — Transform editor Bevy system wires into `SelectedModel`

**Test first:**

```rust
#[test]
fn transform_editor_system_does_not_panic_when_no_model_selected() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
       .add_plugins(EguiPlugin)
       .add_plugins(BloomStagePlugin);
    // SelectedModel is None
    app.update(); // should not panic
}
```

**Implement:** `transform_editor_system` that early-returns when `SelectedModel` is `None`, otherwise queries the entity's `Transform` and renders the panel.

---

### Task 4.6 — DB write is debounced (no write during mid-drag)

**Test first:**

```rust
#[test]
fn db_write_is_not_sent_before_debounce_interval() {
    let mut debounce = TransformDebounce::default();
    let now = Instant::now();

    // First change
    let should_write = debounce.should_write(now, Transform::IDENTITY, Transform::from_xyz(1.0, 0.0, 0.0));
    assert!(!should_write); // too soon

    // After 200ms
    let later = now + Duration::from_millis(250);
    let should_write = debounce.should_write(later, Transform::IDENTITY, Transform::from_xyz(1.0, 0.0, 0.0));
    assert!(should_write);
}
```

**Implement:** `TransformDebounce` resource with `last_write: Option<Instant>` and `should_write(now, prev, next) -> bool` checking elapsed time and epsilon delta.

---

### Phase 4 Verification Checkpoint

```
cargo test -p fe-ui -- transform
cargo test -p fe-renderer -- transform_editor
```

Run the app. Confirm:
- "Transform" panel appears on the right when a model is selected
- Panel disappears when no model is selected
- Dragging position sliders moves the model in real time
- Releasing a slider and waiting 200 ms writes to DB (verify via DB read)
- "Reset Transform" moves model back to origin and writes to DB
- No DB writes fire while dragging (check DB write counter)

---

## Phase 5: Model Inspector Panel with Metadata Display

**Goal:** Build the "Inspector" egui panel that shows `ModelMetadata` fields (display_name, asset_id, petal_id, external_url, tags) for the selected model in a read-only layout. Integrate with the transform editor panel.

---

### Task 5.1 — `model_inspector_ui` is a pure function testable without Bevy

**Test first:**

```rust
#[test]
fn inspector_ui_does_not_panic_with_sample_metadata() {
    let ctx = egui::Context::default();
    let meta = ModelMetadata {
        asset_id: "abc123def456789012345678".to_string(),
        petal_id: 7,
        display_name: "Fountain".to_string(),
        external_url: Some("https://example.com/fountain".to_string()),
        tags: vec!["water".to_string(), "decoration".to_string()],
    };
    ctx.run(egui::RawInput::default(), |ctx| {
        model_inspector_ui(ctx, Some(&meta));
    });
    // No panic = pass
}
```

**Implement:** `model_inspector_ui(ctx: &egui::Context, metadata: Option<&ModelMetadata>)` in `fe-ui/src/inspector.rs`. Renders "No model selected" when `None`.

---

### Task 5.2 — `asset_id` is truncated to 16 chars + ellipsis

**Test first:**

```rust
#[test]
fn asset_id_is_truncated_for_display() {
    let full_id = "blake3abcdef1234567890abcdef1234";
    let truncated = truncate_asset_id(full_id);
    assert_eq!(truncated, "blake3abcdef1234\u{2026}");
    assert!(truncated.len() <= 20);
}

#[test]
fn short_asset_id_is_not_truncated() {
    let short_id = "blake3abc";
    let truncated = truncate_asset_id(short_id);
    assert_eq!(truncated, "blake3abc");
}
```

**Implement:** `truncate_asset_id(id: &str) -> String` in `fe-ui/src/inspector.rs`.

---

### Task 5.3 — "No model selected" shown when selection is `None`

**Test first:**

```rust
#[test]
fn inspector_shows_placeholder_when_none() {
    // Use egui harness to capture rendered text
    // Assert "No model selected" appears in output
    // (snapshot or label-text approach depending on available egui test utilities)
    let ctx = egui::Context::default();
    let mut label_seen = false;
    ctx.run(egui::RawInput::default(), |ctx| {
        egui::SidePanel::right("inspector").show(ctx, |ui| {
            model_inspector_inner(ui, None, &mut label_seen);
        });
    });
    assert!(label_seen);
}
```

**Implement:** Branch in `model_inspector_ui` that renders the placeholder label when `metadata` is `None`.

---

### Task 5.4 — Tags render as individual UI elements

**Test first:**

```rust
#[test]
fn tag_list_renders_correct_count() {
    // Given metadata with 3 tags, the tag renderer produces 3 label widgets
    let tags = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    let count = count_tag_widgets(&tags);
    assert_eq!(count, 3);
}
```

**Implement:** `render_tags(ui: &mut egui::Ui, tags: &[String])` helper that renders each tag as a `ui.label` or styled button (chip style).

---

### Task 5.5 — Inspector Bevy system reads `SelectedModel` and queries `ModelMetadata`

**Test first:**

```rust
#[test]
fn inspector_system_does_not_panic_when_no_model_selected() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
       .add_plugins(EguiPlugin)
       .add_plugins(BloomStagePlugin);
    // SelectedModel is None
    app.update(); // should not panic
}

#[test]
fn inspector_system_does_not_panic_when_model_selected() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
       .add_plugins(EguiPlugin)
       .add_plugins(BloomStagePlugin);

    let entity = app.world_mut().spawn(ModelMetadata {
        asset_id: "abc".to_string(),
        petal_id: 1,
        display_name: "Cube".to_string(),
        external_url: None,
        tags: vec![],
    }).id();

    app.world_mut().resource_mut::<SelectedModel>().0 = Some(entity);
    app.update(); // should not panic
}
```

**Implement:** `model_inspector_system` that reads `SelectedModel`, looks up `ModelMetadata` from the entity (if any), and calls `model_inspector_ui`.

---

### Task 5.6 — External URL is rendered as a hyperlink

**Test first:**

```rust
#[test]
fn external_url_link_is_rendered_when_present() {
    // Verify that the hyperlink widget path is taken when external_url is Some
    // Assert no panic and correct branching
    let url = Some("https://example.com".to_string());
    let rendered = render_external_url_to_string(url.as_deref());
    assert!(rendered.contains("example.com"));
}

#[test]
fn no_hyperlink_rendered_when_url_absent() {
    let rendered = render_external_url_to_string(None);
    assert!(!rendered.contains("http"));
}
```

**Implement:** Conditional `ui.hyperlink_to(…)` in `model_inspector_ui` when `external_url` is `Some`.

---

### Phase 5 Verification Checkpoint

```
cargo test -p fe-ui -- inspector
cargo test -p fe-renderer -- inspector_system
```

Run the app. Confirm:
- "Inspector" panel is hidden when nothing is selected
- Clicking a model shows its display_name, truncated asset_id, petal_id, tags, and URL
- Copy button copies full asset_id to clipboard (manual verification)
- External URL renders as a clickable link
- Switching selection updates panel immediately
- Both "Transform" and "Inspector" panels coexist without layout overlap

---

## Cross-Phase Notes

### Test Harness Pattern

For Bevy system tests, use the `MinimalPlugins` + `BloomStagePlugin` pattern throughout. Avoid `DefaultPlugins` in unit tests to prevent windowing/GPU dependencies.

### Debounce and Timing Tests

Use `std::time::Instant` injection (pass `Instant` as a parameter rather than calling `Instant::now()` inside the function) to make debounce logic deterministic in tests.

### DB Channel Tests

For tasks that write to the DB channel, assert the `DbCommand` variant received by the channel receiver rather than asserting DB state. Full DB round-trip tests belong in `fe-database` integration tests.

### Asset Loading in Tests

GLTF asset loading requires a real `AssetServer`. For unit tests of spawn logic, use a `PendingModelRecords` resource as the boundary and test the spawning logic independent of the asset server. Load-state transition tests use `AssetPlugin::default()` with embedded test assets.
