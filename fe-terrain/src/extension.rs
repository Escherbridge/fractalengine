//! First-party terrain extension metadata and UI registration.
//!
//! Provides [`TerrainExtensionInfo`] with extension metadata and
//! [`register_terrain_ui`] to populate the SDK's [`UiExtensionRegistry`]
//! with terrain-specific UI contributions.

use fe_sdk::manifest_v2::ExtensionManifest;
use fe_sdk::ui::{UiContribution, UiExtensionRegistry, UiWhen};

/// Package id of the built-in terrain extension (reverse-DNS, a valid v2 package id).
pub const TERRAIN_EXTENSION_ID: &str = "org.fractalengine.terrain";

/// Manifest v2 of the terrain extension. Its UI entries bind to host commands, not to
/// hard-wired actions. Hook provision (`petal.map`) is specified in docs/extensions/bindings.md.
pub const TERRAIN_MANIFEST_V2_JSON: &str = r#"{
  "manifest_version": 2,
  "id": "org.fractalengine.terrain",
  "version": "1.0.0",
  "name": "Terrain & GPX",
  "description": "First-party terrain: the petal map, GPX import and the terrain toggle. Built in, native.",
  "kind": "general",
  "contract": "fractalengine:plugin@2.0.0",
  "binding": "petal",
  "scene": { "read": true },
  "bus": {
    "expose": {
      "commands": [
        { "id": "terrain-config", "description": "Reveal the terrain section of the inspector.", "role_floor": "editor", "rate_hz": 1 },
        { "id": "gpx-import", "description": "Import a GPX track into the petal.", "role_floor": "editor", "rate_hz": 1 },
        { "id": "terrain-toggle", "description": "Show or hide the petal terrain.", "role_floor": "viewer", "rate_hz": 1 }
      ]
    }
  },
  "ui": [
    { "id": "terrain-config", "slot": "InspectorSection", "label": "Terrain Config", "icon": "terrain", "command": "terrain-config" },
    { "id": "gpx-import", "slot": "SidebarTool", "label": "GPX Import", "icon": "gpx", "command": "gpx-import" },
    { "id": "terrain-toggle", "slot": "ToolbarAction", "label": "Toggle Terrain", "icon": "mountain", "command": "terrain-toggle" }
  ]
}"#;

/// Parse [`TERRAIN_MANIFEST_V2_JSON`]. The unit tests run the full v2 validator over it.
pub fn terrain_manifest() -> ExtensionManifest {
    serde_json::from_str(TERRAIN_MANIFEST_V2_JSON).expect("TERRAIN_MANIFEST_V2_JSON is a valid v2 manifest")
}

/// Extension display name.
pub const TERRAIN_EXTENSION_NAME: &str = "Terrain & GPX";

/// Metadata descriptor for the terrain first-party extension.
///
/// This mirrors the information a third-party plugin would declare in its
/// manifest, but for the built-in terrain subsystem.
pub struct TerrainExtensionInfo;

impl TerrainExtensionInfo {
    /// The unique extension identifier (URI-style).
    pub fn id() -> &'static str {
        TERRAIN_EXTENSION_ID
    }

    /// Human-readable display name.
    pub fn name() -> &'static str {
        TERRAIN_EXTENSION_NAME
    }

    /// The execution tier. First-party extensions are always `Native`.
    pub fn tier() -> &'static str {
        "Native"
    }
}

/// Register the terrain extension's UI contributions into the given registry.
///
/// Adds three contributions:
/// 1. **Terrain Config** inspector section for terrain settings.
/// 2. **GPX Import** sidebar tool for importing GPX files.
/// 3. **Toggle Terrain** toolbar action for enabling/disabling terrain rendering.
pub fn register_terrain_ui(registry: &mut UiExtensionRegistry) {
    // One contribution per manifest `ui` entry; each carries its bound command id.
    let manifest = terrain_manifest();
    for entry in &manifest.ui {
        registry.register(UiContribution {
            slot: entry.slot,
            plugin_id: manifest.id.clone(),
            label: entry.label.clone(),
            icon: entry.icon.clone(),
            id: entry.id.clone(),
            command: entry.command.clone(),
            properties: entry.properties.clone().unwrap_or_default(),
            stream: entry.stream.clone(),
            field: entry.field.clone(),
            when: entry.when.as_ref().map(|spec| UiWhen { property: spec.property.clone(), equals: spec.equals.clone() }),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_sdk::manifest_v2::parse_and_validate;
    use fe_sdk::ui::UiSlot;

    #[test]
    fn terrain_extension_info_metadata() {
        assert_eq!(TerrainExtensionInfo::id(), "org.fractalengine.terrain");
        assert_eq!(TerrainExtensionInfo::name(), "Terrain & GPX");
        assert_eq!(TerrainExtensionInfo::tier(), "Native");
    }

    #[test]
    fn register_terrain_ui_adds_three_contributions() {
        let mut registry = UiExtensionRegistry::new();
        assert_eq!(registry.total(), 0);

        register_terrain_ui(&mut registry);

        assert_eq!(registry.total(), 3);

        // The manifest the contributions come from must pass the v2 validator (V1..V15).
        let validated = parse_and_validate(TERRAIN_MANIFEST_V2_JSON.as_bytes()).expect("terrain manifest is valid v2");
        assert_eq!(validated.manifest.id, TERRAIN_EXTENSION_ID);

        // Each contribution is bound to the command its manifest entry names.
        let inspectors = registry.get_by_slot(UiSlot::InspectorSection);
        assert_eq!(inspectors.len(), 1);
        assert_eq!(inspectors[0].label, "Terrain Config");
        assert_eq!(inspectors[0].plugin_id, TERRAIN_EXTENSION_ID);
        assert_eq!(inspectors[0].command.as_deref(), Some("terrain-config"));

        let sidebar = registry.get_by_slot(UiSlot::SidebarTool);
        assert_eq!(sidebar.len(), 1);
        assert_eq!(sidebar[0].label, "GPX Import");
        assert_eq!(sidebar[0].plugin_id, TERRAIN_EXTENSION_ID);
        assert_eq!(sidebar[0].command.as_deref(), Some("gpx-import"));

        let toolbar = registry.get_by_slot(UiSlot::ToolbarAction);
        assert_eq!(toolbar.len(), 1);
        assert_eq!(toolbar[0].label, "Toggle Terrain");
        assert_eq!(toolbar[0].plugin_id, TERRAIN_EXTENSION_ID);
        assert_eq!(toolbar[0].command.as_deref(), Some("terrain-toggle"));
    }

    #[test]
    fn unregister_terrain_ui_removes_all() {
        let mut registry = UiExtensionRegistry::new();
        register_terrain_ui(&mut registry);
        assert_eq!(registry.total(), 3);

        registry.unregister_all(TERRAIN_EXTENSION_ID);
        assert_eq!(registry.total(), 0);
    }
}
