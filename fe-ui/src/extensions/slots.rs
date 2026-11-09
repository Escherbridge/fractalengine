//! Declarative slot rendering: command buttons, property rows, stream values; the guest draws nothing.

use std::collections::HashMap;

use bevy_egui::egui;
use fe_sdk::{UiContribution, UiWhen};
use serde_json::Value;

use crate::accessibility::ResponseExt;

/// Longest rendered stream/property value before an ellipsis.
const MAX_VALUE_CHARS: usize = 64;

/// Placeholder shown for an absent value.
const NO_VALUE: &str = "\u{2014}";

/// A command button the user pressed; the binary drains these and dispatches to the package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionCommandRequest {
    pub plugin_id: String,
    pub contribution_id: String,
    pub command: String,
}

/// Slot-host state: latest stream values in, pressed commands out.
#[derive(bevy::prelude::Resource, Debug, Default)]
pub struct ExtensionSlotState {
    /// Latest event per `(plugin_id, stream_id)`, fed by the binary from the integration bus.
    pub stream_values: HashMap<(String, String), Value>,
    /// Commands pressed since the last drain.
    pub pending_commands: Vec<ExtensionCommandRequest>,
}

impl ExtensionSlotState {
    /// Replace the latest event of a stream.
    pub fn set_stream_value(&mut self, plugin_id: &str, stream: &str, value: Value) {
        self.stream_values
            .insert((plugin_id.to_owned(), stream.to_owned()), value);
    }

    /// Take the commands pressed since the last call.
    pub fn take_commands(&mut self) -> Vec<ExtensionCommandRequest> {
        std::mem::take(&mut self.pending_commands)
    }

    /// Drop everything held for a plugin (installation disabled or removed).
    pub fn forget_plugin(&mut self, plugin_id: &str) {
        self.stream_values
            .retain(|(plugin, _), _| plugin != plugin_id);
        self.pending_commands
            .retain(|request| request.plugin_id != plugin_id);
    }

    /// Latest value for a contribution's stream, narrowed by its `field` JSON Pointer.
    pub fn stream_value(&self, contribution: &UiContribution) -> Option<&Value> {
        let stream = contribution.stream.as_ref()?;
        let event = self
            .stream_values
            .get(&(contribution.plugin_id.clone(), stream.clone()))?;
        match contribution.field.as_deref() {
            None | Some("") => Some(event),
            Some(pointer) => event.pointer(pointer),
        }
    }
}

/// Read access to the clicked or selected node's properties, by key.
pub trait SlotContext {
    fn property(&self, key: &str) -> Option<Value>;
}

impl<F: Fn(&str) -> Option<Value>> SlotContext for F {
    fn property(&self, key: &str) -> Option<Value> {
        self(key)
    }
}

/// Contributions with no command, rows or stream: listed in the Extensions details pane, never drawn as dead controls.
pub fn details_only<'a>(contributions: &[&'a UiContribution]) -> Vec<&'a UiContribution> {
    contributions
        .iter()
        .copied()
        .filter(|c| !c.is_renderable())
        .collect()
}

/// True when a context-menu entry's `when` filter admits the clicked node.
pub fn when_matches(when: Option<&UiWhen>, node: &dyn SlotContext) -> bool {
    let Some(when) = when else { return true };
    match (node.property(&when.property), &when.equals) {
        (None, _) => false,
        (Some(_), None) => true,
        (Some(actual), Some(expected)) => &actual == expected,
    }
}

/// Compact display text for a JSON value.
pub fn format_value(value: &Value) -> String {
    let text = match value {
        Value::String(s) => s.clone(),
        Value::Null => NO_VALUE.to_owned(),
        other => other.to_string(),
    };
    if text.chars().count() > MAX_VALUE_CHARS {
        let cut: String = text.chars().take(MAX_VALUE_CHARS - 1).collect();
        format!("{cut}\u{2026}")
    } else {
        text
    }
}

fn press(state: &mut ExtensionSlotState, contribution: &UiContribution, command: &str) {
    state.pending_commands.push(ExtensionCommandRequest {
        plugin_id: contribution.plugin_id.clone(),
        contribution_id: contribution.id.clone(),
        command: command.to_owned(),
    });
}

/// ToolbarAction and SidebarTool: one button per commanded contribution, named `ext:<package>/<id>`.
pub fn draw_command_slot(
    ui: &mut egui::Ui,
    contributions: &[&UiContribution],
    state: &mut ExtensionSlotState,
) {
    for contribution in contributions {
        let Some(command) = contribution.command.as_deref() else {
            continue;
        };
        let name = contribution.accessible_name();
        if name.is_empty() {
            continue;
        }
        let response = ui.button(&contribution.label).button_named(ui, &name);
        if response.clicked() {
            press(state, contribution, command);
        }
    }
}

