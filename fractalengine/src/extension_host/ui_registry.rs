//! Feeds a build's manifest `ui` entries into the slot registry and the `ui.panel` content set (R-UI-03).
//! Pinned seam for I-BIN: [`activate_build`] at activation or update, [`deactivate_package`] at disable, revoke or
//! uninstall; both take the three fe-ui resources by `&mut`. See `src/AGENTS.md` §extension-host.

use fe_sdk::manifest_v2::{ExtensionManifest, UiEntry};
use fe_sdk::{UiContribution, UiExtensionRegistry, UiWhen};
use fe_ui::extensions::{BoundSlotsState, ExtensionSlotState};

/// The binding hook whose `uses` name the ui entries that render as panel content.
const PANEL_HOOK: &str = "ui.panel";

/// One manifest `ui` entry as the registry stores it, attributed to `package_id`.
pub fn contribution_of(package_id: &str, entry: &UiEntry) -> UiContribution {
    UiContribution {
        slot: entry.slot,
        plugin_id: package_id.to_owned(),
        label: entry.label.clone(),
        icon: entry.icon.clone(),
        id: entry.id.clone(),
        command: entry.command.clone(),
        properties: entry.properties.clone().unwrap_or_default(),
        stream: entry.stream.clone(),
        field: entry.field.clone(),
        when: entry.when.as_ref().map(|when| UiWhen { property: when.property.clone(), equals: when.equals.clone() }),
    }
}

/// Every `ui` entry of the manifest as contributions, in declaration order.
pub fn contributions_of(manifest: &ExtensionManifest) -> Vec<UiContribution> {
    manifest.ui.iter().map(|entry| contribution_of(&manifest.id, entry)).collect()
}

/// The ui ids named by the manifest's `ui.panel` provides entries (de-duplicated, declaration order).
pub fn panel_uses_of(manifest: &ExtensionManifest) -> Vec<String> {
    let mut uses: Vec<String> = Vec::new();
    for provided in manifest.provides.iter().filter(|provided| provided.hook == PANEL_HOOK) {
        for id in provided.uses.iter().flatten() {
            if !uses.contains(id) {
                uses.push(id.clone());
            }
        }
    }
    uses
}

/// Register a build's ui entries and panel content set, replacing what an earlier build of the package held.
pub fn activate_build(
    registry: &mut UiExtensionRegistry,
    bound: &mut BoundSlotsState,
    manifest: &ExtensionManifest,
) {
    registry.unregister_all(&manifest.id);
    for contribution in contributions_of(manifest) {
        registry.register(contribution);
    }
    bound.set_panel_uses(&manifest.id, panel_uses_of(manifest));
}

/// Forget everything the UI holds for a package: contributions, panel content, bound rows, streams, queued commands.
pub fn deactivate_package(
    registry: &mut UiExtensionRegistry,
    bound: &mut BoundSlotsState,
    slots: &mut ExtensionSlotState,
    package_id: &str,
) {
    registry.unregister_all(package_id);
    bound.forget_package(package_id);
    slots.forget_plugin(package_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_sdk::UiSlot;
    use serde_json::json;

    fn manifest(command_label: &str) -> ExtensionManifest {
        serde_json::from_value(json!({
            "manifest_version": 2,
            "id": "org.example.fleet",
            "version": "1.0.0",
            "name": "Fleet",
            "kind": "simulation",
            "contract": "fractalengine:plugin@2.0.0",
            "binding": "petal",
            "ui": [
                { "id": "start", "slot": "ToolbarAction", "label": command_label, "command": "start" },
                { "id": "speed", "slot": "DashboardWidget", "label": "Speed", "stream": "telemetry", "field": "/speed" },
                { "id": "track", "slot": "ContextMenu", "label": "Track", "command": "track",
                  "when": { "property": "kind", "equals": "vehicle" } }
            ],
            "provides": [ { "hook": "ui.panel", "uses": ["speed", "start"] } ]
        }))
        .expect("test manifest parses")
    }

    /// Journey: activate, update replaces, panel content set, disable forgets everything.
    #[test]
    fn ui_registry_journey_activate_update_deactivate() {
        let mut registry = UiExtensionRegistry::new();
        let mut bound = BoundSlotsState::default();
        let mut slots = ExtensionSlotState::default();

        activate_build(&mut registry, &mut bound, &manifest("Start"));
        assert_eq!(registry.total(), 3);
        let toolbar = registry.get_by_slot(UiSlot::ToolbarAction);
        assert_eq!(toolbar[0].accessible_name(), "ext:org.example.fleet/start");
        let menu = registry.get_by_slot(UiSlot::ContextMenu);
        assert_eq!(menu[0].when.as_ref().map(|when| when.property.as_str()), Some("kind"));
        assert!(bound.is_panel_content(registry.get_by_slot(UiSlot::DashboardWidget)[0]));
        assert!(!bound.is_panel_content(menu[0]), "only the `uses` ids are panel content");

        // An update replaces the old build's entries instead of stacking duplicates.
        activate_build(&mut registry, &mut bound, &manifest("Launch"));
        assert_eq!(registry.total(), 3);
        assert_eq!(registry.get_by_slot(UiSlot::ToolbarAction)[0].label, "Launch");

        slots.set_stream_value("org.example.fleet", "telemetry", json!({ "speed": 3 }));
        deactivate_package(&mut registry, &mut bound, &mut slots, "org.example.fleet");
        assert_eq!(registry.total(), 0);
        assert!(slots.stream_values.is_empty());
        let speed = UiContribution { plugin_id: "org.example.fleet".into(), id: "speed".into(), ..Default::default() };
        assert!(!bound.is_panel_content(&speed), "panel content is forgotten with the package");
    }
}
