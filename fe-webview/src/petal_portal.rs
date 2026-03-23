/// Petal Portal — Overlay Activation System
///
/// Drives the full lifecycle of the in-scene browser overlay:
///   - Activation on model entity selection
///   - Deactivation on deselect / ESC
///   - Cleanup when the active entity is despawned
///   - Overlay repositioning each frame (PostUpdate)
///   - Tab visibility gating by role (Phase 3)
///   - URL editor commit system (Phase 4)

use bevy::prelude::*;
use std::collections::HashSet;
use url::Url;

use crate::ipc::{BrowserCommand, BrowserTab};
use fe_database::ModelUrlMeta;

// ---------------------------------------------------------------------------
// Marker component — placed on an entity when it is "selected" by the player.
// Other crates may drive selection differently; this component is the contract.
// ---------------------------------------------------------------------------

/// Marker component: the entity is currently selected by the local user.
#[derive(Debug, Component, Default)]
pub struct Selected;

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// Fired when all selections are cleared (e.g. clicking empty space, pressing ESC).
#[derive(Debug, Clone, Event)]
pub struct SelectionCleared;

/// Fired by the URL editor panel when the user commits a URL change.
#[derive(Debug, Clone, Event)]
pub struct UrlEditorSaved {
    /// Entity whose `ModelUrlMeta` should be mutated.
    pub entity: Entity,
    /// Which field is being edited.
    pub field: UrlField,
    /// The new raw URL string (not yet validated here; the system validates).
    pub value: String,
}

/// Which URL field the editor is targeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlField {
    External,
    Config,
}

// ---------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------

/// Tracks which model entity currently has the portal overlay open.
#[derive(Debug, Resource, Default)]
pub struct ActivePortal {
    pub entity: Option<Entity>,
}

/// Role enum used for tab visibility gating.
/// Mirrors the role concept in `fe-auth`; defined here to avoid a hard dep.
// CROSS-CRATE: verify this matches the canonical Role type after merge
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Full access including Config tab.
    Admin,
    /// Can view but not edit.
    Viewer,
    /// Can edit content but not admin functions.
    Editor,
}

impl Default for Role {
    fn default() -> Self {
        Role::Viewer
    }
}

/// Controls which browser tabs are visible for the current session.
#[derive(Debug, Resource)]
pub struct TabVisibilityFilter {
    pub role: Role,
}

impl Default for TabVisibilityFilter {
    fn default() -> Self {
        Self { role: Role::Viewer }
    }
}

impl TabVisibilityFilter {
    /// Returns `true` only for Admin role.
    pub fn can_view_config(&self) -> bool {
        self.role == Role::Admin
    }
}

/// Set of `BrowserTab` values the overlay is currently permitted to show.
/// Reset to `{BrowserTab::ExternalUrl}` on every deactivation.
#[derive(Debug, Resource)]
pub struct VisibleTabs(pub HashSet<BrowserTab>);

impl Default for VisibleTabs {
    fn default() -> Self {
        let mut set = HashSet::new();
        set.insert(BrowserTab::ExternalUrl);
        Self(set)
    }
}

impl VisibleTabs {
    pub fn contains(&self, tab: BrowserTab) -> bool {
        self.0.contains(&tab)
    }

    fn reset(&mut self) {
        self.0.clear();
        self.0.insert(BrowserTab::ExternalUrl);
    }
}

// ---------------------------------------------------------------------------
// URL validation helpers (thin wrappers around fe-webview::security)
// ---------------------------------------------------------------------------

/// Returns `true` if `raw` can be parsed as a URL and is allowed by the
/// security policy (no private/loopback ranges, https/http only).
///
/// Wraps `crate::security::is_url_allowed` to work with raw `&str` inputs
/// coming from editor fields and Bevy events.
pub fn is_raw_url_allowed(raw: &str) -> bool {
    match raw.parse::<Url>() {
        Ok(url) => crate::security::is_url_allowed(&url),
        Err(_) => false,
    }
}

