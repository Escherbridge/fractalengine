# Petal Portal — TDD Implementation Plan

**Track ID:** `petal_portal_20260322`
**Methodology:** Red → Green → Refactor
**Phases:** 4

All tests live in the crate where the code lives, in `#[cfg(test)]` modules or `tests/` integration directories. Each task states the test to write first, then the implementation target, then the refactor notes.

---

## Phase 1: Model URL Schema Extension + Metadata Types

**Goal:** `ModelUrlMeta` component exists, compiles, serializes/deserializes correctly, and attaches to model entities without breaking existing scene loading.

---

### [~] Task 1.1 — Define `ModelUrlMeta` struct

**Red — write this test first:**

```rust
// fe-database/src/model_url_meta.rs (new file) or model_metadata.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_url_meta_defaults_to_none() {
        let meta = ModelUrlMeta::default();
        assert!(meta.external_url.is_none());
        assert!(meta.config_url.is_none());
    }

    #[test]
    fn model_url_meta_roundtrips_serde() {
        let meta = ModelUrlMeta {
            external_url: Some("https://example.com/dashboard".to_string()),
            config_url: Some("https://admin.example.com/config".to_string()),
        };
        let json = serde_json::to_string(&meta).unwrap();
        let back: ModelUrlMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(meta.external_url, back.external_url);
        assert_eq!(meta.config_url, back.config_url);
    }

    #[test]
    fn model_url_meta_deserializes_missing_fields_as_none() {
        let json = "{}";
        let meta: ModelUrlMeta = serde_json::from_str(json).unwrap();
        assert!(meta.external_url.is_none());
        assert!(meta.config_url.is_none());
    }
}
```

**Green:** Add `ModelUrlMeta` in `fe-database` (or wherever model metadata lives):

```rust
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[derive(bevy::prelude::Component, bevy::prelude::Reflect)]
#[reflect(Component)]
pub struct ModelUrlMeta {
    #[serde(default)]
    pub external_url: Option<String>,
    #[serde(default)]
    pub config_url: Option<String>,
}
```

Register in `AppTypeRegistry`.

**Refactor:** Ensure the field is re-exported from `fe-database::prelude` if one exists. Add `#[reflect(Default)]` if needed for scene serialization.

**Verification checkpoint:** `cargo test -p fe-database model_url_meta` passes.

---

### [~] Task 1.2 — Scene round-trip integration test

**Red:**

```rust
// fe-database/tests/scene_roundtrip.rs
#[test]
fn scene_with_model_url_meta_roundtrips_without_error() {
    // Build a minimal Bevy App with AppTypeRegistry
    // Spawn an entity with ModelUrlMeta { external_url: Some("https://example.com"), config_url: None }
    // Serialize scene to RON
    // Deserialize back
    // Assert component is present and fields match
    // Assert loading a scene WITHOUT ModelUrlMeta on an entity does not panic
}
```

**Green:** Verify `#[reflect(Default)]` and `#[serde(default)]` cover the missing-field case. Add `register::<ModelUrlMeta>()` to the app in the test harness.

**Refactor:** If the test harness is duplicated across crates, extract a `test_utils::minimal_app()` helper.

**Verification checkpoint:** Integration test passes. `cargo test -p fe-database scene_roundtrip` green.

---

### [~] Task 1.3 — `ModelUrlMeta` attached by default on model spawn

**Red:**

```rust
#[test]
fn spawned_model_entity_has_model_url_meta_component() {
    let mut app = minimal_app();
    // Use whatever system / command spawns a model entity
    let entity = app.world.spawn(ModelBundle::default()).id();
    app.update();
    assert!(app.world.get::<ModelUrlMeta>(entity).is_some());
}
```

**Green:** Add `ModelUrlMeta::default()` to `ModelBundle` (or wherever model entities are spawned).

**Refactor:** Confirm no existing tests break because of the new bundle field.

**Verification checkpoint:** `cargo test -p fe-database` fully green.

---

## Phase 2: Overlay Activation System (Selection → BrowserCommand Flow)

**Goal:** Selecting a model entity that has a valid `external_url` results in a `BrowserCommand::Navigate` event being written to the Bevy event queue. Deselecting closes or hides.

---

### [~] Task 2.1 — `ActivePortal` resource

**Red:**

