//! Layer Manager card (visibility + opacity for the active petal's terrain
//! layer stack) — embedded in the GIS panel. See `fe-ui/src/AGENTS.md`
//! §gis-query-ui.

use bevy_egui::egui;

use crate::actions::{UiAction, UiManager};
use crate::gis::{self, layer_entries_from_terrain_json};
use crate::terrain_map::PetalMapState;
use crate::theme;

/// Layer names fe-terrain's `petal_binding` maps to a `LayerType` today (see
/// `fe-terrain/src/AGENTS.md` §petal_binding); toggling these actually
/// changes what's rendered.
const HONORED_LAYER_NAMES: &[&str] = &["satellite", "terrain"];

/// Layer names offered as inert placeholders — fe-terrain's `petal_binding`
/// only maps `"satellite"`/`"terrain"` today and warns+skips anything else,
/// so these checkboxes have no rendering effect yet. Tracked as a residual
/// (see FR-3 in the track spec).
const INERT_LAYER_NAMES: &[&str] = &["gpx_track", "geojson_overlay"];

pub(crate) fn layer_manager_section(
    ui: &mut egui::Ui,
    petal_map: &mut PetalMapState,
    ui_mgr: &mut UiManager,
    petal_id: &str,
) {
    ui.label(
        egui::RichText::new("Layers")
            .strong()
            .color(theme::TEXT_SECTION),
    );
    ui.add_space(4.0);

    if petal_map.terrain_json.is_none() {
        ui.label(
            egui::RichText::new("No terrain map assigned to this petal yet.")
                .small()
                .color(theme::TEXT_MUTED)
                .italics(),
        );
        return;
    }

    render_view_mode_row(ui, petal_map, ui_mgr, petal_id);
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);

    for name in HONORED_LAYER_NAMES {
        render_honored_layer_row(ui, petal_map, ui_mgr, petal_id, name);
    }

    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new("Not yet honored by fe-terrain (residual — see AGENTS.md):")
            .small()
            .color(theme::TEXT_MUTED)
            .italics(),
    );
    for name in INERT_LAYER_NAMES {
        let mut inert = false;
        ui.add_enabled(false, egui::Checkbox::new(&mut inert, *name)).on_hover_text(
            "Layer name isn't mapped in fe-terrain's petal_binding yet — see fe-terrain/src/AGENTS.md §petal_binding",
        );
    }
}

/// Splat "View mode" selector (Mesh/Splats/Hybrid) — persisted as the
/// additive `"view_mode"` petal terrain JSON field via the same
/// mutate-and-round-trip idiom as the layer checkboxes below. Another track
/// builds the renderer side reading this field; the values must stay
/// exactly `"mesh"|"splats"|"hybrid"` (see `crate::gis::ViewMode`).
fn render_view_mode_row(
    ui: &mut egui::Ui,
    petal_map: &mut PetalMapState,
    ui_mgr: &mut UiManager,
    petal_id: &str,
) {
    let current = petal_map
        .terrain_json
        .as_ref()
        .map(gis::view_mode_from_terrain_json)
        .unwrap_or_default();

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("View mode")
                .small()
                .color(theme::TEXT_DIM),
        );
        for (mode, label) in [
            (gis::ViewMode::Mesh, "Mesh"),
            (gis::ViewMode::Splats, "Splats"),
            (gis::ViewMode::Hybrid, "Hybrid"),
        ] {
            let active = current == mode;
            let btn = egui::Button::new(label).fill(if active {
                theme::BG_BUTTON_ACTIVE
            } else {
                theme::BG_BUTTON
            });
            if ui.add(btn).clicked() && !active {
                if let Some(current_doc) = &petal_map.terrain_json {
                    petal_map.terrain_json = Some(gis::set_view_mode_field(current_doc, mode));
                }
                ui_mgr.push_action(UiAction::GisSetViewMode {
                    petal_id: petal_id.to_string(),
                    view_mode: mode.as_str().to_string(),
                });
            }
        }
    });
}

/// Renders one honored layer's visible checkbox + opacity slider. Mirrors
/// `dialogs::hexon_manager::render_scale_controls`'s world-scale idiom:
/// mutate the local `petal_map.terrain_json` on every `changed()` for a live
/// preview, but only push the persisting `UiAction::GisSetLayer` on release
/// (slider) or immediately (checkbox, which has no meaningful "drag").
fn render_honored_layer_row(
    ui: &mut egui::Ui,
    petal_map: &mut PetalMapState,
    ui_mgr: &mut UiManager,
    petal_id: &str,
    name: &str,
) {
    let entries = petal_map
        .terrain_json
        .as_ref()
        .map(layer_entries_from_terrain_json)
        .unwrap_or_default();
    let existing = entries.iter().find(|e| e.name == name);
    let mut visible = existing.map(|e| e.visible).unwrap_or(true);
    let mut opacity = existing.map(|e| e.opacity).unwrap_or(1.0);

    ui.horizontal(|ui| {
        if ui.checkbox(&mut visible, capitalize(name)).changed() {
            preview_layer_edit(petal_map, name, Some(visible), None);
            ui_mgr.push_action(UiAction::GisSetLayer {
                petal_id: petal_id.to_string(),
                layer_name: name.to_string(),
                visible: Some(visible),
                opacity: None,
            });
        }
    });
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Opacity")
                .small()
                .color(theme::TEXT_DIM),
        );
        let resp = ui.add(egui::Slider::new(&mut opacity, 0.0..=1.0));
        if resp.changed() {
            preview_layer_edit(petal_map, name, None, Some(opacity));
        }
        if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
            ui_mgr.push_action(UiAction::GisSetLayer {
                petal_id: petal_id.to_string(),
                layer_name: name.to_string(),
                visible: None,
                opacity: Some(opacity),
            });
        }
    });
    ui.add_space(4.0);
}

/// Local-only mutation of the stored terrain JSON for immediate slider/checkbox
/// feedback, ahead of the `UiAction` round-trip that actually persists it.
fn preview_layer_edit(
    petal_map: &mut PetalMapState,
    name: &str,
    visible: Option<bool>,
    opacity: Option<f32>,
) {
    if let Some(current) = &petal_map.terrain_json {
        petal_map.terrain_json = Some(gis::set_layer_field(current, name, visible, opacity));
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capitalize_basic() {
        assert_eq!(capitalize("satellite"), "Satellite");
        assert_eq!(capitalize("gpx_track"), "Gpx_track");
        assert_eq!(capitalize(""), "");
    }
}