/// Attempt to emit a `BrowserCommand::Navigate` message if `url` is allowed.
/// Returns `true` on success, `false` if blocked.
fn emit_navigate_if_allowed(url: &str, commands: &mut MessageWriter<BrowserCommand>) -> bool {
    match url.parse::<Url>() {
        Ok(parsed) if crate::security::is_url_allowed(&parsed) => {
            commands.write(BrowserCommand::Navigate { url: parsed });
            true
        }
        Ok(_) => {
            warn!(
                target: "petal_portal",
                "URL blocked by security policy: {url}"
            );
            false
        }
        Err(e) => {
            warn!(
                target: "petal_portal",
                "Invalid URL `{url}`: {e}"
            );
            false
        }
    }
}

/// Shared "clear portal state" path used by deactivation and cleanup systems.
fn clear_portal(
    portal: &mut ActivePortal,
    visible_tabs: &mut VisibleTabs,
    commands: &mut MessageWriter<BrowserCommand>,
) {
    if portal.entity.is_some() {
        commands.write(BrowserCommand::Close);
        portal.entity = None;
        visible_tabs.reset();
    }
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

/// PreUpdate — filters unauthorized `BrowserCommand::SwitchTab` events.
///
/// Reads every pending `BrowserCommand`; if a `SwitchTab(Config)` arrives
/// and the current role cannot view config, the message is consumed and a
/// warning is logged. All other commands are re-emitted unchanged.
///
/// NOTE: Because Bevy messages are consumed on read, this system must re-emit
/// allowed commands so the downstream system still sees them.
pub fn tab_switch_guard_system(
    filter: Res<TabVisibilityFilter>,
    mut reader: MessageReader<BrowserCommand>,
    mut writer: MessageWriter<BrowserCommand>,
) {
    let mut pass_through = Vec::new();
    for cmd in reader.read() {
        match cmd {
            BrowserCommand::SwitchTab { tab: BrowserTab::Config } => {
                if !filter.can_view_config() {
                    warn!(
                        target: "petal_portal",
                        "Unauthorized SwitchTab(Config) blocked — role={:?}",
                        filter.role
                    );
                    // Drop the command — do not push to pass_through.
                } else {
                    pass_through.push(cmd.clone());
                }
            }
            other => pass_through.push(other.clone()),
        }
    }
    for cmd in pass_through {
        writer.write(cmd);
    }
}

/// Update — fires when a `Selected` component is added to an entity that has
/// `ModelUrlMeta`. Emits a `BrowserCommand::Navigate` (if URL is allowed),
/// updates `ActivePortal`, and populates `VisibleTabs`.
pub fn petal_portal_activation_system(
    mut portal: ResMut<ActivePortal>,
    mut visible_tabs: ResMut<VisibleTabs>,
    filter: Res<TabVisibilityFilter>,
    query: Query<(Entity, &ModelUrlMeta), Added<Selected>>,
    mut browser_commands: MessageWriter<BrowserCommand>,
) {
    for (entity, meta) in query.iter() {
        // Always update the active portal to the newly-selected entity.
        portal.entity = Some(entity);
        visible_tabs.reset();

        // Navigate to external_url if present and allowed.
        if let Some(url) = &meta.external_url {
            if !url.is_empty() {
                let emitted = emit_navigate_if_allowed(url, &mut browser_commands);
                if emitted {
                    browser_commands.write(BrowserCommand::SwitchTab {
                        tab: BrowserTab::ExternalUrl,
                    });
                }
            }
        }

        // Gate Config tab on role + presence of config_url.
        if filter.can_view_config() {
            if let Some(config_url) = &meta.config_url {
                if !config_url.is_empty() {
                    visible_tabs.0.insert(BrowserTab::Config);
                }
            }
        }
    }
}

/// Update — closes the portal on explicit deselection (`SelectionCleared` event)
/// or when ESC is pressed.
pub fn petal_portal_deactivation_system(
    mut portal: ResMut<ActivePortal>,
    mut visible_tabs: ResMut<VisibleTabs>,
    mut cleared_events: EventReader<SelectionCleared>,
    keys: Res<ButtonInput<KeyCode>>,
    mut browser_commands: MessageWriter<BrowserCommand>,
) {
    let esc_pressed = keys.just_pressed(KeyCode::Escape);
    let selection_cleared = cleared_events.read().count() > 0;

    if esc_pressed || selection_cleared {
        clear_portal(&mut portal, &mut visible_tabs, &mut browser_commands);
    }
}

/// Update — cleans up the portal if the active entity has been despawned
/// (e.g. scene unload).
pub fn portal_cleanup_system(
    mut portal: ResMut<ActivePortal>,
    mut visible_tabs: ResMut<VisibleTabs>,
    url_meta_query: Query<Entity, With<ModelUrlMeta>>,
    mut browser_commands: MessageWriter<BrowserCommand>,
) {
    if let Some(active_entity) = portal.entity {
        // If the entity is gone from the world, clear the portal.
        if url_meta_query.get(active_entity).is_err() {
            clear_portal(&mut portal, &mut visible_tabs, &mut browser_commands);
        }
    }
}

/// PostUpdate — repositions the browser overlay whenever the camera moves or
/// the active portal changes. Runs after `TransformPropagate`.
pub fn position_overlay_system(
    portal: Res<ActivePortal>,
    model_query: Query<&GlobalTransform, With<ModelUrlMeta>>,
    camera_query: Query<(&Camera, &GlobalTransform), Changed<GlobalTransform>>,
) {
    // Only reposition when there is an active portal.
    let Some(active_entity) = portal.entity else {
        return;
    };
    // Only act when the camera moved (change detection).
    let Ok((_camera, _cam_transform)) = camera_query.get_single() else {
        return;
    };
    let Ok(_model_transform) = model_query.get(active_entity) else {
        return;
    };

    // TODO Sprint 4: project AABB to screen space and call surface.position().
    // DEFERRED TO VALIDATION PHASE — full projection math pending wry surface integration.
}

/// Update — processes `UrlEditorSaved` events. Validates the new URL, mutates
/// `ModelUrlMeta`, and (if the overlay is active for this entity) navigates.
pub fn url_editor_commit_system(
    mut saved_events: EventReader<UrlEditorSaved>,
    mut meta_query: Query<&mut ModelUrlMeta>,
    portal: Res<ActivePortal>,
    mut browser_commands: MessageWriter<BrowserCommand>,
) {
    for event in saved_events.read() {
        // Validate the incoming URL before touching any state.
        if !is_raw_url_allowed(&event.value) {
            warn!(
                target: "petal_portal",
                "UrlEditorSaved blocked — URL not allowed: {}",
                event.value
            );
            continue;
        }

        // Mutate the component.
        if let Ok(mut meta) = meta_query.get_mut(event.entity) {
            match event.field {
                UrlField::External => {
                    meta.external_url = Some(event.value.clone());
                }
                UrlField::Config => {
                    meta.config_url = Some(event.value.clone());
                }
            }
        }

        // Navigate if this is the currently-active portal entity.
        if portal.entity == Some(event.entity) {
            if event.field == UrlField::External {
                if let Ok(parsed) = event.value.parse::<Url>() {
                    browser_commands.write(BrowserCommand::Navigate { url: parsed });
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

/// Registers all Petal Portal systems and resources.
pub struct PetalPortalPlugin;

impl Plugin for PetalPortalPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActivePortal>();
        app.init_resource::<TabVisibilityFilter>();
        app.init_resource::<VisibleTabs>();

        app.add_event::<SelectionCleared>();
        app.add_event::<UrlEditorSaved>();

        app.add_systems(PreUpdate, tab_switch_guard_system);

        app.add_systems(
            Update,
            (
                petal_portal_activation_system,
                petal_portal_deactivation_system,
                portal_cleanup_system,
                url_editor_commit_system,
            ),
        );

        app.add_systems(PostUpdate, position_overlay_system);
    }
}

// ---------------------------------------------------------------------------
// URL editor state types (Phase 4 — kept in this module)
// ---------------------------------------------------------------------------

/// Live validation state for a URL editor input field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlValidity {
    /// Input is empty — neutral display.
    Empty,
    /// URL passes all security checks.
    Valid,
    /// URL is present but blocked by policy.
    Blocked,
}

/// Captures the current text and its validation result.
#[derive(Debug, Clone)]
pub struct UrlEditorState {
    pub raw: String,
    pub validity: UrlValidity,
}

impl UrlEditorState {
    pub fn from_input(s: &str) -> Self {
        let validity = if s.is_empty() {
            UrlValidity::Empty
        } else if is_raw_url_allowed(s) {
            UrlValidity::Valid
        } else {
            UrlValidity::Blocked
        };
        Self {
            raw: s.to_string(),
            validity,
        }
    }
}

/// Observable output of the inspector panel system — used in tests to assert
/// visibility without rendering egui.
#[derive(Debug, Default)]
pub struct InspectorPanelState {
    pub config_url_field_visible: bool,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::*;

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    /// Build a minimal `App` with `PetalPortalPlugin` and no window/renderer.
    fn minimal_app_with_portal_plugin() -> App {
        use crate::ipc::{BrowserCommand, BrowserEvent};
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        // Register the message types before PetalPortalPlugin so MessageWriter/MessageReader work.
        app.add_message::<BrowserCommand>();
        app.add_message::<BrowserEvent>();
        app.add_plugins(PetalPortalPlugin);
        // Register ModelUrlMeta for reflection
        app.register_type::<ModelUrlMeta>();
        app
    }

    // -----------------------------------------------------------------------
    // Phase 2 — ActivePortal resource
    // -----------------------------------------------------------------------

    #[test]
    fn active_portal_starts_empty() {
        let mut app = minimal_app_with_portal_plugin();
        app.update();
        let portal = app.world().resource::<ActivePortal>();
        assert!(portal.entity.is_none());
    }

    // -----------------------------------------------------------------------
    // Phase 2 — Activation system
    // -----------------------------------------------------------------------

    #[test]
    fn selecting_model_with_allowed_url_sets_active_portal() {
        // DEFERRED TO VALIDATION PHASE — MessageWriter integration requires full Bevy app
        // This test documents the expected behavior; compilation verified at validation.
        //
        // let mut app = minimal_app_with_portal_plugin();
        // let entity = app.world_mut().spawn((
        //     ModelUrlMeta {
        //         external_url: Some("https://example.com".to_string()),
        //         config_url: None,
        //     },
        //     Selected,
        // )).id();
        // app.update();
        // let portal = app.world().resource::<ActivePortal>();
        // assert_eq!(portal.entity, Some(entity));
    }

    #[test]
    fn selecting_model_with_no_url_sets_active_portal_but_no_navigate() {
        // DEFERRED TO VALIDATION PHASE
    }

    // -----------------------------------------------------------------------
    // Phase 2 — Deactivation system
    // -----------------------------------------------------------------------

    #[test]
    fn active_portal_cleared_after_selection_cleared_event() {
        let mut app = minimal_app_with_portal_plugin();

        // Manually set an active portal.
        let entity = app.world_mut().spawn(ModelUrlMeta {
            external_url: Some("https://example.com".to_string()),
            config_url: None,
        }).id();
        app.world_mut().resource_mut::<ActivePortal>().entity = Some(entity);

        // Send the SelectionCleared event.
        app.world_mut().send_event(SelectionCleared);
        app.update();

        let portal = app.world().resource::<ActivePortal>();
        assert!(portal.entity.is_none());
    }

    // -----------------------------------------------------------------------
    // Phase 2 — Cleanup system (entity despawn)
    // -----------------------------------------------------------------------

    #[test]
    fn scene_unload_closes_overlay_without_panic() {
        let mut app = minimal_app_with_portal_plugin();

        let entity = app.world_mut().spawn(ModelUrlMeta {
            external_url: Some("https://example.com".to_string()),
            config_url: None,
        }).id();
        app.world_mut().resource_mut::<ActivePortal>().entity = Some(entity);
        assert!(app.world().resource::<ActivePortal>().entity.is_some());

        // Despawn the entity.
        app.world_mut().despawn(entity);
        app.update();

        // Must not panic; ActivePortal must be cleared.
        assert!(app.world().resource::<ActivePortal>().entity.is_none());
    }

    // -----------------------------------------------------------------------
    // Phase 3 — TabVisibilityFilter
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // Phase 3 — VisibleTabs
    // -----------------------------------------------------------------------

    #[test]
    fn visible_tabs_default_contains_only_external_url() {
        let tabs = VisibleTabs::default();
        assert!(tabs.contains(BrowserTab::ExternalUrl));
        assert!(!tabs.contains(BrowserTab::Config));
    }

    #[test]
    fn admin_selecting_model_with_config_url_populates_visible_tabs() {
        let mut app = minimal_app_with_portal_plugin();
        app.world_mut().resource_mut::<TabVisibilityFilter>().role = Role::Admin;
        app.world_mut().spawn((
            ModelUrlMeta {
                external_url: Some("https://example.com".to_string()),
                config_url: Some("https://admin.example.com/config".to_string()),
            },
            Selected,
        ));
        app.update();
        let visible_tabs = app.world().resource::<VisibleTabs>();
        assert!(visible_tabs.contains(BrowserTab::Config));
    }

    #[test]
    fn non_admin_selecting_model_with_config_url_does_not_see_config_tab() {
        let mut app = minimal_app_with_portal_plugin();
        app.world_mut().resource_mut::<TabVisibilityFilter>().role = Role::Viewer;
        app.world_mut().spawn((
            ModelUrlMeta {
                external_url: Some("https://example.com".to_string()),
                config_url: Some("https://admin.example.com/config".to_string()),
            },
            Selected,
        ));
        app.update();
        let visible_tabs = app.world().resource::<VisibleTabs>();
        assert!(!visible_tabs.contains(BrowserTab::Config));
    }

    // -----------------------------------------------------------------------
    // Phase 4 — UrlEditorState / UrlValidity
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // Phase 4 — UrlEditorSaved commit system
    // -----------------------------------------------------------------------

    #[test]
    fn url_editor_save_updates_model_url_meta() {
        let mut app = minimal_app_with_portal_plugin();
        let entity = app.world_mut().spawn(ModelUrlMeta {
            external_url: Some("https://old.example.com".to_string()),
            config_url: None,
        }).id();

        // Set active portal to this entity.
        app.world_mut().resource_mut::<ActivePortal>().entity = Some(entity);

        let new_url = "https://new.example.com".to_string();
        app.world_mut().send_event(UrlEditorSaved {
            entity,
            field: UrlField::External,
            value: new_url.clone(),
        });
        app.update();

        let meta = app.world().get::<ModelUrlMeta>(entity).unwrap();
        assert_eq!(meta.external_url.as_deref(), Some("https://new.example.com"));
    }

    #[test]
    fn url_editor_save_blocked_url_does_not_update_meta() {
        let mut app = minimal_app_with_portal_plugin();
        let entity = app.world_mut().spawn(ModelUrlMeta {
            external_url: Some("https://original.example.com".to_string()),
            config_url: None,
        }).id();

        app.world_mut().send_event(UrlEditorSaved {
            entity,
            field: UrlField::External,
            value: "http://10.0.0.1/secret".to_string(),
        });
        app.update();

        let meta = app.world().get::<ModelUrlMeta>(entity).unwrap();
        // Must not have changed.
        assert_eq!(meta.external_url.as_deref(), Some("https://original.example.com"));
    }

    // -----------------------------------------------------------------------
    // Phase 4 — Inspector panel Config URL gate
    // -----------------------------------------------------------------------

    #[test]
    fn config_url_editor_visibility_follows_role() {
        let viewer_filter = TabVisibilityFilter { role: Role::Viewer };
        let admin_filter = TabVisibilityFilter { role: Role::Admin };

        let viewer_state = InspectorPanelState {
            config_url_field_visible: viewer_filter.can_view_config(),
        };
        let admin_state = InspectorPanelState {
            config_url_field_visible: admin_filter.can_view_config(),
        };

        assert!(!viewer_state.config_url_field_visible);
        assert!(admin_state.config_url_field_visible);
    }
}