```rust
#[test]
fn active_portal_starts_empty() {
    let mut app = minimal_app_with_portal_plugin();
    app.update();
    let portal = app.world.resource::<ActivePortal>();
    assert!(portal.entity.is_none());
}
```

**Green:**

```rust
#[derive(Resource, Default)]
pub struct ActivePortal {
    pub entity: Option<Entity>,
}
```

Register in `PetalPortalPlugin::build`.

**Refactor:** Consider `ActivePortal` as `bevy::prelude::Resource` with `ReflectResource` if the debugger needs to inspect it.

---

### [~] Task 2.2 — Activation system reads `SelectionChanged` and writes `BrowserCommand`

**Red:**

```rust
#[test]
fn selecting_model_with_allowed_url_emits_navigate_command() {
    let mut app = minimal_app_with_portal_plugin();

    let entity = app.world.spawn((
        ModelUrlMeta {
            external_url: Some("https://example.com".to_string()),
            config_url: None,
        },
        Selected,  // or whatever marker component / event fires
    )).id();

    app.update();

    let commands: Vec<&BrowserCommand> = collect_browser_commands(&app);
    assert!(commands.iter().any(|c| matches!(c, BrowserCommand::Navigate(url) if url == "https://example.com")));
}

#[test]
fn selecting_model_with_no_url_emits_no_navigate_command() {
    let mut app = minimal_app_with_portal_plugin();
    app.world.spawn((ModelUrlMeta::default(), Selected));
    app.update();
    let commands: Vec<&BrowserCommand> = collect_browser_commands(&app);
    assert!(!commands.iter().any(|c| matches!(c, BrowserCommand::Navigate(_))));
}

#[test]
fn selecting_model_with_blocked_url_emits_no_navigate_command() {
    let mut app = minimal_app_with_portal_plugin();
    app.world.spawn((
        ModelUrlMeta {
            external_url: Some("http://192.168.1.1/dashboard".to_string()),
            config_url: None,
        },
        Selected,
    )).id();
    app.update();
    let commands: Vec<&BrowserCommand> = collect_browser_commands(&app);
    assert!(!commands.iter().any(|c| matches!(c, BrowserCommand::Navigate(_))));
}
```

**Green:** Implement `petal_portal_activation_system`:
1. Read `Added<Selected>` query on entities with `ModelUrlMeta`.
2. For each: call `is_url_allowed(url)`. If false, `warn!()` and skip.
3. If valid: write `BrowserCommand::Navigate(url)` and `BrowserCommand::SwitchTab(BrowserTab::External)`.
4. Update `ActivePortal` resource with the entity.

**Refactor:** Extract URL validation into a `validate_and_emit` free function so it can be unit-tested independently of Bevy.

**Verification checkpoint:** All three tests pass. `cargo test -p fe-network` (or wherever the plugin lives) green.

---

### [~] Task 2.3 — Deactivation system on deselect / ESC

**Red:**

```rust
#[test]
fn deselecting_active_model_emits_close_command() {
    let mut app = minimal_app_with_portal_plugin();
    // Simulate open portal
    let entity = app.world.spawn(ModelUrlMeta {
        external_url: Some("https://example.com".to_string()),
        config_url: None,
    }).id();
    app.world.resource_mut::<ActivePortal>().entity = Some(entity);

    // Fire deselect
    app.world.send_event(SelectionCleared);
    app.update();

    let commands: Vec<&BrowserCommand> = collect_browser_commands(&app);
    assert!(commands.iter().any(|c| matches!(c, BrowserCommand::Close)));

    let portal = app.world.resource::<ActivePortal>();
    assert!(portal.entity.is_none());
}

#[test]
fn esc_key_with_active_portal_emits_close_command() {
    let mut app = minimal_app_with_portal_plugin();
    app.world.resource_mut::<ActivePortal>().entity = Some(Entity::PLACEHOLDER);
    // Simulate ESC keypress via Bevy Input resource
    app.world.resource_mut::<Input<KeyCode>>().press(KeyCode::Escape);
    app.update();
    let commands = collect_browser_commands(&app);
    assert!(commands.iter().any(|c| matches!(c, BrowserCommand::Close)));
}
```

**Green:** Implement `petal_portal_deactivation_system`. Watch for `RemovedComponents<Selected>` and `Input<KeyCode>::just_pressed(KeyCode::Escape)`. On either: write `BrowserCommand::Close`, clear `ActivePortal`.

