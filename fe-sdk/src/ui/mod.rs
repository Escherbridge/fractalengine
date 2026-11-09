//! UI slot system for extension-contributed UI elements.
//!
//! Extensions register [`UiContribution`]s into named [`UiSlot`]s. The engine's
//! UI layer queries the [`UiExtensionRegistry`] each frame to render plugin UI
//! in the appropriate locations.

use serde::{Deserialize, Serialize};

/// A named slot in the engine UI where extensions can contribute elements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum UiSlot {
    /// A collapsible section in the node inspector panel.
    #[default]
    InspectorSection,
    /// A tool entry in the left sidebar.
    SidebarTool,
    /// An overlay rendered on top of the 3D viewport.
    ViewportOverlay,
    /// A button in the top toolbar.
    ToolbarAction,
    /// An entry in the right-click context menu.
    ContextMenu,
    /// A widget on the dashboard/home screen.
    DashboardWidget,
}

/// Context-menu filter on the clicked node: a property present, or equal to a value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiWhen {
    /// Property key that must be present.
    pub property: String,
    /// Required value; presence alone when absent.
    #[serde(default)]
    pub equals: Option<serde_json::Value>,
}

/// A single UI element contributed by a plugin.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UiContribution {
    /// Which slot this contribution targets.
    pub slot: UiSlot,
    /// The plugin that registered this contribution.
    pub plugin_id: String,
    /// Display label for the UI element.
    pub label: String,
    /// Optional icon identifier (e.g. an icon name or emoji).
    pub icon: Option<String>,
    /// Manifest-local contribution id; the accessible name is `ext:<plugin_id>/<id>` (empty for built-ins).
    #[serde(default)]
    pub id: String,
    /// Toolbar, sidebar, context menu: an exposed command id of the package.
    #[serde(default)]
    pub command: Option<String>,
    /// Inspector section: property keys shown as read-only rows.
    #[serde(default)]
    pub properties: Vec<String>,
    /// Dashboard widget, viewport overlay: an exposed stream id of the package.
    #[serde(default)]
    pub stream: Option<String>,
    /// JSON Pointer into the stream event whose latest value is shown.
    #[serde(default)]
    pub field: Option<String>,
    /// Context menu: filter on the clicked node.
    #[serde(default)]
    pub when: Option<UiWhen>,
}

impl UiContribution {
    /// Accessible name `ext:<plugin_id>/<id>`; empty when the contribution has no manifest id.
    pub fn accessible_name(&self) -> String {
        if self.id.is_empty() {
            String::new()
        } else {
            format!("ext:{}/{}", self.plugin_id, self.id)
        }
    }

    /// True when it carries an action, rows or a stream, so a slot may render it as a live control.
    pub fn is_renderable(&self) -> bool {
        self.command.is_some() || !self.properties.is_empty() || self.stream.is_some()
    }
}

/// Registry of all plugin-contributed UI elements.
///
/// The engine UI queries this registry each frame to render extension UI.
#[derive(Debug, Clone, Default)]
pub struct UiExtensionRegistry {
    /// Inspector panel sections contributed by plugins.
    pub inspector_panels: Vec<UiContribution>,
    /// Sidebar tool entries contributed by plugins.
    pub sidebar_tools: Vec<UiContribution>,
    /// Toolbar action buttons contributed by plugins.
    pub toolbar_actions: Vec<UiContribution>,
    /// Viewport overlays contributed by plugins.
    pub overlays: Vec<UiContribution>,
    /// Context menu items contributed by plugins.
    pub context_items: Vec<UiContribution>,
    /// Dashboard widgets contributed by plugins.
    pub dashboard_widgets: Vec<UiContribution>,
}

