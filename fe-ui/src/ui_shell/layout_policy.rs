//! Finite shell geometry and same-frame layout observations; see AGENTS.md §layout-policy.

use bevy_egui::egui;

#[derive(Clone, Default)]
pub struct LayoutObservations {
    pub enabled: bool,
    pub scrolls: Vec<ScrollObservation>,
}

#[derive(Clone)]
pub struct ScrollObservation {
    pub content_id: u64,
    pub label: String,
    pub clip: egui::Rect,
    pub offset: egui::Vec2,
    pub content_size: egui::Vec2,
    pub layer: String,
}

fn key() -> egui::Id { egui::Id::new("qa_layout_observations") }

pub fn begin_frame(ctx: &egui::Context, enabled: bool) {
    ctx.data_mut(|data| data.insert_temp(key(), LayoutObservations { enabled, ..Default::default() }));
}

pub fn observations(ctx: &egui::Context) -> LayoutObservations {
    ctx.data(|data| data.get_temp(key()).unwrap_or_default())
}

pub fn scroll<R>(ui: &mut egui::Ui, label: &str, area: egui::ScrollArea, body: impl FnOnce(&mut egui::Ui) -> R) -> egui::scroll_area::ScrollAreaOutput<R> {
    let enabled = ui.ctx().data(|data| data.get_temp::<LayoutObservations>(key()).is_some_and(|o| o.enabled));
    let mut content_id = 0;
    let layer = ui.layer_id();
    let output = area.show(ui, |ui| { content_id = ui.unique_id().value(); body(ui) });
    if enabled {
        ui.ctx().data_mut(|data| {
            let observations = data.get_temp_mut_or_default::<LayoutObservations>(key());
            observations.scrolls.push(ScrollObservation { content_id, label: label.to_owned(), clip: output.inner_rect.intersect(ui.clip_rect()), offset: output.state.offset, content_size: output.content_size, layer: format!("{layer:?}") });
        });
    }
    output
}