**Refactor:** Consolidate the "clear portal" path into one function called by both triggers.

**Verification checkpoint:** Tests pass. No existing selection tests regress.

---

### [~] Task 2.4 — Overlay positioning system

**Red:**

```rust
#[test]
fn position_overlay_system_calls_position_with_clamped_coords() {
    // Uses a mock BrowserSurface that records position() calls
    // Spawn camera + model entity with a known world transform
    // Project expected screen-space rect manually
    // Run position_overlay_system
    // Assert mock received position() call with correct (or clamped) coords
    let mut app = minimal_app_with_portal_plugin();
    let mock_surface = MockBrowserSurface::new();
    // ... setup ...
    app.update();
    let call = mock_surface.last_position_call();
    assert!(call.is_some());
    let (x, y, w, h) = call.unwrap();
    // x, y within viewport; w >= 320; h >= 240
    assert!(x >= 0.0 && y >= 0.0);
    assert!(w >= 320.0 && h >= 240.0);
}

#[test]
fn position_overlay_system_repositions_on_camera_move() {
    // Run system twice; second time with a different camera transform
    // Assert position() was called twice with different coords
}
```

**Green:** Implement `position_overlay_system` in `PostUpdate`:
1. If `ActivePortal` has an entity, query its `GlobalTransform` and the active camera.
2. Project AABB (use `Aabb` component if present, else bounding sphere center) to NDC, then to logical pixels.
3. Place overlay at `(model_screen_right + 8.0, model_screen_top)`, clamped so `x + width <= viewport_width` and `y + height <= viewport_height`.
4. Call `surface.position(x, y, 480.0.max(remaining_width), 320.0)`.

Use `MockBrowserSurface` in tests (a struct implementing `BrowserSurface` that records calls).

**Refactor:** Extract projection math to a pure `fn project_aabb_to_screen(aabb, transform, camera) -> Rect` for isolated testing.

**Verification checkpoint:** Position tests pass. `cargo test position_overlay` green.

---

## Phase 3: Two-Tab System with Role-Gated Visibility

**Goal:** `TabVisibilityFilter` resource controls which tabs are accessible. Admins get both tabs; others get only External. Unauthorized tab-switch attempts are no-ops with warning logs.

---

### [~] Task 3.1 — `TabVisibilityFilter` resource

**Red:**

```rust
#[test]
fn tab_visibility_filter_default_is_not_admin() {
    let filter = TabVisibilityFilter::default();
    assert!(!filter.can_view_config());
}

#[test]
fn tab_visibility_filter_admin_can_view_config() {
    let filter = TabVisibilityFilter { role: Role::Admin };
    assert!(filter.can_view_config());
}

#[test]
fn tab_visibility_filter_non_admin_cannot_view_config() {
    for role in [Role::Viewer, Role::Editor] {
        let filter = TabVisibilityFilter { role };
        assert!(!filter.can_view_config());
    }
}
```

**Green:**

```rust
#[derive(Resource)]
pub struct TabVisibilityFilter {
    pub role: Role,
}

impl TabVisibilityFilter {
    pub fn can_view_config(&self) -> bool {
        self.role == Role::Admin
    }
}
```

Default: `Role::Viewer` or whatever the lowest-privilege role is.

**Refactor:** If `Role` already lives in `fe-auth`, import it; don't redefine it.

---

### [~] Task 3.2 — Config tab registration gated by role

**Red:**

```rust
#[test]
fn admin_selecting_model_with_config_url_sees_config_tab() {
    let mut app = minimal_app_with_portal_plugin();
    app.world.resource_mut::<TabVisibilityFilter>().role = Role::Admin;
    app.world.spawn((
        ModelUrlMeta {
            external_url: Some("https://example.com".to_string()),
            config_url: Some("https://admin.example.com/config".to_string()),
        },
        Selected,
    ));
    app.update();
    // Assert BrowserCommand::SwitchTab(BrowserTab::Config) is available
    // (i.e., the tab was registered / allowed — check via event or resource)
    let visible_tabs = app.world.resource::<VisibleTabs>();
    assert!(visible_tabs.contains(BrowserTab::Config));
}

#[test]
fn non_admin_selecting_model_with_config_url_does_not_see_config_tab() {
    let mut app = minimal_app_with_portal_plugin();
    app.world.resource_mut::<TabVisibilityFilter>().role = Role::Viewer;
    app.world.spawn((
        ModelUrlMeta {
            external_url: Some("https://example.com".to_string()),
            config_url: Some("https://admin.example.com/config".to_string()),
        },
        Selected,
    ));
    app.update();
    let visible_tabs = app.world.resource::<VisibleTabs>();
    assert!(!visible_tabs.contains(BrowserTab::Config));
}
```

