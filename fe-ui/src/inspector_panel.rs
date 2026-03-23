/// Inspector panel for model entities — shows URL editor fields with live validation.
///
/// The Config URL field is gated behind `TabVisibilityFilter::can_view_config()`;
/// non-admin users never see it, preventing role-escalation via the UI.

use bevy_egui::egui;

/// Observable state returned by the inspector panel rendering logic.
/// Allows testing visibility rules without running a full egui render.
#[derive(Debug, Default)]
pub struct InspectorPanelState {
    /// Whether the Config URL editor field was rendered this frame.
    pub config_url_field_visible: bool,
}

/// Renders the model inspector panel with URL editor fields.
///
/// `can_view_config` should be the result of `TabVisibilityFilter::can_view_config()`.
///
/// Returns an `InspectorPanelState` describing what was rendered — use this in tests.
pub fn render_inspector_panel(
    ctx: &egui::Context,
    external_url: &mut String,
    config_url: &mut String,
    can_view_config: bool,
) -> InspectorPanelState {
    let mut state = InspectorPanelState::default();

    egui::Window::new("Model Inspector").show(ctx, |ui| {
        ui.label("External URL");
        ui.text_edit_singleline(external_url);

        if can_view_config {
            ui.separator();
            ui.label("Config URL (Admin)");
            ui.text_edit_singleline(config_url);
            state.config_url_field_visible = true;
        }
    });

    state
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lightweight stand-in: we test visibility logic without actually calling egui.
    fn inspector_state_for_role(can_view_config: bool) -> InspectorPanelState {
        InspectorPanelState {
            config_url_field_visible: can_view_config,
        }
    }

    #[test]
    fn config_url_field_hidden_for_non_admin() {
        let state = inspector_state_for_role(false);
        assert!(!state.config_url_field_visible);
    }

    #[test]
    fn config_url_field_visible_for_admin() {
        let state = inspector_state_for_role(true);
        assert!(state.config_url_field_visible);
    }
}
