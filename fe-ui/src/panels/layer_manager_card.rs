//! Layer Manager card — renders ONLY from the unified overlay registry
//! (`gis::overlay_registry`, D-11 / ui_ux §7): terrain layers, visibility
//! Groups, the scale bar and the metric grid share one row widget. Embedded in
//! the GIS panel. See `fe-ui/src/AGENTS.md` §gis-query-ui.

use bevy_egui::egui;

use crate::actions::{UiAction, UiManager};
use crate::gis::{self, OverlayEntry, OverlayKey};
use crate::settings::AppSettings;
use crate::terrain_map::PetalMapState;
use crate::theme;
use crate::visibility::VisibilityState;

pub(crate) fn layer_manager_section(
    ui: &mut egui::Ui,
    petal_map: &mut PetalMapState,
    ui_mgr: &mut UiManager,
    petal_id: &str,
    visibility: &VisibilityState,
    settings: &AppSettings,
) {
    ui.label(
        egui::RichText::new("Layers")
            .strong()
            .color(theme::TEXT_SECTION),
    );
    ui.add_space(4.0);

    if petal_map.terrain_json.is_some() {
        render_view_mode_row(ui, petal_map, ui_mgr, petal_id);
        ui.add_space(4.0);
        ui.separator();
        ui.add_space(4.0);
    } else {
        ui.label(
            egui::RichText::new("No terrain map assigned to this petal yet.")
                .small()
                .color(theme::TEXT_MUTED)
                .italics(),
        );
    }

    let rows = gis::overlay_registry(petal_map.terrain_json.as_ref(), visibility, settings);
    for row in &rows {
        render_overlay_row(ui, petal_map, ui_mgr, petal_id, visibility, row);
    }
}

/// Splat "View mode" selector (Mesh/Splats/Hybrid) — persisted as the
/// additive `"view_mode"` petal terrain JSON field via the same
/// mutate-and-round-trip idiom as the layer rows. The values must stay
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

/// The one row widget: a visibility checkbox (dispatching
/// `gis::apply_overlay_toggle`) plus, for rows with an opacity, a slider that
/// live-previews on drag and persists on release.
fn render_overlay_row(
    ui: &mut egui::Ui,
    petal_map: &mut PetalMapState,
    ui_mgr: &mut UiManager,
    petal_id: &str,
    visibility: &VisibilityState,
    row: &OverlayEntry,
) {
    let mut visible = row.visible;
    ui.horizontal(|ui| {
        if ui.checkbox(&mut visible, row.name.as_str()).changed() {
            ui_mgr.push_action(gis::apply_overlay_toggle(
                &row.key,
                visible,
                petal_id,
                visibility.groups(),
            ));
        }
    });
    let (Some(mut opacity), OverlayKey::TerrainLayer(name)) = (row.opacity, &row.key) else {
        return;
    };
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Opacity")
                .small()
                .color(theme::TEXT_DIM),
        );
        let resp = ui.add(egui::Slider::new(&mut opacity, 0.0..=1.0));
        if resp.changed() {
            if let Some(current) = &petal_map.terrain_json {
                petal_map.terrain_json =
                    Some(gis::set_layer_field(current, name, None, Some(opacity)));
            }
        }
        if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
            ui_mgr.push_action(UiAction::GisSetLayer {
                petal_id: petal_id.to_string(),
                layer_name: name.clone(),
                visible: None,
                opacity: Some(opacity),
            });
        }
    });
    ui.add_space(4.0);
}