**Green:** Add `VisibleTabs` resource (a `HashSet<BrowserTab>`). In `petal_portal_activation_system`, after processing `external_url`: check `TabVisibilityFilter::can_view_config()` AND `config_url.is_some()` before inserting `BrowserTab::Config` into `VisibleTabs`.

**Refactor:** `VisibleTabs` is reset to `{BrowserTab::External}` on every deactivation.

---

### [~] Task 3.3 — Unauthorized tab-switch is a no-op with warning

**Red:**

```rust
#[test]
fn switch_to_config_as_non_admin_is_noop_and_warns() {
    // Set up a tracing subscriber that captures warn! output, or use a mock
    let mut app = minimal_app_with_portal_plugin();
    app.world.resource_mut::<TabVisibilityFilter>().role = Role::Viewer;
    // Send SwitchTab(Config) command
    app.world.send_event(BrowserCommand::SwitchTab(BrowserTab::Config));
    // Should not panic
    app.update();
    // The Navigate command for config_url should NOT have been emitted
    let commands = collect_browser_commands(&app);
    assert!(!commands.iter().any(|c| matches!(c, BrowserCommand::Navigate(_))));
}
```

**Green:** Add a `tab_switch_guard_system` that runs before the command dispatcher. It reads incoming `BrowserCommand::SwitchTab` events, checks `TabVisibilityFilter`, drops unauthorized ones with `warn!()`.

**Refactor:** Merge guard logic into the activation system if separation adds unnecessary indirection.

**Verification checkpoint:** Phase 3 tests all pass. `cargo test -p fe-network tab` green.

---

## Phase 4: Trust Bar Injection + Lifecycle Management + URL Editor

**Goal:** `TRUST_BAR_JS` is injected at `WryBrowserSurface` construction. Overlay lifecycle handles all trigger cases. Inspector panel shows URL editor fields with live validation.

---

### [~] Task 4.1 — Trust bar JS injected at construction

**Red:**

```rust
#[test]
fn wry_browser_surface_builder_includes_trust_bar_init_script() {
    // Intercept or inspect the WebViewBuilder configuration
    // Assert that TRUST_BAR_JS appears in the init_scripts list
    // This may require a builder-inspection API or a dedicated test constructor
    let config = WebViewConfig {
        petal_id: "test".to_string(),
        initial_url: "https://example.com".to_string(),
        role: Role::Viewer,
    };
    let builder_spy = WryBrowserSurfaceBuilderSpy::new(config);
    assert!(builder_spy.init_scripts().contains(&TRUST_BAR_JS.to_string()));
}
```

**Green:** In `WryBrowserSurface::new()` (or the `WebViewBuilder` call chain), add:

```rust
.with_initialization_script(TRUST_BAR_JS)
```

This is the one-line change. Confirm `TRUST_BAR_JS` is already `pub` in `fe-network`.

**Refactor:** If `WryBrowserSurface` construction is behind a factory trait, ensure the factory always calls `with_initialization_script`. Add a lint comment: `// SECURITY: always inject trust bar — do not remove`.

**Verification checkpoint:** Trust bar test passes. Compile with `--features webview` to confirm no feature-gate issues.

---

### [~] Task 4.2 — Lifecycle: camera move triggers reposition

**Red:**

```rust
#[test]
fn camera_move_while_overlay_open_triggers_reposition() {
    let mut app = minimal_app_with_portal_plugin();
    let mock_surface = insert_mock_surface(&mut app);
    // Open portal
    activate_portal_for_entity(&mut app, "https://example.com");
    let initial_position_call_count = mock_surface.position_call_count();

    // Move the camera
    let camera = app.world.query::<&mut Transform>().single_mut(&mut app.world);
    *camera = Transform::from_xyz(10.0, 5.0, 20.0);
    app.update();

    assert!(mock_surface.position_call_count() > initial_position_call_count);
}
```

