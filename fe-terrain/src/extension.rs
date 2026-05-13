//! First-party terrain extension metadata and UI registration.
//!
//! Provides [`TerrainExtensionInfo`] with extension metadata and
//! [`register_terrain_ui`] to populate the SDK's [`UiExtensionRegistry`]
//! with terrain-specific UI contributions.

use fe_sdk::ui::{UiContribution, UiExtensionRegistry, UiSlot};

/// Extension identifier for the built-in terrain extension.
pub const TERRAIN_EXTENSION_ID: &str = "fractalengine:terrain";

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
    // Inspector section: terrain configuration panel
    registry.register(UiContribution {
        slot: UiSlot::InspectorSection,
        plugin_id: TERRAIN_EXTENSION_ID.to_string(),
        label: "Terrain Config".to_string(),
        icon: Some("terrain".to_string()),
    });

    // Sidebar tool: GPX import
    registry.register(UiContribution {
        slot: UiSlot::SidebarTool,
        plugin_id: TERRAIN_EXTENSION_ID.to_string(),
        label: "GPX Import".to_string(),
        icon: Some("gpx".to_string()),
    });

    // Toolbar action: toggle terrain visibility
    registry.register(UiContribution {
        slot: UiSlot::ToolbarAction,
        plugin_id: TERRAIN_EXTENSION_ID.to_string(),
        label: "Toggle Terrain".to_string(),
        icon: Some("mountain".to_string()),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terrain_extension_info_metadata() {
        assert_eq!(TerrainExtensionInfo::id(), "fractalengine:terrain");
        assert_eq!(TerrainExtensionInfo::name(), "Terrain & GPX");
        assert_eq!(TerrainExtensionInfo::tier(), "Native");
    }

    #[test]
    fn register_terrain_ui_adds_three_contributions() {
        let mut registry = UiExtensionRegistry::new();
        assert_eq!(registry.total(), 0);

        register_terrain_ui(&mut registry);

        assert_eq!(registry.total(), 3);

        // Verify inspector section
        let inspectors = registry.get_by_slot(UiSlot::InspectorSection);
        assert_eq!(inspectors.len(), 1);
        assert_eq!(inspectors[0].label, "Terrain Config");
        assert_eq!(inspectors[0].plugin_id, TERRAIN_EXTENSION_ID);

        // Verify sidebar tool
        let sidebar = registry.get_by_slot(UiSlot::SidebarTool);
        assert_eq!(sidebar.len(), 1);
        assert_eq!(sidebar[0].label, "GPX Import");
        assert_eq!(sidebar[0].plugin_id, TERRAIN_EXTENSION_ID);

        // Verify toolbar action
        let toolbar = registry.get_by_slot(UiSlot::ToolbarAction);
        assert_eq!(toolbar.len(), 1);
        assert_eq!(toolbar[0].label, "Toggle Terrain");
        assert_eq!(toolbar[0].plugin_id, TERRAIN_EXTENSION_ID);
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
