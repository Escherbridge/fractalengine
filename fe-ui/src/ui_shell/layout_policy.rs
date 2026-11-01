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
    let width = ui.available_width();
    let layer = ui.layer_id();
    let output = area.show(ui, |ui| {
        content_id = ui.unique_id().value();
        ui.set_max_width((width - 12.0).max(40.0));
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        ui.spacing_mut().text_edit_width = ui.available_width().min(200.0);
        body(ui)
    });
    if enabled {
        ui.ctx().data_mut(|data| {
            let observations = data.get_temp_mut_or_default::<LayoutObservations>(key());
            observations.scrolls.push(ScrollObservation { content_id, label: label.to_owned(), clip: output.inner_rect.intersect(ui.clip_rect()), offset: output.state.offset, content_size: output.content_size, layer: format!("{layer:?}") });
        });
    }
    output
}

/// Width left after reserving the viewport and a compact opposite sidebar.
pub fn right_width(ctx: &egui::Context) -> f32 {
    let protected = (ctx.viewport_rect().width() * 0.45).max(360.0);
    (ctx.available_rect().width() - protected).max(120.0)
}

pub fn left_width(ctx: &egui::Context) -> f32 {
    let protected = (ctx.viewport_rect().width() * 0.45).max(360.0);
    (ctx.available_rect().width() - protected - 280.0).clamp(120.0, 400.0)
}

pub fn editor_height(ctx: &egui::Context) -> f32 {
    (ctx.available_rect().height() * 0.38).clamp(64.0, 240.0)
}

pub fn trailing_row<R>(ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui) -> R) -> egui::InnerResponse<R> {
    ui.horizontal(|ui| ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), body).inner)
}

pub fn bounded_window<'a>(ctx: &egui::Context, title: impl Into<egui::WidgetText>) -> egui::Window<'a> {
    let screen = ctx.viewport_rect();
    egui::Window::new(title).constrain_to(screen.shrink(8.0))
        .max_width((screen.width() * 0.55).max(280.0))
        .max_height((screen.height() - 90.0).max(160.0)).hscroll(true).vscroll(true)
}

/// Unicode-safe path shortening keeps both the root and filename visible.
pub fn middle_ellipsis(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars { return value.to_owned(); }
    let keep = max_chars.saturating_sub(1);
    let head = keep / 3;
    let tail = keep - head;
    format!("{}?{}", value.chars().take(head).collect::<String>(), value.chars().skip(count - tail).collect::<String>())
}

pub fn apply_style(ctx: &egui::Context) {
    let id = egui::Id::new("layout_policy_style");
    if ctx.data(|d| d.get_temp::<bool>(id).unwrap_or(false)) { return; }
    ctx.style_mut(|style| {
        style.spacing.button_padding.y = style.spacing.button_padding.y.max(5.0);
        style.spacing.interact_size.y = style.spacing.interact_size.y.max(24.0);
    });
    ctx.data_mut(|d| d.insert_temp(id,true));
}