**Green:** The `position_overlay_system` from Task 2.4 already runs in `PostUpdate`. Add a change-detection guard: only call `surface.position()` if `ActivePortal` is `Some` AND (`Added<Selected>` OR camera `Transform` changed). Use `.is_changed()` on the camera query.

**Refactor:** Ensure the system is ordered after transform propagation (`TransformPropagate` set).

---

### [~] Task 4.3 — Lifecycle: scene unload closes overlay cleanly

**Red:**

```rust
#[test]
fn scene_unload_closes_overlay_without_panic() {
    let mut app = minimal_app_with_portal_plugin();
    activate_portal_for_entity(&mut app, "https://example.com");
    assert!(app.world.resource::<ActivePortal>().entity.is_some());

    // Despawn all model entities (simulate scene unload)
    let entities: Vec<Entity> = app.world.query_filtered::<Entity, With<ModelUrlMeta>>()
        .iter(&app.world).collect();
    for e in entities {
        app.world.despawn(e);
    }
    app.update();

    // Must not panic; ActivePortal must be cleared
    assert!(app.world.resource::<ActivePortal>().entity.is_none());
}
```

**Green:** Add a `portal_cleanup_system` that runs when the active portal's entity no longer exists (use `RemovedComponents` or a `With<ModelUrlMeta>` existence check). On missing entity: `BrowserCommand::Close`, clear `ActivePortal`, reset `VisibleTabs`.

**Refactor:** Unify with the deactivation system from Task 2.3 — they share the "clear portal" path.

---

### [~] Task 4.4 — URL editor: validation indicator

**Red:**

```rust
#[test]
fn url_editor_validation_state_allowed_url_is_valid() {
    let state = UrlEditorState::from_input("https://example.com");
    assert_eq!(state.validity, UrlValidity::Valid);
}

#[test]
fn url_editor_validation_state_blocked_url_is_invalid() {
    let state = UrlEditorState::from_input("http://192.168.1.1/dashboard");
    assert_eq!(state.validity, UrlValidity::Blocked);
}

#[test]
fn url_editor_validation_state_empty_is_neutral() {
    let state = UrlEditorState::from_input("");
    assert_eq!(state.validity, UrlValidity::Empty);
}
```

**Green:**

```rust
pub enum UrlValidity { Valid, Blocked, Empty }

pub struct UrlEditorState {
    pub raw: String,
    pub validity: UrlValidity,
}

impl UrlEditorState {
    pub fn from_input(s: &str) -> Self {
        let validity = if s.is_empty() {
            UrlValidity::Empty
        } else if is_url_allowed(s) {
            UrlValidity::Valid
        } else {
            UrlValidity::Blocked
        };
        Self { raw: s.to_string(), validity }
    }
}
```

**Refactor:** `UrlEditorState` lives next to the inspector panel UI code; it does not need to be in `fe-network`.

---

### [~] Task 4.5 — URL editor: Save commits and navigates

**Red:**

```rust
#[test]
fn url_editor_save_updates_model_url_meta_and_emits_navigate() {
    let mut app = minimal_app_with_portal_plugin();
    let entity = app.world.spawn(ModelUrlMeta {
        external_url: Some("https://old.example.com".to_string()),
        config_url: None,
    }).id();
    activate_portal_for_entity_explicit(&mut app, entity);

    // Simulate user editing and saving
    let new_url = "https://new.example.com".to_string();
    app.world.send_event(UrlEditorSaved { entity, field: UrlField::External, value: new_url.clone() });
    app.update();

    // ModelUrlMeta updated
    let meta = app.world.get::<ModelUrlMeta>(entity).unwrap();
    assert_eq!(meta.external_url.as_deref(), Some("https://new.example.com"));

    // Navigate command emitted
    let commands = collect_browser_commands(&app);
    assert!(commands.iter().any(|c| matches!(c, BrowserCommand::Navigate(u) if u == &new_url)));
}

#[test]
fn url_editor_save_blocked_url_does_not_update_meta() {
    let mut app = minimal_app_with_portal_plugin();
    let entity = app.world.spawn(ModelUrlMeta {
        external_url: Some("https://original.example.com".to_string()),
        config_url: None,
    }).id();
    app.world.send_event(UrlEditorSaved {
        entity,
        field: UrlField::External,
        value: "http://10.0.0.1/secret".to_string(),
    });
    app.update();
    let meta = app.world.get::<ModelUrlMeta>(entity).unwrap();
    // Must not have changed
    assert_eq!(meta.external_url.as_deref(), Some("https://original.example.com"));
}
```