impl UiExtensionRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a UI contribution into the appropriate slot.
    pub fn register(&mut self, contribution: UiContribution) {
        match contribution.slot {
            UiSlot::InspectorSection => self.inspector_panels.push(contribution),
            UiSlot::SidebarTool => self.sidebar_tools.push(contribution),
            UiSlot::ToolbarAction => self.toolbar_actions.push(contribution),
            UiSlot::ViewportOverlay => self.overlays.push(contribution),
            UiSlot::ContextMenu => self.context_items.push(contribution),
            UiSlot::DashboardWidget => self.dashboard_widgets.push(contribution),
        }
    }

    /// Remove all contributions from a specific plugin.
    pub fn unregister_all(&mut self, plugin_id: &str) {
        self.inspector_panels.retain(|c| c.plugin_id != plugin_id);
        self.sidebar_tools.retain(|c| c.plugin_id != plugin_id);
        self.toolbar_actions.retain(|c| c.plugin_id != plugin_id);
        self.overlays.retain(|c| c.plugin_id != plugin_id);
        self.context_items.retain(|c| c.plugin_id != plugin_id);
        self.dashboard_widgets.retain(|c| c.plugin_id != plugin_id);
    }

    /// Get all contributions for a specific slot.
    pub fn get_by_slot(&self, slot: UiSlot) -> Vec<&UiContribution> {
        match slot {
            UiSlot::InspectorSection => self.inspector_panels.iter().collect(),
            UiSlot::SidebarTool => self.sidebar_tools.iter().collect(),
            UiSlot::ToolbarAction => self.toolbar_actions.iter().collect(),
            UiSlot::ViewportOverlay => self.overlays.iter().collect(),
            UiSlot::ContextMenu => self.context_items.iter().collect(),
            UiSlot::DashboardWidget => self.dashboard_widgets.iter().collect(),
        }
    }

    /// Total number of registered contributions across all slots.
    pub fn total(&self) -> usize {
        self.inspector_panels.len()
            + self.sidebar_tools.len()
            + self.toolbar_actions.len()
            + self.overlays.len()
            + self.context_items.len()
            + self.dashboard_widgets.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_get_by_slot() {
        let mut reg = UiExtensionRegistry::new();

        reg.register(UiContribution {
            slot: UiSlot::ToolbarAction,
            plugin_id: "plugin-a".into(),
            label: "Export".into(),
            icon: Some("download".into()),
            id: "export".into(),
            command: Some("export".into()),
            ..Default::default()
        });
        reg.register(UiContribution {
            slot: UiSlot::InspectorSection,
            plugin_id: "plugin-a".into(),
            label: "Physics".into(),
            properties: vec!["physics.mass".into()],
            ..Default::default()
        });
        reg.register(UiContribution {
            slot: UiSlot::ToolbarAction,
            plugin_id: "plugin-b".into(),
            label: "Share".into(),
            icon: Some("share".into()),
            ..Default::default()
        });

        assert_eq!(reg.total(), 3);
        assert_eq!(reg.get_by_slot(UiSlot::ToolbarAction).len(), 2);
        assert_eq!(reg.get_by_slot(UiSlot::InspectorSection).len(), 1);
        assert_eq!(reg.get_by_slot(UiSlot::SidebarTool).len(), 0);

        let toolbar = reg.get_by_slot(UiSlot::ToolbarAction);
        assert_eq!(toolbar[0].accessible_name(), "ext:plugin-a/export");
        assert!(toolbar[0].is_renderable());
        assert!(!toolbar[1].is_renderable(), "no command, rows or stream: details pane only");
    }

    #[test]
    fn unregister_all_removes_only_target_plugin() {
        let mut reg = UiExtensionRegistry::new();

        reg.register(UiContribution {
            slot: UiSlot::ToolbarAction,
            plugin_id: "plugin-a".into(),
            label: "Export".into(),
            ..Default::default()
        });
        reg.register(UiContribution {
            slot: UiSlot::SidebarTool,
            plugin_id: "plugin-a".into(),
            label: "Brush".into(),
            ..Default::default()
        });
        reg.register(UiContribution {
            slot: UiSlot::ToolbarAction,
            plugin_id: "plugin-b".into(),
            label: "Share".into(),
            ..Default::default()
        });

        assert_eq!(reg.total(), 3);

        reg.unregister_all("plugin-a");

        assert_eq!(reg.total(), 1);
        assert_eq!(reg.toolbar_actions.len(), 1);
        assert_eq!(reg.toolbar_actions[0].plugin_id, "plugin-b");
        assert!(reg.sidebar_tools.is_empty());
    }
}