/// ContextMenu: commanded entries whose `when` admits the clicked node; closes the menu on press.
pub fn draw_context_menu_items(
    ui: &mut egui::Ui,
    contributions: &[&UiContribution],
    node: &dyn SlotContext,
    state: &mut ExtensionSlotState,
) {
    for contribution in contributions {
        let Some(command) = contribution.command.as_deref() else {
            continue;
        };
        let name = contribution.accessible_name();
        if name.is_empty() || !when_matches(contribution.when.as_ref(), node) {
            continue;
        }
        let response = ui.button(&contribution.label).button_named(ui, &name);
        if response.clicked() {
            press(state, contribution, command);
            ui.close();
        }
    }
}

/// InspectorSection: a collapsing section of read-only key/value rows for the selected node.
pub fn draw_inspector_rows(
    ui: &mut egui::Ui,
    contributions: &[&UiContribution],
    node: &dyn SlotContext,
) {
    for contribution in contributions.iter().filter(|c| !c.properties.is_empty()) {
        let name = contribution.accessible_name();
        let section = egui::CollapsingHeader::new(&contribution.label)
            .id_salt(("ext-section", &contribution.plugin_id, &contribution.id))
            .default_open(true)
            .show(ui, |ui| {
                for key in &contribution.properties {
                    ui.horizontal(|ui| {
                        ui.label(key);
                        let text = node
                            .property(key)
                            .map(|v| format_value(&v))
                            .unwrap_or_else(|| NO_VALUE.to_owned());
                        ui.label(text);
                    });
                }
            });
        if !name.is_empty() {
            section.header_response.named(ui, name);
        }
    }
}

/// DashboardWidget and ViewportOverlay: label plus the latest stream value (a dash until one arrives).
pub fn draw_stream_slot(
    ui: &mut egui::Ui,
    contributions: &[&UiContribution],
    state: &ExtensionSlotState,
) {
    for contribution in contributions.iter().filter(|c| c.stream.is_some()) {
        let name = contribution.accessible_name();
        let text = state
            .stream_value(contribution)
            .map(format_value)
            .unwrap_or_else(|| NO_VALUE.to_owned());
        let response = ui
            .horizontal(|ui| {
                ui.label(&contribution.label);
                ui.strong(text);
            })
            .response;
        if !name.is_empty() {
            response.named(ui, name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_sdk::{UiExtensionRegistry, UiSlot};
    use serde_json::json;

    /// Slot journey: register, filter, press, receive a stream value, disable.
    #[test]
    fn slot_journey_declared_contributions_to_pressed_commands() {
        let mut registry = UiExtensionRegistry::new();
        registry.register(UiContribution {
            slot: UiSlot::ToolbarAction,
            plugin_id: "fleet".into(),
            label: "Start fleet".into(),
            id: "start".into(),
            command: Some("start".into()),
            ..Default::default()
        });
        registry.register(UiContribution {
            slot: UiSlot::ContextMenu,
            plugin_id: "fleet".into(),
            label: "Track".into(),
            id: "track".into(),
            command: Some("track".into()),
            when: Some(UiWhen {
                property: "kind".into(),
                equals: Some(json!("vehicle")),
            }),
            ..Default::default()
        });
        registry.register(UiContribution {
            slot: UiSlot::DashboardWidget,
            plugin_id: "fleet".into(),
            label: "Speed".into(),
            id: "speed".into(),
            stream: Some("telemetry".into()),
            field: Some("/speed".into()),
            ..Default::default()
        });
        registry.register(UiContribution {
            slot: UiSlot::ToolbarAction,
            plugin_id: "terrain".into(),
            label: "Label only".into(),
            ..Default::default()
        });

        let ctx = egui::Context::default();
        let mut state = ExtensionSlotState::default();
        let toolbar = registry.get_by_slot(UiSlot::ToolbarAction);
        assert_eq!(
            details_only(&toolbar).len(),
            1,
            "commandless contribution goes to the details pane"
        );

        // A frame with no click queues nothing; a press is queued the way a click would.
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                draw_command_slot(ui, &toolbar, &mut state);
                draw_stream_slot(ui, &registry.get_by_slot(UiSlot::DashboardWidget), &state);
            });
        });
        assert!(state.pending_commands.is_empty());
        press(&mut state, toolbar[0], "start");
        assert_eq!(toolbar[0].accessible_name(), "ext:fleet/start");
        assert_eq!(state.take_commands()[0].command, "start");

        let vehicle = |key: &str| (key == "kind").then(|| json!("vehicle"));
        let building = |key: &str| (key == "kind").then(|| json!("building"));
        let menu = registry.get_by_slot(UiSlot::ContextMenu);
        assert!(when_matches(menu[0].when.as_ref(), &vehicle));
        assert!(!when_matches(menu[0].when.as_ref(), &building));

        let widget = registry.get_by_slot(UiSlot::DashboardWidget)[0].clone();
        assert!(state.stream_value(&widget).is_none());
        state.set_stream_value("fleet", "telemetry", json!({ "speed": 12.5 }));
        assert_eq!(format_value(state.stream_value(&widget).unwrap()), "12.5");

        registry.unregister_all("fleet");
        state.forget_plugin("fleet");
        assert!(
            registry
                .get_by_slot(UiSlot::ToolbarAction)
                .iter()
                .all(|c| c.plugin_id != "fleet")
        );
        assert!(state.stream_value(&widget).is_none());
    }
}