**Green:** Add `UrlEditorSaved` event and `url_editor_commit_system`. On event: validate URL via `is_url_allowed()`. If valid: mutate `ModelUrlMeta`, if overlay is active for this entity emit `BrowserCommand::Navigate(new_url)`. If blocked: `warn!()` and no-op.

**Refactor:** The validation guard is the same pattern as Task 2.2. Consider a shared `fn emit_navigate_if_allowed(url, commands)` helper.

---

### [~] Task 4.6 — URL editor: Config URL field hidden from non-admins

**Red:**

```rust
#[test]
fn config_url_editor_visible_to_admin_only() {
    // This is a UI system test. Use a Bevy egui test harness or check the
    // inspector_panel_system's output state.
    let mut app = minimal_app_with_portal_plugin();

    app.world.resource_mut::<TabVisibilityFilter>().role = Role::Viewer;
    let viewer_state = run_inspector_panel_system(&mut app);
    assert!(!viewer_state.config_url_field_visible);

    app.world.resource_mut::<TabVisibilityFilter>().role = Role::Admin;
    let admin_state = run_inspector_panel_system(&mut app);
    assert!(admin_state.config_url_field_visible);
}
```

**Green:** In the inspector panel system, gate the Config URL field behind `TabVisibilityFilter::can_view_config()`. Use a helper struct or bool in the inspector state to make the condition testable without rendering.

**Refactor:** The `TabVisibilityFilter` resource is the single source of truth; no ad-hoc role checks in UI code.

---

## Verification Checkpoints

| After Phase | Command | Must pass |
|---|---|---|
| Phase 1 | `cargo test -p fe-database` | All model_url_meta + scene_roundtrip tests |
| Phase 2 | `cargo test -p fe-network` | activation, deactivation, position tests |
| Phase 3 | `cargo test -p fe-network tab` | tab visibility + switch guard tests |
| Phase 4 | `cargo test` (workspace) | All above + trust bar + lifecycle + editor tests |
| Final | `cargo clippy --all-targets -- -D warnings` | Zero warnings |
| Final | `cargo build --features webview` | Clean compile with wry feature enabled |

---

## System Registration Order

All systems added in `PetalPortalPlugin::build`:

```
PreUpdate:
  tab_switch_guard_system         (filters BrowserCommand::SwitchTab events)

Update:
  petal_portal_activation_system  (after SelectionChanged)
  petal_portal_deactivation_system
  portal_cleanup_system           (handles entity despawn)
  url_editor_commit_system

PostUpdate:
  position_overlay_system         (after TransformPropagate)
```

---

## Test Helper Inventory

| Helper | Purpose |
|---|---|
| `minimal_app_with_portal_plugin()` | Bevy App with `PetalPortalPlugin` + minimal deps, no window |
| `MockBrowserSurface` | Records `show/hide/navigate/position` calls |
| `WryBrowserSurfaceBuilderSpy` | Captures `WebViewBuilder` configuration for trust bar assertion |
| `collect_browser_commands(app)` | Drains `EventReader<BrowserCommand>` into a `Vec` |
| `activate_portal_for_entity(app, url)` | Shorthand: spawn model + select it + `app.update()` |
| `run_inspector_panel_system(app)` | Runs the panel system and returns observable state struct |

All helpers live in `tests/common/` or `#[cfg(test)]` modules co-located with the system they test.

---

## Risk Register

| Risk | Mitigation |
|---|---|
| wry `WebViewBuilder` API differs from expected in 0.54 | Spike in a standalone `wry-spike` binary before Task 4.1 |
| `BrowserTab` enum does not have `External` / `Config` variants | Add variants in Phase 3 setup; no breaking change to existing match arms if `#[non_exhaustive]` is handled |
| Screen-space projection differs by camera type (perspective vs orthographic) | Abstract behind `CameraProjection` trait method; test both projections in Task 2.4 |
| `is_url_allowed()` is not `pub` from outside `fe-network` | Re-export in `fe-network::prelude`; no logic change |
| Inspector panel test harness for `bevy_egui` is non-trivial | Use state-extraction pattern (Task 4.6) to avoid full egui render in tests |
